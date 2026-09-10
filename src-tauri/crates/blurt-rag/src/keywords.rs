//! Keyword extraction — `MODULE_04_EMBEDDINGS_RAG.md` §3.
//!
//! Runs alongside embedding, using **YAKE**: unsupervised, needing no reference
//! corpus, and well suited to the short-to-medium text Blurt mostly captures.
//! §3 picks it over TF-IDF (which needs a comparison corpus Blurt does not
//! have) and RAKE (noisier phrases).
//!
//! Extracted keywords serve two purposes at once, and both matter to the shape
//! of this module. They go into the `keywords` table for cheap exact-match
//! filtering alongside semantic search (§4's hybrid ranking), and they are
//! **shown to the user as visible tags** — §3 is explicit that this is not a
//! hidden backend mechanism. So the output is ordered by importance rather than
//! returned as a set: the order is what a tag list renders in.
//!
//! Scores come back attached rather than discarded. The `keywords` table stores
//! only the text, but Module 6 has a legitimate use for relative weight when
//! rendering tags, and dropping it here would mean re-running extraction to get
//! it back.
//!
//! **English only.** `yake-rust` selects its stopword list by language and §3
//! does not discuss multilingual capture; a non-English note will still produce
//! keywords, just with English stopwords failing to filter its function words.
//! Worth revisiting if capture ever goes multilingual.

use yake_rust::{Config, StopWords};

/// Most keywords returned for one text.
///
/// §3 sets no number. Ten is an upper bound rather than a target — YAKE returns
/// fewer when a text has fewer distinct phrases to offer, which is the common
/// case for a one-line capture. Whether a short item is worth rendering tags
/// for at all is a display question, and Module 6's to answer.
pub const MAX_KEYWORDS: usize = 10;

/// Longest key phrase, in words.
pub const NGRAM_SIZE: usize = 3;

/// A strict upper bound on how many key phrases YAKE can produce for `text`.
///
/// A text of `W` words holds at most `W` 1-grams, `W-1` 2-grams and `W-2`
/// 3-grams, so `W * NGRAM_SIZE` always exceeds the real candidate count —
/// before YAKE discards the stopword-bearing ones, which shrinks it further.
///
/// Asking for this many is what makes [`extract`] deterministic. A fixed
/// over-fetch would not: it would hold for short captures and quietly fail for
/// a long note, which is exactly where tags matter most.
fn candidate_ceiling(text: &str) -> usize {
    text.split_whitespace()
        .count()
        .saturating_mul(NGRAM_SIZE)
        .max(MAX_KEYWORDS)
}

/// The scale the total order over scores is defined at — see [`ordering_score`].
const ORDER_PRECISION: f64 = 1e9;

/// Rounds a YAKE score to the precision ordering is decided at.
///
/// The second half of decision #23, and the half the first pass missed. Asking
/// YAKE for its whole candidate set stopped *it* truncating arbitrarily, but the
/// scores this crate then sorts on are not themselves stable: YAKE accumulates
/// its statistics in `HashMap` iteration order, and Rust derives a fresh hash
/// seed for every map instance — so two [`extract`] calls on the same text, in
/// the same process, return scores differing in their last bit or two.
///
/// That noise is many orders of magnitude below any real difference in
/// importance, but `total_cmp` faithfully respects it, which is enough to swap
/// two adjacent tags — and at the [`MAX_KEYWORDS`] cut, to change which tag
/// survives at all. Rounding here collapses the noise so genuinely-tied phrases
/// compare equal and the text tiebreak decides, deterministically.
///
/// Only ordering is affected. [`Keyword::score`] still carries the raw value,
/// which is what Module 6 renders relative tag weight from.
fn ordering_score(score: f64) -> f64 {
    (score * ORDER_PRECISION).round()
}

/// An extracted key phrase.
#[derive(Debug, Clone, PartialEq)]
pub struct Keyword {
    /// The phrase, lowercased — `keywords.keyword`. Lowercasing is what makes
    /// the exact-match filtering in §4 case-insensitive without a `LOWER()`
    /// call on every query row.
    pub text: String,
    /// YAKE importance, where **lower is more important** and 0 is the maximum.
    /// Counter-intuitive, and the reason [`extract`] sorts ascending.
    pub score: f64,
}

