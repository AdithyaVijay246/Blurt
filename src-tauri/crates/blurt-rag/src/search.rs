//! Hybrid retrieval — `MODULE_04_EMBEDDINGS_RAG.md` §4–§7.
//!
//! The one retrieval path shared by plain search and Sleep-Mode's "ask". §4 is
//! explicit that both run the *same* underlying query; they differ only in the
//! time window they ask for (§5) and in whether a generative model reads the
//! results afterwards. So there is one function here, not two.
//!
//! ## What "hybrid" means here
//!
//! Semantic similarity from the vector store, plus an exact-match **boost**
//! from the `keywords` table — §4's two halves. The boost is deliberately a
//! re-ranking of the vector candidate set rather than a second source of
//! results: a keyword-only item has no *matching chunk*, and §7 requires every
//! result to be able to jump to the chunk that matched, scrolled and
//! highlighted. A result with nothing to jump to would be a different kind of
//! object wearing the same shape. [`VECTOR_OVERFETCH`] is what keeps that
//! honest — the vector side is asked for several times the requested page so
//! an exact-term match is in the candidate set to be boosted.
//!
//! ## Sort order
//!
//! Pure relevance, with no recency re-weighting. §4 is unusually direct about
//! this: time-scoping already handles recency at the window level (§5), so
//! re-applying it inside the window would bury a better match for being older.
//! The only thing that outranks relevance is `@` scope, and that is a grouping
//! rather than a score adjustment — see [`ScopeGroup`].
//!
//! ## Sensitive content
//!
//! Never indexed, so in the normal case there is nothing here to exclude. The
//! filter exists anyway, for the case §3 does not cover: a destination marked
//! sensitive *after* its items were indexed. Until the Phase 6 command that
//! flips that flag also retracts the index, those vectors are still in
//! LanceDB — so every candidate is re-checked against
//! `blurt_schema::repository::indexing::is_item_indexable` before it can
//! become a result. That function is named for indexing but states exactly the
//! rule retrieval needs (live item, live destination, not sensitive), and
//! asking it is what makes the exclusion structural on the way out as well as
//! on the way in.

use std::collections::HashMap;

use rusqlite::Connection;
use uuid::Uuid;

use blurt_schema::repository::destinations::{self, Destination};
use blurt_schema::repository::indexing::is_item_indexable;
use blurt_schema::repository::{edits, embeddings, items, keywords as keyword_rows};

use crate::embedding::Embedder;
use crate::error::Result;
use crate::keywords::NGRAM_SIZE;
use crate::vectorstore::{VectorMatch, VectorStore};

/// Most distinct terms a query is broken into before the keyword lookup.
///
/// The lookup binds one SQL variable per term and SQLite's default ceiling is
/// 999, so an unbounded expansion of a pasted-in wall of text would fail the
/// query rather than merely slow it. Unigrams are emitted first and truncation
/// takes from the tail, so what a very long query loses is its longest phrases,
/// never its individual words.
const MAX_QUERY_TERMS: usize = 200;

/// Breaks a query into the terms worth looking up in the `keywords` table.
///
/// Every contiguous run of 1..=[`NGRAM_SIZE`] words, lowercased and stripped of
/// edge punctuation — matching how [`crate::keywords::extract`] stores them, so
/// comparison is exact-match with no `LOWER()` per row.
///
/// Deliberately *not* YAKE. Running keyword extraction on a two-word query
/// would be asking an unsupervised statistical method to find the important
/// terms in a text that is all important terms; enumerating n-grams instead
/// costs nothing at this length and cannot discard the word the user actually
/// searched for. Stopwords need no filtering on this side either: the stored
/// side is already stopword-free, so a query n-gram containing one simply
/// matches nothing.
pub fn query_terms(query: &str) -> Vec<String> {
    let words: Vec<&str> = query
        .split_whitespace()
        .map(|word| word.trim_matches(|c: char| !c.is_alphanumeric()))
        .filter(|word| !word.is_empty())
        .collect();

    let mut terms = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for size in 1..=NGRAM_SIZE {
        for window in words.windows(size) {
            let term = window.join(" ").to_lowercase();
            if seen.insert(term.clone()) {
                terms.push(term);
            }
        }
    }

    terms.truncate(MAX_QUERY_TERMS);
    terms
}

/// Timestamps are Unix milliseconds throughout the schema.
const MS_PER_DAY: i64 = 24 * 60 * 60 * 1000;

/// How far back a query looks — `MODULE_04_EMBEDDINGS_RAG.md` §5.
///
/// The one knob that differs between the two callers of this module. Plain
/// search is [`Unbounded`](Self::Unbounded): there is no LLM context to manage,
/// so it searches the whole history and paginates. Sleep-Mode starts at
/// [`Self::SLEEP_MODE_DEFAULT`] and widens through §5's "load more" ladder if
/// the answer feels thin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetrievalWindow {
    /// The last `n` days, measured from now.
    Days(u32),
    /// All of history.
    Unbounded,
}

impl RetrievalWindow {
    /// §5's Sleep-Mode default.
    pub const SLEEP_MODE_DEFAULT: Self = Self::Days(30);

