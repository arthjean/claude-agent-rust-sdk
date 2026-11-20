#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

common::transcripts::shared_config_dir_before_main!();

use std::collections::HashSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use claude_agent_rust_sdk::ErrorKind;
use claude_agent_rust_sdk::{
    Error, ForkSessionOptions, ImportSessionOptions, InMemorySessionStore, ListSessionsOptions,
    SdkSessionInfo, SessionKey, SessionListSubkeysKey, SessionMessagesOptions, SessionStore,
    SessionStoreCapabilities, SessionStoreEntry, SessionStoreError, SessionStoreListEntry,
    SessionSummaryEntry, UnsupportedStoreOperation, delete_session_via_store, fold_session_summary,
    fork_session_via_store, get_session_info, get_session_info_from_store, get_session_messages,
    get_session_messages_from_store, get_subagent_messages, get_subagent_messages_from_store,
    import_session_to_store, list_sessions, list_sessions_from_store, list_subagents,
    list_subagents_from_store, project_key_for_directory, rename_session_via_store,
    tag_session_via_store,
};
use serde_json::{Value, json};

use common::transcripts::{Project, assistant, conversation, new_id, user, write_lines};

fn entry(value: Value) -> SessionStoreEntry {
    value.as_object().cloned().unwrap()
}

fn comparable(info: &SdkSessionInfo) -> Value {
    json!({
        "session_id": info.session_id, "summary": info.summary, "custom_title": info.custom_title,
        "first_prompt": info.first_prompt, "git_branch": info.git_branch, "cwd": info.cwd,
        "tag": info.tag, "created_at": info.created_at,
    })
}

fn rich_session(project: &Project) -> String {
    let id = new_id();
    let mut first = user("u1", None, &id, Value::from("Explain the code"));
    first["gitBranch"] = Value::from("main");
    let lines = vec![
        first,
        assistant("a1", Some("u1"), &id, "Sure"),
        user("u2", Some("a1"), &id, Value::from("And the tests?")),
        json!({"type": "custom-title", "customTitle": "Code tour", "sessionId": id}),
        json!({"type": "tag", "tag": "docs", "sessionId": id}),
    ];
    project.write(&id, &lines);
    let subagents = project.dir.join(&id).join("subagents");
    write_lines(
        &subagents.join("agent-alpha.jsonl"),
        &[
            user("s1", None, &id, Value::from("sub task")),
            assistant("s2", Some("s1"), &id, "sub done"),
        ],
    );
    std::fs::write(
        subagents.join("agent-alpha.meta.json"),
        json!({"toolUseId": "toolu_9", "parentAgentId": "root"}).to_string(),
    )
    .unwrap();
    write_lines(
        &subagents.join("nested").join("agent-beta.jsonl"),
        &[user("b1", None, &id, Value::from("nested task"))],
    );
    id
}

#[tokio::test]
async fn store_readers_match_the_disk_readers_for_the_same_content() {
    let project = Project::new();
    let id = rich_session(&project);
    let second = new_id();
    project.write(&second, &conversation(&second, "another", json!({})));
    let store = InMemorySessionStore::new();
    let directory = Some(project.path());
    import_session_to_store(
        &id,
        &store,
        ImportSessionOptions::new().directory(project.path()),
    )
    .await
    .unwrap();
    import_session_to_store(
        &second,
        &store,
        ImportSessionOptions::new().directory(project.path()),
    )
    .await
    .unwrap();

    let disk = list_sessions(
        ListSessionsOptions::new()
            .directory(project.path())
            .include_worktrees(false),
    );
    let stored =
        list_sessions_from_store(&store, ListSessionsOptions::new().directory(project.path()))
            .await
            .unwrap();
    let mut disk: Vec<Value> = disk.iter().map(comparable).collect();
    let mut stored: Vec<Value> = stored.iter().map(comparable).collect();
    disk.sort_by_key(|info| info["session_id"].to_string());
    stored.sort_by_key(|info| info["session_id"].to_string());
    assert_eq!(disk, stored);

    let disk_info = get_session_info(&id, directory).unwrap();
    let store_info = get_session_info_from_store(&store, &id, directory)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(comparable(&disk_info), comparable(&store_info));
    assert_eq!(disk_info.summary, "Code tour");
    assert_eq!(store_info.tag.as_deref(), Some("docs"));

    assert_eq!(
        get_session_messages(&id, SessionMessagesOptions::new().directory(project.path())),
        get_session_messages_from_store(
            &store,
            &id,
            SessionMessagesOptions::new().directory(project.path())
        )
        .await
        .unwrap()
    );
    assert_eq!(
        get_session_messages(
            &id,
            SessionMessagesOptions::new()
                .directory(project.path())
                .limit(1)
                .offset(1)
        ),
        get_session_messages_from_store(
            &store,
            &id,
            SessionMessagesOptions::new()
                .directory(project.path())
                .limit(1)
                .offset(1)
        )
        .await
        .unwrap()
    );
    assert_eq!(
        list_subagents(&id, directory),
        list_subagents_from_store(&store, &id, directory)
            .await
            .unwrap()
    );
    for agent in ["alpha", "beta"] {
        assert_eq!(
            get_subagent_messages(
                &id,
                agent,
                SessionMessagesOptions::new().directory(project.path())
            ),
            get_subagent_messages_from_store(
                &store,
                &id,
                agent,
                SessionMessagesOptions::new().directory(project.path())
            )
            .await
            .unwrap(),
            "{agent}"
        );
    }
    let alpha = get_subagent_messages_from_store(
        &store,
        &id,
        "alpha",
        SessionMessagesOptions::new().directory(project.path()),
    )
    .await
    .unwrap();
    assert_eq!(alpha[0].parent_tool_use_id.as_deref(), Some("toolu_9"));
    assert_eq!(alpha[0].parent_agent_id.as_deref(), Some("root"));

    assert!(
        get_session_info_from_store(&store, "bad", directory)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        get_session_info_from_store(&store, &new_id(), directory)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        get_session_messages_from_store(
            &store,
            &new_id(),
            SessionMessagesOptions::new().directory(project.path())
        )
        .await
        .unwrap()
        .is_empty()
    );
    assert!(
        get_subagent_messages_from_store(
            &store,
            &id,
            "missing",
            SessionMessagesOptions::new().directory(project.path())
        )
        .await
        .unwrap()
        .is_empty()
    );
}

#[tokio::test]
async fn import_appends_the_transcript_in_one_call_per_batch() {
    let project = Project::new();
    let id = new_id();
    let lines: Vec<Value> = (0..5)
        .map(|index| {
            user(
                &format!("u{index}"),
                None,
                &id,
                Value::from(format!("m{index}")),
            )
        })
        .collect();
    project.write(&id, &lines);
    let store = CountingStore::new(SessionStoreCapabilities::all(), None);
    import_session_to_store(
        &id,
        &store,
        ImportSessionOptions::new()
            .directory(project.path())
            .batch_size(2),
    )
    .await
    .unwrap();

    let key = SessionKey::new(project_key_for_directory(Some(project.path())), id.as_str());
    let entries: Vec<SessionStoreEntry> = lines.iter().cloned().map(entry).collect();
    let appended = store.appended.lock().unwrap().clone();
    assert_eq!(
        appended,
        [
            (key.clone(), entries[0..2].to_vec()),
            (key.clone(), entries[2..4].to_vec()),
            (key.clone(), entries[4..5].to_vec()),
        ]
    );
    assert_eq!(store.inner.get_entries(&key), entries);
}

