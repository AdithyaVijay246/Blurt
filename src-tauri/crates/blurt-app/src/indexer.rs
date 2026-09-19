//! Background indexing — `MODULE_04_EMBEDDINGS_RAG.md` §3.
//!
//! §3 asks for two triggers. A capture is "queued as a background job
//! immediately after"; an edit waits "~1–2 seconds after the user stops typing"
//! so that only the settled version is embedded. `blurt-rag::indexing` has no
//! timing state by design, so the scheduling lives here, per
//! `MODULE_01_ARCHITECTURE.md` §3.
//!
//! ## One worker, one job at a time
//!
//! Every request goes through a single task. That is what keeps the scheduling
//! correct without locks: the debounce table has one owner, a newer edit can
//! replace an older one before it runs, and `index_item` — which is not
//! idempotent — is never run twice for the same version by two racing tasks.
//! Embedding is serialized behind the embedder's mutex anyway, so parallel jobs
//! would gain nothing.
//!
//! ## Nothing here is durable
//!
//! The queue is in memory. Whatever it holds at quit, crash, or lock is gone,
//! and the catch-up pass on unlock (`blurt_schema::repository::indexing::
//! pending_index_versions`) is what re-queues it.

use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::time::Duration;

use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TryRecvError;
use tokio::time::Instant;
use uuid::Uuid;

use blurt_rag::indexing::{self, IndexOutcome};

use crate::error::{CommandError, CommandResult};
use crate::state::{AppState, RagPaths};

/// §3's "~1–2 seconds after the user stops typing", taken at the midpoint.
pub const EDIT_QUIET_PERIOD: Duration = Duration::from_millis(1500);

/// One version of one item to index. `edit_id: None` is the original capture.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct IndexJob {
    pub item_id: Uuid,
    pub edit_id: Option<Uuid>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Request {
    /// Run as soon as the worker is free.
    Now(IndexJob),
    /// Run once this item has had no newer edit for [`EDIT_QUIET_PERIOD`].
    AfterQuiet(IndexJob),
    /// Drop everything queued — the vault is locking.
    Clear,
}

/// How commands talk to the worker. Sending never blocks and never fails the
/// caller: a capture must not wait on, or be refused because of, indexing.
#[derive(Debug, Clone)]
pub struct IndexerHandle {
    tx: mpsc::UnboundedSender<Request>,
}

impl IndexerHandle {
    pub fn index_now(&self, job: IndexJob) {
        let _ = self.tx.send(Request::Now(job));
    }

    pub fn index_after_quiet(&self, job: IndexJob) {
        let _ = self.tx.send(Request::AfterQuiet(job));
    }

    pub fn clear(&self) {
        let _ = self.tx.send(Request::Clear);
    }

    /// A handle with no worker behind it, whose requests the test reads back.
    #[cfg(test)]
    pub(crate) fn recording() -> (Self, mpsc::UnboundedReceiver<Request>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (Self { tx }, rx)
    }
}

/// Builds a handle and the worker future that serves it. The caller spawns the
/// worker on whichever runtime it has; it ends when every handle is dropped.
pub fn new_indexer<F, Fut>(run: F) -> (IndexerHandle, impl Future<Output = ()> + Send + 'static)
where
    F: FnMut(IndexJob) -> Fut + Send + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    let (tx, rx) = mpsc::unbounded_channel();
    (IndexerHandle { tx }, worker(rx, run))
}

async fn worker<F, Fut>(mut rx: mpsc::UnboundedReceiver<Request>, mut run: F)
where
    F: FnMut(IndexJob) -> Fut + Send + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    let mut ready: VecDeque<IndexJob> = VecDeque::new();
    // Keyed by item: a newer edit to the same item replaces the waiting one.
    let mut quiet: HashMap<Uuid, (IndexJob, Instant)> = HashMap::new();

    loop {
        // Apply everything already sent before choosing what to run, so a
        // `Clear` sent during a long job takes effect before the next one.
        loop {
            match rx.try_recv() {
                Ok(request) => apply(request, &mut ready, &mut quiet),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return,
            }
        }

        let now = Instant::now();
        let due: Vec<Uuid> = quiet
            .iter()
            .filter(|(_, (_, at))| *at <= now)
            .map(|(item, _)| *item)
            .collect();
        for item in due {
            if let Some((job, _)) = quiet.remove(&item) {
                ready.push_back(job);
            }
        }

        if let Some(job) = ready.pop_front() {
            run(job).await;
            continue;
        }

        let request = match quiet.values().map(|(_, at)| *at).min() {
            Some(deadline) => tokio::select! {
                request = rx.recv() => request,
                _ = tokio::time::sleep_until(deadline) => continue,
            },
            None => rx.recv().await,
        };
        match request {
            Some(request) => apply(request, &mut ready, &mut quiet),
            None => return,
        }
    }
}