/// Extracts up to [`MAX_KEYWORDS`] key phrases, most important first.
///
/// Text with nothing to extract from yields an empty list rather than a
/// placeholder tag.
pub fn extract(text: &str) -> Vec<Keyword> {
    if text.trim().is_empty() {
        return Vec::new();
    }

    let config = Config { ngrams: NGRAM_SIZE, ..Config::default() };
    let stop_words =
        StopWords::predefined("en").expect("yake-rust ships a predefined 'en' stopword list");

    // The *whole* candidate set, then cut below. YAKE ties scores often — a
    // single note routinely produces several phrases scoring bit-identically —
    // and it breaks those ties by hash iteration order, which Rust reseeds per
    // process. Any truncation YAKE performs is therefore arbitrary and varies
    // between app launches, so the same note would show different tags on
    // different days. Asking past its candidate ceiling moves every cut to the
    // total order below, which is stable.
    let candidates = yake_rust::get_n_best(candidate_ceiling(text), text, &stop_words, &config);

    let mut keywords: Vec<Keyword> = candidates
        .into_iter()
        .map(|item| Keyword {
            // Already lowercased by yake-rust; re-applied so the invariant the
            // `keywords` table relies on does not depend on that staying true.
            text: item.keyword.to_lowercase(),
            score: item.score,
        })
        .collect();

    // Ascending, because YAKE scores importance downward from 0; then by text,
    // which is the tiebreak that makes the order total rather than incidental.
    // Scores are compared at `ordering_score`'s precision so that float noise
    // between runs cannot pre-empt that tiebreak.
    keywords.sort_by(|a, b| {
        ordering_score(a.score)
            .total_cmp(&ordering_score(b.score))
            .then_with(|| a.text.cmp(&b.text))
    });

    let mut seen = std::collections::HashSet::new();
    keywords.retain(|keyword| seen.insert(keyword.text.clone()));

    keywords.truncate(MAX_KEYWORDS);
    keywords
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOTE: &str = "The quarterly planning meeting covered the migration of the \
        billing service to the new payments platform. Migration timelines were \
        debated at length: the billing service has to keep running throughout, so \
        the payments platform cutover happens in stages rather than all at once. \
        Nobody wanted another billing outage.";

    fn texts(keywords: &[Keyword]) -> Vec<&str> {
        keywords.iter().map(|k| k.text.as_str()).collect()
    }

    #[test]
    fn empty_text_has_no_keywords() {
        assert!(extract("").is_empty());
    }

    #[test]
    fn whitespace_only_text_has_no_keywords() {
        assert!(extract("   \n\t ").is_empty());
    }

    #[test]
    fn extracts_the_salient_terms_of_a_note() {
        let found = texts(&extract(NOTE)).join(" | ");
        assert!(
            found.contains("billing") || found.contains("payments") || found.contains("migration"),
            "none of the note's repeated domain terms surfaced: {found}"
        );
    }

    #[test]
    fn never_returns_more_than_the_cap() {
        assert!(extract(NOTE).len() <= MAX_KEYWORDS);
    }

    #[test]
    fn results_are_ordered_most_important_first() {
        let keywords = extract(NOTE);
        assert!(keywords.len() > 1, "need several keywords to check ordering");

        for pair in keywords.windows(2) {
            assert!(
                pair[0].score <= pair[1].score,
                "YAKE scores ascending means most-important first; got {:?} before {:?}",
                pair[0],
                pair[1]
            );
        }
    }

    #[test]
    fn keywords_are_lowercased_for_exact_match_filtering() {
        for keyword in extract("Quarterly Billing Migration Meeting Notes") {
            assert_eq!(
                keyword.text,
                keyword.text.to_lowercase(),
                "{} is not lowercased",
                keyword.text
            );
        }
    }

    #[test]
    fn keywords_are_deduplicated() {
        let keywords = extract(NOTE);
        let mut seen: Vec<&str> = texts(&keywords);
        let before = seen.len();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), before, "duplicate tags would render twice");
    }

    /// Two values actually observed for the same phrase, from two `extract`
    /// calls in a single process. A difference this small must not be allowed
    /// to decide which of two tags ranks higher.
    #[test]
    fn scores_within_float_noise_order_as_a_tie() {
        let a = 0.004808761140491637_f64;
        let b = 0.004808761140491634_f64;

        assert_ne!(a, b, "precondition: these really are distinct floats");
        assert_eq!(
            ordering_score(a).total_cmp(&ordering_score(b)),
            std::cmp::Ordering::Equal,
            "float noise must not break the tie that text ordering exists to break"
        );
    }

    #[test]
    fn a_real_difference_in_importance_still_orders() {
        // The rounding must not be so coarse that it flattens distinctions that
        // actually matter. These two are adjacent real scores from `NOTE`.
        assert_eq!(
            ordering_score(0.004808761140491637).total_cmp(&ordering_score(0.026774707253466916)),
            std::cmp::Ordering::Less
        );
    }

    #[test]
    fn extraction_is_deterministic() {
        let first = extract(NOTE);
        let second = extract(NOTE);

        // The tags and their order are the guarantee: that is what renders as a
        // tag list, and what the `MAX_KEYWORDS` cut selects from. Scores are
        // compared with a tolerance rather than bit-for-bit — see
        // `ordering_score` for why they carry noise between calls.
        assert_eq!(
            texts(&first),
            texts(&second),
            "the same text must always produce the same tags, in the same order"
        );
        for (a, b) in first.iter().zip(&second) {
            assert!(
                (a.score - b.score).abs() < 1e-12,
                "scores for {:?} drifted further than float noise explains: {} vs {}",
                a.text,
                a.score,
                b.score
            );
        }
    }

    #[test]
    fn a_short_capture_still_works_without_panicking() {
        // Most blurts are one line. Whatever comes back, it must not be more
        // than the text itself can support, and it must not blow up.
        let keywords = extract("buy tomatoes");
        assert!(keywords.len() <= MAX_KEYWORDS);
        for keyword in keywords {
            assert!(!keyword.text.trim().is_empty());
        }
    }


    /// The tie-break is the whole reason extraction is stable across app
    /// launches, so pin it directly rather than trusting the sort call to stay
    /// as written.
    #[test]
    fn tied_scores_are_broken_deterministically_by_text() {
        let keywords = extract(NOTE);

        assert!(
            keywords.windows(2).any(|pair| pair[0].score == pair[1].score),
            "this fixture no longer produces tied scores, so the tie-break is untested"
        );

        for pair in keywords.windows(2) {
            if pair[0].score == pair[1].score {
                assert!(
                    pair[0].text < pair[1].text,
                    "tied scores must order by text, got {:?} before {:?}",
                    pair[0].text,
                    pair[1].text
                );
            }
        }
    }

    /// ~700 words of varied text, well past any fixed over-fetch a previous
    /// version of this module might have used.
    fn long_note() -> String {
        (0..100)
            .map(|i| format!("topic{i} report covering the quarterly billing item{i} in detail"))
            .collect::<Vec<_>>()
            .join(". ")
    }

    /// The premise of [`extract`]'s determinism: if YAKE ever returned exactly
    /// as many candidates as we asked for, it truncated, and *which* of the
    /// tied phrases it dropped would be hash-order dependent again.
    #[test]
    fn yake_never_truncates_at_the_candidate_ceiling() {
        let note = long_note();
        let ceiling = candidate_ceiling(&note);

        let config = Config { ngrams: NGRAM_SIZE, ..Config::default() };
        let stop_words = StopWords::predefined("en").unwrap();
        let candidates = yake_rust::get_n_best(ceiling, &note, &stop_words, &config);

        assert!(
            candidates.len() < ceiling,
            "YAKE returned {} candidates for a ceiling of {ceiling} — the ceiling is not an upper bound",
            candidates.len()
        );
    }

    #[test]
    fn a_long_note_is_ordered_as_deterministically_as_a_short_one() {
        let keywords = extract(&long_note());
        // Not necessarily a full MAX_KEYWORDS: deduplication can collapse the
        // candidate set below the cap, which is fine. The ordering is the
        // property under test.
        assert!(!keywords.is_empty());
        assert!(keywords.len() <= MAX_KEYWORDS);

        for pair in keywords.windows(2) {
            assert!(pair[0].score <= pair[1].score);
            if pair[0].score == pair[1].score {
                assert!(
                    pair[0].text < pair[1].text,
                    "tied scores must order by text, got {:?} before {:?}",
                    pair[0].text,
                    pair[1].text
                );
            }
        }
    }

    #[test]
    fn the_ceiling_never_drops_below_the_number_of_keywords_wanted() {
        assert!(candidate_ceiling("") >= MAX_KEYWORDS);
        assert!(candidate_ceiling("one") >= MAX_KEYWORDS);
    }
}