#[tokio::test]
async fn import_batches_entries_and_stores_sidecars_as_agent_metadata() {
    let project = Project::new();
    let id = rich_session(&project);
    let counting = CountingStore::new(SessionStoreCapabilities::all(), None);
    import_session_to_store(
        &id,
        &counting,
        ImportSessionOptions::new()
            .directory(project.path())
            .batch_size(2),
    )
    .await
    .unwrap();
    let key = SessionKey::new(project_key_for_directory(Some(project.path())), id.as_str());
    assert_eq!(counting.inner.get_entries(&key).len(), 5);
    let alpha = counting
        .inner
        .get_entries(&key.clone().with_subpath("subagents/agent-alpha"));
    assert_eq!(alpha.last().unwrap()["type"], "agent_metadata");
    assert_eq!(alpha.last().unwrap()["toolUseId"], "toolu_9");
    let beta = counting
        .inner
        .get_entries(&key.clone().with_subpath("subagents/nested/agent-beta"));
    assert_eq!(beta.len(), 1);
    assert_eq!(counting.appends.load(Ordering::SeqCst), 3 + 1 + 1 + 1);

    let without_subagents = InMemorySessionStore::new();
    import_session_to_store(
        &id,
        &without_subagents,
        ImportSessionOptions::new().include_subagents(false),
    )
    .await
    .unwrap();
    assert!(
        without_subagents
            .list_subkeys(&SessionListSubkeysKey::new(
                key.project_key.clone(),
                id.as_str()
            ))
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        import_session_to_store("bad", &without_subagents, ImportSessionOptions::new())
            .await
            .as_ref()
            .err()
            .map(Error::kind),
        Some(ErrorKind::Config)
    );
    assert_eq!(
        import_session_to_store(
            &new_id(),
            &without_subagents,
            ImportSessionOptions::new().directory(project.path())
        )
        .await
        .as_ref()
        .err()
        .map(Error::kind),
        Some(ErrorKind::SessionNotFound)
    );
}

struct CountingStore {
    inner: InMemorySessionStore,
    capabilities: SessionStoreCapabilities,
    load_delay: Option<Duration>,
    active: AtomicUsize,
    peak: AtomicUsize,
    loads: AtomicUsize,
    appends: AtomicUsize,
    failing: std::sync::Mutex<Vec<String>>,
    summaries: std::sync::Mutex<Option<Vec<SessionSummaryEntry>>>,
    /// When set, every load waits until the value is true.
    gate: Option<tokio::sync::watch::Receiver<bool>>,
    /// Every append call, in order.
    appended: std::sync::Mutex<Vec<(SessionKey, Vec<SessionStoreEntry>)>>,
}

impl CountingStore {
    fn new(capabilities: SessionStoreCapabilities, load_delay: Option<Duration>) -> Self {
        Self {
            inner: InMemorySessionStore::new(),
            capabilities,
            load_delay,
            active: AtomicUsize::new(0),
            peak: AtomicUsize::new(0),
            loads: AtomicUsize::new(0),
            appends: AtomicUsize::new(0),
            failing: std::sync::Mutex::new(Vec::new()),
            summaries: std::sync::Mutex::new(None),
            gate: None,
            appended: std::sync::Mutex::new(Vec::new()),
        }
    }
}

impl SessionStore for CountingStore {
    async fn append(
        &self,
        key: &SessionKey,
        entries: &[SessionStoreEntry],
    ) -> Result<(), SessionStoreError> {
        self.appends.fetch_add(1, Ordering::SeqCst);
        self.appended
            .lock()
            .unwrap()
            .push((key.clone(), entries.to_vec()));
        self.inner.append(key, entries).await
    }

    async fn load(
        &self,
        key: &SessionKey,
    ) -> Result<Option<Vec<SessionStoreEntry>>, SessionStoreError> {
        self.loads.fetch_add(1, Ordering::SeqCst);
        let now = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak.fetch_max(now, Ordering::SeqCst);
        if let Some(delay) = self.load_delay {
            tokio::time::sleep(delay).await;
        }
        if let Some(gate) = &self.gate {
            gate.clone().wait_for(|open| *open).await.unwrap();
        }
        self.active.fetch_sub(1, Ordering::SeqCst);
        if self.failing.lock().unwrap().contains(&key.session_id) {
            return Err("backend unavailable".into());
        }
        self.inner.load(key).await
    }

    fn capabilities(&self) -> SessionStoreCapabilities {
        self.capabilities
    }

    async fn list_sessions(
        &self,
        project_key: &str,
    ) -> Result<Vec<SessionStoreListEntry>, SessionStoreError> {
        offered(self.capabilities.list_sessions, "list_sessions")?;
        self.inner.list_sessions(project_key).await
    }

    async fn list_session_summaries(
        &self,
        project_key: &str,
    ) -> Result<Vec<SessionSummaryEntry>, SessionStoreError> {
        offered(
            self.capabilities.list_session_summaries,
            "list_session_summaries",
        )?;
        if let Some(summaries) = self.summaries.lock().unwrap().clone() {
            return Ok(summaries);
        }
        self.inner.list_session_summaries(project_key).await
    }

    async fn list_subkeys(
        &self,
        key: &SessionListSubkeysKey,
    ) -> Result<Vec<String>, SessionStoreError> {
        offered(self.capabilities.list_subkeys, "list_subkeys")?;
        self.inner.list_subkeys(key).await
    }
}

/// Answers as the default implementation of an optional method does when
/// the store under test does not offer it.
fn offered(offered: bool, operation: &'static str) -> Result<(), SessionStoreError> {
    if offered {
        Ok(())
    } else {
        Err(Box::new(UnsupportedStoreOperation { operation }))
    }
}

/// Implements every optional method but keeps the default
/// `capabilities`, which declares none.
#[derive(Default)]
struct UndeclaredStore(InMemorySessionStore);

impl SessionStore for UndeclaredStore {
    async fn append(
        &self,
        key: &SessionKey,
        entries: &[SessionStoreEntry],
    ) -> Result<(), SessionStoreError> {
        self.0.append(key, entries).await
    }

    async fn load(
        &self,
        key: &SessionKey,
    ) -> Result<Option<Vec<SessionStoreEntry>>, SessionStoreError> {
        self.0.load(key).await
    }

    async fn list_sessions(
        &self,
        project_key: &str,
    ) -> Result<Vec<SessionStoreListEntry>, SessionStoreError> {
        self.0.list_sessions(project_key).await
    }

