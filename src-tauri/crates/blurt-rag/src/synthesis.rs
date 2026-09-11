//! Sleep-Mode answer synthesis — `MODULE_04_EMBEDDINGS_RAG.md` §6 and §8.
//!
//! Turns retrieved blurts into a grounded natural-language answer. The
//! retrieval half is [`crate::search`]; the model lifecycle is
//! [`crate::model_manager`]. What lives here is the part in between: choosing
//! which results the model actually sees, and building a prompt that keeps it
//! answering from them rather than from itself.
//!
//! ## Grounding is the whole job
//!
//! §8 sets the standard: the answer is a synthesis over the user's *own*
//! blurts, shown above the real sources so it can be checked. A model that
//! answers from its training data instead has not just been unhelpful, it has
//! quietly invented a memory the user will read as their own. So the prompt
//! says to use only the supplied notes and to say plainly when they do not
//! contain the answer — and the sources travel back with the answer so the UI
//! can show what it was built from.
//!
//! ## Why the search is not called from here
//!
//! [`answer_from_sources`] takes results rather than fetching them, and there
//! is deliberately no all-in-one `answer_question`. Retrieval is `async`
//! (LanceDB), while generation is a long CPU burn behind a blocking mutex, and
//! a single function spanning both would either stall the async executor for
//! the length of an inference or need `tokio` as a runtime dependency of this
//! crate purely to work around itself. Phase 6 composes the two explicitly:
//!
//! ```ignore
//! let sources = search::hybrid_search(conn, store, embedder, question,
//!                                     &synthesis::sleep_mode_options(scope)).await?;
//! let outcome = tokio::task::spawn_blocking(move || {
//!     synthesis::answer_from_sources(&manager, &question, sources, window, now_ms)
//! }).await??;
//! ```
//!
//! That also keeps the expensive half testable without a model, exactly as
//! `search::rank` is testable without one.

use uuid::Uuid;

use crate::error::Result;
use crate::model_manager::ModelManager;
use crate::search::{RetrievalWindow, SearchOptions, SearchResult};

/// Tokens of retrieved context one prompt may spend.
///
/// §11 left the top-N cap undecided; this is the resolution, and it is a token
/// budget rather than a count of results on purpose. Blurts vary from four
/// words to several hundred, so any fixed N is either wasteful on short ones or
/// silently over-long on a handful of real notes — and llama.cpp truncates an
/// over-long prompt without reporting it, which would degrade answers in a way
/// nothing in the app could observe. Spending a budget cannot do that.
///
/// 2400 leaves comfortable room alongside [`ANSWER_TOKEN_LIMIT`] and the
/// instruction scaffolding inside a 4096-token context, and works out to
/// roughly 8-15 sources depending on their length.
pub const CONTEXT_TOKEN_BUDGET: usize = 2400;

/// Longest answer the model may produce, in tokens. Two or three paragraphs —
/// §8 asks for a synthesis, not an essay, and the sources below it carry the
/// detail.
pub const ANSWER_TOKEN_LIMIT: usize = 400;

/// How many results to retrieve before the budget trims them.
///
/// Generous, because [`select_sources`] is what actually decides — this only
/// has to be wide enough that the budget, not the search limit, is the binding
/// constraint.
pub const SEARCH_LIMIT: usize = 30;

/// Estimated BPE tokens per English word.
///
/// The model is not loaded when the prompt is built — loading it to count
/// tokens would invert Sleep-Mode's whole lifecycle — so the budget is spent
/// against an estimate. 1.4 is the conservative end of the ~1.3-1.4 range
/// `chunking.rs` reasons about (decision #21), and erring high is the safe
/// direction: overestimating wastes a little context, underestimating overruns
/// it silently.
const ESTIMATED_TOKENS_PER_WORD: f64 = 1.4;

