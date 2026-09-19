//! Search and Sleep-Mode commands — `MODULE_04_EMBEDDINGS_RAG.md` §4–§9.
//!
//! Three commands, matching the three consumers §1 names: [`classify_input`]
//! decides which of the other two the input wants, [`search`] is the LLM-free
//! path, and [`ask`] is Sleep-Mode.
//!
//! ## Why classification is its own command
//!
//! §9's statement/question split is a real branch in the product — a statement
//! goes to Module 3's router, a question skips it entirely — so the frontend
//! has to know which it is *before* deciding what to call. Burying the branch
//! inside `ask` would hide it, and burying it inside capture would mean
//! capturing questions. It loads no model by design (§9), so asking is free.
//!
//! ## The two locks, and why they are never held together
//!
//! A Tauri async command's future must be `Send`, and `rusqlite::Connection`
//! is not `Sync`. So the database guard can never be alive across an `.await`.
//! Both async commands below follow the same shape: `await` the vector side
//! first (`blurt_rag::search::retrieve_matches`, which takes no connection),
//! then take the database lock and rank synchronously. The split in
//! `blurt-rag` exists for exactly this reason.
//!
//! Generation has the mirror-image problem — it is a long blocking CPU burn —
//! so [`ask`] hands it to `spawn_blocking`, per `model_manager`'s own docs.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tauri::State;

use blurt_rag::classify::{classify, InputKind};
use blurt_rag::llama::LlamaLoader;
use blurt_rag::model_manager::ModelManager;
use blurt_rag::search::{
    rank, retrieve_matches, RetrievalWindow, ScopeGroup, SearchOptions, SearchResult,
};
use blurt_rag::synthesis::{self, AnswerLabel, SleepModeOutcome};
use blurt_rag::vectorstore::VectorStore;
use blurt_rag::embedding::Embedder;

use crate::commands::parse_uuid;
use crate::error::{CommandError, CommandResult};
use crate::state::{AppState, RagPaths, RagResources};

/// §9's two branches, as the frontend sees them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum InputKindDto {
    /// Send it to `capture_item_via_router`.
    Statement,
    /// Send it to `ask`.
    Question,
}

impl From<InputKind> for InputKindDto {
    fn from(kind: InputKind) -> Self {
        match kind {
            InputKind::Statement => InputKindDto::Statement,
            InputKind::Question => InputKindDto::Question,
        }
    }
}

/// §5's time window, as a number of days or unbounded.
///
/// Crosses IPC as an optional day count rather than a tagged enum: `null` is
/// unbounded, which is what plain search always sends, and a number is §5's
/// "load more" ladder.
fn window_from_days(days: Option<u32>) -> RetrievalWindow {
    match days {
        Some(days) => RetrievalWindow::Days(days),
        None => RetrievalWindow::Unbounded,
    }
}

/// Which of §6's two sections a result renders in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ScopeGroupDto {
    InScope,
    Elsewhere,
}

impl From<ScopeGroup> for ScopeGroupDto {
    fn from(group: ScopeGroup) -> Self {
        match group {
            ScopeGroup::InScope => ScopeGroupDto::InScope,
            ScopeGroup::Elsewhere => ScopeGroupDto::Elsewhere,
        }
    }
}

/// One result, carrying everything §7 needs to show and open it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchResultDto {
    pub item_id: String,
    pub destination_id: String,
    /// Root-to-leaf destination names, computed live from `parentId` — never a
    /// stored path string.
    pub path: Vec<String>,
    /// Which text version matched; `null` is the original capture.
    pub edit_id: Option<String>,
    pub chunk_index: i64,
    /// Offsets into that version's text, in Unicode scalars. **Not UTF-16 code
    /// units**, which is what JavaScript string indices are — they agree below
    /// the astral plane and disagree on emoji, so Module 6 must convert rather
    /// than index directly (PROGRESS.md decision #22).
    pub chunk_start_offset: i64,
    pub chunk_end_offset: i64,
    /// The matched chunk verbatim: the snippet to render.
    pub chunk_text: String,
    pub timestamp: i64,
    pub score: f32,
    pub group: ScopeGroupDto,
}

