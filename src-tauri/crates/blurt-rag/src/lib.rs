//! Module 4 — embeddings, LanceDB, hybrid retrieval and Sleep-Mode.
//!
//! See `docs/MODULE_04_EMBEDDINGS_RAG.md`.
//!
//! One constraint governs this whole crate: **content from an `isSensitive`
//! destination never enters it**. §3 makes the exclusion absolute — no
//! embedding, no chunking, no keyword extraction — and §10 makes it
//! independent of whether a cloud provider is configured, since content that
//! was never indexed has nothing to send. The check is not repeated by hand
//! here; eligibility is asked of `blurt_schema::repository::indexing`, where
//! it lives in the SQL itself.

pub mod chunking;
pub mod classify;
pub mod embedding;
pub mod error;
pub mod indexing;
pub mod keywords;
pub mod search;
pub mod vectorstore;

pub use error::{RagError, Result};
