//! The indexing pass — `MODULE_04_EMBEDDINGS_RAG.md` §3.
//!
//! One text version in, chunks and tags out. [`index_item`] is a plain
//! per-call function with no timing state of its own: §3's debounce ("edits
//! wait ~1–2 seconds after the user stops typing") is a scheduling concern and
//! belongs to `blurt-app`'s Tokio task, not here. That keeps this testable
//! without waiting on a timer.
//!
//! ## Write order
//!
//! LanceDB first, then SQLCipher in a single transaction
//! (`blurt_schema::repository::indexing::store_index_results`). Interrupted
//! between the two, the result is orphaned vectors that nothing joins to —
//! unreachable and harmless, cleared by the next pass over that item. The
//! reverse order would leave `embeddings` rows pointing at vectors that do not
//! exist, which a search would surface as results that cannot be opened.
//!
//! ## Eligibility is asked, never assumed
//!
//! Every pass begins with `is_item_indexable`, which resolves the
//! `isSensitive` rule in SQL. It is re-checked at index time rather than
//! captured when the job was queued, because a debounced pass fires seconds
//! after the edit that scheduled it — long enough for the item to have been
//! moved into a sensitive destination, or for the destination itself to have
//! been marked sensitive.
//!
//! ## Known gap, for Phase 6
//!
//! Marking an *existing* destination sensitive stops future indexing but does
//! not retract what was already indexed. §3 says sensitive content is invisible
//! to this pipeline, so that transition has to purge its items via
//! [`remove_item`]. Doing it is orchestration — it belongs with the command
//! that flips the flag, which does not exist yet.

use rusqlite::Connection;
use uuid::Uuid;

use blurt_schema::repository::embeddings::Embedding;
use blurt_schema::repository::indexing::{is_item_indexable, store_index_results};
use blurt_schema::repository::{edits, embeddings, items, keywords as keyword_rows};

use crate::chunking;
use crate::embedding::Embedder;
use crate::error::{RagError, Result};
use crate::keywords;
use crate::vectorstore::{VectorRecord, VectorStore};

/// What one pass did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexOutcome {
    Indexed { chunks: usize, keywords: usize },
    /// Nothing was written. The item is in an `isSensitive` destination, is
    /// tombstoned, or its destination is — all ordinary, none an error.
    Skipped,
}

/// One version read and chunked under the connection, ready to embed.
///
/// Owns everything it needs and borrows nothing, which is the point: the
/// embedding half that follows is `async` and must not hold a `Connection`.
#[derive(Debug, Clone, PartialEq)]
pub struct PreparedIndex {
    pub item_id: Uuid,
    pub edit_id: Option<Uuid>,
    text: String,
    chunks: Vec<chunking::Chunk>,
}

/// One version embedded and written to LanceDB, waiting for its SQLCipher half.
#[derive(Debug, Clone)]
pub struct StagedIndex {
    pub item_id: Uuid,
    rows: Vec<Embedding>,
    tags: Vec<String>,
}

/// The first, synchronous third of [`index_item`]: eligibility, then the text
/// of the requested version, then its chunks. `None` means skip — the item is
/// sensitive, tombstoned, in a tombstoned destination, or gone.
pub fn prepare(conn: &Connection, item_id: Uuid, edit_id: Option<Uuid>) -> Result<Option<PreparedIndex>> {
    if !is_item_indexable(conn, item_id)? {
        return Ok(None);
    }
    let text = match version_text(conn, item_id, edit_id)? {
        Some(text) => text,
        None => return Ok(None),
    };
    let chunks = chunking::chunk(&text);
    Ok(Some(PreparedIndex { item_id, edit_id, text, chunks }))
}

