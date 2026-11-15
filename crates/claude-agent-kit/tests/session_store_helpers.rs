#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use claude_agent_kit::{
    Error, InMemorySessionStore, SdkSessionInfo, SessionKey, SessionListSubkeysKey, SessionStore,
    SessionStoreCapabilities, SessionStoreEntry, SessionStoreError, SessionStoreListEntry,
    SessionSummaryEntry, delete_session_via_store, fold_session_summary, fork_session_via_store,
    get_session_info, get_session_info_from_store, get_session_messages,
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
    import_session_to_store(&id, &store, directory, true, 0)
        .await
        .unwrap();
    import_session_to_store(&second, &store, directory, true, 0)
        .await
        .unwrap();

    let disk = list_sessions(directory, None, 0, false);
    let stored = list_sessions_from_store(&store, directory, None, 0)
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
        get_session_messages(&id, directory, None, 0),
        get_session_messages_from_store(&store, &id, directory, None, 0)
            .await
            .unwrap()
    );
    assert_eq!(
        get_session_messages(&id, directory, Some(1), 1),
        get_session_messages_from_store(&store, &id, directory, Some(1), 1)
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
            get_subagent_messages(&id, agent, directory, None, 0),
            get_subagent_messages_from_store(&store, &id, agent, directory, None, 0)
                .await
                .unwrap(),
            "{agent}"
        );
    }
    let alpha = get_subagent_messages_from_store(&store, &id, "alpha", directory, None, 0)
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
        get_session_messages_from_store(&store, &new_id(), directory, None, 0)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        get_subagent_messages_from_store(&store, &id, "missing", directory, None, 0)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn import_batches_entries_and_stores_sidecars_as_agent_metadata() {
    let project = Project::new();
    let id = rich_session(&project);
    let counting = CountingStore::new(SessionStoreCapabilities::all(), None);
    import_session_to_store(&id, &counting, Some(project.path()), true, 2)
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
    import_session_to_store(&id, &without_subagents, None, false, 500)
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
    assert!(matches!(
        import_session_to_store("bad", &without_subagents, None, true, 0).await,
        Err(Error::Config { .. })
    ));
    assert!(matches!(
        import_session_to_store(&new_id(), &without_subagents, Some(project.path()), true, 0).await,
        Err(Error::SessionNotFound { .. })
    ));
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
        self.inner.list_sessions(project_key).await
    }

    async fn list_session_summaries(
        &self,
        project_key: &str,
    ) -> Result<Vec<SessionSummaryEntry>, SessionStoreError> {
        if let Some(summaries) = self.summaries.lock().unwrap().clone() {
            return Ok(summaries);
        }
        self.inner.list_session_summaries(project_key).await
    }

    async fn list_subkeys(
        &self,
        key: &SessionListSubkeysKey,
    ) -> Result<Vec<String>, SessionStoreError> {
        self.inner.list_subkeys(key).await
    }
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
    let store = CountingStore::new(
        SessionStoreCapabilities::none().with_list_sessions(),
        Some(Duration::from_millis(20)),
    );
    let ids = fill(&store, &key, 40).await;
    let listed = list_sessions_from_store(&store, Some(project.path()), None, 0)
        .await
        .unwrap();
    assert_eq!(listed.len(), 40);
    assert_eq!(store.loads.load(Ordering::SeqCst), 40);
    let peak = store.peak.load(Ordering::SeqCst);
    assert!((2..=16).contains(&peak), "peak concurrency {peak}");
    assert_eq!(listed[0].session_id, ids[39], "newest first");
    let page = list_sessions_from_store(&store, Some(project.path()), Some(5), 2)
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
    let listed = list_sessions_from_store(&store, Some(project.path()), None, 0)
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

    let listed = list_sessions_from_store(&store, Some(project.path()), None, 0)
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
    let page = list_sessions_from_store(&store, Some(project.path()), Some(2), 0)
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
    let error = list_sessions_from_store(&store, None, None, 0)
        .await
        .unwrap_err();
    assert!(
        matches!(error, Error::StoreUnsupported(ref unsupported) if unsupported.operation == "list_sessions"),
        "{error}"
    );
    let error = list_subagents_from_store(&store, &new_id(), None)
        .await
        .unwrap_err();
    assert!(
        matches!(error, Error::StoreUnsupported(ref unsupported) if unsupported.operation == "list_subkeys"),
        "{error}"
    );
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
    let messages =
        get_subagent_messages_from_store(&store, &id, "x", Some(project.path()), None, 0)
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
    assert_eq!(stored[2]["tag"], "ab");
    assert_eq!(stored[3]["tag"], "");
    assert!(stored[3]["uuid"].is_string() && stored[3]["timestamp"].is_string());
    let info = get_session_info_from_store(&store, &id, directory)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(info.summary, "Renamed");
    assert_eq!(info.tag, None);
    assert!(matches!(
        rename_session_via_store(&store, &id, " ", directory).await,
        Err(Error::Config { .. })
    ));
    assert!(matches!(
        tag_session_via_store(&store, "bad", None, directory).await,
        Err(Error::Config { .. })
    ));

    let fork = fork_session_via_store(&store, &id, directory, None, None)
        .await
        .unwrap();
    let forked = store.get_entries(&SessionKey::new(
        key_text.as_str(),
        fork.session_id.as_str(),
    ));
    assert_eq!(forked.len(), 2);
    assert_eq!(forked[0]["forkedFrom"]["sessionId"], id.as_str());
    assert_eq!(forked[1]["customTitle"], "Renamed (fork)");
    assert!(matches!(
        fork_session_via_store(&store, &new_id(), directory, None, None).await,
        Err(Error::SessionNotFound { .. })
    ));
    assert!(matches!(
        fork_session_via_store(&store, &id, directory, Some(&new_id()), None).await,
        Err(Error::Config { .. })
    ));

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
    let shared = claude_agent_kit::SharedSessionStore::new(InMemorySessionStore::new());
    let project = Project::new();
    let ids = fill(&shared, &project_key_for_directory(Some(project.path())), 2).await;
    let listed = list_sessions_from_store(&shared, Some(project.path()), None, 0)
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
        let messages = get_subagent_messages_from_store(&store, &id, agent, directory, None, 0)
            .await
            .unwrap();
        assert!(messages.is_empty(), "{agent:?}");
    }
    assert_eq!(store.loads.load(Ordering::SeqCst), 0);
    let messages = get_subagent_messages_from_store(&store, &id, "ok", directory, None, 0)
        .await
        .unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(store.loads.load(Ordering::SeqCst), 1);
}