    async fn list_session_summaries(
        &self,
        project_key: &str,
    ) -> Result<Vec<SessionSummaryEntry>, SessionStoreError> {
        self.0.list_session_summaries(project_key).await
    }

    async fn delete(&self, key: &SessionKey) -> Result<(), SessionStoreError> {
        self.0.delete(key).await
    }

    async fn list_subkeys(
        &self,
        key: &SessionListSubkeysKey,
    ) -> Result<Vec<String>, SessionStoreError> {
        self.0.list_subkeys(key).await
    }
}

#[tokio::test]
async fn listing_methods_a_store_implements_are_called_without_being_declared() {
    let project = Project::new();
    let key = project_key_for_directory(Some(project.path()));
    let store = UndeclaredStore::default();
    assert_eq!(store.capabilities(), SessionStoreCapabilities::none());
    let ids = fill(&store, &key, 2).await;
    let sub =
        SessionKey::new(key.as_str(), ids[0].as_str()).with_subpath("subagents/nested/agent-n1");
    store
        .append(&sub, &[entry(user("n1", None, &ids[0], Value::from("t")))])
        .await
        .unwrap();

    let listed =
        list_sessions_from_store(&store, ListSessionsOptions::new().directory(project.path()))
            .await
            .unwrap();
    assert_eq!(listed.len(), 2);

    assert_eq!(
        list_subagents_from_store(&store, &ids[0], Some(project.path()))
            .await
            .unwrap(),
        ["n1"]
    );
    let messages = get_subagent_messages_from_store(
        &store,
        &ids[0],
        "n1",
        SessionMessagesOptions::new().directory(project.path()),
    )
    .await
    .unwrap();
    assert_eq!(messages.len(), 1, "found through list_subkeys");
}

#[tokio::test]
async fn delete_via_store_deletes_from_a_store_that_implements_delete_without_declaring_it() {
    let project = Project::new();
    let key = project_key_for_directory(Some(project.path()));
    let store = UndeclaredStore::default();
    let ids = fill(&store, &key, 1).await;
    delete_session_via_store(&store, &ids[0], Some(project.path()))
        .await
        .unwrap();
    assert_eq!(
        store
            .load(&SessionKey::new(key.as_str(), ids[0].as_str()))
            .await
            .unwrap(),
        None,
        "the session is deleted"
    );
}

#[tokio::test]
async fn delete_via_store_leaves_a_store_without_delete_unchanged() {
    let project = Project::new();
    let key = project_key_for_directory(Some(project.path()));
    let store = CountingStore::new(SessionStoreCapabilities::none(), None);
    let ids = fill(&store, &key, 1).await;
    delete_session_via_store(&store, &ids[0], Some(project.path()))
        .await
        .unwrap();
    assert!(
        store
            .load(&SessionKey::new(key.as_str(), ids[0].as_str()))
            .await
            .unwrap()
            .is_some()
    );
}

async fn fill(store: &impl SessionStore, project_key: &str, count: usize) -> Vec<String> {
    let mut ids = Vec::new();
    for index in 0..count {
        let id = new_id();
        let key = SessionKey::new(project_key, id.as_str());
        let lines = conversation(&id, &format!("prompt {index}"), json!({}));
        let entries: Vec<SessionStoreEntry> = lines.into_iter().map(entry).collect();
        store.append(&key, &entries).await.unwrap();
        ids.push(id);
    }
    ids
}

#[tokio::test]
async fn store_listing_loads_at_most_16_sessions_at_once() {
    let project = Project::new();
    let key = project_key_for_directory(Some(project.path()));
    let (open, gate) = tokio::sync::watch::channel(false);
    let mut store = CountingStore::new(SessionStoreCapabilities::none().with_list_sessions(), None);
    store.gate = Some(gate);
    let ids = fill(&store, &key, 48).await;
    let opener = async {
        let saturated = async {
            while store.active.load(Ordering::SeqCst) < 16 {
                tokio::task::yield_now().await;
            }
        };
        tokio::time::timeout(Duration::from_secs(10), saturated)
            .await
            .expect("16 loads never ran at once");
        for _ in 0..50 {
            tokio::task::yield_now().await;
        }
        assert_eq!(
            store.active.load(Ordering::SeqCst),
            16,
            "the loads beyond 16 wait for a free slot"
        );
        open.send(true).unwrap();
    };
    let (listed, ()) = tokio::join!(
        list_sessions_from_store(&store, ListSessionsOptions::new().directory(project.path())),
        opener
    );
    let listed = listed.unwrap();
    assert_eq!(listed.len(), 48);
    assert_eq!(store.loads.load(Ordering::SeqCst), 48);
    assert_eq!(store.peak.load(Ordering::SeqCst), 16);
    assert_eq!(listed[0].session_id, ids[47], "newest first");
    let page = list_sessions_from_store(
        &store,
        ListSessionsOptions::new()
            .directory(project.path())
            .limit(5)
            .offset(2),
    )
    .await
    .unwrap();
    let expected: Vec<&String> = ids.iter().rev().skip(2).take(5).collect();
    assert_eq!(
        page.iter().map(|info| &info.session_id).collect::<Vec<_>>(),
        expected
    );
}

#[tokio::test]
async fn a_failed_load_gives_an_empty_row_and_the_others_are_listed() {
    let project = Project::new();
    let key = project_key_for_directory(Some(project.path()));
    let store = CountingStore::new(SessionStoreCapabilities::none().with_list_sessions(), None);
    let ids = fill(&store, &key, 3).await;
    store.failing.lock().unwrap().push(ids[1].clone());
    let listed =
        list_sessions_from_store(&store, ListSessionsOptions::new().directory(project.path()))
            .await
            .unwrap();
    assert_eq!(listed.len(), 3);
    let failed = listed
        .iter()
        .find(|info| info.session_id == ids[1])
        .unwrap();
    assert_eq!(failed.summary, "");
    assert!(
        listed
            .iter()
            .filter(|info| info.session_id != ids[1])
            .all(|info| info.summary.starts_with("prompt"))
    );
}