fn apply(request: Request, ready: &mut VecDeque<IndexJob>, quiet: &mut HashMap<Uuid, (IndexJob, Instant)>) {
    match request {
        Request::Now(job) => ready.push_back(job),
        Request::AfterQuiet(job) => {
            quiet.insert(job.item_id, (job, Instant::now() + EDIT_QUIET_PERIOD));
        }
        Request::Clear => {
            ready.clear();
            quiet.clear();
        }
    }
}

/// Runs one job against the live vault.
///
/// Three phases, and the database lock is taken separately for the first and
/// the last and never held across either `.await` (decision #59). The cheap
/// eligibility read comes first, so a job for a sensitive or deleted item
/// returns before LanceDB is even opened.
///
/// A locked vault is `Skipped`, not an error: a job queued just before a lock
/// is ordinary, and the catch-up pass on the next unlock re-queues it.
pub(crate) async fn run_job(state: &AppState, paths: &RagPaths, job: IndexJob) -> CommandResult<IndexOutcome> {
    let prepared = {
        let guard = state.db.lock().unwrap();
        let Some(db) = guard.as_ref() else {
            return Ok(IndexOutcome::Skipped);
        };
        indexing::prepare(db.conn(), job.item_id, job.edit_id).map_err(rag_error)?
    };
    let Some(prepared) = prepared else {
        return Ok(IndexOutcome::Skipped);
    };

    let resources = crate::commands::search::rag_resources(state, paths).await?;
    let staged = {
        let mut embedder = resources.embedder.lock().await;
        indexing::embed_and_store(&resources.store, &mut embedder, prepared)
            .await
            .map_err(rag_error)?
    };

    let guard = state.db.lock().unwrap();
    let Some(db) = guard.as_ref() else {
        // Locked mid-pass. The vectors just written are orphans nothing joins
        // to; the catch-up pass re-queues the item on the next unlock.
        return Ok(IndexOutcome::Skipped);
    };
    indexing::commit(db.conn(), staged).map_err(rag_error)
}

