#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};

use claude_agent_rust_sdk::{
    InMemorySessionStore, SdkSessionInfo, SessionKey, SessionStore, SessionStoreCapabilities,
    SessionStoreEntry, SessionStoreError, SessionStoreListEntry, SessionSummaryEntry,
    UnsupportedStoreOperation, fold_session_summary, list_sessions_from_store,
    project_key_for_directory,
};
use serde_json::{Value, json};
use tokio::sync::watch;

use common::transcripts::new_id;

const LOAD_CONCURRENCY: usize = 16;

fn directory() -> &'static Path {
    Path::new("/workspace/project")
}

fn key(session_id: &str) -> SessionKey {
    SessionKey::new(project_key_for_directory(Some(directory())), session_id)
}

fn entry(value: Value) -> SessionStoreEntry {
    value.as_object().cloned().unwrap()
}

fn user(text: &str, timestamp: &str) -> SessionStoreEntry {
    entry(json!({
        "type": "user",
        "timestamp": timestamp,
        "message": {"role": "user", "content": text},
    }))
}

fn sidechain(timestamp: &str) -> SessionStoreEntry {
    entry(json!({
        "type": "user",
        "timestamp": timestamp,
        "isSidechain": true,
        "message": {"content": "x"},
    }))
}

fn ids(sessions: &[SdkSessionInfo]) -> Vec<&str> {
    sessions
        .iter()
        .map(|session| session.session_id.as_str())
        .collect()
}

async fn list(
    store: &impl SessionStore,
    limit: Option<usize>,
    offset: usize,
) -> Vec<SdkSessionInfo> {
    list_sessions_from_store(store, Some(directory()), limit, offset)
        .await
        .unwrap()
}

enum Summaries {
    Stored,
    Unsupported,
    Fixed(Vec<SessionSummaryEntry>),
}

struct ScriptedStore {
    inner: InMemorySessionStore,
    capabilities: SessionStoreCapabilities,
    summaries: Summaries,
    unlisted: Vec<String>,
    gate: Option<watch::Sender<bool>>,
    active: AtomicUsize,
    peak: AtomicUsize,
    loads: AtomicUsize,
}

impl ScriptedStore {
    fn new(capabilities: SessionStoreCapabilities, summaries: Summaries) -> Self {
        Self {
            inner: InMemorySessionStore::new(),
            capabilities,
            summaries,
            unlisted: Vec::new(),
            gate: None,
            active: AtomicUsize::new(0),
            peak: AtomicUsize::new(0),
            loads: AtomicUsize::new(0),
        }
    }
}

impl SessionStore for ScriptedStore {
    async fn append(
        &self,
        key: &SessionKey,
        entries: &[SessionStoreEntry],
    ) -> Result<(), SessionStoreError> {
        self.inner.append(key, entries).await
    }

    async fn load(
        &self,
        key: &SessionKey,
    ) -> Result<Option<Vec<SessionStoreEntry>>, SessionStoreError> {
        self.loads.fetch_add(1, Ordering::SeqCst);
        let now = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak.fetch_max(now, Ordering::SeqCst);
        if let Some(gate) = &self.gate {
            gate.subscribe()
                .wait_for(|open| *open)
                .await
                .map(drop)
                .unwrap();
        }
        self.active.fetch_sub(1, Ordering::SeqCst);
        self.inner.load(key).await
    }

    fn capabilities(&self) -> SessionStoreCapabilities {
        self.capabilities
    }

    async fn list_sessions(
        &self,
        project_key: &str,
    ) -> Result<Vec<SessionStoreListEntry>, SessionStoreError> {
        let mut listing = self.inner.list_sessions(project_key).await?;
        listing.retain(|listed| !self.unlisted.contains(&listed.session_id));
        Ok(listing)
    }

    async fn list_session_summaries(
        &self,
        project_key: &str,
    ) -> Result<Vec<SessionSummaryEntry>, SessionStoreError> {
        match &self.summaries {
            Summaries::Stored => self.inner.list_session_summaries(project_key).await,
            Summaries::Unsupported => Err(Box::new(UnsupportedStoreOperation {
                operation: "list_session_summaries",
            })),
            Summaries::Fixed(summaries) => Ok(summaries.clone()),
        }
    }
}