#[tokio::test]
async fn fresh_summaries_are_used_and_stale_or_missing_ones_are_refilled_after_pagination() {
    let project = Project::new();
    let key = project_key_for_directory(Some(project.path()));
    let store = CountingStore::new(SessionStoreCapabilities::all(), None);
    let ids = fill(&store, &key, 4).await;
    let listing = store.inner.list_sessions(&key).await.unwrap();
    let mtime = |id: &str| {
        listing
            .iter()
            .find(|entry| entry.session_id == id)
            .unwrap()
            .mtime
    };
    let mut summaries = store.inner.list_session_summaries(&key).await.unwrap();
    summaries.retain(|summary| summary.session_id != ids[0]);
    for summary in &mut summaries {
        if summary.session_id == ids[1] {
            summary.mtime = mtime(&ids[1]) - 1;
            summary
                .data
                .insert("custom_title".to_owned(), Value::from("stale title"));
        }
        if summary.session_id == ids[2] {
            summary
                .data
                .insert("custom_title".to_owned(), Value::from("fresh title"));
        }
    }
    *store.summaries.lock().unwrap() = Some(summaries);

    let listed =
        list_sessions_from_store(&store, ListSessionsOptions::new().directory(project.path()))
            .await
            .unwrap();
    assert_eq!(
        store.loads.load(Ordering::SeqCst),
        2,
        "only the missing and the stale summaries are loaded"
    );
    let summary_of = |id: &str| {
        listed
            .iter()
            .find(|info| info.session_id == id)
            .unwrap()
            .summary
            .clone()
    };
    assert_eq!(summary_of(&ids[0]), "prompt 0");
    assert_eq!(summary_of(&ids[1]), "prompt 1");
    assert_eq!(summary_of(&ids[2]), "fresh title");
    assert_eq!(summary_of(&ids[3]), "prompt 3");

    store.loads.store(0, Ordering::SeqCst);
    let page = list_sessions_from_store(
        &store,
        ListSessionsOptions::new()
            .directory(project.path())
            .limit(2),
    )
    .await
    .unwrap();
    assert_eq!(
        page.iter()
            .map(|info| info.session_id.clone())
            .collect::<Vec<_>>(),
        [ids[3].clone(), ids[2].clone()]
    );
    assert_eq!(
        store.loads.load(Ordering::SeqCst),
        0,
        "the page holds only fresh summaries"
    );
}

#[tokio::test]
async fn listing_requires_list_sessions_or_summaries() {
    let store = CountingStore::new(SessionStoreCapabilities::none(), None);
    let error = list_sessions_from_store(&store, ListSessionsOptions::new())
        .await
        .unwrap_err();
    assert!(
        error
            .unsupported_operation()
            .map(|unsupported| unsupported.operation)
            == Some("list_sessions"),
        "{error}"
    );
    let error = list_subagents_from_store(&store, &new_id(), None)
        .await
        .unwrap_err();
    assert!(
        error
            .unsupported_operation()
            .map(|unsupported| unsupported.operation)
            == Some("list_subkeys"),
        "{error}"
    );
}

#[tokio::test]
async fn agent_metadata_without_ids_is_filtered_out_and_sets_no_parent_ids() {
    let project = Project::new();
    let key = project_key_for_directory(Some(project.path()));
    let store = InMemorySessionStore::new();
    let id = new_id();
    let sub = SessionKey::new(key.as_str(), id.as_str()).with_subpath("subagents/agent-x");
    store
        .append(
            &sub,
            &[
                entry(json!({"type": "agent_metadata", "name": "x"})),
                entry(user("x1", None, &id, Value::from("hi"))),
            ],
        )
        .await
        .unwrap();
    let messages = get_subagent_messages_from_store(
        &store,
        &id,
        "x",
        SessionMessagesOptions::new().directory(project.path()),
    )
    .await
    .unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].uuid, "x1");
    assert_eq!(messages[0].parent_tool_use_id, None);
    assert_eq!(messages[0].parent_agent_id, None);
}

#[tokio::test]
async fn store_session_messages_never_carry_parent_ids() {
    let project = Project::new();
    let key = project_key_for_directory(Some(project.path()));
    let store = InMemorySessionStore::new();
    let id = new_id();
    let entries: Vec<SessionStoreEntry> = conversation(&id, "hello", json!({}))
        .into_iter()
        .map(entry)
        .collect();
    store
        .append(&SessionKey::new(key.as_str(), id.as_str()), &entries)
        .await
        .unwrap();
    let messages = get_session_messages_from_store(
        &store,
        &id,
        SessionMessagesOptions::new().directory(project.path()),
    )
    .await
    .unwrap();
    assert_eq!(messages.len(), 2);
    assert!(messages.iter().all(|message| {
        message.parent_tool_use_id.is_none() && message.parent_agent_id.is_none()
    }));
}

#[tokio::test]
async fn subagents_are_read_at_the_direct_path_without_list_subkeys() {
    let project = Project::new();
    let key = project_key_for_directory(Some(project.path()));
    let store = CountingStore::new(SessionStoreCapabilities::none(), None);
    let id = new_id();
    let sub = SessionKey::new(key.as_str(), id.as_str()).with_subpath("subagents/agent-x");
    store
        .append(
            &sub,
            &[
                entry(user("x1", None, &id, Value::from("t"))),
                entry(json!({"type": "agent_metadata", "toolUseId": "old"})),
                entry(json!({"type": "agent_metadata", "toolUseId": "new"})),
            ],
        )
        .await
        .unwrap();
    let messages = get_subagent_messages_from_store(
        &store,
        &id,
        "x",
        SessionMessagesOptions::new().directory(project.path()),
    )
    .await
    .unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].parent_tool_use_id.as_deref(), Some("new"));
}