    /// Whether a version stamped `timestamp_ms` falls inside this window.
    ///
    /// `now_ms` is passed rather than read from the clock so the boundary is
    /// testable without waiting for one.
    pub fn contains(self, timestamp_ms: i64, now_ms: i64) -> bool {
        match self {
            Self::Unbounded => true,
            // Only the lower edge is a bound. A timestamp ahead of `now` stays
            // in: Module 5 syncs from devices whose clocks may run fast, and
            // dropping the newest thing the user wrote for being *too* recent
            // is never the behaviour anyone wants.
            Self::Days(days) => timestamp_ms >= now_ms - i64::from(days) * MS_PER_DAY,
        }
    }
}

/// How much a full keyword match can add to a result's score.
///
/// §4 calls the keyword half a *boost*, and the number encodes that literally.
/// Semantic similarity occupies `0.0..=1.0`, so 0.25 is enough to lift an
/// exact-term match past a marginally closer paraphrase — which is the whole
/// point of hybrid ranking — and never enough to lift a semantically unrelated
/// item above a strong match. No formula for this appears in the design doc,
/// and it is not in §11's deferred list either, so it is an implementation
/// detail in the same sense as `blurt-router`'s NL confidence score.
const KEYWORD_BOOST: f32 = 0.25;

/// The share of [`KEYWORD_BOOST`] earned by `hits` matching keywords.
///
/// Diminishing returns: the first hit earns half the boost, the second three
/// quarters, and the curve approaches the full boost without ever reaching it.
/// Both ends of that shape are load-bearing. The first hit has to be worth
/// enough on its own to close a small semantic gap — most queries are a single
/// phrase, and a scheme that needed several distinct tags before it did
/// anything would leave §4's "exact-term lookup" case unserved. And the
/// asymptote is what stops a heavily-tagged note out-ranking a precisely
/// relevant one purely for carrying more tags.
fn keyword_boost_fraction(hits: usize) -> f32 {
    1.0 - 0.5_f32.powi(hits.min(i32::MAX as usize) as i32)
}

/// How many vector candidates to pull per result actually wanted.
///
/// The candidate set shrinks twice on the way to a page — once when chunks
/// collapse to one row per item, and again when the window and eligibility
/// filters run — so asking LanceDB for exactly `limit` would routinely return
/// a short page. It also does the work described in the module docs: keyword
/// boosting can only re-rank what the vector side surfaced, so the surface has
/// to be wider than the page.
const VECTOR_OVERFETCH: usize = 8;

/// Results per page when a caller does not say.
const DEFAULT_LIMIT: usize = 20;

/// Which of §6's two sections a result belongs to.
///
/// `@` scoping is soft: both groups always render, because a hard filter would
/// hide a real match that was simply filed somewhere the user did not expect.
/// Grouping is the *only* thing that outranks relevance in the final order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScopeGroup {
    /// Section "In [Scope]" — the scoped destination itself or a descendant.
    InScope,
    /// Section "Elsewhere".
    Elsewhere,
}

/// Everything a caller can vary about one query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchOptions {
    /// §5's time window. Plain search leaves this [`RetrievalWindow::Unbounded`].
    pub window: RetrievalWindow,
    /// §6's `@` scope. `None` puts every result in [`ScopeGroup::Elsewhere`].
    pub scope: Option<Uuid>,
    pub limit: usize,
    /// Results to skip — §5's pagination for plain search.
    pub offset: usize,
}

impl Default for SearchOptions {
    fn default() -> Self {
        Self {
            window: RetrievalWindow::Unbounded,
            scope: None,
            limit: DEFAULT_LIMIT,
            offset: 0,
        }
    }
}

/// One item that matched, with everything §7 needs to display and open it.
#[derive(Debug, Clone, PartialEq)]
pub struct SearchResult {
    pub item_id: Uuid,
    pub destination_id: Uuid,
    /// Root-to-leaf ancestor chain, computed live from `parentId` per §7 —
    /// never a stored path string.
    pub path: Vec<Destination>,
    /// Which text version matched. `None` is the original capture.
    pub edit_id: Option<Uuid>,
    pub chunk_index: i64,
    /// Offsets into *this version's* text, in Unicode scalars — §7's
    /// jump-to-chunk target. Not into `items.currentText`, which may since have
    /// been rewritten.
    pub chunk_start_offset: i64,
    pub chunk_end_offset: i64,
    /// The matched chunk verbatim: the snippet a result renders. Full context
    /// is fetched on tap, so a page of results never carries whole notes.
    pub chunk_text: String,
    /// The version's own timestamp — `edits.editedAt`, or `items.createdAt` for
    /// an original capture. The value the window filtered on, so §7's displayed
    /// timestamp and §5's scoping can never disagree.
    pub timestamp: i64,
    pub score: f32,
    pub group: ScopeGroup,
}

