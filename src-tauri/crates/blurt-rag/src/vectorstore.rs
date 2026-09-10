//! LanceDB vector store — `MODULE_04_EMBEDDINGS_RAG.md` §3–§4.
//!
//! Vectors live here; everything *about* them — which item and which text
//! version a chunk came from, its character offsets — lives in the SQLCipher
//! `embeddings` table. The two are joined by [`VectorRecord::vector_ref`],
//! which is `embeddings.vectorRef`.
//!
//! ## Why the split, and which side is authoritative
//!
//! SQLCipher is authoritative. LanceDB is an index that can be rebuilt from it,
//! and that asymmetry is deliberate: LanceDB holds no plaintext of its own
//! beyond the vectors, so the encrypted database stays the single place a
//! user's words are stored. A vector with no `embeddings` row is unreachable —
//! nothing joins to it — which is what makes the write order in
//! [`crate::indexing`] safe to interrupt.
//!
//! ## Async
//!
//! `lancedb`'s API is async throughout, so this module is too. That reaches
//! `blurt-app`, whose search and ask commands become `async fn` — supported
//! natively by Tauri v2, and consistent with the Tokio task the indexing
//! debounce needs anyway.

use std::path::Path;
use std::sync::Arc;

use arrow_array::{Array, FixedSizeListArray, RecordBatch, StringArray};
use arrow_array::types::Float32Type;
use arrow_schema::{DataType, Field, Schema};
use futures::TryStreamExt;
use lancedb::query::{ExecutableQuery, QueryBase};
use uuid::Uuid;

use crate::embedding::EMBEDDING_DIMENSIONS;
use crate::error::{RagError, Result};

/// Table holding every chunk vector.
const TABLE_NAME: &str = "embeddings";

/// Join key back to `embeddings.vectorRef`.
const COLUMN_VECTOR_REF: &str = "vectorRef";
/// Denormalized so tombstoning an item is one predicate delete rather than a
/// lookup of every `vectorRef` it owns.
const COLUMN_ITEM_ID: &str = "itemId";
const COLUMN_VECTOR: &str = "vector";

/// One chunk's vector, ready to store.
#[derive(Debug, Clone, PartialEq)]
pub struct VectorRecord {
    /// Matches `embeddings.vectorRef` — the row in SQLCipher that knows what
    /// this vector actually is.
    pub vector_ref: String,
    pub item_id: Uuid,
    pub vector: Vec<f32>,
}

/// A search hit: the stored reference and how far it sat from the query.
#[derive(Debug, Clone, PartialEq)]
pub struct VectorMatch {
    pub vector_ref: String,
    pub item_id: Uuid,
    /// LanceDB's distance — **lower is closer**. Converted to a similarity
    /// score by [`crate::search`], not here.
    pub distance: f32,
}

pub struct VectorStore {
    table: lancedb::table::Table,
}

impl VectorStore {
    /// Opens the store at `dir`, creating the table on first run.
    pub async fn open(dir: &Path) -> Result<Self> {
        let uri = dir.to_string_lossy().to_string();
        let connection = lancedb::connect(&uri)
            .execute()
            .await
            .map_err(|e| RagError::VectorStore(e.to_string()))?;

        let existing = connection
            .table_names()
            .execute()
            .await
            .map_err(|e| RagError::VectorStore(e.to_string()))?;

        let table = if existing.iter().any(|name| name == TABLE_NAME) {
            connection
                .open_table(TABLE_NAME)
                .execute()
                .await
                .map_err(|e| RagError::VectorStore(e.to_string()))?
        } else {
            connection
                .create_empty_table(TABLE_NAME, schema())
                .execute()
                .await
                .map_err(|e| RagError::VectorStore(e.to_string()))?
        };

        Ok(Self { table })
    }

    /// Appends vectors.
    ///
    /// Append, never replace. §3 embeds every version of an item's text and
    /// keeps them all, so indexing an edit adds to what is already stored
    /// rather than superseding it — that is what lets a search for the old
    /// wording still find the item. [`Self::delete_for_item`] is for
    /// tombstoning, not for re-indexing.
    pub async fn add(&self, records: &[VectorRecord]) -> Result<()> {
        if records.is_empty() {
            return Ok(());
        }
        for record in records {
            if record.vector.len() != EMBEDDING_DIMENSIONS {
                return Err(RagError::VectorStore(format!(
                    "vector for {} has {} dimensions, expected {EMBEDDING_DIMENSIONS}",
                    record.vector_ref,
                    record.vector.len()
                )));
            }
        }

        // `Vec<RecordBatch>` is one of the shapes lancedb's `Scannable` accepts
        // directly, so there is no reader to wrap it in.
        self.table
            .add(vec![to_record_batch(records)?])
            .execute()
            .await
            .map(|_| ())
            .map_err(|e| RagError::VectorStore(e.to_string()))
    }

