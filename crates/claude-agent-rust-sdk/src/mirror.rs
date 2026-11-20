//! Copies the transcript to a [`SessionStore`] while the session runs.
//!
//! With a store in the options, the CLI runs with `--session-mirror` and
//! emits a `transcript_mirror` frame for every batch of lines it appends to
//! a transcript file. The batcher buffers those entries and appends them to
//! the store: before each result message is delivered, when the output of
//! the CLI ends, when the session closes, and in the background when more
//! than 500 entries or 1 MiB are pending. A failed append, or one whose
//! store panics, is retried; a batch dropped for good is reported as a
//! `mirror_error` system message. The local transcript is already durable,
//! so mirroring never stops the session.
//!
//! The mirror differs from upstream in deliberate ways:
//!
//! - A frame is matched against the projects directory and then against
//!   its real path, so a frame the CLI reports under the resolved path of
//!   a linked config directory (`/private/var` for `/var` on macOS) is
//!   mirrored; Python compares the paths lexically and drops it.
//! - The `mirror_error` of an append that timed out says so, with the
//!   limit in milliseconds; Python reports the empty text of its
//!   `TimeoutError`.
//! - The 1 MiB threshold counts the UTF-8 bytes of the compact frame, the
//!   size an append sends. Python counts the characters of the frame as
//!   `json.dumps` escapes it, about twice as many for Chinese text.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError, Weak};
use std::time::Duration;

use std::panic::AssertUnwindSafe;

use futures_util::FutureExt;
use indexmap::IndexMap;
use serde_json::Value;

use crate::control::{MirrorErrorReporter, TranscriptMirror};
use crate::session_store::{
    SessionKey, SessionStoreEntry, SessionStoreFlushMode, SharedSessionStore,
    file_path_to_session_key,
};
use crate::sessions::{MAX_PENDING_BYTES, MAX_PENDING_ENTRIES};
use crate::types::BoxFuture;

/// Longest a single append may take; a timed-out append is not retried,
/// since it may still land.
pub(crate) const SEND_TIMEOUT: Duration = Duration::from_secs(60);

/// Attempts per batch, and the waits between them.
pub(crate) const APPEND_BACKOFF: [Duration; 2] =
    [Duration::from_millis(200), Duration::from_millis(800)];

struct Pending {
    file_path: String,
    entries: Vec<Value>,
}

#[derive(Default)]
struct Buffer {
    items: Vec<Pending>,
    entries: usize,
    bytes: usize,
    taken: VecDeque<Vec<Pending>>,
}

/// Buffers `transcript_mirror` frames and appends them to a store.
pub(crate) struct TranscriptMirrorBatcher {
    store: SharedSessionStore,
    projects_dirs: Vec<PathBuf>,
    max_pending_entries: usize,
    max_pending_bytes: usize,
    send_timeout: Duration,
    buffer: Mutex<Buffer>,
    flushing: tokio::sync::Mutex<()>,
    reporter: OnceLock<MirrorErrorReporter>,
    this: Weak<Self>,
}

impl TranscriptMirrorBatcher {
    /// A batcher for the projects directory the child writes to. In eager
    /// mode both thresholds are 0, so every frame starts a background flush.
    pub(crate) fn new(
        store: SharedSessionStore,
        projects_dir: PathBuf,
        flush_mode: &SessionStoreFlushMode,
    ) -> Arc<Self> {
        let eager = matches!(flush_mode, SessionStoreFlushMode::Eager);
        Self::with_limits(
            store,
            projects_dir,
            if eager { 0 } else { MAX_PENDING_ENTRIES },
            if eager { 0 } else { MAX_PENDING_BYTES },
            SEND_TIMEOUT,
        )
    }