/// Turns raw vector hits into a ranked page of results.
///
/// The half of retrieval that needs no embedding model: given the hits,
/// everything else is SQLCipher reads and arithmetic. [`hybrid_search`] is the
/// thin wrapper that produces the hits. Keeping the split here is what lets
/// ranking, deduplication, windowing and scoping be tested without loading
/// ~100MB of model to assert that an old note sorts after a new one.
pub(crate) fn rank(
    conn: &Connection,
    matches: &[VectorMatch],
    query: &str,
    options: &SearchOptions,
    now_ms: i64,
) -> Result<Vec<SearchResult>> {
    // Collapse to the best chunk per item before touching SQLCipher. §4 ranks
    // items, not chunks, and doing it first means one round of lookups per
    // item rather than per chunk of every item.
    let mut best: HashMap<Uuid, &VectorMatch> = HashMap::new();
    for candidate in matches {
        best.entry(candidate.item_id)
            .and_modify(|current| {
                if candidate.distance < current.distance {
                    *current = candidate;
                }
            })
            .or_insert(candidate);
    }

    let keyword_hits: HashMap<Uuid, usize> =
        keyword_rows::items_matching_any(conn, &query_terms(query))?
            .into_iter()
            .collect();

    let mut results = Vec::new();
    for candidate in best.into_values() {
        // Asked, never assumed — see the module docs. This is what keeps a
        // destination marked sensitive *after* indexing from leaking through
        // vectors that are still in LanceDB.
        if !is_item_indexable(conn, candidate.item_id)? {
            continue;
        }
        let Some(item) = items::get_by_id(conn, candidate.item_id)? else {
            continue;
        };
        // An orphaned vector: written to LanceDB by a pass that was interrupted
        // before its SQLCipher commit. Unreachable by design, so skip it rather
        // than failing the whole query over one dangling row.
        let Some(chunk) = embeddings::get_by_vector_ref(conn, &candidate.vector_ref)? else {
            continue;
        };

        // The version's own text and timestamp. An edit's chunk offsets index
        // that edit's wording, so reading either from the item would slice the
        // wrong string and date the result to a capture the user has since
        // rewritten.
        let (text, timestamp) = match chunk.edit_id {
            None => (item.original_text.clone(), item.created_at),
            Some(edit_id) => match edits::get_by_id(conn, edit_id)? {
                Some(edit) => (edit.text, edit.edited_at),
                None => continue,
            },
        };

        if !options.window.contains(timestamp, now_ms) {
            continue;
        }

        let path = destinations::path(conn, item.destination_id)?;
        let group = match options.scope {
            // A descendant counts: the path is the ancestor chain, so the scope
            // appearing anywhere in it means this item sits at or below it.
            Some(scope) if path.iter().any(|d| d.id == scope) => ScopeGroup::InScope,
            _ => ScopeGroup::Elsewhere,
        };

        let hits = keyword_hits.get(&item.id).copied().unwrap_or(0);
        let boost = KEYWORD_BOOST * keyword_boost_fraction(hits);
        // Cosine distance is `1 - cos`, so this recovers the cosine similarity.
        // Clamped because the metric runs to 2, and an unclamped negative would
        // sort a weak match below one that never matched at all.
        let similarity = (1.0 - candidate.distance).clamp(0.0, 1.0);

        results.push(SearchResult {
            item_id: item.id,
            destination_id: item.destination_id,
            path,
            edit_id: chunk.edit_id,
            chunk_index: chunk.chunk_index,
            chunk_start_offset: chunk.chunk_start_offset,
            chunk_end_offset: chunk.chunk_end_offset,
            chunk_text: slice_by_scalar(&text, chunk.chunk_start_offset, chunk.chunk_end_offset),
            timestamp,
            score: similarity + boost,
            group,
        });
    }

    results.sort_by(|a, b| {
        // Scope group outranks score — §6's two sections. Everything below it is
        // pure relevance, with no recency term: §4 rules that out explicitly.
        // `item_id` last so a tie cannot reorder between identical queries,
        // which iteration over a `HashMap` would otherwise make arbitrary.
        (a.group == ScopeGroup::Elsewhere)
            .cmp(&(b.group == ScopeGroup::Elsewhere))
            .then_with(|| b.score.total_cmp(&a.score))
            .then_with(|| a.item_id.cmp(&b.item_id))
    });

    Ok(results.into_iter().skip(options.offset).take(options.limit).collect())
}

/// Slices `text` by Unicode scalar offsets, the units chunk offsets are counted
/// in (see [`crate::chunking`]). Byte slicing would panic mid-character on any
/// text with an accent in it.
fn slice_by_scalar(text: &str, start: i64, end: i64) -> String {
    let start = start.max(0) as usize;
    let end = end.max(0) as usize;
    text.chars().skip(start).take(end.saturating_sub(start)).collect()
}

/// Unix milliseconds, matching every timestamp in the schema.
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Runs one query — `MODULE_04_EMBEDDINGS_RAG.md` §4.
///
/// The entry point for both of §1's LLM-free consumers: plain search calls it
/// with an [`Unbounded`](RetrievalWindow::Unbounded) window, and Sleep-Mode
/// calls it with [`RetrievalWindow::SLEEP_MODE_DEFAULT`] to assemble the
/// context it hands the generative model. Neither loads a generative model
/// here; §9's classification decides which of them runs, upstream.
///
/// The vector side is over-fetched by [`VECTOR_OVERFETCH`] — see that constant
/// for why. One consequence worth knowing at the call site: deep pagination
/// degrades, because page N is taken from a candidate set sized for pages 1..N
/// rather than from a cursor. §5 asks for pagination on plain search, which in
/// a personal log is a handful of pages at most; a real cursor would mean
/// keeping ranking state between calls, and is not worth it until the
/// behaviour is observed to matter.
pub async fn hybrid_search(
    conn: &Connection,
    store: &VectorStore,
    embedder: &mut Embedder,
    query: &str,
    options: &SearchOptions,
) -> Result<Vec<SearchResult>> {
    // Both of these decide the answer before the model is needed. Loading
    // ~100MB to embed nothing, or to rank a page of zero results, is pure cost.
    if query.trim().is_empty() || options.limit == 0 {
        return Ok(Vec::new());
    }

    let vectors = embedder.embed(&[query.to_string()])?;
    let Some(query_vector) = vectors.first() else {
        return Ok(Vec::new());
    };

    let wanted = options.offset.saturating_add(options.limit);
    let matches = store
        .search(query_vector, wanted.saturating_mul(VECTOR_OVERFETCH))
        .await?;

    rank(conn, &matches, query, options, now_ms())
}