    /// The `limit` nearest vectors to `query`, closest first.
    ///
    /// Distances are **cosine** distances, not LanceDB's default squared L2.
    /// The metric is fixed here rather than left to the caller because
    /// [`crate::search`] has to turn a distance into a relevance score, and
    /// only cosine gives that conversion a stable meaning: `1 - distance` is
    /// the cosine similarity, 1 for an identical direction and 0 for an
    /// orthogonal one. Squared L2 would put an orthogonal pair at 2 and read
    /// back as a negative similarity.
    pub async fn search(&self, query: &[f32], limit: usize) -> Result<Vec<VectorMatch>> {
        if query.len() != EMBEDDING_DIMENSIONS {
            return Err(RagError::VectorStore(format!(
                "query vector has {} dimensions, expected {EMBEDDING_DIMENSIONS}",
                query.len()
            )));
        }
        if limit == 0 {
            return Ok(Vec::new());
        }

        let batches: Vec<RecordBatch> = self
            .table
            .query()
            .nearest_to(query)
            .map_err(|e| RagError::VectorStore(e.to_string()))?
            .distance_type(lancedb::DistanceType::Cosine)
            .limit(limit)
            .execute()
            .await
            .map_err(|e| RagError::VectorStore(e.to_string()))?
            .try_collect()
            .await
            .map_err(|e| RagError::VectorStore(e.to_string()))?;

        let mut matches = Vec::new();
        for batch in &batches {
            matches.extend(from_record_batch(batch)?);
        }
        Ok(matches)
    }

    /// Removes every vector belonging to one item.
    ///
    /// Used both when an item is tombstoned and before re-indexing an edit.
    pub async fn delete_for_item(&self, item_id: Uuid) -> Result<()> {
        // The id is a UUID rendered by `Uuid::to_string`, so it cannot contain
        // a quote; there is nothing here for a crafted value to escape into.
        self.table
            .delete(&format!("{COLUMN_ITEM_ID} = '{item_id}'"))
            .await
            .map(|_| ())
            .map_err(|e| RagError::VectorStore(e.to_string()))
    }

    /// Vectors currently stored. Test and diagnostic use.
    pub async fn count(&self) -> Result<usize> {
        self.table
            .count_rows(None)
            .await
            .map_err(|e| RagError::VectorStore(e.to_string()))
    }
}

fn schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new(COLUMN_VECTOR_REF, DataType::Utf8, false),
        Field::new(COLUMN_ITEM_ID, DataType::Utf8, false),
        Field::new(
            COLUMN_VECTOR,
            DataType::FixedSizeList(
                Arc::new(Field::new("item", DataType::Float32, true)),
                EMBEDDING_DIMENSIONS as i32,
            ),
            false,
        ),
    ]))
}

fn to_record_batch(records: &[VectorRecord]) -> Result<RecordBatch> {
    let refs = StringArray::from(
        records.iter().map(|r| r.vector_ref.clone()).collect::<Vec<_>>(),
    );
    let items = StringArray::from(
        records.iter().map(|r| r.item_id.to_string()).collect::<Vec<_>>(),
    );
    let vectors = FixedSizeListArray::from_iter_primitive::<Float32Type, _, _>(
        records
            .iter()
            .map(|r| Some(r.vector.iter().map(|v| Some(*v)).collect::<Vec<_>>())),
        EMBEDDING_DIMENSIONS as i32,
    );

    RecordBatch::try_new(
        schema(),
        vec![Arc::new(refs), Arc::new(items), Arc::new(vectors)],
    )
    .map_err(|e| RagError::VectorStore(e.to_string()))
}