#[tokio::test]
async fn store_mutations_write_entries_with_uuid_and_timestamp() {
    let project = Project::new();
    let directory = Some(project.path());
    let key_text = project_key_for_directory(directory);
    let store = InMemorySessionStore::new();
    let id = new_id();
    let key = SessionKey::new(key_text.as_str(), id.as_str());
    let lines: Vec<SessionStoreEntry> =
        vec![entry(user(&new_id(), None, &id, Value::from("hello")))];
    store.append(&key, &lines).await.unwrap();

    rename_session_via_store(&store, &id, "  Renamed ", directory)
        .await
        .unwrap();
    tag_session_via_store(&store, &id, Some("a\u{200b}b"), directory)
        .await
        .unwrap();
    tag_session_via_store(&store, &id, None, directory)
        .await
        .unwrap();
    let stored = store.get_entries(&key);
    let rename = &stored[1];
    assert_eq!(rename["type"], "custom-title");
    assert_eq!(rename["customTitle"], "Renamed");
    assert!(rename["uuid"].as_str().is_some_and(|uuid| uuid.len() == 36));
    assert!(
        rename["timestamp"]
            .as_str()
            .is_some_and(|stamp| stamp.ends_with('Z'))
    );
    assert_eq!(rename["sessionId"], id.as_str());
    assert_eq!(stored[2]["type"], "tag");
    assert_eq!(stored[2]["tag"], "ab");
    assert_eq!(stored[2]["sessionId"], id.as_str());
    assert_eq!(stored[3]["type"], "tag");
    assert_eq!(stored[3]["tag"], "");
    assert!(stored[3]["uuid"].is_string() && stored[3]["timestamp"].is_string());
    let info = get_session_info_from_store(&store, &id, directory)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(info.summary, "Renamed");
    assert_eq!(info.tag, None);
    assert_eq!(
        rename_session_via_store(&store, &id, " ", directory)
            .await
            .as_ref()
            .err()
            .map(Error::kind),
        Some(ErrorKind::Config)
    );
    assert_eq!(
        rename_session_via_store(&store, "not-a-uuid", "t", directory)
            .await
            .as_ref()
            .err()
            .map(Error::kind),
        Some(ErrorKind::Config)
    );
    assert_eq!(
        rename_session_via_store(&store, &new_id(), "  ", directory)
            .await
            .as_ref()
            .err()
            .map(Error::kind),
        Some(ErrorKind::Config),
        "a blank title is refused before the session is looked up"
    );
    assert_eq!(
        tag_session_via_store(&store, "bad", None, directory)
            .await
            .as_ref()
            .err()
            .map(Error::kind),
        Some(ErrorKind::Config)
    );

    let fork = fork_session_via_store(
        &store,
        &id,
        ForkSessionOptions::new().directory(project.path()),
    )
    .await
    .unwrap();
    let forked = store.get_entries(&SessionKey::new(
        key_text.as_str(),
        fork.session_id.as_str(),
    ));
    assert_eq!(forked.len(), 2);
    assert_eq!(forked[0]["forkedFrom"]["sessionId"], id.as_str());
    assert_eq!(forked[1]["customTitle"], "Renamed (fork)");
    assert_eq!(
        fork_session_via_store(
            &store,
            &new_id(),
            ForkSessionOptions::new().directory(project.path())
        )
        .await
        .as_ref()
        .err()
        .map(Error::kind),
        Some(ErrorKind::SessionNotFound)
    );
    assert_eq!(
        fork_session_via_store(
            &store,
            &id,
            ForkSessionOptions::new()
                .directory(project.path())
                .up_to_message_id(new_id())
        )
        .await
        .as_ref()
        .err()
        .map(Error::kind),
        Some(ErrorKind::Config)
    );

    delete_session_via_store(&store, &id, directory)
        .await
        .unwrap();
    assert!(store.load(&key).await.unwrap().is_none());
    let append_only = CountingStore::new(SessionStoreCapabilities::none(), None);
    append_only.append(&key, &lines).await.unwrap();
    delete_session_via_store(&append_only, &id, directory)
        .await
        .unwrap();
    assert!(
        append_only.load(&key).await.unwrap().is_some(),
        "a store without delete is left unchanged"
    );
}

#[test]
fn fold_reproduces_the_python_reference() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/fold_session_summary.json")).unwrap();
    let key = SessionKey::new("proj", "sess");
    let mut summary: Option<SessionSummaryEntry> = None;
    for (index, (batch, expected)) in fixture["batches"]
        .as_array()
        .unwrap()
        .iter()
        .zip(fixture["outputs"].as_array().unwrap())
        .enumerate()
    {
        let entries: Vec<SessionStoreEntry> = batch
            .as_array()
            .unwrap()
            .iter()
            .cloned()
            .map(entry)
            .collect();
        let mut folded = fold_session_summary(summary.as_ref(), &key, &entries);
        folded.mtime = 1000 + i64::try_from(index).unwrap();
        assert_eq!(folded.session_id, expected["session_id"].as_str().unwrap());
        assert_eq!(folded.mtime, expected["mtime"].as_i64().unwrap());
        assert_eq!(
            Value::Object(folded.data.clone()),
            expected["data"],
            "batch {index}"
        );
        summary = Some(folded);
    }
}

#[tokio::test]
async fn shared_stores_work_with_the_helpers() {
    let shared = claude_agent_rust_sdk::SharedSessionStore::new(InMemorySessionStore::new());
    let project = Project::new();
    let ids = fill(&shared, &project_key_for_directory(Some(project.path())), 2).await;
    let listed = list_sessions_from_store(
        &shared,
        ListSessionsOptions::new().directory(project.path()),
    )
    .await
    .unwrap();
    assert_eq!(listed.len(), 2);
    assert!(listed.iter().all(|info| ids.contains(&info.session_id)));
}

#[tokio::test]
async fn an_agent_id_that_is_not_one_segment_loads_nothing() {
    let project = Project::new();
    let directory = Some(project.path());
    let store = CountingStore::new(SessionStoreCapabilities::none(), None);
    let id = new_id();
    let key = SessionKey::new(project_key_for_directory(directory), id.as_str());
    store
        .append(
            &key.clone().with_subpath("subagents/agent-ok"),
            &[entry(
                json!({"type": "user", "uuid": "u1", "message": {"content": "task"}}),
            )],
        )
        .await
        .unwrap();
    for agent in ["", ".", "..", "x/../../secret", r"x\..\secret", "a\0b"] {
        let messages = get_subagent_messages_from_store(
            &store,
            &id,
            agent,
            SessionMessagesOptions::new().directory(project.path()),
        )
        .await
        .unwrap();
        assert!(messages.is_empty(), "{agent:?}");
    }
    assert_eq!(store.loads.load(Ordering::SeqCst), 0);
    let messages = get_subagent_messages_from_store(
        &store,
        &id,
        "ok",
        SessionMessagesOptions::new().directory(project.path()),
    )
    .await
    .unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(store.loads.load(Ordering::SeqCst), 1);
}

fn sidechain(session_id: &str) -> SessionStoreEntry {
    let mut line = user(&new_id(), None, session_id, Value::from("internal"));
    line["isSidechain"] = Value::Bool(true);
    entry(line)
}

#[tokio::test]
async fn sidechain_sessions_are_omitted_from_a_store_listing() {
    let project = Project::new();
    let key = project_key(&project);
    let store = InMemorySessionStore::new();
    let normal = new_id();
    let hidden = new_id();
    store
        .append(
            &SessionKey::new(key.as_str(), normal.as_str()),
            &[entry(user(
                &new_id(),
                None,
                &normal,
                Value::from("hello world"),
            ))],
        )
        .await
        .unwrap();
    store
        .append(
            &SessionKey::new(key.as_str(), hidden.as_str()),
            &[sidechain(&hidden)],
        )
        .await
        .unwrap();

    let listed =
        list_sessions_from_store(&store, ListSessionsOptions::new().directory(project.path()))
            .await
            .unwrap();
    let ids: Vec<&str> = listed.iter().map(|info| info.session_id.as_str()).collect();
    assert_eq!(ids, [normal.as_str()]);
    assert!(listed.iter().all(|info| !info.summary.is_empty()));
}

#[tokio::test]
async fn the_per_session_load_path_drops_sidechains_before_it_paginates() {
    let project = Project::new();
    let key = project_key(&project);
    let store = CountingStore::new(SessionStoreCapabilities::none().with_list_sessions(), None);
    let valid: HashSet<String> = fill(&store, &key, 5).await.into_iter().collect();
    for _ in 0..3 {
        let hidden = new_id();
        store
            .append(
                &SessionKey::new(key.as_str(), hidden.as_str()),
                &[sidechain(&hidden)],
            )
            .await
            .unwrap();
    }

    let page = list_sessions_from_store(
        &store,
        ListSessionsOptions::new()
            .directory(project.path())
            .limit(5),
    )
    .await
    .unwrap();
    assert_eq!(page.len(), 5, "sidechains take no slot of the page");
    let listed: HashSet<String> = page.into_iter().map(|info| info.session_id).collect();
    assert_eq!(listed, valid);

    let second = list_sessions_from_store(
        &store,
        ListSessionsOptions::new()
            .directory(project.path())
            .limit(5)
            .offset(2),
    )
    .await
    .unwrap();
    assert_eq!(second.len(), 3);
    assert!(second.iter().all(|info| valid.contains(&info.session_id)));
}