/// How the UI must present an answer, without dictating its words.
///
/// §11 assigns the exact copy for this to Module 6, so this is an identifier
/// rather than display text — Module 6 maps it to whatever it renders. The
/// point is that it cannot be forgotten: `CLAUDE.md`'s "transparency without
/// interruption" requires generated content to be labeled as generated, and a
/// marker that crosses IPC as structured data is one the frontend has to
/// handle rather than one it might omit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnswerLabel {
    /// Machine-generated synthesis. Rendered distinctly from the sources below
    /// it — §8's "AI summary".
    AiSummary,
}

/// A grounded answer and the blurts it was built from.
#[derive(Debug, Clone, PartialEq)]
pub struct SleepModeAnswer {
    pub answer: String,
    /// Exactly what the model was shown, in the order it was shown them — not
    /// merely everything that matched. §8 requires the user be able to check
    /// the summary against the real underlying blurts, which only works if
    /// these are the ones it actually read.
    pub sources: Vec<SearchResult>,
    pub label: AnswerLabel,
}

/// What one question produced.
#[derive(Debug, Clone, PartialEq)]
pub enum SleepModeOutcome {
    Answered(SleepModeAnswer),
    /// §8's empty state. Nothing matched inside the window, so **the model was
    /// never loaded** — there would be nothing for it to ground an answer in,
    /// and inventing one is the specific failure §8 guards against.
    ///
    /// A distinct variant rather than an empty [`SleepModeAnswer`] because it
    /// is a different screen: §8 pairs it with the widen-the-window action, and
    /// an empty answer string would invite Module 6 to render a blank summary
    /// card instead. The window that came up empty travels with it so the UI
    /// can say what was searched and offer the next step up §5's ladder.
    NothingFound { window: RetrievalWindow },
}

/// §5's Sleep-Mode retrieval defaults: the last 30 days, optionally `@`-scoped.
pub fn sleep_mode_options(scope: Option<Uuid>) -> SearchOptions {
    SearchOptions {
        window: RetrievalWindow::SLEEP_MODE_DEFAULT,
        scope,
        limit: SEARCH_LIMIT,
        offset: 0,
    }
}

/// Answers `question` from `sources`, loading and unloading the model once.
///
/// `sources` must already be ranked — [`crate::search::hybrid_search`] puts
/// in-scope results first, which is what implements §6's "synthesizes from
/// in-scope results first, falling back gracefully to the best elsewhere-match"
/// without a second scoping mechanism here.
pub fn answer_from_sources(
    manager: &ModelManager,
    question: &str,
    sources: Vec<SearchResult>,
    window: RetrievalWindow,
    now_ms: i64,
) -> Result<SleepModeOutcome> {
    let sources = select_sources(sources, CONTEXT_TOKEN_BUDGET);
    if sources.is_empty() {
        return Ok(SleepModeOutcome::NothingFound { window });
    }

    let prompt = build_prompt(question, &sources, now_ms);
    let answer = manager.generate(&prompt, ANSWER_TOKEN_LIMIT)?;

    Ok(SleepModeOutcome::Answered(SleepModeAnswer {
        answer: answer.trim().to_string(),
        sources,
        label: AnswerLabel::AiSummary,
    }))
}

/// Takes results in rank order while the token budget lasts.
///
/// Stops at the first result that does not fit rather than skipping it to fit a
/// smaller one behind it: §4 established the ranking, and reordering by length
/// would quietly promote short blurts over more relevant long ones.
///
/// The highest-ranked result is always included, even if it alone exceeds the
/// budget — answering with no context at all is strictly worse than answering
/// with one long note. In practice this cannot trigger, since `chunking.rs`
/// caps a chunk at 180 words (~252 tokens) and the budget is an order of
/// magnitude larger; it is here so the function has no input that returns
/// nothing from something.
fn select_sources(results: Vec<SearchResult>, budget: usize) -> Vec<SearchResult> {
    let mut selected = Vec::new();
    let mut spent = 0;

    for result in results {
        let cost = estimate_tokens(&result.chunk_text);
        if !selected.is_empty() && spent + cost > budget {
            break;
        }
        spent += cost;
        selected.push(result);
    }

    selected
}