#[cfg(test)]
mod tests {
    use super::*;

    use blurt_schema::repository::destinations::DestinationKind;
    use blurt_schema::{Database, MasterKey};

    const NOW: i64 = 1_700_000_000_000;

    fn migrated() -> Database {
        let db = Database::open_in_memory(&MasterKey::generate()).unwrap();
        blurt_schema::migrations::run(db.conn()).unwrap();
        db
    }

    fn destination(db: &Database, name: &str, parent: Option<Uuid>, sensitive: bool) -> Uuid {
        destinations::create(
            db.conn(),
            name,
            &name.to_lowercase().replace(' ', ""),
            DestinationKind::List,
            parent,
            false,
            sensitive,
            0,
        )
        .unwrap()
        .id
    }

    /// Captures an item and indexes one chunk covering its whole text, exactly
    /// as `indexing::index_item` would — minus the model. Returns the
    /// `vectorRef` a hand-built [`VectorMatch`] can point at.
    fn indexed(db: &Database, destination_id: Uuid, text: &str) -> (Uuid, String) {
        let item = items::capture(db.conn(), destination_id, text).unwrap();
        let vector_ref = Uuid::new_v4().to_string();
        embeddings::insert_many(
            db.conn(),
            &[embeddings::Embedding {
                id: Uuid::new_v4(),
                item_id: item.id,
                edit_id: None,
                vector_ref: vector_ref.clone(),
                chunk_index: 0,
                chunk_start_offset: 0,
                chunk_end_offset: text.chars().count() as i64,
            }],
        )
        .unwrap();
        (item.id, vector_ref)
    }

    fn hit(item_id: Uuid, vector_ref: &str, distance: f32) -> VectorMatch {
        VectorMatch { vector_ref: vector_ref.to_string(), item_id, distance }
    }

    /// Backdates an item so the window filter has something to exclude.
    fn backdate_item(db: &Database, item_id: Uuid, days: i64) {
        db.conn()
            .execute(
                "UPDATE items SET createdAt = ?1 WHERE id = ?2",
                rusqlite::params![NOW - days * MS_PER_DAY, item_id.to_string()],
            )
            .unwrap();
    }

    #[test]
    fn a_hit_becomes_a_result_carrying_what_section_7_needs_to_display_it() {
        let db = migrated();
        let notes = destination(&db, "Notes", None, false);
        let (item, vector_ref) = indexed(&db, notes, "buy oranges");

        let results = rank(
            db.conn(),
            &[hit(item, &vector_ref, 0.1)],
            "oranges",
            &SearchOptions::default(),
            NOW,
        )
        .unwrap();

        assert_eq!(results.len(), 1);
        let result = &results[0];
        assert_eq!(result.item_id, item);
        assert_eq!(result.destination_id, notes);
        assert_eq!(result.chunk_text, "buy oranges");
        assert_eq!(result.edit_id, None);
        assert_eq!(
            result.path.iter().map(|d| d.name.as_str()).collect::<Vec<_>>(),
            vec!["Notes"],
            "§7 wants a live path, not a stored string"
        );
    }

    #[test]
    fn the_path_is_the_full_ancestor_chain_root_first() {
        let db = migrated();
        let shopping = destination(&db, "Shopping", None, false);
        let weekly = destination(&db, "Weekly", Some(shopping), false);
        let (item, vector_ref) = indexed(&db, weekly, "oranges");

        let results = rank(
            db.conn(),
            &[hit(item, &vector_ref, 0.1)],
            "oranges",
            &SearchOptions::default(),
            NOW,
        )
        .unwrap();

        assert_eq!(
            results[0].path.iter().map(|d| d.name.as_str()).collect::<Vec<_>>(),
            vec!["Shopping", "Weekly"]
        );
    }

    #[test]
    fn a_nearer_vector_outranks_a_further_one() {
        let db = migrated();
        let notes = destination(&db, "Notes", None, false);
        let (near, near_ref) = indexed(&db, notes, "oranges are citrus");
        let (far, far_ref) = indexed(&db, notes, "the car needs a service");

        let results = rank(
            db.conn(),
            &[hit(far, &far_ref, 0.9), hit(near, &near_ref, 0.05)],
            "oranges",
            &SearchOptions::default(),
            NOW,
        )
        .unwrap();

        assert_eq!(results[0].item_id, near);
        assert_eq!(results[1].item_id, far);
        assert!(results[0].score > results[1].score);
    }

    #[test]
    fn similarity_is_one_minus_the_cosine_distance() {
        let db = migrated();
        let notes = destination(&db, "Notes", None, false);
        let (item, vector_ref) = indexed(&db, notes, "oranges");

        // No keyword row was written, so the score is the semantic half alone.
        let results = rank(
            db.conn(),
            &[hit(item, &vector_ref, 0.25)],
            "oranges",
            &SearchOptions::default(),
            NOW,
        )
        .unwrap();

        assert!((results[0].score - 0.75).abs() < 1e-5, "got {}", results[0].score);
    }

