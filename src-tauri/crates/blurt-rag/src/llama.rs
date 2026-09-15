//! The llama.cpp-backed generative model — `MODULE_04_EMBEDDINGS_RAG.md` §2.
//!
//! Implements [`ModelLoader`]/[`TextGenerator`] for real inference, so
//! [`crate::model_manager::ModelManager`] can drive an actual GGUF. Everything
//! about *when* the model loads and unloads lives there; this module only knows
//! how to turn a prompt into text.
//!
//! Run in-process rather than through an Ollama sidecar, per
//! `MODULE_01_ARCHITECTURE.md` §1's rule against a second runtime.
//!
//! ## The backend is a process-wide singleton
//!
//! `LlamaBackend::init()` is guarded by a global flag and returns
//! `BackendAlreadyInitialized` on a second call while one is alive. It is also
//! cheap — it sets up llama.cpp's globals and holds none of the weights. So it
//! is initialized once in a [`OnceLock`] and never dropped, while the *model*
//! is what [`ModelManager`](crate::model_manager::ModelManager) loads and drops
//! per question. Owning a backend per loader would make a second `ModelManager`
//! unconstructible, which would break tests before it broke anything else.
//!
//! ## Two details that the published examples get wrong for this use
//!
//! **Sampling index.** `sample(&ctx, -1)` — *not* `0`. llama.cpp's
//! `output_resolve_row` treats a negative index as "last output row", while a
//! non-negative one is a *batch token index* that throws if that token was not
//! configured to produce logits. Since a prompt batch sets `logits = true` only
//! on its final token, passing `0` would abort with `batch.logits[0] != true`.
//!
//! **Detokenization.** `token_to_str` and its `Special` argument are both
//! deprecated; the current entry point is `token_to_piece`, which needs an
//! `encoding_rs::Decoder` for streaming. This module collects raw bytes with
//! `token_to_piece_bytes` and decodes once at the end instead. That avoids a
//! dependency, and is strictly more correct here: a multi-byte character split
//! across two tokens is reassembled by concatenation, where per-token decoding
//! risks emitting replacement characters. Nothing streams — Sleep-Mode returns
//! a whole answer — so there is no reason to decode incrementally.

use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use llama_cpp_2::context::params::LlamaContextParams;
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::model::{AddBos, LlamaModel};
use llama_cpp_2::sampling::LlamaSampler;
use llama_cpp_2::token::LlamaToken;
use llama_cpp_2::TokenToStringError;

use crate::error::{RagError, Result};
use crate::model_manager::{ModelLoader, TextGenerator};

/// Context window, in tokens.
///
/// Sized from what [`crate::synthesis`] actually asks for: a 2400-token context
/// budget plus a 400-token answer plus the instruction scaffolding, with room
/// to spare. Raising the budget without raising this would silently truncate
/// prompts, which is the failure decision #43 chose a token budget to avoid.
pub const CONTEXT_TOKENS: u32 = 4096;

/// First guess at a token's byte length; grown on demand.
const PIECE_BUFFER: usize = 32;

/// The process-wide llama.cpp backend, initialized at most once.
///
/// The error is stored as a `String` rather than the original type because a
/// `OnceLock` hands out shared references and the error must be cloneable into
/// every later caller's [`RagError`].
fn backend() -> Result<&'static LlamaBackend> {
    static BACKEND: OnceLock<std::result::Result<LlamaBackend, String>> = OnceLock::new();

    BACKEND
        .get_or_init(|| LlamaBackend::init().map_err(|e| e.to_string()))
        .as_ref()
        .map_err(|e| RagError::ModelLoad(format!("llama backend init failed: {e}")))
}

/// Loads a GGUF from disk on demand.
pub struct LlamaLoader {
    model_path: PathBuf,
}

impl LlamaLoader {
    /// `model_path` points at the bundled GGUF. Nothing is read until
    /// [`ModelLoader::load`] is called, so constructing this is free and can
    /// happen at startup.
    pub fn new(model_path: PathBuf) -> Self {
        Self { model_path }
    }

    pub fn model_path(&self) -> &Path {
        &self.model_path
    }
}