/// Conservative token estimate for a piece of English text.
fn estimate_tokens(text: &str) -> usize {
    let words = text.split_whitespace().count();
    (words as f64 * ESTIMATED_TOKENS_PER_WORD).ceil() as usize
}

/// Builds the grounded-only prompt.
///
/// Sources are numbered so the model can refer to one, and each carries its
/// destination path and age — §7 shows both alongside a result, and a question
/// like "what did I decide about the migration" is often really a question
/// about *when*.
fn build_prompt(question: &str, sources: &[SearchResult], now_ms: i64) -> String {
    let mut prompt = String::new();

    prompt.push_str(
        "You are answering a question using only the personal notes provided below.\n\
         Use nothing except these notes. Do not add outside knowledge, and do not\n\
         guess. If the notes do not contain the answer, say so plainly.\n\
         Answer in a few sentences.\n\n\
         Notes:\n",
    );

    for (index, source) in sources.iter().enumerate() {
        let path = source
            .path
            .iter()
            .map(|destination| destination.name.as_str())
            .collect::<Vec<_>>()
            .join(" / ");

        prompt.push_str(&format!(
            "[{}] ({}, {}) {}\n",
            index + 1,
            if path.is_empty() { "Unfiled" } else { path.as_str() },
            relative_age(source.timestamp, now_ms),
            source.chunk_text.trim(),
        ));
    }

    prompt.push_str(&format!("\nQuestion: {}\nAnswer:", question.trim()));
    prompt
}

