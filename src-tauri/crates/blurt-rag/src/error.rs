//! Error type for `blurt-rag`.

use thiserror::Error;

/// Everything that can go wrong in the memory layer.
///
/// Grows a variant per external dependency as the phases land — the embedding
/// model, the vector store, keyword extraction. Chunking contributes none:
/// it is pure text arithmetic with no failure mode of its own.
#[derive(Debug, Error)]
pub enum RagError {
    #[error("storage error: {0}")]
    Schema(#[from] blurt_schema::SchemaError),

    /// The embedding model could not be loaded — missing or corrupt model
    /// files, or no network on a cold cache. Distinct from [`Self::Embedding`]
    /// because it is the one the "bundle the model" decision fixes.
    #[error("could not load the embedding model: {0}")]
    EmbeddingModel(String),

    /// The model loaded but inference failed.
    #[error("embedding failed: {0}")]
    Embedding(String),

    /// The LanceDB vector store could not be opened, written, or queried.
    ///
    /// Carries a message rather than wrapping `lancedb::Error`, so a LanceDB
    /// version bump cannot change this crate's public error type — the roadmap
    /// flags that API as the one most likely to shift under us.
    #[error("vector store error: {0}")]
    VectorStore(String),

    /// An edit id was handed in that no `edits` row matches. A caller bug
    /// rather than an ordinary skip, so it surfaces rather than passing
    /// quietly as "nothing to index".
    #[error("no such edit: {0}")]
    UnknownEdit(uuid::Uuid),

    /// An edit was handed in that belongs to a different item. Indexing it
    /// would attribute one item's text — and its character offsets — to
    /// another, so it is refused outright.
    #[error("edit {edit_id} belongs to item {actual_item_id}, not {item_id}")]
    EditItemMismatch {
        edit_id: uuid::Uuid,
        item_id: uuid::Uuid,
        actual_item_id: uuid::Uuid,
    },
}

pub type Result<T> = std::result::Result<T, RagError>;
