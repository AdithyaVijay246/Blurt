//! Embedding generation — `MODULE_04_EMBEDDINGS_RAG.md` §2–3.
//!
//! A thin wrapper over `fastembed` and **bge-small-en-v1.5**, the ~100MB model
//! §2 classifies as effectively always-on: cheap enough to run on every capture
//! and every settled edit without a battery or latency cost worth managing.
//! That is the opposite of the generative model's Sleep-Mode lifecycle, and the
//! two must not be conflated — this one is meant to stay warm.
//!
//! ## Lazy loading
//!
//! [`Embedder::new`] costs nothing; the model loads on the first call that
//! actually has text to embed. Two reasons. It lets `blurt-app` construct an
//! `Embedder` at startup and hold it in `AppState` without paying a model load
//! before the user has captured anything. And it keeps the trivial paths —
//! empty input, an item whose text is only whitespace — testable with no
//! download at all, which is what the non-`#[ignore]`d tests below exercise.
//!
//! ## Where the model files come from
//!
//! `fastembed` fetches from the Hugging Face Hub into a cache directory on
//! first load. `BLUEPRINT.md` §2's bundling requirement is written about the
//! *generative* model, so this one is not strictly covered — but "zero AI setup
//! friction and offline capability from first launch" reads no differently for
//! a model that runs on every single capture. [`Embedder::new`] therefore takes
//! the cache directory rather than defaulting it: pointing it at a Tauri
//! resource directory shipped with the app makes the load offline and the
//! download path dead, with no change here. That bundling is Phase 5's
//! resource-path work, which has to solve the same problem for the GGUF.

use std::path::PathBuf;

use fastembed::{EmbeddingModel, TextEmbedding, TextInitOptions};

use crate::error::{RagError, Result};

/// Dimensionality of a bge-small-en-v1.5 vector.
///
/// The LanceDB table's `FixedSizeList` width is declared from this, so a model
/// swap that changes it is a schema migration, not a config tweak.
pub const EMBEDDING_DIMENSIONS: usize = 384;

/// Generates embeddings, loading the model on first real use.
pub struct Embedder {
    cache_dir: PathBuf,
    model: Option<TextEmbedding>,
}

impl Embedder {
    /// Prepares an embedder without loading anything.
    ///
    /// `cache_dir` is where the model files live — see the module docs on
    /// bundling versus downloading.
    pub fn new(cache_dir: PathBuf) -> Self {
        Self { cache_dir, model: None }
    }

    /// Whether the model is currently resident.
    ///
    /// Exists so the lazy-loading contract can be asserted in tests rather than
    /// merely described.
    pub fn is_loaded(&self) -> bool {
        self.model.is_some()
    }

    /// Embeds each text, in order, one vector of [`EMBEDDING_DIMENSIONS`] per
    /// input.
    ///
    /// An empty batch returns an empty result without loading the model: an
    /// item whose text is blank has nothing to retrieve on, and a zero vector
    /// would be a row that matches every query weakly instead of no query at
    /// all.
    pub fn embed(&mut self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }

        let model = match self.model {
            Some(ref mut model) => model,
            None => {
                let options = TextInitOptions::new(EmbeddingModel::BGESmallENV15)
                    .with_cache_dir(self.cache_dir.clone());
                let loaded = TextEmbedding::try_new(options)
                    .map_err(|e| RagError::EmbeddingModel(e.to_string()))?;
                self.model.insert(loaded)
            }
        };

        model
            .embed(texts, None)
            .map_err(|e| RagError::Embedding(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn embedder() -> Embedder {
        Embedder::new(std::env::temp_dir().join("blurt-test-fastembed-cache"))
    }

    #[test]
    fn constructing_an_embedder_loads_nothing() {
        let embedder = embedder();
        assert!(
            !embedder.is_loaded(),
            "the model must not load until there is text to embed"
        );
    }

    #[test]
    fn an_empty_batch_returns_nothing_without_loading_the_model() {
        let mut embedder = embedder();

        let vectors = embedder.embed(&[]).unwrap();

        assert!(vectors.is_empty());
        assert!(
            !embedder.is_loaded(),
            "an empty batch must not trigger a 100MB model load"
        );
    }

    /// Loads the real model and reaches the network on a cold cache, so it is
    /// not part of the default suite. Run explicitly with
    /// `cargo test -p blurt-rag -- --ignored`.
    #[test]
    #[ignore = "downloads and loads the ~100MB embedding model"]
    fn embeds_a_batch_into_one_vector_per_input() {
        let mut embedder = embedder();
        let texts = vec!["buy tomatoes".to_string(), "the sky was purple".to_string()];

        let vectors = embedder.embed(&texts).unwrap();

        assert_eq!(vectors.len(), 2, "one vector per input, in order");
        for vector in &vectors {
            assert_eq!(vector.len(), EMBEDDING_DIMENSIONS);
            assert!(
                vector.iter().any(|component| *component != 0.0),
                "an all-zero vector means the model did not actually run"
            );
        }
        assert!(embedder.is_loaded());
    }

    /// The mind-map philosophy in §3 only works if near-synonymous phrasings
    /// land near each other — otherwise searching the old wording of an edited
    /// item finds nothing.
    #[test]
    #[ignore = "downloads and loads the ~100MB embedding model"]
    fn related_text_embeds_closer_than_unrelated_text() {
        let mut embedder = embedder();
        let vectors = embedder
            .embed(&[
                "buy tomatoes at the grocery store".to_string(),
                "pick up vegetables from the shop".to_string(),
                "the quarterly tax filing deadline".to_string(),
            ])
            .unwrap();

        let related = cosine(&vectors[0], &vectors[1]);
        let unrelated = cosine(&vectors[0], &vectors[2]);
        assert!(
            related > unrelated,
            "related {related:.3} should outscore unrelated {unrelated:.3}"
        );
    }

    #[test]
    #[ignore = "downloads and loads the ~100MB embedding model"]
    fn a_second_call_reuses_the_already_loaded_model() {
        let mut embedder = embedder();
        embedder.embed(&["first".to_string()]).unwrap();
        assert!(embedder.is_loaded());

        let again = embedder.embed(&["second".to_string()]).unwrap();
        assert_eq!(again.len(), 1);
    }

    fn cosine(a: &[f32], b: &[f32]) -> f32 {
        let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
        let magnitude = |v: &[f32]| v.iter().map(|x| x * x).sum::<f32>().sqrt();
        dot / (magnitude(a) * magnitude(b))
    }
}