#[tokio::test]
async fn agent_metadata_values_that_are_not_strings_give_no_parent_ids() {
    let project = Project::new();
    let store = InMemorySessionStore::new();
    let id = new_id();
    store
        .append(
            &SessionKey::new(project_key(&project), id.as_str()).with_subpath("subagents/agent-x"),
            &[
                entry(json!({"type": "agent_metadata", "toolUseId": 7, "parentAgentId": null})),
                entry(user(&new_id(), None, &id, Value::from("hi"))),
            ],
        )
        .await
        .unwrap();
    let messages = get_subagent_messages_from_store(
        &store,
        &id,
        "x",
        SessionMessagesOptions::new().directory(project.path()),
    )
    .await
    .unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].parent_tool_use_id, None);
    assert_eq!(messages[0].parent_agent_id, None);
}

#[tokio::test]
async fn an_agent_id_under_several_subpaths_is_listed_once() {
    let project = Project::new();
    let store = InMemorySessionStore::new();
    let id = new_id();
    let key = SessionKey::new(project_key(&project), id.as_str());
    let message = new_id();
    for subpath in ["subagents/agent-abc", "subagents/workflows/run-1/agent-abc"] {
        store
            .append(
                &key.clone().with_subpath(subpath),
                &[entry(user(&message, None, &id, Value::from("x")))],
            )
            .await
            .unwrap();
    }
    assert_eq!(
        list_subagents_from_store(&store, &id, Some(project.path()))
            .await
            .unwrap(),
        ["abc"]
    );
}

#[tokio::test]
async fn subagent_helpers_return_nothing_for_a_session_id_that_is_not_a_uuid() {
    let project = Project::new();
    let directory = Some(project.path());
    let store = InMemorySessionStore::new();
    assert!(
        list_subagents_from_store(&store, "not-a-uuid", directory)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        get_subagent_messages_from_store(
            &store,
            "not-a-uuid",
            "x",
            SessionMessagesOptions::new().directory(project.path())
        )
        .await
        .unwrap()
        .is_empty()
    );
}

fn config_message<T: std::fmt::Debug>(outcome: Result<T, Error>) -> String {
    match outcome {
        Err(error) if error.kind() == ErrorKind::Config => error.to_string(),
        other => panic!("expected a Config error, got {other:?}"),
    }
}

#[tokio::test]
async fn delete_and_tag_via_store_reject_an_id_that_is_not_a_uuid_and_leave_the_store_alone() {
    let project = Project::new();
    let directory = Some(project.path());
    let store = CountingStore::new(SessionStoreCapabilities::all(), None);
    let deleted = delete_session_via_store(&store, "not-a-uuid", directory).await;
    assert!(config_message(deleted).contains("not-a-uuid"));
    let tagged = tag_session_via_store(&store, "not-a-uuid", Some("tag"), directory).await;
    assert!(config_message(tagged).contains("not-a-uuid"));
    assert_eq!(store.appends.load(Ordering::SeqCst), 0);
    assert_eq!(store.inner.size(), 0);
}

struct Source {
    project: Project,
    store: InMemorySessionStore,
    id: String,
}

impl Source {
    fn new() -> Self {
        Self {
            project: Project::new(),
            store: InMemorySessionStore::new(),
            id: new_id(),
        }
    }

    fn key_of(&self, session_id: &str) -> SessionKey {
        SessionKey::new(project_key(&self.project), session_id)
    }

    async fn append(&self, lines: Vec<Value>) {
        let entries: Vec<SessionStoreEntry> = lines.into_iter().map(entry).collect();
        self.store
            .append(&self.key_of(&self.id), &entries)
            .await
            .unwrap();
    }

    async fn seed_chain(&self, pairs: usize) -> Vec<String> {
        let mut uuids: Vec<String> = Vec::new();
        let mut lines = Vec::new();
        for index in 0..pairs {
            let prompt = new_id();
            let reply = new_id();
            lines.push(user(
                &prompt,
                uuids.last().map(String::as_str),
                &self.id,
                Value::from(format!("prompt {index}")),
            ));
            lines.push(assistant(
                &reply,
                Some(&prompt),
                &self.id,
                &format!("reply {index}"),
            ));
            uuids.extend([prompt, reply]);
        }
        self.append(lines).await;
        uuids
    }

    async fn fork(
        &self,
        up_to_message_id: Option<&str>,
        title: Option<&str>,
    ) -> (String, Vec<SessionStoreEntry>) {
        let mut options = ForkSessionOptions::new().directory(self.project.path());
        options.up_to_message_id = up_to_message_id.map(str::to_owned);
        options.title = title.map(str::to_owned);
        let fork = fork_session_via_store(&self.store, &self.id, options)
            .await
            .unwrap();
        let entries = self.store.get_entries(&self.key_of(&fork.session_id));
        (fork.session_id, entries)
    }
}

fn has_a_uuid_and_a_timestamp(entry: &SessionStoreEntry) -> bool {
    let filled = |name: &str| {
        entry
            .get(name)
            .and_then(Value::as_str)
            .is_some_and(|text| !text.is_empty())
    };
    filled("uuid") && filled("timestamp")
}

#[tokio::test]
async fn a_store_fork_without_a_custom_title_is_titled_after_the_generated_title() {
    let source = Source::new();
    source.seed_chain(1).await;
    source
        .append(vec![
            json!({"type": "ai-title", "aiTitle": "Generated", "sessionId": source.id}),
        ])
        .await;
    let (_, forked) = source.fork(None, None).await;
    let title = forked.last().unwrap();
    assert_eq!(title["type"], "custom-title");
    assert_eq!(title["customTitle"], "Generated (fork)");
}

#[tokio::test]
async fn the_content_replacement_entry_of_a_store_fork_is_stamped_for_the_new_session() {
    let source = Source::new();
    source.seed_chain(1).await;
    source
        .append(vec![json!({
            "type": "content-replacement",
            "sessionId": source.id,
            "replacements": [{"toolUseId": "t1", "value": "redacted"}],
        })])
        .await;
    let (fork_id, forked) = source.fork(None, None).await;
    let replacement = forked
        .iter()
        .find(|entry| entry["type"] == "content-replacement")
        .expect("a content-replacement entry");
    assert_eq!(replacement["sessionId"], fork_id.as_str());
    assert!(has_a_uuid_and_a_timestamp(replacement), "{replacement:?}");
}