#[test]
fn a_later_title_or_branch_replaces_the_earlier_one_within_a_batch_and_across_folds() {
    let key = key("11111111-1111-4111-8111-111111111111");
    let first = fold_session_summary(
        None,
        &key,
        &[
            entry(json!({
                "type": "x", "timestamp": "2024-01-01T00:00:00Z",
                "customTitle": "t1", "gitBranch": "main",
            })),
            entry(json!({"type": "x", "timestamp": "2024-01-01T00:00:01Z", "customTitle": "t2"})),
        ],
    );
    assert_eq!(first.data["custom_title"], "t2");
    assert_eq!(first.data["git_branch"], "main");

    let second = fold_session_summary(
        Some(&first),
        &key,
        &[entry(json!({
            "type": "x", "aiTitle": "ai", "lastPrompt": "lp", "summary": "sm", "gitBranch": "dev",
        }))],
    );
    assert_eq!(second.data["custom_title"], "t2");
    assert_eq!(second.data["ai_title"], "ai");
    assert_eq!(second.data["last_prompt"], "lp");
    assert_eq!(second.data["summary_hint"], "sm");
    assert_eq!(second.data["git_branch"], "dev");
}

#[test]
fn sidechain_latches_from_a_first_entry_without_a_timestamp() {
    let summary = fold_session_summary(
        None,
        &key("11111111-1111-4111-8111-111111111111"),
        &[
            entry(json!({"type": "user", "isSidechain": true})),
            entry(json!({"type": "x", "timestamp": "2024-01-01T00:00:00Z"})),
        ],
    );
    assert_eq!(summary.data["is_sidechain"], true);
    assert_eq!(summary.data["created_at"], 1_704_067_200_000_i64);
}

async fn listed_from_summary(data: &Value) -> SdkSessionInfo {
    let store = ScriptedStore::new(
        SessionStoreCapabilities::none().with_list_session_summaries(),
        Summaries::Fixed(vec![SessionSummaryEntry::new(
            "s",
            1,
            data.as_object().cloned().unwrap(),
        )]),
    );
    let mut listed = list(&store, None, 0).await;
    assert_eq!(listed.len(), 1);
    listed.remove(0)
}

#[tokio::test]
async fn a_summary_is_titled_by_the_first_field_of_the_precedence_chain_it_holds() {
    let mut data = json!({
        "first_prompt": "fp",
        "first_prompt_locked": true,
        "command_fallback": "/cmd",
        "summary_hint": "sh",
        "last_prompt": "lp",
        "ai_title": "ai",
        "custom_title": "ct",
    });
    let info = listed_from_summary(&data).await;
    assert_eq!(info.summary, "ct");
    assert_eq!(info.custom_title.as_deref(), Some("ct"));

    data.as_object_mut().unwrap().remove("custom_title");
    let info = listed_from_summary(&data).await;
    assert_eq!(info.summary, "ai");
    assert_eq!(info.custom_title.as_deref(), Some("ai"));

    data.as_object_mut().unwrap().remove("ai_title");
    let info = listed_from_summary(&data).await;
    assert_eq!(info.summary, "lp");
    assert_eq!(info.custom_title, None);

    data.as_object_mut().unwrap().remove("last_prompt");
    let info = listed_from_summary(&data).await;
    assert_eq!(info.summary, "sh");

    data.as_object_mut().unwrap().remove("summary_hint");
    let info = listed_from_summary(&data).await;
    assert_eq!(info.summary, "fp");
    assert_eq!(info.first_prompt.as_deref(), Some("fp"));

    data["first_prompt_locked"] = Value::Bool(false);
    let info = listed_from_summary(&data).await;
    assert_eq!(info.summary, "/cmd");
    assert_eq!(info.first_prompt.as_deref(), Some("/cmd"));
}

#[tokio::test]
async fn summaries_of_sidechain_and_untitled_sessions_are_left_out() {
    let store = InMemorySessionStore::new();
    let (main, side, empty) = (new_id(), new_id(), new_id());
    store
        .append(&key(&main), &[user("hello", "2024-01-01T00:00:00Z")])
        .await
        .unwrap();
    store
        .append(&key(&side), &[sidechain("2024-01-01T00:00:00Z")])
        .await
        .unwrap();
    store
        .append(
            &key(&empty),
            &[entry(
                json!({"type": "x", "timestamp": "2024-01-01T00:00:00Z"}),
            )],
        )
        .await
        .unwrap();
    assert_eq!(ids(&list(&store, None, 0).await), [main.as_str()]);
}