fn from_record_batch(batch: &RecordBatch) -> Result<Vec<VectorMatch>> {
    let column = |name: &str| -> Result<&StringArray> {
        batch
            .column_by_name(name)
            .and_then(|c| c.as_any().downcast_ref::<StringArray>())
            .ok_or_else(|| RagError::VectorStore(format!("result is missing a {name} column")))
    };

    let refs = column(COLUMN_VECTOR_REF)?;
    let items = column(COLUMN_ITEM_ID)?;
    let distances = batch
        .column_by_name("_distance")
        .and_then(|c| c.as_any().downcast_ref::<arrow_array::Float32Array>())
        .ok_or_else(|| RagError::VectorStore("result is missing a _distance column".to_string()))?;

    let mut matches = Vec::with_capacity(batch.num_rows());
    for row in 0..batch.num_rows() {
        let item_id = Uuid::parse_str(items.value(row))
            .map_err(|e| RagError::VectorStore(format!("stored itemId is not a UUID: {e}")))?;
        matches.push(VectorMatch {
            vector_ref: refs.value(row).to_string(),
            item_id,
            distance: distances.value(row),
        });
    }
    Ok(matches)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A deterministic unit-length-ish vector pointing mostly along `axis`, so
    /// tests can reason about which of two vectors is nearer without a model.
    fn vector(axis: usize) -> Vec<f32> {
        let mut v = vec![0.0; EMBEDDING_DIMENSIONS];
        v[axis % EMBEDDING_DIMENSIONS] = 1.0;
        v
    }

    fn record(vector_ref: &str, item_id: Uuid, axis: usize) -> VectorRecord {
        VectorRecord {
            vector_ref: vector_ref.to_string(),
            item_id,
            vector: vector(axis),
        }
    }

    async fn store(dir: &tempfile::TempDir) -> VectorStore {
        VectorStore::open(dir.path()).await.unwrap()
    }

    #[tokio::test]
    async fn opening_a_fresh_directory_creates_an_empty_table() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(&dir).await;
        assert_eq!(store.count().await.unwrap(), 0);
    }

    #[tokio::test]
    async fn reopening_finds_the_existing_table_rather_than_replacing_it() {
        let dir = tempfile::tempdir().unwrap();
        let item = Uuid::new_v4();
        {
            let store = store(&dir).await;
            store.add(&[record("a", item, 0)]).await.unwrap();
        }

        let reopened = VectorStore::open(dir.path()).await.unwrap();
        assert_eq!(
            reopened.count().await.unwrap(),
            1,
            "reopening must not clobber the stored vectors"
        );
    }

    #[tokio::test]
    async fn adding_nothing_is_a_no_op() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(&dir).await;
        store.add(&[]).await.unwrap();
        assert_eq!(store.count().await.unwrap(), 0);
    }

    #[tokio::test]
    async fn search_returns_the_nearest_vector_first() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(&dir).await;
        let item = Uuid::new_v4();
        store
            .add(&[
                record("near", item, 0),
                record("far", item, 5),
                record("further", item, 9),
            ])
            .await
            .unwrap();

        let hits = store.search(&vector(0), 3).await.unwrap();
        assert_eq!(hits.len(), 3);
        assert_eq!(hits[0].vector_ref, "near");
        for pair in hits.windows(2) {
            assert!(
                pair[0].distance <= pair[1].distance,
                "results must come back closest-first"
            );
        }
    }

    #[tokio::test]
    async fn search_respects_the_limit() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(&dir).await;
        let item = Uuid::new_v4();
        store
            .add(&[record("a", item, 0), record("b", item, 1), record("c", item, 2)])
            .await
            .unwrap();

        assert_eq!(store.search(&vector(0), 2).await.unwrap().len(), 2);
        assert!(store.search(&vector(0), 0).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_hit_carries_back_the_ids_needed_to_join_to_sqlcipher() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(&dir).await;
        let item = Uuid::new_v4();
        store.add(&[record("chunk-0", item, 0)]).await.unwrap();

        let hits = store.search(&vector(0), 1).await.unwrap();
        assert_eq!(hits[0].vector_ref, "chunk-0");
        assert_eq!(hits[0].item_id, item, "without the item id a hit joins to nothing");
    }

    #[tokio::test]
    async fn distance_is_cosine_distance_so_it_can_be_read_as_similarity() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(&dir).await;
        let item = Uuid::new_v4();
        store
            .add(&[record("same", item, 0), record("orthogonal", item, 1)])
            .await
            .unwrap();

        let hits = store.search(&vector(0), 2).await.unwrap();
        let distance = |vector_ref: &str| {
            hits.iter().find(|h| h.vector_ref == vector_ref).unwrap().distance
        };

        // Cosine distance is `1 - cos`, so an identical vector sits at 0 and an
        // orthogonal one at exactly 1. Under LanceDB's default L2 metric the
        // orthogonal pair would come back as 2 instead, which `search.rs` would
        // silently read as a *negative* similarity.
        assert!(distance("same").abs() < 1e-5, "got {}", distance("same"));
        assert!(
            (distance("orthogonal") - 1.0).abs() < 1e-5,
            "got {}",
            distance("orthogonal")
        );
    }

    #[tokio::test]
    async fn deleting_an_item_removes_only_its_own_vectors() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(&dir).await;
        let doomed = Uuid::new_v4();
        let kept = Uuid::new_v4();
        store
            .add(&[
                record("doomed-0", doomed, 0),
                record("doomed-1", doomed, 1),
                record("kept-0", kept, 2),
            ])
            .await
            .unwrap();

        store.delete_for_item(doomed).await.unwrap();

        assert_eq!(store.count().await.unwrap(), 1);
        let remaining = store.search(&vector(2), 5).await.unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].item_id, kept);
    }

    #[tokio::test]
    async fn deleting_an_item_with_no_vectors_is_harmless() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(&dir).await;
        store.delete_for_item(Uuid::new_v4()).await.unwrap();
        assert_eq!(store.count().await.unwrap(), 0);
    }

    #[tokio::test]
    async fn a_wrongly_sized_vector_is_rejected_rather_than_stored() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(&dir).await;
        let bad = VectorRecord {
            vector_ref: "bad".to_string(),
            item_id: Uuid::new_v4(),
            vector: vec![1.0, 2.0, 3.0],
        };

        assert!(store.add(&[bad]).await.is_err());
        assert_eq!(store.count().await.unwrap(), 0, "nothing partial was written");
    }

    #[tokio::test]
    async fn a_wrongly_sized_query_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(&dir).await;
        assert!(store.search(&[1.0, 2.0], 5).await.is_err());
    }
}
