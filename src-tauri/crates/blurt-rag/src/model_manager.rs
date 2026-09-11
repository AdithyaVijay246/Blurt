//! Generative-model lifecycle — `MODULE_01_ARCHITECTURE.md` §3 and
//! `MODULE_04_EMBEDDINGS_RAG.md` §2.
//!
//! §3 names this "the one place in the codebase responsible for enforcing
//! Module 4's battery-saving premise", and states the rule it enforces without
//! room for interpretation: **load only on a classified question, unload
//! immediately after, never resident "just in case".** `CLAUDE.md` repeats it
//! as a non-negotiable.
//!
//! So [`ModelManager::generate`] loads, generates, and unloads within a single
//! call, every time. It is not a cache and must not become one. A follow-up
//! question pays the load again — that cost is the deliberate trade §2 makes,
//! because the alternative is several gigabytes resident all day for a feature
//! used a handful of times.
//!
//! ## Why the unload is unconditional
//!
//! The obvious bug in a load/generate/unload sequence is an early return
//! between the second and third step, which leaves the model resident exactly
//! when something has already gone wrong. The unload here happens before the
//! result is propagated, so a failed generation unloads on the way out —
//! guarded by `a_failed_generation_still_unloads` rather than left to review.
//!
//! ## Why there is a trait seam
//!
//! [`TextGenerator`] and [`ModelLoader`] exist so the policy above can be
//! tested without a multi-gigabyte model: every invariant in this file is
//! about *when* the model is loaded and dropped, not about what it generates.
//! The real llama.cpp-backed implementation satisfies the same traits, and its
//! own tests are `#[ignore]`d. Same testability-driven split as the `_impl`
//! pattern in `blurt-app` — see PROGRESS.md decision #13.
//!
//! ## Blocking, not async
//!
//! Generation is CPU-bound blocking work, and [`ModelManager::generate`] is a
//! plain synchronous function guarded by a `std::sync::Mutex`. Callers in
//! `blurt-app` must invoke it inside `tokio::task::spawn_blocking`; holding
//! this lock directly on an async executor thread would stall every other task
//! for the length of an inference. The mutex is deliberately not
//! `tokio::sync::Mutex` — that would invite exactly the `.await`-across-a-
//! long-CPU-burn shape this is meant to avoid.

use std::sync::Mutex;

use crate::error::Result;

/// A loaded generative model, ready to answer one prompt.
///
/// `Send` because the manager stores it inside a `Mutex` shared across threads.
pub trait TextGenerator: Send {
    /// Produces text for `prompt`, stopping at `max_tokens`.
    fn generate(&mut self, prompt: &str, max_tokens: usize) -> Result<String>;
}

/// Loads a generative model on demand.
///
/// Separate from [`TextGenerator`] because loading and generating fail for
/// different reasons and at different costs — and because the manager needs to
/// hold something that can produce a model repeatedly, not a model.
pub trait ModelLoader: Send + Sync {
    fn load(&self) -> Result<Box<dyn TextGenerator>>;
}

/// Owns load/unload state for the generative model — §3's mandated service.
pub struct ModelManager {
    loader: Box<dyn ModelLoader>,
    /// `Some` only for the duration of one [`Self::generate`] call. Between
    /// calls this is always `None`, which is the invariant the whole module
    /// exists to hold.
    loaded: Mutex<Option<Box<dyn TextGenerator>>>,
}

impl ModelManager {
    pub fn new(loader: Box<dyn ModelLoader>) -> Self {
        Self {
            loader,
            loaded: Mutex::new(None),
        }
    }

    /// Whether the model is resident right now.
    ///
    /// Outside a `generate` call this must always be `false`. It exists so that
    /// claim is assertable in a test rather than merely stated here.
    pub fn is_loaded(&self) -> bool {
        self.lock().is_some()
    }

    /// Answers one prompt: load, generate, unload.
    ///
    /// Serialized — a second caller waits rather than loading a second copy of
    /// a multi-gigabyte model alongside the first.
    pub fn generate(&self, prompt: &str, max_tokens: usize) -> Result<String> {
        let mut slot = self.lock();

        // Assigning rather than filling an empty slot: if a previous call
        // unwound mid-generation the slot may still hold a model, and one that
        // was interrupted partway through inference is not one to reuse. The
        // assignment drops it.
        *slot = Some(self.loader.load()?);

        let answer = slot
            .as_mut()
            .expect("just assigned above")
            .generate(prompt, max_tokens);

        // Before propagating `answer`, never after. An early return here is the
        // one mistake that would silently defeat §2's entire premise.
        *slot = None;

        answer
    }

    /// Takes the lock, recovering from poisoning.
    ///
    /// A panic during generation poisons the mutex, and refusing every
    /// subsequent question because one inference panicked would be a worse
    /// outcome than continuing. Nothing here is left inconsistent by a panic:
    /// the only shared state is the model slot, and `generate` overwrites it
    /// before use.
    fn lock(&self) -> std::sync::MutexGuard<'_, Option<Box<dyn TextGenerator>>> {
        self.loaded.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl std::fmt::Debug for ModelManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ModelManager")
            .field("is_loaded", &self.is_loaded())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::RagError;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    /// Shared bookkeeping, so a test can see what the manager did to the model
    /// rather than only what it returned.
    #[derive(Default)]
    struct Journal {
        loads: AtomicUsize,
        drops: AtomicUsize,
        /// Generations in flight — the evidence for serialization.
        in_flight: AtomicUsize,
        peak_in_flight: AtomicUsize,
    }

    struct FakeGenerator {
        journal: Arc<Journal>,
        fail: bool,
    }

