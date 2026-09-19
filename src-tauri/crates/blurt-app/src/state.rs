//! Shared application state, held behind `tauri::State`.
//!
//! Every field starts empty: the app is locked until `commands::vault` fills
//! them, since `MODULE_02_SCHEMA.md`'s repository layer needs a live, keyed
//! `rusqlite::Connection` that only exists once unlocked.

use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use blurt_rag::embedding::Embedder;
use blurt_rag::model_manager::ModelManager;
use blurt_rag::vectorstore::VectorStore;
use blurt_schema::{Database, Keyring};

use crate::indexer::IndexerHandle;

/// Module 4's three long-lived handles.
///
/// Grouped rather than held as three separate `Option`s because they are
/// meaningless individually: all three exist only once there is a decrypted
/// database to index and search, and they are created and discarded together.
pub struct RagResources {
    pub store: VectorStore,
    /// `Embedder::embed` takes `&mut self`, and the lock is held across an
    /// `.await` in the search path, so this is a `tokio` mutex rather than a
    /// `std` one — a `std` guard is not `Send` and a Tauri async command's
    /// future must be.
    pub embedder: tokio::sync::Mutex<Embedder>,
    /// Holds no model between questions; see `blurt_rag::model_manager`.
    pub manager: ModelManager,
}

/// Where Module 4's files live.
///
/// Passed in rather than derived here so the `_impl` functions stay testable
/// against a temp directory. The `#[tauri::command]` wrappers resolve the real
/// values, and the two model paths come from Tauri's bundled resources
/// (PROGRESS.md decision #51) rather than from the data directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RagPaths {
    /// LanceDB's directory. Lives beside the database, since it is derived
    /// from it and is rebuilt if lost.
    pub vectors: PathBuf,
    /// Where `fastembed` looks for the embedding model.
    pub embedding_cache: PathBuf,
    /// The generative GGUF.
    pub gguf: PathBuf,
}

#[derive(Default)]
pub struct AppState {
    pub db: Mutex<Option<Database>>,
    pub keyring: Mutex<Option<Keyring>>,
    /// Opened lazily on the first search or ask rather than at unlock, because
    /// opening LanceDB is `async` and unlock is not. Behind an `Arc` so a
    /// caller can clone it out, release the lock, and then `await` or
    /// `spawn_blocking` without holding the state lock across either.
    pub rag: tokio::sync::Mutex<Option<std::sync::Arc<RagResources>>>,
    /// Set once, by the app's `setup` hook, when the background worker starts.
    /// Empty in unit tests that do not install one, in which case commands skip
    /// queuing — so a missing worker can never make a capture fail.
    pub indexer: OnceLock<IndexerHandle>,
}

impl AppState {
    pub fn indexer(&self) -> Option<&IndexerHandle> {
        self.indexer.get()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Pure data definition, no logic — exempt from the TDD Red/Green loop
    // per this project's CLAUDE.md ("pure config/data... no logic"). This
    // test just documents the starting-locked invariant.
    #[test]
    fn starts_locked() {
        let state = AppState::default();
        assert!(state.db.lock().unwrap().is_none());
        assert!(state.keyring.lock().unwrap().is_none());
        assert!(state.indexer().is_none());
    }

    #[tokio::test]
    async fn starts_with_no_rag_resources() {
        // Nothing to index or search before there is a decrypted database, and
        // in particular no embedding model loaded at startup.
        let state = AppState::default();
        assert!(state.rag.lock().await.is_none());
    }
}