impl ModelLoader for LlamaLoader {
    fn load(&self) -> Result<Box<dyn TextGenerator>> {
        let backend = backend()?;

        // Checked here rather than left to `load_from_file`, which opens with a
        // `debug_assert!(path.exists())` — in a debug build a missing model
        // would abort the process instead of returning an error, and a missing
        // model is an ordinary condition until the GGUF is bundled.
        if !self.model_path.is_file() {
            return Err(RagError::ModelLoad(format!(
                "no model file at {}",
                self.model_path.display()
            )));
        }

        let model =
            LlamaModel::load_from_file(backend, &self.model_path, &LlamaModelParams::default())
                .map_err(|e| {
                    RagError::ModelLoad(format!("{}: {e}", self.model_path.display()))
                })?;

        Ok(Box::new(LlamaGenerator { model }))
    }
}

/// A loaded GGUF. Dropping it frees the weights, which is what
/// [`ModelManager`](crate::model_manager::ModelManager) relies on to unload.
pub struct LlamaGenerator {
    model: LlamaModel,
}

impl TextGenerator for LlamaGenerator {
    fn generate(&mut self, prompt: &str, max_tokens: usize) -> Result<String> {
        let backend = backend()?;

        // Built per call: `LlamaContext` borrows the model, so the two cannot be
        // stored together. That suits load/generate/unload exactly, and means
        // each answer starts with an empty KV cache rather than inheriting the
        // previous question's.
        let params = LlamaContextParams::default().with_n_ctx(NonZeroU32::new(CONTEXT_TOKENS));
        let mut context = self
            .model
            .new_context(backend, params)
            .map_err(|e| RagError::Generation(format!("could not create context: {e}")))?;

        let tokens = self
            .model
            .str_to_token(prompt, AddBos::Always)
            .map_err(|e| RagError::Generation(format!("could not tokenize the prompt: {e}")))?;

        if tokens.is_empty() {
            return Ok(String::new());
        }
        // Refused rather than truncated. llama.cpp would drop the tail silently,
        // and a grounded answer built from a half-read prompt is worse than an
        // error because nothing downstream could tell.
        if tokens.len() + max_tokens > CONTEXT_TOKENS as usize {
            return Err(RagError::Generation(format!(
                "prompt is {} tokens and {max_tokens} were requested, over the {CONTEXT_TOKENS}-token context",
                tokens.len()
            )));
        }

        let mut batch = LlamaBatch::new(tokens.len(), 1);
        let last = tokens.len() - 1;
        for (position, token) in tokens.iter().enumerate() {
            // Logits only on the final token: the rest are prompt context and
            // nothing samples from them.
            batch
                .add(*token, position as i32, &[0], position == last)
                .map_err(|e| RagError::Generation(format!("could not build the batch: {e}")))?;
        }
        context
            .decode(&mut batch)
            .map_err(|e| RagError::Generation(format!("could not decode the prompt: {e}")))?;

        // Greedy: deterministic, which suits a grounded summarizer and makes a
        // given prompt reproducible. §8 wants a faithful synthesis of the
        // user's own notes, not a creative one.
        let mut sampler = LlamaSampler::greedy();
        let mut bytes: Vec<u8> = Vec::new();

        // The position of each generated token continues the prompt's numbering,
        // so the range *is* the counter rather than a variable tracking one.
        let first = tokens.len() as i32;
        for position in first..first + max_tokens as i32 {
            let token = sampler.sample(&context, -1);
            if self.model.is_eog_token(token) {
                break;
            }

            bytes.extend(self.piece_bytes(token)?);

            batch.clear();
            batch
                .add(token, position, &[0], true)
                .map_err(|e| RagError::Generation(format!("could not extend the batch: {e}")))?;
            context
                .decode(&mut batch)
                .map_err(|e| RagError::Generation(format!("could not decode a token: {e}")))?;
        }

        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }
}