    impl TextGenerator for FakeGenerator {
        fn generate(&mut self, prompt: &str, _max_tokens: usize) -> Result<String> {
            let now = self.journal.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
            self.journal.peak_in_flight.fetch_max(now, Ordering::SeqCst);
            // Long enough that genuinely concurrent calls would overlap.
            std::thread::sleep(std::time::Duration::from_millis(20));
            self.journal.in_flight.fetch_sub(1, Ordering::SeqCst);

            if self.fail {
                return Err(RagError::Generation("model exploded".to_string()));
            }
            Ok(format!("answered: {prompt}"))
        }
    }

    impl Drop for FakeGenerator {
        fn drop(&mut self) {
            self.journal.drops.fetch_add(1, Ordering::SeqCst);
        }
    }

    struct FakeLoader {
        journal: Arc<Journal>,
        /// How many of the next loads should fail. Counted down rather than a
        /// flag, so a test can fail one load and succeed on the retry.
        failing_loads: AtomicUsize,
        fail_generate: bool,
    }

    impl ModelLoader for FakeLoader {
        fn load(&self) -> Result<Box<dyn TextGenerator>> {
            self.journal.loads.fetch_add(1, Ordering::SeqCst);

            let remaining = self.failing_loads.load(Ordering::SeqCst);
            if remaining > 0 {
                self.failing_loads.store(remaining - 1, Ordering::SeqCst);
                return Err(RagError::ModelLoad("no such model file".to_string()));
            }

            Ok(Box::new(FakeGenerator {
                journal: Arc::clone(&self.journal),
                fail: self.fail_generate,
            }))
        }
    }

    fn working() -> (ModelManager, Arc<Journal>) {
        build(0, false)
    }

    fn build(failing_loads: usize, fail_generate: bool) -> (ModelManager, Arc<Journal>) {
        let journal = Arc::new(Journal::default());
        let loader = FakeLoader {
            journal: Arc::clone(&journal),
            failing_loads: AtomicUsize::new(failing_loads),
            fail_generate,
        };
        (ModelManager::new(Box::new(loader)), journal)
    }

    #[test]
    fn generate_returns_the_models_output() {
        let (manager, _) = working();
        assert_eq!(
            manager.generate("why is the sky blue", 128).unwrap(),
            "answered: why is the sky blue"
        );
    }

    #[test]
    fn the_model_is_not_resident_before_or_after_a_question() {
        let (manager, journal) = working();

        assert!(!manager.is_loaded(), "nothing should be loaded at rest");
        manager.generate("anything", 128).unwrap();
        assert!(!manager.is_loaded(), "§3: unload immediately after");

        assert_eq!(journal.loads.load(Ordering::SeqCst), 1);
        assert_eq!(
            journal.drops.load(Ordering::SeqCst),
            1,
            "the model must actually be dropped, not merely unreferenced"
        );
    }

    #[test]
    fn every_question_loads_again_because_this_is_not_a_cache() {
        // The point of the whole module. If this ever reads 1, someone has
        // "optimized" it into keeping the model resident and §2's battery
        // premise is gone.
        let (manager, journal) = working();

        manager.generate("first", 128).unwrap();
        manager.generate("second", 128).unwrap();

        assert_eq!(journal.loads.load(Ordering::SeqCst), 2);
        assert_eq!(journal.drops.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn a_failed_generation_still_unloads() {
        // The early-return bug: failing between load and unload is exactly when
        // leaving gigabytes resident would be easiest to miss.
        let (manager, journal) = build(0, true);

        let error = manager.generate("anything", 128).unwrap_err();
        assert!(matches!(error, RagError::Generation(_)), "got {error:?}");

        assert!(!manager.is_loaded(), "a failed answer must not stay loaded");
        assert_eq!(journal.drops.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_failed_load_leaves_nothing_loaded() {
        let (manager, journal) = build(1, false);

        let error = manager.generate("anything", 128).unwrap_err();
        assert!(matches!(error, RagError::ModelLoad(_)), "got {error:?}");

        assert!(!manager.is_loaded());
        assert_eq!(journal.drops.load(Ordering::SeqCst), 0, "nothing was created");
    }

    #[test]
    fn a_later_question_still_works_after_a_failed_load() {
        // A failed load must not wedge the manager: the next question simply
        // tries again. Same manager, so this actually tests recovery rather
        // than the construction of a fresh one.
        let (manager, journal) = build(1, false);

        assert!(manager.generate("first", 128).is_err());
        assert!(
            manager.generate("second", 128).is_ok(),
            "one bad load must not wedge the manager permanently"
        );

        assert_eq!(journal.loads.load(Ordering::SeqCst), 2);
        assert!(!manager.is_loaded());
    }

    #[test]
    fn concurrent_questions_serialize_rather_than_loading_twice_at_once() {
        // Two copies of a multi-gigabyte model resident simultaneously is the
        // failure this mutex exists to prevent.
        let (manager, journal) = working();
        let manager = Arc::new(manager);

        let handles: Vec<_> = (0..4)
            .map(|n| {
                let manager = Arc::clone(&manager);
                std::thread::spawn(move || manager.generate(&format!("q{n}"), 128).unwrap())
            })
            .collect();

        for handle in handles {
            handle.join().unwrap();
        }

        assert_eq!(
            journal.peak_in_flight.load(Ordering::SeqCst),
            1,
            "generations overlapped; the load/generate/unload span is not serialized"
        );
        assert_eq!(journal.loads.load(Ordering::SeqCst), 4);
        assert_eq!(journal.drops.load(Ordering::SeqCst), 4);
        assert!(!manager.is_loaded());
    }
}