/// The middle third: embeds and writes vectors. Takes no connection, so its
/// future is `Send` and can run from a Tauri command or a spawned task.
pub async fn embed_and_store(
    store: &VectorStore,
    embedder: &mut Embedder,
    prepared: PreparedIndex,
) -> Result<StagedIndex> {
    let PreparedIndex { item_id, edit_id, text, chunks } = prepared;
    if chunks.is_empty() {
        return Ok(StagedIndex { item_id, rows: Vec::new(), tags: Vec::new() });
    }

    let chunk_texts: Vec<String> = chunks.iter().map(|c| c.text.clone()).collect();
    let vectors = embedder.embed(&chunk_texts)?;
    if vectors.len() != chunks.len() {
        return Err(RagError::Embedding(format!(
            "expected {} vectors for {} chunks, got {}",
            chunks.len(),
            chunks.len(),
            vectors.len()
        )));
    }

    let mut records = Vec::with_capacity(chunks.len());
    let mut rows = Vec::with_capacity(chunks.len());
    for (chunk, vector) in chunks.iter().zip(vectors) {
        let vector_ref = Uuid::new_v4().to_string();
        records.push(VectorRecord {
            vector_ref: vector_ref.clone(),
            item_id,
            vector,
        });
        rows.push(Embedding {
            id: Uuid::new_v4(),
            item_id,
            edit_id,
            vector_ref,
            chunk_index: chunk.index as i64,
            chunk_start_offset: chunk.start_offset as i64,
            chunk_end_offset: chunk.end_offset as i64,
        });
    }

    // Vectors first — see the module docs on write order.
    store.add(&records).await?;

    let tags = keywords::extract(&text).into_iter().map(|k| k.text).collect();
    Ok(StagedIndex { item_id, rows, tags })
}

/// The last third: stores chunk rows and tags, after re-checking eligibility.
pub fn commit(conn: &Connection, staged: StagedIndex) -> Result<IndexOutcome> {
    if staged.rows.is_empty() {
        return Ok(IndexOutcome::Indexed { chunks: 0, keywords: 0 });
    }
    // Eligibility again: embedding happened since `prepare` read it. If the
    // item turned sensitive meanwhile, its vectors are left orphaned — nothing
    // joins to them, and retrieval re-checks per candidate anyway (#36).
    if !is_item_indexable(conn, staged.item_id)? {
        return Ok(IndexOutcome::Skipped);
    }
    store_index_results(conn, staged.item_id, &staged.rows, &staged.tags)?;
    Ok(IndexOutcome::Indexed {
        chunks: staged.rows.len(),
        keywords: staged.tags.len(),
    })
}

/// Indexes one version of one item's text.
///
/// `edit_id` selects the version: `None` is the original capture, `Some(id)`
/// that edit. Each version is embedded separately and none replaces another,
/// per §3 — searching the old wording of an edited item still finds it.
///
/// **Not idempotent.** Calling it twice for the same version stores a second
/// set of chunks. Search dedupes by item, so the user-visible effect is nil,
/// but it wastes space — the scheduler in `blurt-app` owns not double-firing.
///
/// This is the composition of [`prepare`], [`embed_and_store`] and [`commit`],
/// for callers that already hold a connection. It cannot be used from a Tauri
/// command or a spawned task: it holds `conn` across an `.await`, and
/// `Connection` is not `Sync`. Call the three halves instead (decision #59).
pub async fn index_item(
    conn: &Connection,
    store: &VectorStore,
    embedder: &mut Embedder,
    item_id: Uuid,
    edit_id: Option<Uuid>,
) -> Result<IndexOutcome> {
    let Some(prepared) = prepare(conn, item_id, edit_id)? else {
        return Ok(IndexOutcome::Skipped);
    };
    let staged = embed_and_store(store, embedder, prepared).await?;
    commit(conn, staged)
}