#[tokio::test]
async fn limit_and_offset_page_through_the_summaries_newest_first() {
    let store = InMemorySessionStore::new();
    let mut sessions = Vec::new();
    for index in 0..5 {
        let id = new_id();
        let timestamp = format!("2024-01-0{}T00:00:00Z", index + 1);
        store
            .append(&key(&id), &[user(&format!("p{index}"), &timestamp)])
            .await
            .unwrap();
        sessions.push(id);
    }
    let page = list(&store, Some(2), 1).await;
    assert_eq!(ids(&page), [sessions[3].as_str(), sessions[2].as_str()]);
}

#[tokio::test]
async fn a_store_whose_summaries_are_unsupported_is_listed_through_load() {
    let store = ScriptedStore::new(SessionStoreCapabilities::all(), Summaries::Unsupported);
    let id = new_id();
    store
        .append(&key(&id), &[user("hi", "2024-01-01T00:00:00Z")])
        .await
        .unwrap();
    let listed = list(&store, None, 0).await;
    assert_eq!(ids(&listed), [id.as_str()]);
    assert_eq!(listed[0].summary, "hi");
    assert_eq!(store.loads.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_sidechain_summary_does_not_take_a_slot_of_the_page() {
    let store = InMemorySessionStore::new();
    let (older, newer, side) = (new_id(), new_id(), new_id());
    store
        .append(&key(&older), &[user("real 2", "2024-01-01T00:00:00Z")])
        .await
        .unwrap();
    store
        .append(&key(&newer), &[user("real 1", "2024-01-02T00:00:00Z")])
        .await
        .unwrap();
    store
        .append(&key(&side), &[sidechain("2024-01-03T00:00:00Z")])
        .await
        .unwrap();
    let page = list(&store, Some(2), 0).await;
    assert_eq!(ids(&page), [newer.as_str(), older.as_str()]);
}

#[tokio::test]
async fn a_summary_of_a_session_that_is_no_longer_listed_is_dropped() {
    let (real, ghost) = (new_id(), new_id());
    let mut store = ScriptedStore::new(SessionStoreCapabilities::all(), Summaries::Stored);
    store.unlisted.push(ghost.clone());
    store
        .append(&key(&real), &[user("real", "2024-01-02T00:00:00Z")])
        .await
        .unwrap();
    store
        .append(&key(&ghost), &[user("ghost", "2024-01-01T00:00:00Z")])
        .await
        .unwrap();
    let summarized = store
        .list_session_summaries(&key(&ghost).project_key)
        .await
        .unwrap();
    assert_eq!(summarized.len(), 2);

    assert_eq!(ids(&list(&store, None, 0).await), [real.as_str()]);
    assert_eq!(store.loads.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn gap_fill_loads_run_at_most_16_at_once() {
    let mut store = ScriptedStore::new(
        SessionStoreCapabilities::all(),
        Summaries::Fixed(Vec::new()),
    );
    let (gate, _) = watch::channel(false);
    store.gate = Some(gate);
    let count = LOAD_CONCURRENCY * 2;
    for index in 0..count {
        store
            .append(
                &key(&new_id()),
                &[user(&format!("p{index}"), "2024-01-01T00:00:00.000Z")],
            )
            .await
            .unwrap();
    }

    let saturation = async {
        for _ in 0..5 {
            tokio::task::yield_now().await;
        }
        let peak = store.peak.load(Ordering::SeqCst);
        store.gate.as_ref().unwrap().send_replace(true);
        peak
    };
    let (listed, peak_at_saturation) = tokio::join!(list(&store, None, 0), saturation);
    assert_eq!(listed.len(), count);
    assert!(
        (1..=LOAD_CONCURRENCY).contains(&peak_at_saturation),
        "peak while the loads were held: {peak_at_saturation}"
    );
    assert_eq!(store.peak.load(Ordering::SeqCst), LOAD_CONCURRENCY);
    assert_eq!(store.loads.load(Ordering::SeqCst), count);
}