impl LlamaGenerator {
    /// One token's raw bytes, retrying once with the size llama.cpp asks for.
    ///
    /// `token_to_piece_bytes` reports "too small" by returning the required
    /// length negated, so the retry is exact rather than a guess.
    fn piece_bytes(&self, token: LlamaToken) -> Result<Vec<u8>> {
        match self.model.token_to_piece_bytes(token, PIECE_BUFFER, false, None) {
            Err(TokenToStringError::InsufficientBufferSpace(needed)) => self
                .model
                .token_to_piece_bytes(token, needed.unsigned_abs() as usize, false, None)
                .map_err(|e| RagError::Generation(format!("could not decode a token: {e}"))),
            other => other.map_err(|e| RagError::Generation(format!("could not decode a token: {e}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model_manager::ModelManager;

    /// Where an `#[ignore]`d test finds a real GGUF. Nothing is bundled yet,
    /// so the end-to-end tests read this rather than hardcoding a path.
    const MODEL_ENV: &str = "BLURT_TEST_GGUF";

    #[test]
    fn a_missing_model_file_is_an_error_not_a_panic() {
        // `load_from_file` opens with `debug_assert!(path.exists())`, so
        // without the guard in `load` this aborts the test binary in a debug
        // build instead of returning. Until the GGUF is bundled, a missing
        // model is an ordinary condition.
        let loader = LlamaLoader::new(PathBuf::from("no-such-model-anywhere.gguf"));

        let Err(error) = loader.load() else { panic!("expected a load error") };
        assert!(matches!(error, RagError::ModelLoad(_)), "got {error:?}");
        assert!(error.to_string().contains("no-such-model-anywhere.gguf"));
    }

    #[test]
    fn a_directory_is_rejected_like_a_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let loader = LlamaLoader::new(dir.path().to_path_buf());

        let Err(error) = loader.load() else { panic!("expected a load error") };
        assert!(matches!(error, RagError::ModelLoad(_)), "got {error:?}");
    }

    #[test]
    fn a_file_that_is_not_a_gguf_fails_to_load_rather_than_aborting() {
        // Exercises the real `load_from_file` error path — llama.cpp rejects
        // the magic bytes — which is the only way to check that failure maps to
        // `ModelLoad` rather than unwinding out of the FFI boundary.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("not-really.gguf");
        std::fs::write(&path, b"this is not a GGUF file").unwrap();

        let Err(error) = LlamaLoader::new(path).load() else { panic!("expected a load error") };
        assert!(matches!(error, RagError::ModelLoad(_)), "got {error:?}");
    }

    #[test]
    fn the_backend_initializes_once_and_is_reusable() {
        // `LlamaBackend::init()` errors if called twice while one is alive, so
        // the `OnceLock` is load-bearing: without it the second question of a
        // session would fail.
        let first = backend().expect("backend should initialize");
        let second = backend().expect("a second call must reuse, not re-init");

        assert!(std::ptr::eq(first, second), "the backend must be a singleton");
    }

    #[test]
    fn a_failed_load_leaves_the_manager_unloaded() {
        // The real loader driven through the real lifecycle service, with no
        // model present — the state Phase 6 will ship in until the GGUF is
        // bundled, so it must degrade cleanly rather than panic.
        let loader = LlamaLoader::new(PathBuf::from("still-no-model.gguf"));
        let manager = ModelManager::new(Box::new(loader));

        assert!(manager.generate("anything", 64).is_err());
        assert!(!manager.is_loaded());
    }

    #[tokio::test]
    #[ignore = "needs a real GGUF; set BLURT_TEST_GGUF to its path"]
    async fn generates_grounded_text_from_a_real_model() {
        let Ok(path) = std::env::var(MODEL_ENV) else {
            panic!("set {MODEL_ENV} to a GGUF path to run this test");
        };
        let manager = ModelManager::new(Box::new(LlamaLoader::new(PathBuf::from(path))));

        let answer = manager
            .generate(
                "Answer with a single word. What colour is a ripe banana?",
                32,
            )
            .unwrap();

        assert!(!answer.trim().is_empty(), "the model produced nothing");
        assert!(
            answer.to_lowercase().contains("yellow"),
            "expected a usable answer, got: {answer}"
        );
        assert!(!manager.is_loaded(), "§3: unload immediately after");
    }

    #[test]
    #[ignore = "needs a real GGUF; set BLURT_TEST_GGUF to its path"]
    fn an_over_long_prompt_is_refused_rather_than_truncated() {
        let Ok(path) = std::env::var(MODEL_ENV) else {
            panic!("set {MODEL_ENV} to a GGUF path to run this test");
        };
        let Ok(mut generator) = LlamaLoader::new(PathBuf::from(path)).load() else {
            panic!("model should load")
        };

        let huge = "word ".repeat(CONTEXT_TOKENS as usize);
        let error = generator.generate(&huge, 64).unwrap_err();

        assert!(matches!(error, RagError::Generation(_)), "got {error:?}");
        assert!(error.to_string().contains("context"));
    }
}