    #[test]
    fn an_opposed_vector_scores_zero_rather_than_negative() {
        let db = migrated();
        let notes = destination(&db, "Notes", None, false);
        let (item, vector_ref) = indexed(&db, notes, "oranges");

        // Cosine distance runs to 2. Left unclamped that reads as -1, which
        // would sort below a result that did not match at all.
        let results = rank(
            db.conn(),
            &[hit(item, &vector_ref, 1.8)],
            "citrus",
            &SearchOptions::default(),
            NOW,
        )
        .unwrap();

        assert_eq!(results[0].score, 0.0);
    }

    #[test]
    fn only_the_best_chunk_of_an_item_survives() {
        let db = migrated();
        let notes = destination(&db, "Notes", None, false);
        let item = items::capture(db.conn(), notes, "oranges and lemons and limes").unwrap();
        let refs: Vec<String> = (0..3).map(|_| Uuid::new_v4().to_string()).collect();
        embeddings::insert_many(
            db.conn(),
            &refs
                .iter()
                .enumerate()
                .map(|(index, vector_ref)| embeddings::Embedding {
                    id: Uuid::new_v4(),
                    item_id: item.id,
                    edit_id: None,
                    vector_ref: vector_ref.clone(),
                    chunk_index: index as i64,
                    chunk_start_offset: index as i64,
                    chunk_end_offset: index as i64 + 7,
                })
                .collect::<Vec<_>>(),
        )
        .unwrap();

        let results = rank(
            db.conn(),
            &[
                hit(item.id, &refs[0], 0.6),
                hit(item.id, &refs[1], 0.1),
                hit(item.id, &refs[2], 0.4),
            ],
            "oranges",
            &SearchOptions::default(),
            NOW,
        )
        .unwrap();

        assert_eq!(results.len(), 1, "one item must not occupy three result slots");
        assert_eq!(results[0].chunk_index, 1, "the nearest chunk is the one worth showing");
    }

    #[test]
    fn a_keyword_match_boosts_an_item_past_a_slightly_nearer_one() {
        let db = migrated();
        let notes = destination(&db, "Notes", None, false);
        let (tagged, tagged_ref) = indexed(&db, notes, "the orange delivery is late");
        let (untagged, untagged_ref) = indexed(&db, notes, "citrus season starts soon");
        keyword_rows::replace_for_item(db.conn(), tagged, &["orange delivery".to_string()]).unwrap();

        let plain = rank(
            db.conn(),
            &[hit(tagged, &tagged_ref, 0.4), hit(untagged, &untagged_ref, 0.3)],
            "citrus",
            &SearchOptions::default(),
            NOW,
        )
        .unwrap();
        assert_eq!(plain[0].item_id, untagged, "without the term, the nearer vector wins");

        let boosted = rank(
            db.conn(),
            &[hit(tagged, &tagged_ref, 0.4), hit(untagged, &untagged_ref, 0.3)],
            "orange delivery",
            &SearchOptions::default(),
            NOW,
        )
        .unwrap();
        assert_eq!(boosted[0].item_id, tagged, "the exact term should pull it ahead");
    }

    #[test]
    fn the_keyword_boost_cannot_outweigh_a_genuinely_better_match() {
        let db = migrated();
        let notes = destination(&db, "Notes", None, false);
        let (tagged, tagged_ref) = indexed(&db, notes, "orange");
        let (near, near_ref) = indexed(&db, notes, "oranges");
        keyword_rows::replace_for_item(
            db.conn(),
            tagged,
            &[
                "orange".to_string(),
                "fruit".to_string(),
                "citrus".to_string(),
                "peel".to_string(),
            ],
        )
        .unwrap();

        let results = rank(
            db.conn(),
            &[hit(tagged, &tagged_ref, 0.9), hit(near, &near_ref, 0.05)],
            "orange fruit citrus peel",
            &SearchOptions::default(),
            NOW,
        )
        .unwrap();

        assert_eq!(
            results[0].item_id, near,
            "a saturated boost is still only worth {KEYWORD_BOOST}"
        );
    }

    #[test]
    fn a_keyword_only_item_is_not_conjured_into_the_results() {
        let db = migrated();
        let notes = destination(&db, "Notes", None, false);
        let (present, present_ref) = indexed(&db, notes, "oranges");
        let (absent, _) = indexed(&db, notes, "orange marmalade");
        keyword_rows::replace_for_item(db.conn(), absent, &["orange".to_string()]).unwrap();

        let results = rank(
            db.conn(),
            &[hit(present, &present_ref, 0.2)],
            "orange",
            &SearchOptions::default(),
            NOW,
        )
        .unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(
            results[0].item_id, present,
            "§7 needs a matching chunk to jump to; a keyword-only hit has none"
        );
    }

    #[test]
    fn an_item_whose_destination_became_sensitive_after_indexing_is_dropped() {
        // The gap `indexing.rs` documents: flipping the flag stops future
        // indexing but leaves the existing vectors in LanceDB. Retrieval must
        // not surface them in the meantime.
        let db = migrated();
        let notes = destination(&db, "Notes", None, false);
        let (item, vector_ref) = indexed(&db, notes, "gmail password");
        db.conn()
            .execute(
                "UPDATE destinations SET isSensitive = 1 WHERE id = ?1",
                [notes.to_string()],
            )
            .unwrap();

        let results = rank(
            db.conn(),
            &[hit(item, &vector_ref, 0.01)],
            "password",
            &SearchOptions::default(),
            NOW,
        )
        .unwrap();

        assert!(
            results.is_empty(),
            "sensitive content must be unreachable through every path"
        );
    }