#[tokio::test]
async fn a_store_fork_is_read_back_with_every_source_message() {
    let source = Source::new();
    source.seed_chain(2).await;
    let (fork_id, _) = source.fork(None, Some("Forked")).await;
    let messages = get_session_messages_from_store(
        &source.store,
        &fork_id,
        SessionMessagesOptions::new().directory(source.project.path()),
    )
    .await
    .unwrap();
    assert_eq!(messages.len(), 4);
}

#[tokio::test]
async fn a_store_fork_rejects_a_session_id_or_a_cut_message_id_that_is_not_a_uuid() {
    let source = Source::new();
    let bad_session = fork_session_via_store(
        &source.store,
        "not-a-uuid",
        ForkSessionOptions::new().directory(source.project.path()),
    )
    .await;
    assert!(config_message(bad_session).contains("Invalid session_id"));
    source.seed_chain(2).await;
    let bad_cut = fork_session_via_store(
        &source.store,
        &source.id,
        ForkSessionOptions::new()
            .directory(source.project.path())
            .up_to_message_id("not-a-uuid"),
    )
    .await;
    assert!(config_message(bad_cut).contains("Invalid up_to_message_id"));
    assert_eq!(source.store.size(), 1, "no fork was written");
}

#[tokio::test]
async fn a_store_fork_cut_at_a_message_keeps_the_chain_and_stamps_its_synthetic_entries() {
    let source = Source::new();
    let ids = [new_id(), new_id(), new_id()];
    source
        .append(vec![
            user(&ids[0], None, &source.id, Value::from("one")),
            assistant(&ids[1], Some(&ids[0]), &source.id, "two"),
            user(&ids[2], Some(&ids[1]), &source.id, Value::from("three")),
            json!({
                "type": "content-replacement",
                "sessionId": source.id,
                "replacements": [{"toolUseId": "tu_1", "newContent": "x"}],
            }),
        ])
        .await;

    let (fork_id, forked) = source.fork(Some(&ids[1]), Some("My Fork")).await;
    assert_ne!(fork_id, source.id);
    assert_eq!(
        forked.len(),
        4,
        "two messages, the content replacement, the title"
    );
    let (first, second, replacement, title) = (&forked[0], &forked[1], &forked[2], &forked[3]);

    assert_ne!(first["uuid"], ids[0].as_str());
    assert!(first["parentUuid"].is_null());
    assert_eq!(second["parentUuid"], first["uuid"]);
    assert_eq!(first["sessionId"], fork_id.as_str());
    assert_eq!(first["forkedFrom"]["messageUuid"], ids[0].as_str());

    assert_eq!(replacement["type"], "content-replacement");
    assert_eq!(replacement["sessionId"], fork_id.as_str());
    assert!(has_a_uuid_and_a_timestamp(replacement), "{replacement:?}");

    assert_eq!(title["type"], "custom-title");
    assert_eq!(title["customTitle"], "My Fork");
    assert!(has_a_uuid_and_a_timestamp(title), "{title:?}");
}

fn plain(index: usize) -> Value {
    json!({
        "type": "user",
        "uuid": format!("u{index}"),
        "timestamp": format!("2026-01-01T00:00:{index:02}Z"),
    })
}

fn plain_entries(indexes: impl IntoIterator<Item = usize>) -> Vec<SessionStoreEntry> {
    indexes.into_iter().map(plain).map(entry).collect()
}

fn project_key(project: &Project) -> String {
    project_key_for_directory(Some(project.path()))
}

#[tokio::test]
async fn import_skips_the_blank_lines_of_a_transcript() {
    let project = Project::new();
    let id = new_id();
    std::fs::write(
        project.transcript(&id),
        format!("{}\n\n{}\n", plain(0), plain(1)),
    )
    .unwrap();
    let store = InMemorySessionStore::new();
    import_session_to_store(
        &id,
        &store,
        ImportSessionOptions::new().directory(project.path()),
    )
    .await
    .unwrap();
    assert_eq!(
        store.get_entries(&SessionKey::new(project_key(&project), id.as_str())),
        plain_entries([0, 1])
    );
}

#[tokio::test]
async fn import_skips_a_transcript_line_that_is_not_a_json_object() {
    let project = Project::new();
    let id = new_id();
    std::fs::write(
        project.transcript(&id),
        format!("{}\n[1,2]\n\"text\"\n{}\n", plain(0), plain(1)),
    )
    .unwrap();
    let store = InMemorySessionStore::new();
    import_session_to_store(
        &id,
        &store,
        ImportSessionOptions::new().directory(project.path()),
    )
    .await
    .unwrap();
    assert_eq!(
        store.get_entries(&SessionKey::new(project_key(&project), id.as_str())),
        plain_entries([0, 1])
    );
}

#[tokio::test]
async fn import_leaves_out_a_subagent_file_named_only_jsonl() {
    let project = Project::new();
    let id = new_id();
    project.write(&id, &[plain(0)]);
    let subagents = project.dir.join(&id).join("subagents");
    write_lines(&subagents.join(".jsonl"), &[plain(5)]);
    write_lines(&subagents.join("agent-a.jsonl"), &[plain(6)]);
    let store = InMemorySessionStore::new();
    import_session_to_store(
        &id,
        &store,
        ImportSessionOptions::new().directory(project.path()),
    )
    .await
    .unwrap();
    assert_eq!(
        store
            .list_subkeys(&SessionListSubkeysKey::new(
                project_key(&project),
                id.as_str()
            ))
            .await
            .unwrap(),
        ["subagents/agent-a"]
    );
}

/// The bytes of a transcript line whose prompt holds `ab`, the byte 0xff,
/// which is not UTF-8, and `cd`.
fn line_with_invalid_utf8(line: &Value) -> Vec<u8> {
    let text = line.to_string();
    let (before, after) = text.split_once("MARK").unwrap();
    [before.as_bytes(), b"ab\xffcd", after.as_bytes(), b"\n"].concat()
}

#[tokio::test]
async fn transcripts_that_are_not_utf8_are_read_and_imported_with_replacement_characters() {
    let project = Project::new();
    let id = new_id();
    let prompt = user("u1", None, &id, Value::from("MARK"));
    std::fs::write(project.transcript(&id), line_with_invalid_utf8(&prompt)).unwrap();
    let subagent = project
        .dir
        .join(&id)
        .join("subagents")
        .join("agent-a.jsonl");
    std::fs::create_dir_all(subagent.parent().unwrap()).unwrap();
    std::fs::write(
        &subagent,
        line_with_invalid_utf8(&user("s1", None, &id, Value::from("MARK"))),
    )
    .unwrap();

    let messages =
        get_session_messages(&id, SessionMessagesOptions::new().directory(project.path()));
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].message["content"], "ab\u{fffd}cd");
    let messages = get_subagent_messages(
        &id,
        "a",
        SessionMessagesOptions::new().directory(project.path()),
    );
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].message["content"], "ab\u{fffd}cd");

    let store = InMemorySessionStore::new();
    import_session_to_store(
        &id,
        &store,
        ImportSessionOptions::new().directory(project.path()),
    )
    .await
    .unwrap();
    let imported = store.get_entries(&SessionKey::new(project_key(&project), id.as_str()));
    assert_eq!(imported[0]["message"]["content"], "ab\u{fffd}cd");
}