/// Removes everything indexed for an item, from both stores.
///
/// A hard delete on both sides. The tombstone rule protects the user's words,
/// which live in `items` and `edits` and are untouched here; an index entry is
/// derived data, and keeping a tombstoned one would mean either filtering it on
/// every search or surfacing deleted text as a result.
pub async fn remove_item(conn: &Connection, store: &VectorStore, item_id: Uuid) -> Result<()> {
    // SQLCipher first: interrupted here, the leftover vectors are unreachable
    // because nothing joins to them. The reverse would leave rows pointing at
    // vectors that no longer exist.
    embeddings::delete_for_item(conn, item_id)?;
    keyword_rows::delete_for_item(conn, item_id)?;
    store.delete_for_item(item_id).await?;
    Ok(())
}

/// The exact text of one version, or `None` if the item has gone.
fn version_text(conn: &Connection, item_id: Uuid, edit_id: Option<Uuid>) -> Result<Option<String>> {
    match edit_id {
        None => Ok(items::get_by_id(conn, item_id)?.map(|item| item.original_text)),
        Some(edit_id) => {
            let edit = edits::get_by_id(conn, edit_id)?
                .ok_or(RagError::UnknownEdit(edit_id))?;
            if edit.item_id != item_id {
                return Err(RagError::EditItemMismatch {
                    edit_id,
                    item_id,
                    actual_item_id: edit.item_id,
                });
            }
            Ok(Some(edit.text))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use blurt_schema::repository::destinations::{self, DestinationKind};
    use blurt_schema::{Database, MasterKey};

    fn migrated() -> Database {
        let db = Database::open_in_memory(&MasterKey::generate()).unwrap();
        blurt_schema::migrations::run(db.conn()).unwrap();
        db
    }

    fn destination(db: &Database, name: &str, trigger: &str, sensitive: bool) -> Uuid {
        destinations::create(
            db.conn(), name, trigger, DestinationKind::List, None, false, sensitive, 0,
        )
        .unwrap()
        .id
    }

    async fn store(dir: &tempfile::TempDir) -> VectorStore {
        VectorStore::open(dir.path()).await.unwrap()
    }

    fn embedder() -> Embedder {
        Embedder::new(std::env::temp_dir().join("blurt-test-fastembed-cache"))
    }

    /// The guarantee this crate cannot get wrong. It needs no model: the
    /// eligibility check comes before anything loads, which is itself the
    /// point — a sensitive item must not even reach the embedder.
    #[tokio::test]
    async fn a_sensitive_destinations_item_is_skipped_before_any_model_loads() {
        let db = migrated();
        let dir = tempfile::tempdir().unwrap();
        let store = store(&dir).await;
        let mut embedder = embedder();

        let secret = destination(&db, "Passwords", "pw", true);
        let item = items::capture(db.conn(), secret, "gmail: hunter2").unwrap();

        let outcome = index_item(db.conn(), &store, &mut embedder, item.id, None)
            .await
            .unwrap();

        assert_eq!(outcome, IndexOutcome::Skipped);
        assert!(!embedder.is_loaded(), "a sensitive item must never reach the model");
        assert_eq!(store.count().await.unwrap(), 0);
        assert!(embeddings::list_for_item(db.conn(), item.id).unwrap().is_empty());
        assert!(keyword_rows::list_for_item(db.conn(), item.id).unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_tombstoned_item_is_skipped() {
        let db = migrated();
        let dir = tempfile::tempdir().unwrap();
        let store = store(&dir).await;
        let mut embedder = embedder();

        let notes = destination(&db, "Notes", "notes", false);
        let item = items::capture(db.conn(), notes, "buy milk").unwrap();
        items::tombstone(db.conn(), item.id).unwrap();

        let outcome = index_item(db.conn(), &store, &mut embedder, item.id, None)
            .await
            .unwrap();

        assert_eq!(outcome, IndexOutcome::Skipped);
        assert!(!embedder.is_loaded());
    }

    #[tokio::test]
    async fn an_item_that_becomes_sensitive_after_capture_is_skipped_on_reindex() {
        let db = migrated();
        let dir = tempfile::tempdir().unwrap();
        let store = store(&dir).await;
        let mut embedder = embedder();

        let notes = destination(&db, "Notes", "notes", false);
        let item = items::capture(db.conn(), notes, "buy milk").unwrap();
        db.conn()
            .execute("UPDATE destinations SET isSensitive = 1 WHERE id = ?1", [notes.to_string()])
            .unwrap();

        let outcome = index_item(db.conn(), &store, &mut embedder, item.id, None)
            .await
            .unwrap();

        assert_eq!(
            outcome,
            IndexOutcome::Skipped,
            "eligibility is re-read at index time, not when the job was queued"
        );
    }

    #[tokio::test]
    async fn an_item_with_no_text_indexes_nothing_without_loading_the_model() {
        let db = migrated();
        let dir = tempfile::tempdir().unwrap();
        let store = store(&dir).await;
        let mut embedder = embedder();

        let notes = destination(&db, "Notes", "notes", false);
        let item = items::capture(db.conn(), notes, "   ").unwrap();

        let outcome = index_item(db.conn(), &store, &mut embedder, item.id, None)
            .await
            .unwrap();

        assert_eq!(outcome, IndexOutcome::Indexed { chunks: 0, keywords: 0 });
        assert!(!embedder.is_loaded());
    }

    #[tokio::test]
    async fn an_unknown_edit_id_is_an_error_not_a_silent_skip() {
        let db = migrated();
        let dir = tempfile::tempdir().unwrap();
        let store = store(&dir).await;
        let mut embedder = embedder();

        let notes = destination(&db, "Notes", "notes", false);
        let item = items::capture(db.conn(), notes, "buy milk").unwrap();

        let outcome =
            index_item(db.conn(), &store, &mut embedder, item.id, Some(Uuid::new_v4())).await;

        assert!(matches!(outcome, Err(RagError::UnknownEdit(_))));
    }

    #[tokio::test]
    async fn an_edit_belonging_to_a_different_item_is_rejected() {
        let db = migrated();
        let dir = tempfile::tempdir().unwrap();
        let store = store(&dir).await;
        let mut embedder = embedder();

        let notes = destination(&db, "Notes", "notes", false);
        let mine = items::capture(db.conn(), notes, "buy milk").unwrap();
        let theirs = items::capture(db.conn(), notes, "buy bread").unwrap();
        let their_edit = edits::append(db.conn(), theirs.id, "buy sourdough").unwrap();

        let outcome =
            index_item(db.conn(), &store, &mut embedder, mine.id, Some(their_edit.id)).await;

        assert!(
            matches!(outcome, Err(RagError::EditItemMismatch { .. })),
            "indexing another item's text under this item would corrupt every offset"
        );
    }

    #[tokio::test]
    async fn removing_an_item_clears_both_stores() {
        let db = migrated();
        let dir = tempfile::tempdir().unwrap();
        let store = store(&dir).await;

        let notes = destination(&db, "Notes", "notes", false);
        let item = items::capture(db.conn(), notes, "buy milk").unwrap();

        // Stand in for a completed pass without loading a model.
        let vector_ref = Uuid::new_v4().to_string();
        store
            .add(&[VectorRecord {
                vector_ref: vector_ref.clone(),
                item_id: item.id,
                vector: vec![0.0; crate::embedding::EMBEDDING_DIMENSIONS],
            }])
            .await
            .unwrap();
        store_index_results(
            db.conn(),
            item.id,
            &[Embedding {
                id: Uuid::new_v4(),
                item_id: item.id,
                edit_id: None,
                vector_ref,
                chunk_index: 0,
                chunk_start_offset: 0,
                chunk_end_offset: 8,
            }],
            &["milk".to_string()],
        )
        .unwrap();

        remove_item(db.conn(), &store, item.id).await.unwrap();

        assert_eq!(store.count().await.unwrap(), 0);
        assert!(embeddings::list_for_item(db.conn(), item.id).unwrap().is_empty());
        assert!(keyword_rows::list_for_item(db.conn(), item.id).unwrap().is_empty());
    }

    #[tokio::test]
    async fn removing_an_item_that_was_never_indexed_is_harmless() {
        let db = migrated();
        let dir = tempfile::tempdir().unwrap();
        let store = store(&dir).await;
        let notes = destination(&db, "Notes", "notes", false);
        let item = items::capture(db.conn(), notes, "buy milk").unwrap();

        remove_item(db.conn(), &store, item.id).await.unwrap();
    }

    // ---- Passes that load the real embedding model ----

    #[tokio::test]
    #[ignore = "downloads and loads the ~100MB embedding model"]
    async fn indexes_a_capture_into_chunks_vectors_and_tags() {
        let db = migrated();
        let dir = tempfile::tempdir().unwrap();
        let store = store(&dir).await;
        let mut embedder = embedder();

        let notes = destination(&db, "Notes", "notes", false);
        let item = items::capture(
            db.conn(),
            notes,
            "The quarterly planning meeting covered the billing service migration.",
        )
        .unwrap();

        let outcome = index_item(db.conn(), &store, &mut embedder, item.id, None)
            .await
            .unwrap();

        let IndexOutcome::Indexed { chunks, keywords } = outcome else {
            panic!("expected an indexed outcome, got {outcome:?}");
        };
        assert_eq!(chunks, 1);
        assert!(keywords > 0, "a real sentence should yield tags");

        let rows = embeddings::list_for_item(db.conn(), item.id).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].edit_id, None, "a capture attributes to the original");
        assert_eq!(store.count().await.unwrap(), 1);

        // The stored offsets must still slice the item's own text.
        let stored = items::get_by_id(db.conn(), item.id).unwrap().unwrap();
        let start = rows[0].chunk_start_offset as usize;
        let end = rows[0].chunk_end_offset as usize;
        let span: String = stored.original_text.chars().skip(start).take(end - start).collect();
        assert_eq!(span, stored.original_text.trim());
    }

    /// §3's per-version rule end to end: an edit adds a version, it does not
    /// replace one.
    #[tokio::test]
    #[ignore = "downloads and loads the ~100MB embedding model"]
    async fn indexing_an_edit_adds_a_version_without_replacing_the_original() {
        let db = migrated();
        let dir = tempfile::tempdir().unwrap();
        let store = store(&dir).await;
        let mut embedder = embedder();

        let notes = destination(&db, "Notes", "notes", false);
        let item = items::capture(db.conn(), notes, "buy tomatoes for the week").unwrap();
        index_item(db.conn(), &store, &mut embedder, item.id, None).await.unwrap();

        let edit = edits::append(db.conn(), item.id, "buy cherry tomatoes for the week").unwrap();
        index_item(db.conn(), &store, &mut embedder, item.id, Some(edit.id))
            .await
            .unwrap();

        let rows = embeddings::list_for_item(db.conn(), item.id).unwrap();
        assert_eq!(rows.len(), 2, "the original wording must stay searchable");
        assert_eq!(rows[0].edit_id, None);
        assert_eq!(rows[1].edit_id, Some(edit.id));
        assert_eq!(store.count().await.unwrap(), 2);
    }

    /// Tags describe the item now, so the later pass replaces them even though
    /// its chunks accumulate.
    #[tokio::test]
    #[ignore = "downloads and loads the ~100MB embedding model"]
    async fn indexing_an_edit_replaces_the_tags() {
        let db = migrated();
        let dir = tempfile::tempdir().unwrap();
        let store = store(&dir).await;
        let mut embedder = embedder();

        let notes = destination(&db, "Notes", "notes", false);
        let item = items::capture(db.conn(), notes, "quarterly billing migration notes").unwrap();
        index_item(db.conn(), &store, &mut embedder, item.id, None).await.unwrap();
        let before = keyword_rows::list_for_item(db.conn(), item.id).unwrap();

        let edit = edits::append(db.conn(), item.id, "annual payroll audit checklist").unwrap();
        index_item(db.conn(), &store, &mut embedder, item.id, Some(edit.id))
            .await
            .unwrap();
        let after = keyword_rows::list_for_item(db.conn(), item.id).unwrap();

        assert_ne!(before, after, "tags must follow the current wording");
    }

    #[test]
    fn prepare_skips_a_sensitive_item() {
        let db = migrated();
        let secret = destination(&db, "Passwords", "pw", true);
        let item = items::capture(db.conn(), secret, "gmail: hunter2").unwrap();

        assert_eq!(prepare(db.conn(), item.id, None).unwrap(), None);
    }

    #[test]
    fn prepare_reads_the_requested_version_not_the_current_text() {
        let db = migrated();
        let notes = destination(&db, "Notes", "notes", false);
        let item = items::capture(db.conn(), notes, "milk").unwrap();
        let first = edits::append(db.conn(), item.id, "oat milk").unwrap();
        edits::append(db.conn(), item.id, "oat milk, 2 cartons").unwrap();

        let prepared = prepare(db.conn(), item.id, Some(first.id)).unwrap().unwrap();

        assert_eq!(prepared.edit_id, Some(first.id));
        assert_eq!(prepared.text, "oat milk");
        assert_eq!(prepared.chunks.len(), 1);
    }

    fn staged_by_hand(item_id: Uuid) -> StagedIndex {
        StagedIndex {
            item_id,
            rows: vec![Embedding {
                id: Uuid::new_v4(),
                item_id,
                edit_id: None,
                vector_ref: Uuid::new_v4().to_string(),
                chunk_index: 0,
                chunk_start_offset: 0,
                chunk_end_offset: 8,
            }],
            tags: vec!["oat milk".to_string()],
        }
    }

    #[test]
    fn commit_stores_rows_and_tags_for_a_still_eligible_item() {
        let db = migrated();
        let notes = destination(&db, "Notes", "notes", false);
        let item = items::capture(db.conn(), notes, "oat milk").unwrap();

        let outcome = commit(db.conn(), staged_by_hand(item.id)).unwrap();

        assert_eq!(outcome, IndexOutcome::Indexed { chunks: 1, keywords: 1 });
        assert_eq!(embeddings::list_for_item(db.conn(), item.id).unwrap().len(), 1);
    }

    /// The split widens the window between reading eligibility and writing —
    /// embedding now happens in between — so commit asks again. Without this,
    /// an item moved into a sensitive destination mid-pass would be indexed.
    #[test]
    fn commit_writes_nothing_for_an_item_that_became_sensitive_after_prepare() {
        let db = migrated();
        let notes = destination(&db, "Notes", "notes", false);
        let item = items::capture(db.conn(), notes, "oat milk").unwrap();
        let staged = staged_by_hand(item.id);

        db.conn()
            .execute("UPDATE destinations SET isSensitive = 1 WHERE id = ?1", [notes.to_string()])
            .unwrap();

        assert_eq!(commit(db.conn(), staged).unwrap(), IndexOutcome::Skipped);
        assert!(embeddings::list_for_item(db.conn(), item.id).unwrap().is_empty());
        assert!(keyword_rows::list_for_item(db.conn(), item.id).unwrap().is_empty());
    }

    /// Compile-time guard, never called: if `embed_and_store` ever starts
    /// holding something `!Send` across an await, this stops compiling — which
    /// is the regression that made the split necessary in the first place.
    #[allow(dead_code)]
    fn embed_and_store_is_send(store: &VectorStore, embedder: &mut Embedder, prepared: PreparedIndex) {
        fn assert_send<T: Send>(_: T) {}
        assert_send(embed_and_store(store, embedder, prepared));
    }
}