    #[test]
    fn a_tombstoned_item_is_dropped() {
        let db = migrated();
        let notes = destination(&db, "Notes", None, false);
        let (item, vector_ref) = indexed(&db, notes, "oranges");
        items::tombstone(db.conn(), item).unwrap();

        let results = rank(
            db.conn(),
            &[hit(item, &vector_ref, 0.01)],
            "oranges",
            &SearchOptions::default(),
            NOW,
        )
        .unwrap();

        assert!(results.is_empty());
    }

    #[test]
    fn a_vector_with_no_embedding_row_is_skipped_rather_than_failing_the_query() {
        // The orphan the write order in `indexing.rs` deliberately tolerates.
        let db = migrated();
        let notes = destination(&db, "Notes", None, false);
        let (item, _) = indexed(&db, notes, "oranges");

        let results = rank(
            db.conn(),
            &[hit(item, "a-vector-ref-nothing-joins-to", 0.01)],
            "oranges",
            &SearchOptions::default(),
            NOW,
        )
        .unwrap();

        assert!(results.is_empty());
    }

    #[test]
    fn the_window_filters_on_the_versions_own_timestamp() {
        let db = migrated();
        let notes = destination(&db, "Notes", None, false);
        let (recent, recent_ref) = indexed(&db, notes, "oranges this week");
        let (old, old_ref) = indexed(&db, notes, "oranges last spring");
        backdate_item(&db, old, 90);

        let hits = [hit(recent, &recent_ref, 0.1), hit(old, &old_ref, 0.1)];
        let options = SearchOptions {
            window: RetrievalWindow::SLEEP_MODE_DEFAULT,
            ..SearchOptions::default()
        };

        let scoped = rank(db.conn(), &hits, "oranges", &options, NOW).unwrap();
        assert_eq!(scoped.len(), 1);
        assert_eq!(scoped[0].item_id, recent);

        let unbounded = rank(db.conn(), &hits, "oranges", &SearchOptions::default(), NOW).unwrap();
        assert_eq!(unbounded.len(), 2, "plain search has no window");
    }

    #[test]
    fn an_edits_chunk_is_dated_and_sliced_by_that_edit_not_the_original() {
        let db = migrated();
        let notes = destination(&db, "Notes", None, false);
        let item = items::capture(db.conn(), notes, "buy oranges").unwrap();
        backdate_item(&db, item.id, 90);
        let edit = edits::append(db.conn(), item.id, "buy oranges and lemons").unwrap();

        let vector_ref = Uuid::new_v4().to_string();
        embeddings::insert_many(
            db.conn(),
            &[embeddings::Embedding {
                id: Uuid::new_v4(),
                item_id: item.id,
                edit_id: Some(edit.id),
                vector_ref: vector_ref.clone(),
                chunk_index: 0,
                chunk_start_offset: 4,
                chunk_end_offset: 22,
            }],
        )
        .unwrap();

        let options = SearchOptions {
            window: RetrievalWindow::SLEEP_MODE_DEFAULT,
            ..SearchOptions::default()
        };
        let results =
            rank(db.conn(), &[hit(item.id, &vector_ref, 0.1)], "lemons", &options, NOW).unwrap();

        assert_eq!(results.len(), 1, "an old item edited today is recent content");
        assert_eq!(results[0].edit_id, Some(edit.id));
        assert_eq!(results[0].timestamp, edit.edited_at);
        assert_eq!(
            results[0].chunk_text, "oranges and lemons",
            "offsets index the edit's text, not the original capture"
        );
    }

    #[test]
    fn chunk_offsets_are_sliced_by_unicode_scalar_not_byte() {
        let db = migrated();
        let notes = destination(&db, "Notes", None, false);
        let text = "café crème brûlée";
        let item = items::capture(db.conn(), notes, text).unwrap();
        let vector_ref = Uuid::new_v4().to_string();
        embeddings::insert_many(
            db.conn(),
            &[embeddings::Embedding {
                id: Uuid::new_v4(),
                item_id: item.id,
                edit_id: None,
                vector_ref: vector_ref.clone(),
                chunk_index: 0,
                chunk_start_offset: 5,
                chunk_end_offset: 10,
            }],
        )
        .unwrap();

        let results = rank(
            db.conn(),
            &[hit(item.id, &vector_ref, 0.1)],
            "creme",
            &SearchOptions::default(),
            NOW,
        )
        .unwrap();

        assert_eq!(
            results[0].chunk_text, "crème",
            "byte slicing would land mid-character"
        );
    }

    #[test]
    fn scope_puts_the_scoped_destination_and_its_descendants_first() {
        let db = migrated();
        let shopping = destination(&db, "Shopping", None, false);
        let produce = destination(&db, "Produce", Some(shopping), false);
        let journal = destination(&db, "Journal", None, false);

        let (elsewhere, elsewhere_ref) = indexed(&db, journal, "oranges are in season");
        let (child, child_ref) = indexed(&db, produce, "oranges");
        let (direct, direct_ref) = indexed(&db, shopping, "orange juice");

        // The elsewhere hit is the *nearest*, so scope must be what reorders it.
        let results = rank(
            db.conn(),
            &[
                hit(elsewhere, &elsewhere_ref, 0.01),
                hit(child, &child_ref, 0.30),
                hit(direct, &direct_ref, 0.20),
            ],
            "oranges",
            &SearchOptions {
                scope: Some(shopping),
                ..SearchOptions::default()
            },
            NOW,
        )
        .unwrap();

        assert_eq!(
            results.iter().map(|r| r.item_id).collect::<Vec<_>>(),
            vec![direct, child, elsewhere]
        );
        assert_eq!(results[0].group, ScopeGroup::InScope);
        assert_eq!(results[1].group, ScopeGroup::InScope, "a descendant is in scope");
        assert_eq!(results[2].group, ScopeGroup::Elsewhere);
    }