#[tokio::test]
async fn the_import_byte_limit_counts_utf8_bytes() {
    let project = Project::new();
    let id = new_id();
    let lines: Vec<Value> = (0..3)
        .map(|index| {
            let mut line = plain(index);
            line["data"] = Value::from("\u{4f60}".repeat(200_000));
            line
        })
        .collect();
    project.write(&id, &lines);
    let store = CountingStore::new(SessionStoreCapabilities::all(), None);
    import_session_to_store(
        &id,
        &store,
        ImportSessionOptions::new()
            .directory(project.path())
            .include_subagents(false),
    )
    .await
    .unwrap();
    assert_eq!(
        store.appends.load(Ordering::SeqCst),
        2,
        "two lines of 600 KB reach 1 MiB, then the last one"
    );
}

#[tokio::test]
async fn a_store_session_has_the_size_of_the_transcript_the_cli_writes() {
    let project = Project::new();
    let id = new_id();
    let path = project.write(
        &id,
        &conversation(&id, "caf\u{e9} \u{4f60}\u{597d} \u{1f600}", json!({})),
    );
    let store = InMemorySessionStore::new();
    import_session_to_store(
        &id,
        &store,
        ImportSessionOptions::new()
            .directory(project.path())
            .include_subagents(false),
    )
    .await
    .unwrap();
    let on_disk = std::fs::metadata(&path).unwrap().len();
    let stored = get_session_info_from_store(&store, &id, Some(project.path()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.file_size, Some(on_disk));
    assert_eq!(
        get_session_info(&id, Some(project.path()))
            .unwrap()
            .file_size,
        Some(on_disk)
    );
}

#[tokio::test]
async fn a_batch_size_of_zero_imports_in_batches_of_500_entries() {
    let project = Project::new();
    let small = new_id();
    project.write(&small, &[plain(0), plain(1), plain(2)]);
    let store = CountingStore::new(SessionStoreCapabilities::all(), None);
    import_session_to_store(
        &small,
        &store,
        ImportSessionOptions::new()
            .directory(project.path())
            .batch_size(0),
    )
    .await
    .unwrap();
    assert_eq!(store.appends.load(Ordering::SeqCst), 1);
    assert_eq!(
        store
            .inner
            .get_entries(&SessionKey::new(project_key(&project), small.as_str())),
        plain_entries(0..3)
    );

    let large = new_id();
    project.write(&large, &(0..501).map(plain).collect::<Vec<_>>());
    store.appends.store(0, Ordering::SeqCst);
    import_session_to_store(
        &large,
        &store,
        ImportSessionOptions::new()
            .directory(project.path())
            .batch_size(0),
    )
    .await
    .unwrap();
    assert_eq!(
        store.appends.load(Ordering::SeqCst),
        2,
        "500 entries, then the last one"
    );
}

fn subagent_with_sidecar(project: &Project, sidecar: &str) -> (String, SessionKey) {
    let id = new_id();
    project.write(&id, &[plain(0)]);
    let subagents = project.dir.join(&id).join("subagents");
    write_lines(&subagents.join("agent-abc.jsonl"), &[plain(10)]);
    std::fs::write(subagents.join("agent-abc.meta.json"), sidecar).unwrap();
    let key =
        SessionKey::new(project_key(project), id.as_str()).with_subpath("subagents/agent-abc");
    (id, key)
}

#[tokio::test]
async fn a_type_key_in_a_sidecar_cannot_replace_the_agent_metadata_marker() {
    let project = Project::new();
    let (id, key) = subagent_with_sidecar(
        &project,
        &json!({"type": "something-else", "toolUseId": "toolu_1"}).to_string(),
    );
    let store = InMemorySessionStore::new();
    import_session_to_store(
        &id,
        &store,
        ImportSessionOptions::new().directory(project.path()),
    )
    .await
    .unwrap();
    assert_eq!(
        store.get_entries(&key),
        [
            entry(plain(10)),
            entry(json!({"type": "agent_metadata", "toolUseId": "toolu_1"})),
        ]
    );
}

#[tokio::test]
async fn an_unreadable_sidecar_fails_the_import() {
    let project = Project::new();
    let (id, _) = subagent_with_sidecar(&project, "{}");
    let sidecar = project
        .dir
        .join(&id)
        .join("subagents")
        .join("agent-abc.meta.json");
    std::fs::remove_file(&sidecar).unwrap();
    std::fs::create_dir(&sidecar).unwrap();
    let store = InMemorySessionStore::new();
    let error = import_session_to_store(
        &id,
        &store,
        ImportSessionOptions::new().directory(project.path()),
    )
    .await
    .unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Io, "{error:?}");
    assert!(error.to_string().contains("agent-abc.meta.json"), "{error}");
}

#[tokio::test]
async fn a_sidecar_that_is_not_utf8_is_imported_as_if_it_were_absent() {
    let project = Project::new();
    let (id, key) = subagent_with_sidecar(&project, "{}");
    std::fs::write(
        project
            .dir
            .join(&id)
            .join("subagents")
            .join("agent-abc.meta.json"),
        b"{\"agentType\": \"\xff\"}",
    )
    .unwrap();
    let store = InMemorySessionStore::new();
    import_session_to_store(
        &id,
        &store,
        ImportSessionOptions::new().directory(project.path()),
    )
    .await
    .unwrap();
    assert_eq!(store.get_entries(&key), plain_entries([10]));
}

#[tokio::test]
async fn a_corrupt_or_non_object_sidecar_is_imported_as_if_it_were_absent() {
    let project = Project::new();
    for sidecar in ["not json {", "[1, 2]", "42"] {
        let (id, key) = subagent_with_sidecar(&project, sidecar);
        let store = InMemorySessionStore::new();
        import_session_to_store(
            &id,
            &store,
            ImportSessionOptions::new().directory(project.path()),
        )
        .await
        .unwrap();
        assert_eq!(store.get_entries(&key), plain_entries([10]), "{sidecar}");
        assert_eq!(
            store.get_entries(&SessionKey::new(project_key(&project), id.as_str())),
            plain_entries([0]),
            "{sidecar}"
        );
    }
}

#[tokio::test]
async fn import_without_a_directory_keys_the_session_by_the_project_it_was_found_in() {
    let project = Project::new();
    let id = new_id();
    project.write(&id, &[plain(0)]);
    assert_ne!(
        project_key_for_directory(None),
        project_key(&project),
        "the current directory is another project"
    );
    let store = InMemorySessionStore::new();
    import_session_to_store(&id, &store, ImportSessionOptions::new())
        .await
        .unwrap();
    assert_eq!(
        store.get_entries(&SessionKey::new(project_key(&project), id.as_str())),
        plain_entries([0])
    );
    assert_eq!(store.size(), 1);
}