    pub(crate) fn with_limits(
        store: SharedSessionStore,
        projects_dir: PathBuf,
        max_pending_entries: usize,
        max_pending_bytes: usize,
        send_timeout: Duration,
    ) -> Arc<Self> {
        let canonical = PathBuf::from(crate::sessions::canonicalize_path(&projects_dir));
        let mut projects_dirs = vec![projects_dir];
        if canonical.is_dir() && !projects_dirs.contains(&canonical) {
            projects_dirs.push(canonical);
        }
        Arc::new_cyclic(|this| Self {
            store,
            projects_dirs,
            max_pending_entries,
            max_pending_bytes,
            send_timeout,
            buffer: Mutex::new(Buffer::default()),
            flushing: tokio::sync::Mutex::new(()),
            reporter: OnceLock::new(),
            this: this.clone(),
        })
    }

    fn buffer(&self) -> MutexGuard<'_, Buffer> {
        self.buffer.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Takes what is pending, waits for the flush in progress, then sends
    /// every batch taken so far, its own and those of drains still waiting.
    /// Taking first lets frames keep arriving while a flush runs. Batches
    /// are queued in the order they were taken, under the same lock as the
    /// buffer, so whichever drain holds the flush lock sends them in that
    /// order, and a drain returns only once everything taken before it has
    /// been sent, even when tasks on other threads interleave.
    async fn drain(&self) {
        {
            let mut buffer = self.buffer();
            let items = std::mem::take(&mut buffer.items);
            buffer.entries = 0;
            buffer.bytes = 0;
            if !items.is_empty() {
                buffer.taken.push_back(items);
            }
        }
        let errors = {
            let _serialized = self.flushing.lock().await;
            let items: Vec<Pending> = self.buffer().taken.drain(..).flatten().collect();
            if items.is_empty() {
                return;
            }
            self.send(items).await
        };
        if let Some(reporter) = self.reporter.get() {
            for (key, error) in errors {
                reporter.report(Some(&key), &error);
            }
        }
    }

    async fn send(&self, items: Vec<Pending>) -> Vec<(SessionKey, String)> {
        let mut by_path: IndexMap<String, Vec<SessionStoreEntry>> = IndexMap::new();
        for item in items {
            let bucket = by_path.entry(item.file_path).or_default();
            bucket.extend(item.entries.into_iter().filter_map(|entry| match entry {
                Value::Object(entry) => Some(entry),
                _ => None,
            }));
        }
        let mut errors = Vec::new();
        for (file_path, entries) in by_path {
            if entries.is_empty() {
                continue;
            }
            let file = std::path::Path::new(&file_path);
            let Some(key) = self
                .projects_dirs
                .iter()
                .find_map(|projects| file_path_to_session_key(file, projects))
            else {
                tracing::warn!(
                    "[SessionStore] dropping mirror frame: filePath {file_path} is not under {}; the \
                     CLAUDE_CONFIG_DIR of the child likely differs from this process",
                    self.projects_dirs[0].display()
                );
                continue;
            };
            if let Err(error) = self.append_with_retry(&key, &entries).await {
                tracing::error!("[TranscriptMirrorBatcher] flush failed for {file_path}: {error}");
                errors.push((key, error));
            }
        }
        errors
    }

    async fn append_with_retry(
        &self,
        key: &SessionKey,
        entries: &[SessionStoreEntry],
    ) -> Result<(), String> {
        let mut last_error = String::new();
        for attempt in 0..=APPEND_BACKOFF.len() {
            if attempt > 0 {
                tokio::time::sleep(APPEND_BACKOFF[attempt - 1]).await;
            }
            let append = AssertUnwindSafe(async { self.store.append(key, entries).await });
            match tokio::time::timeout(self.send_timeout, append.catch_unwind()).await {
                Ok(Ok(Ok(()))) => return Ok(()),
                Ok(Ok(Err(error))) => {
                    tracing::debug!(
                        "[TranscriptMirrorBatcher] append attempt {} failed: {error}",
                        attempt + 1
                    );
                    last_error = error.to_string();
                }
                Ok(Err(_)) => {
                    tracing::debug!(
                        "[TranscriptMirrorBatcher] append attempt {} panicked",
                        attempt + 1
                    );
                    last_error = "Session store append panicked".to_owned();
                }
                Err(_) => {
                    return Err(format!(
                        "Session store append timed out after {} ms",
                        self.send_timeout.as_millis()
                    ));
                }
            }
        }
        Err(last_error)
    }
}