    #[test]
    fn scope_never_hides_a_match() {
        // §6 is explicit that scoping is a display hint, not a `WHERE` clause:
        // a hard filter would hide a real match filed somewhere unexpected.
        let db = migrated();
        let shopping = destination(&db, "Shopping", None, false);
        let pantry = destination(&db, "Pantry", None, false);
        let (item, vector_ref) = indexed(&db, pantry, "oranges");

        let results = rank(
            db.conn(),
            &[hit(item, &vector_ref, 0.1)],
            "oranges",
            &SearchOptions {
                scope: Some(shopping),
                ..SearchOptions::default()
            },
            NOW,
        )
        .unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].group, ScopeGroup::Elsewhere);
    }

    #[test]
    fn without_a_scope_everything_is_elsewhere() {
        let db = migrated();
        let notes = destination(&db, "Notes", None, false);
        let (item, vector_ref) = indexed(&db, notes, "oranges");

        let results = rank(
            db.conn(),
            &[hit(item, &vector_ref, 0.1)],
            "oranges",
            &SearchOptions::default(),
            NOW,
        )
        .unwrap();

        assert_eq!(results[0].group, ScopeGroup::Elsewhere);
    }

    #[test]
    fn limit_and_offset_page_through_one_ranking() {
        let db = migrated();
        let notes = destination(&db, "Notes", None, false);
        let mut hits = Vec::new();
        let mut expected = Vec::new();
        for n in 0..5 {
            let (item, vector_ref) = indexed(&db, notes, &format!("oranges note {n}"));
            hits.push(hit(item, &vector_ref, 0.1 * n as f32));
            expected.push(item);
        }

        let page = |offset: usize, limit: usize| {
            rank(
                db.conn(),
                &hits,
                "oranges",
                &SearchOptions {
                    limit,
                    offset,
                    ..SearchOptions::default()
                },
                NOW,
            )
            .unwrap()
            .iter()
            .map(|r| r.item_id)
            .collect::<Vec<_>>()
        };

        assert_eq!(page(0, 2), expected[0..2]);
        assert_eq!(page(2, 2), expected[2..4]);
        assert_eq!(page(4, 2), expected[4..5], "a short final page, not a wrapped one");
        assert_eq!(page(9, 2), Vec::<Uuid>::new());
    }

    #[test]
    fn no_hits_is_an_empty_page_rather_than_an_error() {
        // §8's empty state is a UI affordance; the retrieval layer just returns
        // nothing.
        let db = migrated();
        assert!(rank(db.conn(), &[], "oranges", &SearchOptions::default(), NOW)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn results_are_ordered_deterministically_when_scores_tie() {
        let db = migrated();
        let notes = destination(&db, "Notes", None, false);
        let (a, a_ref) = indexed(&db, notes, "oranges one");
        let (b, b_ref) = indexed(&db, notes, "oranges two");

        let hits = [hit(a, &a_ref, 0.2), hit(b, &b_ref, 0.2)];
        let first = rank(db.conn(), &hits, "oranges", &SearchOptions::default(), NOW).unwrap();
        let reversed: Vec<VectorMatch> = hits.iter().rev().cloned().collect();
        let second = rank(db.conn(), &reversed, "oranges", &SearchOptions::default(), NOW).unwrap();

        assert_eq!(
            first.iter().map(|r| r.item_id).collect::<Vec<_>>(),
            second.iter().map(|r| r.item_id).collect::<Vec<_>>(),
            "tied results must not reorder between identical queries"
        );
    }

    #[test]
    fn an_unbounded_window_contains_everything() {
        let now = 1_700_000_000_000;
        assert!(RetrievalWindow::Unbounded.contains(0, now));
        assert!(RetrievalWindow::Unbounded.contains(now, now));
    }

    #[test]
    fn a_day_window_keeps_what_is_inside_it_and_drops_what_is_not() {
        let now = 1_700_000_000_000;
        let window = RetrievalWindow::Days(30);

        assert!(window.contains(now - 29 * MS_PER_DAY, now));
        assert!(!window.contains(now - 31 * MS_PER_DAY, now));
    }

    #[test]
    fn the_window_edge_is_inclusive() {
        let now = 1_700_000_000_000;
        assert!(RetrievalWindow::Days(30).contains(now - 30 * MS_PER_DAY, now));
    }

    #[test]
    fn a_future_timestamp_stays_in_every_window() {
        // Module 5 syncs from other devices, whose clocks may run ahead. An
        // item stamped in the future is still the most recent thing the user
        // wrote; treating it as outside the window would hide it entirely.
        let now = 1_700_000_000_000;
        assert!(RetrievalWindow::Days(30).contains(now + MS_PER_DAY, now));
    }

    #[test]
    fn a_zero_day_window_still_contains_this_moment() {
        let now = 1_700_000_000_000;
        assert!(RetrievalWindow::Days(0).contains(now, now));
        assert!(!RetrievalWindow::Days(0).contains(now - 1, now));
    }

    #[test]
    fn a_single_word_query_is_one_term() {
        assert_eq!(query_terms("oranges"), vec!["oranges"]);
    }

    #[test]
    fn terms_are_every_ngram_up_to_the_stored_limit_unigrams_first() {
        assert_eq!(
            query_terms("billing service migration"),
            vec![
                "billing",
                "service",
                "migration",
                "billing service",
                "service migration",
                "billing service migration",
            ]
        );
    }

    #[test]
    fn terms_are_lowercased_and_stripped_of_edge_punctuation() {
        // The stored side is lowercased by `keywords::extract`, so a query that
        // kept its capitals or its trailing question mark would match nothing.
        assert_eq!(query_terms("Oranges?"), vec!["oranges"]);
        assert_eq!(query_terms("(migration)"), vec!["migration"]);
    }

    #[test]
    fn punctuation_inside_a_word_is_kept() {
        // Splitting on it would turn one term into two that the stored side
        // never produced.
        assert_eq!(query_terms("well-known"), vec!["well-known"]);
    }

    #[test]
    fn an_empty_query_yields_no_terms() {
        assert!(query_terms("").is_empty());
        assert!(query_terms("   ").is_empty());
        assert!(query_terms("!!!").is_empty());
    }

    #[test]
    fn repeated_words_do_not_produce_duplicate_terms() {
        // Duplicates would bind the same SQL variable twice and inflate the
        // match count that becomes the keyword boost.
        assert_eq!(query_terms("milk milk"), vec!["milk", "milk milk"]);
    }

    #[test]
    fn a_very_long_query_is_capped_but_keeps_every_single_word() {
        let words: Vec<String> = (0..300).map(|n| format!("w{n}")).collect();
        let terms = query_terms(&words.join(" "));

        assert_eq!(terms.len(), MAX_QUERY_TERMS);
        assert_eq!(terms[0], "w0");
        assert_eq!(
            terms[MAX_QUERY_TERMS - 1],
            format!("w{}", MAX_QUERY_TERMS - 1),
            "truncation must take from the phrase tail, not from the unigrams"
        );
    }

    fn embedder() -> Embedder {
        Embedder::new(std::env::temp_dir().join("blurt-test-fastembed-cache"))
    }

    async fn vector_store(dir: &tempfile::TempDir) -> VectorStore {
        VectorStore::open(dir.path()).await.unwrap()
    }

    #[tokio::test]
    async fn an_empty_query_returns_nothing_without_loading_the_model() {
        let db = migrated();
        let dir = tempfile::tempdir().unwrap();
        let store = vector_store(&dir).await;
        let mut embedder = embedder();

        for query in ["", "   "] {
            let results =
                hybrid_search(db.conn(), &store, &mut embedder, query, &SearchOptions::default())
                    .await
                    .unwrap();
            assert!(results.is_empty());
        }

        assert!(
            !embedder.is_loaded(),
            "an empty query has nothing to embed; loading ~100MB to find that out is waste"
        );
    }

    #[tokio::test]
    async fn a_zero_limit_returns_nothing_without_loading_the_model() {
        let db = migrated();
        let dir = tempfile::tempdir().unwrap();
        let store = vector_store(&dir).await;
        let mut embedder = embedder();

        let results = hybrid_search(
            db.conn(),
            &store,
            &mut embedder,
            "oranges",
            &SearchOptions {
                limit: 0,
                ..SearchOptions::default()
            },
        )
        .await
        .unwrap();

        assert!(results.is_empty());
        assert!(!embedder.is_loaded());
    }

    /// The real thing: capture, index and retrieve with the actual model, so
    /// that "semantic search finds a paraphrase" is demonstrated rather than
    /// asserted. `#[ignore]`d because it downloads and loads ~100MB.
    #[tokio::test]
    #[ignore = "loads the real embedding model"]
    async fn a_paraphrased_query_finds_the_item_end_to_end() {
        let db = migrated();
        let dir = tempfile::tempdir().unwrap();
        let store = vector_store(&dir).await;
        let mut embedder = embedder();

        let notes = destination(&db, "Notes", None, false);
        let citrus = items::capture(db.conn(), notes, "pick up oranges and lemons").unwrap();
        let car = items::capture(db.conn(), notes, "the car is due for a service").unwrap();
        for item in [citrus.id, car.id] {
            crate::indexing::index_item(db.conn(), &store, &mut embedder, item, None)
                .await
                .unwrap();
        }

        // No word here appears in either capture, so only the semantic half can
        // find it.
        let results = hybrid_search(
            db.conn(),
            &store,
            &mut embedder,
            "buy some citrus fruit",
            &SearchOptions::default(),
        )
        .await
        .unwrap();

        assert_eq!(results[0].item_id, citrus.id);
        assert_eq!(results[0].chunk_text, "pick up oranges and lemons");
    }

    #[tokio::test]
    #[ignore = "loads the real embedding model"]
    async fn a_sensitive_destination_is_unreachable_end_to_end() {
        let db = migrated();
        let dir = tempfile::tempdir().unwrap();
        let store = vector_store(&dir).await;
        let mut embedder = embedder();

        let vault = destination(&db, "Passwords", None, true);
        let secret = items::capture(db.conn(), vault, "gmail password is hunter2").unwrap();
        crate::indexing::index_item(db.conn(), &store, &mut embedder, secret.id, None)
            .await
            .unwrap();

        let results = hybrid_search(
            db.conn(),
            &store,
            &mut embedder,
            "gmail password",
            &SearchOptions::default(),
        )
        .await
        .unwrap();

        assert!(results.is_empty(), "sensitive content is never indexed, so never found");
    }
}