/// A blurt's age in words, for the prompt.
///
/// Relative rather than a calendar date, which avoids both a date-formatting
/// dependency and the timezone question a Unix-millisecond timestamp cannot
/// answer on its own: the schema stores UTC milliseconds, and rendering a local
/// civil date would need an offset this crate has no business knowing. "3 days
/// ago" is also the form a recall question is usually asked in.
fn relative_age(timestamp_ms: i64, now_ms: i64) -> String {
    const MS_PER_DAY: i64 = 24 * 60 * 60 * 1000;
    let days = (now_ms - timestamp_ms).div_euclid(MS_PER_DAY);

    match days {
        // Negative: a device whose clock runs ahead, per Module 5. "today" is a
        // better answer than a negative day count.
        i64::MIN..=0 => "today".to_string(),
        1 => "yesterday".to_string(),
        2..=13 => format!("{days} days ago"),
        14..=60 => format!("{} weeks ago", days / 7),
        _ => format!("{} months ago", days / 30),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::RagError;
    use crate::model_manager::{ModelLoader, TextGenerator};
    use crate::search::ScopeGroup;
    use blurt_schema::repository::destinations::{Destination, DestinationKind};
    use std::sync::{Arc, Mutex};

    const NOW: i64 = 1_700_000_000_000;
    const MS_PER_DAY: i64 = 24 * 60 * 60 * 1000;

    /// Records the prompt it was given and returns a canned answer, so tests
    /// can assert on what the model was actually asked.
    struct SpyGenerator {
        seen: Arc<Mutex<Vec<String>>>,
        fail: bool,
    }

    impl TextGenerator for SpyGenerator {
        fn generate(&mut self, prompt: &str, _max_tokens: usize) -> Result<String> {
            self.seen.lock().unwrap().push(prompt.to_string());
            if self.fail {
                return Err(RagError::Generation("nope".to_string()));
            }
            Ok("  you bought oranges  ".to_string())
        }
    }

    struct SpyLoader {
        seen: Arc<Mutex<Vec<String>>>,
        fail: bool,
    }

    impl ModelLoader for SpyLoader {
        fn load(&self) -> Result<Box<dyn TextGenerator>> {
            Ok(Box::new(SpyGenerator {
                seen: Arc::clone(&self.seen),
                fail: self.fail,
            }))
        }
    }

    fn spy(fail: bool) -> (ModelManager, Arc<Mutex<Vec<String>>>) {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let loader = SpyLoader {
            seen: Arc::clone(&seen),
            fail,
        };
        (ModelManager::new(Box::new(loader)), seen)
    }

    fn destination(name: &str) -> Destination {
        Destination {
            id: Uuid::new_v4(),
            parent_id: None,
            name: name.to_string(),
            trigger: name.to_lowercase(),
            kind: DestinationKind::List,
            is_system: false,
            is_sensitive: false,
            sort_order: 0,
            created_at: NOW,
            deleted_at: None,
        }
    }

    fn source(text: &str, path: Vec<&str>, age_days: i64) -> SearchResult {
        SearchResult {
            item_id: Uuid::new_v4(),
            destination_id: Uuid::new_v4(),
            path: path.into_iter().map(destination).collect(),
            edit_id: None,
            chunk_index: 0,
            chunk_start_offset: 0,
            chunk_end_offset: text.chars().count() as i64,
            chunk_text: text.to_string(),
            timestamp: NOW - age_days * MS_PER_DAY,
            score: 0.9,
            group: ScopeGroup::Elsewhere,
        }
    }

    /// A source whose chunk text is `words` words long, for budget arithmetic.
    fn sized_source(words: usize) -> SearchResult {
        let text = (0..words).map(|n| format!("w{n}")).collect::<Vec<_>>().join(" ");
        source(&text, vec!["Notes"], 1)
    }

    #[test]
    fn an_answer_carries_its_sources_and_its_label() {
        let (manager, _) = spy(false);
        let sources = vec![source("pick up oranges", vec!["Shopping"], 2)];

        let outcome =
            answer_from_sources(&manager, "did I buy oranges", sources.clone(), RetrievalWindow::SLEEP_MODE_DEFAULT, NOW)
                .unwrap();

        match outcome {
            SleepModeOutcome::Answered(answer) => {
                assert_eq!(answer.answer, "you bought oranges", "the answer is trimmed");
                assert_eq!(answer.sources, sources);
                assert_eq!(
                    answer.label,
                    AnswerLabel::AiSummary,
                    "generated content must be marked as generated"
                );
            }
            other => panic!("expected an answer, got {other:?}"),
        }
    }

    #[test]
    fn no_sources_means_no_answer_and_no_model_load() {
        // §8's empty state. Loading a multi-gigabyte model to answer from
        // nothing is both wasteful and the exact setup for an invented memory.
        let (manager, seen) = spy(false);

        let outcome = answer_from_sources(
            &manager,
            "did I buy oranges",
            Vec::new(),
            RetrievalWindow::SLEEP_MODE_DEFAULT,
            NOW,
        )
        .unwrap();

        assert_eq!(
            outcome,
            SleepModeOutcome::NothingFound {
                window: RetrievalWindow::SLEEP_MODE_DEFAULT
            }
        );
        assert!(seen.lock().unwrap().is_empty(), "the model must never load");
        assert!(!manager.is_loaded());
    }

    #[test]
    fn the_prompt_forbids_answering_from_anything_but_the_notes() {
        let (manager, seen) = spy(false);
        answer_from_sources(
            &manager,
            "did I buy oranges",
            vec![source("pick up oranges", vec!["Shopping"], 2)],
            RetrievalWindow::SLEEP_MODE_DEFAULT,
            NOW,
        )
        .unwrap();

        let prompt = seen.lock().unwrap()[0].clone();
        assert!(prompt.contains("only the personal notes"), "prompt was:\n{prompt}");
        assert!(prompt.contains("Do not add outside knowledge"), "prompt was:\n{prompt}");
        assert!(
            prompt.contains("say so plainly"),
            "the model needs an explicit out, or it will invent one: \n{prompt}"
        );
    }

    #[test]
    fn the_prompt_carries_the_question_and_every_source() {
        let (manager, seen) = spy(false);
        answer_from_sources(
            &manager,
            "what did I buy",
            vec![
                source("pick up oranges", vec!["Shopping", "Weekly"], 2),
                source("collect the dry cleaning", vec!["Errands"], 9),
            ],
            RetrievalWindow::SLEEP_MODE_DEFAULT,
            NOW,
        )
        .unwrap();

        let prompt = seen.lock().unwrap()[0].clone();
        assert!(prompt.contains("what did I buy"));
        assert!(prompt.contains("pick up oranges"));
        assert!(prompt.contains("collect the dry cleaning"));
        // Numbered, so the model can point at one.
        assert!(prompt.contains("[1]"), "prompt was:\n{prompt}");
        assert!(prompt.contains("[2]"), "prompt was:\n{prompt}");
        // §7's path and timestamp, which a recall question often turns on.
        assert!(prompt.contains("Shopping / Weekly"), "prompt was:\n{prompt}");
        assert!(prompt.contains("2 days ago"), "prompt was:\n{prompt}");
    }

    #[test]
    fn a_failed_generation_propagates_rather_than_becoming_an_empty_answer() {
        // Silently returning "" would render as a blank AI summary card, which
        // reads as "your notes say nothing" rather than "something broke".
        let (manager, _) = spy(true);

        let error = answer_from_sources(
            &manager,
            "did I buy oranges",
            vec![source("pick up oranges", vec!["Shopping"], 2)],
            RetrievalWindow::SLEEP_MODE_DEFAULT,
            NOW,
        )
        .unwrap_err();

        assert!(matches!(error, RagError::Generation(_)), "got {error:?}");
        assert!(!manager.is_loaded(), "and it still unloads");
    }

    #[test]
    fn sources_are_taken_in_rank_order_until_the_budget_runs_out() {
        // 100 words ~= 140 tokens each, so a 300-token budget fits two.
        let results = vec![sized_source(100), sized_source(100), sized_source(100)];
        let selected = select_sources(results, 300);

        assert_eq!(selected.len(), 2);
    }

    #[test]
    fn the_budget_never_reorders_by_length() {
        // A short result behind a too-large one must not jump the queue: rank
        // order is §4's, and length is not a relevance signal.
        let results = vec![sized_source(100), sized_source(500), sized_source(1)];
        let selected = select_sources(results, 300);

        assert_eq!(selected.len(), 1, "selection stops, it does not skip ahead");
        assert_eq!(selected[0].chunk_text.split_whitespace().count(), 100);
    }

    #[test]
    fn the_best_result_is_kept_even_if_it_alone_exceeds_the_budget() {
        let selected = select_sources(vec![sized_source(5000)], 10);
        assert_eq!(selected.len(), 1, "one long note beats no context at all");
    }

    #[test]
    fn an_empty_result_set_selects_nothing() {
        assert!(select_sources(Vec::new(), CONTEXT_TOKEN_BUDGET).is_empty());
    }

    #[test]
    fn token_estimates_err_upward() {
        // Underestimating overruns the context silently, so the estimate must
        // never come in under one token per word.
        assert!(estimate_tokens("one two three four five") >= 5);
        assert_eq!(estimate_tokens(""), 0);
    }

    #[test]
    fn relative_age_reads_the_way_a_recall_question_is_asked() {
        assert_eq!(relative_age(NOW, NOW), "today");
        assert_eq!(relative_age(NOW - MS_PER_DAY, NOW), "yesterday");
        assert_eq!(relative_age(NOW - 3 * MS_PER_DAY, NOW), "3 days ago");
        assert_eq!(relative_age(NOW - 21 * MS_PER_DAY, NOW), "3 weeks ago");
        assert_eq!(relative_age(NOW - 90 * MS_PER_DAY, NOW), "3 months ago");
    }

    #[test]
    fn a_future_timestamp_reads_as_today_rather_than_negative() {
        // Module 5 syncs from devices whose clocks may run ahead.
        assert_eq!(relative_age(NOW + 5 * MS_PER_DAY, NOW), "today");
    }
}