impl TranscriptMirror for TranscriptMirrorBatcher {
    fn enqueue(&self, file_path: String, entries: Vec<Value>) {
        let size = serde_json::to_string(&entries).map_or(0, |text| text.len());
        let over = {
            let mut buffer = self.buffer();
            buffer.entries += entries.len();
            buffer.bytes += size;
            buffer.items.push(Pending { file_path, entries });
            buffer.entries > self.max_pending_entries || buffer.bytes > self.max_pending_bytes
        };
        if over
            && let Some(this) = self.this.upgrade()
            && let Ok(runtime) = tokio::runtime::Handle::try_current()
        {
            runtime.spawn(crate::caller_context::in_current_context(async move {
                this.drain().await;
            }));
        }
    }

    fn flush(&self) -> BoxFuture<'_, ()> {
        Box::pin(self.drain())
    }

    fn close(&self) -> BoxFuture<'_, ()> {
        Box::pin(self.drain())
    }

    fn attach(&self, reporter: MirrorErrorReporter) {
        let _ = self.reporter.set(reporter);
    }
}

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use serde_json::json;

    use super::*;
    use crate::session_store::{InMemorySessionStore, SessionStore, SessionStoreError};

    fn projects() -> PathBuf {
        PathBuf::from("/config/projects")
    }

    fn path(session: &str) -> String {
        projects()
            .join("proj")
            .join(format!("{session}.jsonl"))
            .to_string_lossy()
            .into_owned()
    }

    fn entries(count: usize, tag: &str) -> Vec<Value> {
        (0..count)
            .map(|index| json!({"type": "user", "uuid": format!("{tag}-{index}")}))
            .collect()
    }

    #[tokio::test]
    async fn entries_are_grouped_per_file_in_first_seen_order() {
        let store = InMemorySessionStore::new();
        let batcher = TranscriptMirrorBatcher::new(
            SharedSessionStore::new(store.clone()),
            projects(),
            &SessionStoreFlushMode::Batched,
        );
        batcher.enqueue(path("b"), entries(1, "b1"));
        batcher.enqueue(path("a"), entries(2, "a1"));
        batcher.enqueue(path("b"), entries(1, "b2"));
        batcher.enqueue(
            projects()
                .join("proj")
                .join("a")
                .join("subagents")
                .join("agent-x.jsonl")
                .to_string_lossy()
                .into_owned(),
            entries(1, "x"),
        );
        assert!(
            store.get_entries(&SessionKey::new("proj", "a")).is_empty(),
            "batched until a flush"
        );
        batcher.flush().await;
        let uuids = |key: &SessionKey| -> Vec<String> {
            store
                .get_entries(key)
                .iter()
                .map(|entry| entry["uuid"].as_str().unwrap_or_default().to_owned())
                .collect()
        };
        assert_eq!(uuids(&SessionKey::new("proj", "b")), ["b1-0", "b2-0"]);
        assert_eq!(uuids(&SessionKey::new("proj", "a")), ["a1-0", "a1-1"]);
        assert_eq!(
            uuids(&SessionKey::new("proj", "a").with_subpath("subagents/agent-x")),
            ["x-0"]
        );
    }

    #[tokio::test]
    async fn thresholds_start_a_background_flush() {
        let store = InMemorySessionStore::new();
        let batcher = TranscriptMirrorBatcher::new(
            SharedSessionStore::new(store.clone()),
            projects(),
            &SessionStoreFlushMode::Batched,
        );
        batcher.enqueue(path("s"), entries(500, "first"));
        tokio::task::yield_now().await;
        assert!(
            store.get_entries(&SessionKey::new("proj", "s")).is_empty(),
            "500 is not above the threshold"
        );
        batcher.enqueue(path("s"), entries(1, "second"));
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }
        assert_eq!(store.get_entries(&SessionKey::new("proj", "s")).len(), 501);

        let big =
            vec![json!({"type": "user", "uuid": "big", "data": "x".repeat(MAX_PENDING_BYTES)})];
        batcher.enqueue(path("big"), big);
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }
        assert_eq!(store.get_entries(&SessionKey::new("proj", "big")).len(), 1);
    }

    #[tokio::test]
    async fn the_byte_threshold_counts_the_utf8_bytes_of_the_compact_frame() {
        let store = InMemorySessionStore::new();
        let batcher = TranscriptMirrorBatcher::new(
            SharedSessionStore::new(store.clone()),
            projects(),
            &SessionStoreFlushMode::Batched,
        );
        let text = "\u{4f60}".repeat(200_000);
        let escaped = serde_json::to_string(&text).unwrap().len();
        assert!(escaped < MAX_PENDING_BYTES, "{escaped} UTF-8 bytes");
        assert!(
            text.chars().count() * 6 > MAX_PENDING_BYTES,
            "ASCII-escaped"
        );
        batcher.enqueue(
            path("cjk"),
            vec![json!({"type": "user", "uuid": "c", "data": text})],
        );
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }
        assert!(
            store
                .get_entries(&SessionKey::new("proj", "cjk"))
                .is_empty(),
            "600 KB of UTF-8 is under the threshold"
        );
        batcher.enqueue(
            path("cjk"),
            vec![json!({"type": "user", "uuid": "d", "data": "\u{4f60}".repeat(200_000)})],
        );
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }
        assert_eq!(store.get_entries(&SessionKey::new("proj", "cjk")).len(), 2);
    }

    #[tokio::test]
    async fn eager_mode_flushes_every_frame() {
        let store = InMemorySessionStore::new();
        let batcher = TranscriptMirrorBatcher::new(
            SharedSessionStore::new(store.clone()),
            projects(),
            &SessionStoreFlushMode::Eager,
        );
        assert_eq!(
            (batcher.max_pending_entries, batcher.max_pending_bytes),
            (0, 0)
        );
        batcher.enqueue(path("s"), entries(1, "e"));
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }
        assert_eq!(store.get_entries(&SessionKey::new("proj", "s")).len(), 1);
    }

    struct FlakyStore {
        failures_left: AtomicUsize,
        attempts: AtomicUsize,
        delay: Option<Duration>,
        inner: InMemorySessionStore,
        timeline: Mutex<Vec<tokio::time::Instant>>,
    }

    impl FlakyStore {
        fn new(failures: usize, delay: Option<Duration>) -> Self {
            Self {
                failures_left: AtomicUsize::new(failures),
                attempts: AtomicUsize::new(0),
                delay,
                inner: InMemorySessionStore::new(),
                timeline: Mutex::new(Vec::new()),
            }
        }
    }

    impl SessionStore for FlakyStore {
        async fn append(
            &self,
            key: &SessionKey,
            entries: &[SessionStoreEntry],
        ) -> Result<(), SessionStoreError> {
            self.attempts.fetch_add(1, Ordering::SeqCst);
            self.timeline
                .lock()
                .unwrap()
                .push(tokio::time::Instant::now());
            if let Some(delay) = self.delay {
                tokio::time::sleep(delay).await;
            }
            if self.failures_left.load(Ordering::SeqCst) > 0 {
                self.failures_left.fetch_sub(1, Ordering::SeqCst);
                return Err("store is down".into());
            }
            self.inner.append(key, entries).await
        }

        async fn load(
            &self,
            key: &SessionKey,
        ) -> Result<Option<Vec<SessionStoreEntry>>, SessionStoreError> {
            self.inner.load(key).await
        }
    }

    #[derive(Clone)]
    struct Shared(Arc<FlakyStore>);

    impl SessionStore for Shared {
        fn append(
            &self,
            key: &SessionKey,
            entries: &[SessionStoreEntry],
        ) -> impl Future<Output = Result<(), SessionStoreError>> + Send {
            let store = Arc::clone(&self.0);
            let key = key.clone();
            let entries = entries.to_vec();
            async move { store.append(&key, &entries).await }
        }

        fn load(
            &self,
            key: &SessionKey,
        ) -> impl Future<Output = Result<Option<Vec<SessionStoreEntry>>, SessionStoreError>> + Send
        {
            let store = Arc::clone(&self.0);
            let key = key.clone();
            async move { store.load(&key).await }
        }
    }

    #[tokio::test(start_paused = true)]
    async fn failed_appends_are_retried_after_200_then_800_ms() {
        let flaky = Arc::new(FlakyStore::new(2, None));
        let batcher = TranscriptMirrorBatcher::new(
            SharedSessionStore::new(Shared(Arc::clone(&flaky))),
            projects(),
            &SessionStoreFlushMode::Batched,
        );
        batcher.enqueue(path("s"), entries(1, "r"));
        batcher.flush().await;
        assert_eq!(flaky.attempts.load(Ordering::SeqCst), 3);
        let timeline = flaky.timeline.lock().unwrap().clone();
        assert_eq!(timeline[1] - timeline[0], Duration::from_millis(200));
        assert_eq!(timeline[2] - timeline[1], Duration::from_millis(800));
        assert_eq!(
            flaky.inner.get_entries(&SessionKey::new("proj", "s")).len(),
            1
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_batch_dropped_after_three_attempts_is_reported() {
        let flaky = Arc::new(FlakyStore::new(10, None));
        let batcher = TranscriptMirrorBatcher::new(
            SharedSessionStore::new(Shared(Arc::clone(&flaky))),
            projects(),
            &SessionStoreFlushMode::Batched,
        );
        let (sender, mut receiver) = tokio::sync::mpsc::channel(4);
        batcher.attach(MirrorErrorReporter::new(sender.downgrade()));
        batcher.enqueue(path("s"), entries(1, "d"));
        batcher.flush().await;
        assert_eq!(flaky.attempts.load(Ordering::SeqCst), 3);
        let message = receiver.try_recv().unwrap().unwrap();
        assert_eq!(message["subtype"], "mirror_error");
        assert_eq!(message["error"], "store is down");
        assert_eq!(
            message["key"],
            json!({"project_key": "proj", "session_id": "s"})
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_timed_out_append_is_not_retried() {
        let flaky = Arc::new(FlakyStore::new(0, Some(Duration::from_secs(61))));
        let batcher = TranscriptMirrorBatcher::new(
            SharedSessionStore::new(Shared(Arc::clone(&flaky))),
            projects(),
            &SessionStoreFlushMode::Batched,
        );
        let (sender, mut receiver) = tokio::sync::mpsc::channel(4);
        batcher.attach(MirrorErrorReporter::new(sender.downgrade()));
        batcher.enqueue(path("s"), entries(1, "t"));
        batcher.flush().await;
        assert_eq!(flaky.attempts.load(Ordering::SeqCst), 1);
        let message = receiver.try_recv().unwrap().unwrap();
        assert!(
            message["error"]
                .as_str()
                .unwrap()
                .contains("timed out after 60000 ms")
        );
    }

    #[tokio::test]
    async fn paths_outside_the_projects_directory_are_dropped() {
        let store = InMemorySessionStore::new();
        let batcher = TranscriptMirrorBatcher::new(
            SharedSessionStore::new(store.clone()),
            projects(),
            &SessionStoreFlushMode::Batched,
        );
        batcher.enqueue("/elsewhere/proj/s.jsonl".to_owned(), entries(1, "o"));
        batcher.enqueue(path("s"), Vec::new());
        batcher.flush().await;
        assert_eq!(store.size(), 0);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_frame_under_the_real_path_of_a_linked_projects_directory_is_mirrored() {
        let root = tempfile::tempdir().unwrap();
        let real = root.path().join("real");
        std::fs::create_dir_all(real.join("projects")).unwrap();
        let link = root.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let store = InMemorySessionStore::new();
        let batcher = TranscriptMirrorBatcher::new(
            SharedSessionStore::new(store.clone()),
            link.join("projects"),
            &SessionStoreFlushMode::Batched,
        );
        let canonical = PathBuf::from(crate::sessions::canonicalize_path(&real));
        let file = canonical.join("projects").join("proj").join("s.jsonl");
        batcher.enqueue(file.to_string_lossy().into_owned(), entries(1, "r"));
        batcher.flush().await;
        assert_eq!(store.get_entries(&SessionKey::new("proj", "s")).len(), 1);
    }

    fn counting() -> (Arc<FlakyStore>, Arc<TranscriptMirrorBatcher>) {
        let store = Arc::new(FlakyStore::new(0, None));
        let batcher = TranscriptMirrorBatcher::new(
            SharedSessionStore::new(Shared(Arc::clone(&store))),
            projects(),
            &SessionStoreFlushMode::Batched,
        );
        (store, batcher)
    }

    #[tokio::test]
    async fn an_empty_batch_makes_no_append_call() {
        let (store, batcher) = counting();
        batcher.enqueue(path("s"), Vec::new());
        batcher.flush().await;
        batcher.close().await;
        assert_eq!(store.attempts.load(Ordering::SeqCst), 0);

        batcher.enqueue(path("s"), entries(1, "kept"));
        batcher.flush().await;
        assert_eq!(store.attempts.load(Ordering::SeqCst), 1, "the store counts");
    }

    #[tokio::test]
    async fn a_frame_outside_the_projects_directory_makes_no_append_call_and_reports_no_error() {
        let (store, batcher) = counting();
        let (sender, mut receiver) = tokio::sync::mpsc::channel(4);
        batcher.attach(MirrorErrorReporter::new(sender.downgrade()));
        batcher.enqueue("/elsewhere/proj/s.jsonl".to_owned(), entries(1, "o"));
        batcher.flush().await;
        batcher.close().await;
        assert_eq!(store.attempts.load(Ordering::SeqCst), 0);
        assert!(receiver.try_recv().is_err(), "a mirror error was reported");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_background_flushes_keep_the_order_of_a_key() {
        for _ in 0..20 {
            let store = InMemorySessionStore::new();
            let batcher = TranscriptMirrorBatcher::with_limits(
                SharedSessionStore::new(store.clone()),
                projects(),
                0,
                0,
                SEND_TIMEOUT,
            );
            let mut expected = Vec::new();
            for index in 0..200 {
                let tag = format!("c{index}");
                expected.push(format!("{tag}-0"));
                batcher.enqueue(path("s"), entries(1, &tag));
                if index % 7 == 0 {
                    batcher.flush().await;
                }
            }
            batcher.flush().await;
            let order: Vec<String> = store
                .get_entries(&SessionKey::new("proj", "s"))
                .iter()
                .map(|entry| entry["uuid"].as_str().unwrap_or_default().to_owned())
                .collect();
            assert_eq!(order, expected);
        }
    }

    #[tokio::test(start_paused = true)]
    async fn flushes_are_serialized_so_appends_keep_their_order() {
        let slow = Arc::new(FlakyStore::new(0, Some(Duration::from_millis(50))));
        let batcher = TranscriptMirrorBatcher::with_limits(
            SharedSessionStore::new(Shared(Arc::clone(&slow))),
            projects(),
            0,
            0,
            SEND_TIMEOUT,
        );
        for index in 0..5 {
            batcher.enqueue(path("s"), entries(1, &format!("o{index}")));
            tokio::task::yield_now().await;
        }
        batcher.flush().await;
        let order: Vec<String> = slow
            .inner
            .get_entries(&SessionKey::new("proj", "s"))
            .iter()
            .map(|entry| entry["uuid"].as_str().unwrap_or_default().to_owned())
            .collect();
        assert_eq!(order, ["o0-0", "o1-0", "o2-0", "o3-0", "o4-0"]);
    }
}