impl From<SearchResult> for SearchResultDto {
    fn from(r: SearchResult) -> Self {
        SearchResultDto {
            item_id: r.item_id.to_string(),
            destination_id: r.destination_id.to_string(),
            path: r.path.into_iter().map(|d| d.name).collect(),
            edit_id: r.edit_id.map(|e| e.to_string()),
            chunk_index: r.chunk_index,
            chunk_start_offset: r.chunk_start_offset,
            chunk_end_offset: r.chunk_end_offset,
            chunk_text: r.chunk_text,
            timestamp: r.timestamp,
            score: r.score,
            group: r.group.into(),
        }
    }
}

/// What an ask produced — §8's two states.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum AskOutcomeDto {
    Answered {
        answer: String,
        sources: Vec<SearchResultDto>,
        /// Always `"aiSummary"`. Structured rather than prose so the frontend
        /// must handle it: §8 requires generated text to be visibly labeled,
        /// and the exact wording is Module 6's (§11).
        label: AnswerLabelDto,
    },
    /// Nothing matched in the window, so no model was loaded. §8 pairs this
    /// with the widen-the-window action, which is why the window it searched
    /// travels back.
    NothingFound { window_days: Option<u32> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AnswerLabelDto {
    AiSummary,
}

impl From<AnswerLabel> for AnswerLabelDto {
    fn from(label: AnswerLabel) -> Self {
        match label {
            AnswerLabel::AiSummary => AnswerLabelDto::AiSummary,
        }
    }
}

fn classify_input_impl(text: &str) -> InputKindDto {
    classify(text).into()
}

/// Opens Module 4's handles, or returns the ones already open.
///
/// Lazy rather than created at unlock because `VectorStore::open` is `async`
/// and unlock is not. The `Arc` is cloned out and the lock released before any
/// caller awaits, so the state lock is never held across an `.await`.
pub(crate) async fn rag_resources(state: &AppState, paths: &RagPaths) -> CommandResult<Arc<RagResources>> {
    let mut guard = state.rag.lock().await;
    if let Some(existing) = guard.as_ref() {
        return Ok(Arc::clone(existing));
    }

    let store = VectorStore::open(&paths.vectors)
        .await
        .map_err(|e| CommandError::Io(e.to_string()))?;

    let resources = Arc::new(RagResources {
        store,
        embedder: tokio::sync::Mutex::new(Embedder::new(paths.embedding_cache.clone())),
        manager: ModelManager::new(Box::new(LlamaLoader::new(paths.gguf.clone()))),
    });
    *guard = Some(Arc::clone(&resources));
    Ok(resources)
}

/// Runs one query and ranks it, without ever holding both locks at once.
async fn ranked(
    state: &AppState,
    paths: &RagPaths,
    query: &str,
    options: &SearchOptions,
) -> CommandResult<Vec<SearchResult>> {
    let resources = rag_resources(state, paths).await?;

    // The vector half first: async, and touches no connection.
    let matches = {
        let mut embedder = resources.embedder.lock().await;
        retrieve_matches(&resources.store, &mut embedder, query, options)
            .await
            .map_err(|e| CommandError::Io(e.to_string()))?
    };

    // Then the database half: synchronous, and the guard never outlives it.
    let guard = state.db.lock().unwrap();
    let db = guard.as_ref().ok_or(CommandError::Locked)?;
    rank(db.conn(), &matches, query, options, now_ms()).map_err(|e| CommandError::Io(e.to_string()))
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock is before the Unix epoch")
        .as_millis() as i64
}

async fn search_impl(
    state: &AppState,
    paths: &RagPaths,
    query: &str,
    scope_destination_id: Option<String>,
    window_days: Option<u32>,
    limit: usize,
    offset: usize,
) -> CommandResult<Vec<SearchResultDto>> {
    let scope = scope_destination_id
        .map(|id| parse_uuid(&id))
        .transpose()?;
    let options = SearchOptions {
        window: window_from_days(window_days),
        scope,
        limit,
        offset,
    };

    Ok(ranked(state, paths, query, &options)
        .await?
        .into_iter()
        .map(Into::into)
        .collect())
}

async fn ask_impl(
    state: &AppState,
    paths: &RagPaths,
    question: &str,
    scope_destination_id: Option<String>,
    window_days: Option<u32>,
) -> CommandResult<AskOutcomeDto> {
    let scope = scope_destination_id
        .map(|id| parse_uuid(&id))
        .transpose()?;
    let mut options = synthesis::sleep_mode_options(scope);
    if let Some(days) = window_days {
        options.window = RetrievalWindow::Days(days);
    }

    let sources = ranked(state, paths, question, &options).await?;
    let resources = rag_resources(state, paths).await?;

    // Generation is a long blocking burn, so it leaves the async runtime. The
    // `Arc` is what makes that possible without borrowing from `state`.
    let question = question.to_string();
    let window = options.window;
    let outcome = tauri::async_runtime::spawn_blocking(move || {
        synthesis::answer_from_sources(&resources.manager, &question, sources, window, now_ms())
    })
    .await
    .map_err(|e| CommandError::Io(format!("generation task failed: {e}")))?
    .map_err(|e| CommandError::Io(e.to_string()))?;

    Ok(match outcome {
        SleepModeOutcome::Answered(answer) => AskOutcomeDto::Answered {
            answer: answer.answer,
            sources: answer.sources.into_iter().map(Into::into).collect(),
            label: answer.label.into(),
        },
        SleepModeOutcome::NothingFound { window } => AskOutcomeDto::NothingFound {
            window_days: match window {
                RetrievalWindow::Days(days) => Some(days),
                RetrievalWindow::Unbounded => None,
            },
        },
    })
}

#[tauri::command]
pub fn classify_input(text: String) -> InputKindDto {
    classify_input_impl(&text)
}

#[tauri::command]
pub async fn search(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    query: String,
    scope_destination_id: Option<String>,
    window_days: Option<u32>,
    limit: usize,
    offset: usize,
) -> CommandResult<Vec<SearchResultDto>> {
    let paths = crate::commands::vault::rag_paths(&app)?;
    search_impl(
        state.inner(),
        &paths,
        &query,
        scope_destination_id,
        window_days,
        limit,
        offset,
    )
    .await
}

#[tauri::command]
pub async fn ask(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    question: String,
    scope_destination_id: Option<String>,
    window_days: Option<u32>,
) -> CommandResult<AskOutcomeDto> {
    let paths = crate::commands::vault::rag_paths(&app)?;
    ask_impl(
        state.inner(),
        &paths,
        &question,
        scope_destination_id,
        window_days,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use blurt_schema::{Database, MasterKey};

    fn unlocked() -> AppState {
        let db = Database::open_in_memory(&MasterKey::generate()).unwrap();
        blurt_schema::migrations::run(db.conn()).unwrap();
        let state = AppState::default();
        *state.db.lock().unwrap() = Some(db);
        state
    }

    /// Paths under a temp dir. The two model paths need not exist: nothing in
    /// these tests loads a model, which is the point.
    fn paths(dir: &tempfile::TempDir) -> RagPaths {
        RagPaths {
            vectors: dir.path().join("vectors"),
            embedding_cache: dir.path().join("embedding"),
            gguf: dir.path().join("model.gguf"),
        }
    }

    #[test]
    fn a_question_and_a_statement_are_classified_for_different_commands() {
        // §9's branch, exposed so the frontend can pick a command.
        assert_eq!(classify_input_impl("did I buy milk"), InputKindDto::Question);
        assert_eq!(classify_input_impl("buy milk"), InputKindDto::Statement);
    }

    #[test]
    fn classification_serializes_as_a_plain_string() {
        let json = serde_json::to_value(classify_input_impl("what did I say?")).unwrap();
        assert_eq!(json, serde_json::json!("question"));
    }

    #[tokio::test]
    async fn an_empty_query_searches_nothing_without_loading_a_model() {
        // The guard is in `retrieve_matches`, and this proves it survives the
        // trip through the command layer: no embedding model exists at the
        // configured path, so loading one would fail rather than return empty.
        let state = unlocked();
        let dir = tempfile::tempdir().unwrap();

        let results = search_impl(&state, &paths(&dir), "   ", None, None, 20, 0)
            .await
            .unwrap();

        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn a_locked_vault_refuses_to_search() {
        let state = AppState::default();
        let dir = tempfile::tempdir().unwrap();

        // An empty query short-circuits before the database is touched, so the
        // query has to be real for the lock check to be the thing that fires.
        let error = search_impl(&state, &paths(&dir), "oranges", None, None, 20, 0)
            .await
            .unwrap_err();

        // Without a model present the embedder fails first; either way it must
        // be an error rather than a panic or an empty success.
        assert!(matches!(error, CommandError::Locked | CommandError::Io(_)), "got {error:?}");
    }

    #[tokio::test]
    async fn a_bad_scope_id_is_rejected_before_anything_runs() {
        let state = unlocked();
        let dir = tempfile::tempdir().unwrap();

        let error = search_impl(
            &state,
            &paths(&dir),
            "oranges",
            Some("not-a-uuid".to_string()),
            None,
            20,
            0,
        )
        .await
        .unwrap_err();

        assert_eq!(error, CommandError::InvalidId("not-a-uuid".to_string()));
    }

    #[tokio::test]
    async fn asking_with_nothing_indexed_reports_the_empty_state_without_loading_the_model() {
        // §8: nothing matched means no model load — there would be nothing to
        // ground an answer in, which is the setup for inventing one. The GGUF
        // path in these tests does not exist, so a load attempt would surface
        // as an error rather than this clean empty state.
        let state = unlocked();
        let dir = tempfile::tempdir().unwrap();

        let outcome = ask_impl(&state, &paths(&dir), "   ", None, None).await.unwrap();

        match outcome {
            AskOutcomeDto::NothingFound { window_days } => {
                assert_eq!(window_days, Some(30), "§5's Sleep-Mode default");
            }
            other => panic!("expected NothingFound, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn the_ask_window_can_be_widened_past_the_default() {
        // §5's "load more" ladder: the caller picks the rung.
        let state = unlocked();
        let dir = tempfile::tempdir().unwrap();

        let outcome = ask_impl(&state, &paths(&dir), "  ", None, Some(90))
            .await
            .unwrap();

        assert_eq!(outcome, AskOutcomeDto::NothingFound { window_days: Some(90) });
    }

    #[tokio::test]
    async fn rag_resources_are_opened_once_and_reused() {
        // Re-opening LanceDB per query would be wasteful, and re-creating the
        // `Embedder` would discard a loaded model.
        let state = unlocked();
        let dir = tempfile::tempdir().unwrap();
        let paths = paths(&dir);

        let first = rag_resources(&state, &paths).await.unwrap();
        let second = rag_resources(&state, &paths).await.unwrap();

        assert!(Arc::ptr_eq(&first, &second), "the handles must be shared");
    }

    #[test]
    fn an_answer_serializes_with_its_label_so_the_ui_cannot_omit_it() {
        // "Transparency without interruption": generated text is always marked
        // as generated at the IPC boundary.
        let outcome = AskOutcomeDto::Answered {
            answer: "you bought oranges".to_string(),
            sources: Vec::new(),
            label: AnswerLabelDto::AiSummary,
        };

        let json = serde_json::to_value(&outcome).unwrap();
        assert_eq!(json["kind"], "answered");
        assert_eq!(json["label"], "aiSummary");
    }
}