fn rag_error(e: blurt_rag::RagError) -> CommandError {
    CommandError::Io(e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use tokio::time::timeout;

    fn job(item: Uuid, edit: Option<Uuid>) -> IndexJob {
        IndexJob { item_id: item, edit_id: edit }
    }

    /// A worker whose runner reports each job it ran back to the test.
    fn start() -> (IndexerHandle, mpsc::UnboundedReceiver<IndexJob>) {
        let (ran_tx, ran_rx) = mpsc::unbounded_channel();
        let (handle, worker) = new_indexer(move |job| {
            let ran_tx = ran_tx.clone();
            async move {
                let _ = ran_tx.send(job);
            }
        });
        tokio::spawn(worker);
        (handle, ran_rx)
    }

    #[tokio::test(start_paused = true)]
    async fn a_capture_is_indexed_without_waiting() {
        let (handle, mut ran) = start();
        let capture = job(Uuid::new_v4(), None);

        handle.index_now(capture);

        assert_eq!(timeout(Duration::from_millis(10), ran.recv()).await.unwrap(), Some(capture));
    }

    #[tokio::test(start_paused = true)]
    async fn an_edit_waits_for_the_quiet_period() {
        let (handle, mut ran) = start();
        let edit = job(Uuid::new_v4(), Some(Uuid::new_v4()));

        handle.index_after_quiet(edit);

        assert!(timeout(Duration::from_millis(1400), ran.recv()).await.is_err(), "ran too early");
        assert_eq!(timeout(Duration::from_millis(200), ran.recv()).await.unwrap(), Some(edit));
    }

    /// §3: "only the settled version gets embedded".
    #[tokio::test(start_paused = true)]
    async fn a_newer_edit_replaces_the_pending_one_and_restarts_the_wait() {
        let (handle, mut ran) = start();
        let item = Uuid::new_v4();
        let first = job(item, Some(Uuid::new_v4()));
        let second = job(item, Some(Uuid::new_v4()));

        handle.index_after_quiet(first);
        tokio::time::sleep(Duration::from_millis(1000)).await;
        handle.index_after_quiet(second);

        // The first edit's deadline (1500ms) has passed, but it was replaced.
        assert!(timeout(Duration::from_millis(1400), ran.recv()).await.is_err());
        assert_eq!(timeout(Duration::from_millis(200), ran.recv()).await.unwrap(), Some(second));
        assert!(
            timeout(Duration::from_secs(5), ran.recv()).await.is_err(),
            "the replaced edit must never run"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn edits_to_different_items_debounce_independently() {
        let (handle, mut ran) = start();
        let a = job(Uuid::new_v4(), Some(Uuid::new_v4()));
        let b = job(Uuid::new_v4(), Some(Uuid::new_v4()));

        handle.index_after_quiet(a);
        handle.index_after_quiet(b);

        let mut got = vec![
            timeout(Duration::from_millis(1600), ran.recv()).await.unwrap().unwrap(),
            timeout(Duration::from_millis(100), ran.recv()).await.unwrap().unwrap(),
        ];
        got.sort_by_key(|j| j.item_id);
        let mut want = vec![a, b];
        want.sort_by_key(|j| j.item_id);
        assert_eq!(got, want);
    }

    #[tokio::test(start_paused = true)]
    async fn clearing_discards_everything_queued() {
        let (handle, mut ran) = start();

        handle.index_after_quiet(job(Uuid::new_v4(), Some(Uuid::new_v4())));
        handle.clear();

        assert!(timeout(Duration::from_secs(5), ran.recv()).await.is_err());
    }

    #[tokio::test(start_paused = true)]
    async fn jobs_run_one_at_a_time() {
        let running = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let (done_tx, mut done) = mpsc::unbounded_channel();

        let (handle, worker) = new_indexer({
            let (running, peak) = (Arc::clone(&running), Arc::clone(&peak));
            move |job| {
                let (running, peak, done_tx) = (Arc::clone(&running), Arc::clone(&peak), done_tx.clone());
                async move {
                    let now = running.fetch_add(1, Ordering::SeqCst) + 1;
                    peak.fetch_max(now, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    running.fetch_sub(1, Ordering::SeqCst);
                    let _ = done_tx.send(job);
                }
            }
        });
        tokio::spawn(worker);

        for _ in 0..3 {
            handle.index_now(job(Uuid::new_v4(), None));
        }
        for _ in 0..3 {
            timeout(Duration::from_secs(1), done.recv()).await.unwrap().unwrap();
        }

        assert_eq!(peak.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn the_worker_stops_once_every_handle_is_dropped() {
        let (handle, worker) = new_indexer(|_job| async {});
        let task = tokio::spawn(worker);

        drop(handle);

        assert!(timeout(Duration::from_secs(1), task).await.is_ok());
    }

    use blurt_schema::repository::{destinations, embeddings, items};
    use blurt_schema::{Database, MasterKey};

    fn unlocked() -> AppState {
        let db = Database::open_in_memory(&MasterKey::generate()).unwrap();
        blurt_schema::migrations::run(db.conn()).unwrap();
        let state = AppState::default();
        *state.db.lock().unwrap() = Some(db);
        state
    }

    /// The model paths need not exist for the non-ignored tests: none of them
    /// reaches the embedder.
    fn paths(dir: &tempfile::TempDir) -> RagPaths {
        RagPaths {
            vectors: dir.path().join("vectors"),
            embedding_cache: std::env::temp_dir().join("blurt-test-fastembed-cache"),
            gguf: dir.path().join("model.gguf"),
        }
    }

    fn capture(state: &AppState, sensitive: bool) -> Uuid {
        let guard = state.db.lock().unwrap();
        let conn = guard.as_ref().unwrap().conn();
        let dest = destinations::create(
            conn, "Somewhere", &format!("t{}", Uuid::new_v4().simple()),
            destinations::DestinationKind::List, None, false, sensitive, 0,
        )
        .unwrap();
        items::capture(conn, dest.id, "oat milk and bread").unwrap().id
    }

    #[tokio::test]
    async fn a_job_on_a_locked_vault_is_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::default();

        let outcome = run_job(&state, &paths(&dir), job(Uuid::new_v4(), None)).await.unwrap();

        assert_eq!(outcome, IndexOutcome::Skipped);
        assert!(state.rag.lock().await.is_none());
    }

    #[tokio::test]
    async fn a_sensitive_items_job_never_opens_the_vector_store() {
        let dir = tempfile::tempdir().unwrap();
        let state = unlocked();
        let secret = capture(&state, true);

        let outcome = run_job(&state, &paths(&dir), job(secret, None)).await.unwrap();

        assert_eq!(outcome, IndexOutcome::Skipped);
        assert!(state.rag.lock().await.is_none(), "LanceDB was opened for a sensitive item");
    }

    #[tokio::test]
    async fn a_job_for_an_item_that_no_longer_exists_is_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let state = unlocked();

        let outcome = run_job(&state, &paths(&dir), job(Uuid::new_v4(), None)).await.unwrap();

        assert_eq!(outcome, IndexOutcome::Skipped);
    }

    /// Loads the real embedding model (downloads it on first run). Run with
    /// `cargo test -p blurt-app -- --ignored --test-threads=1`.
    #[tokio::test]
    #[ignore = "downloads and loads the ~100MB embedding model"]
    async fn a_captured_item_is_embedded_once_its_job_runs() {
        let dir = tempfile::tempdir().unwrap();
        let state = unlocked();
        let item = capture(&state, false);

        let outcome = run_job(&state, &paths(&dir), job(item, None)).await.unwrap();

        assert!(matches!(outcome, IndexOutcome::Indexed { chunks: 1, .. }), "{outcome:?}");
        let guard = state.db.lock().unwrap();
        assert_eq!(embeddings::list_for_item(guard.as_ref().unwrap().conn(), item).unwrap().len(), 1);
    }
}
