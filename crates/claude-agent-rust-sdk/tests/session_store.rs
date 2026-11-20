#![cfg(feature = "testing")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use claude_agent_rust_sdk::ErrorKind;
use claude_agent_rust_sdk::testing::{
    MockTransport, run_session_store_conformance, run_session_store_conformance_skipping,
};
use claude_agent_rust_sdk::{
    ClaudeAgentOptions, ClaudeSdkClient, Error, InMemorySessionStore, Message, SessionKey,
    SessionListSubkeysKey, SessionStore, SessionStoreCapabilities, SessionStoreEntry,
    SessionStoreError, SharedSessionStore, UnsupportedStoreOperation, file_path_to_session_key,
    project_key_for_directory, query_with_transport,
};
use futures_util::StreamExt;
use serde_json::json;

fn entry(value: serde_json::Value) -> SessionStoreEntry {
    value.as_object().cloned().unwrap()
}

#[tokio::test]
async fn the_in_memory_store_passes_every_contract() {
    run_session_store_conformance(|| async { InMemorySessionStore::new() })
        .await
        .unwrap();
}

#[tokio::test]
async fn the_shared_handle_passes_every_contract() {
    run_session_store_conformance(|| async {
        SharedSessionStore::new(InMemorySessionStore::new())
    })
    .await
    .unwrap();
}

#[derive(Default, Clone)]
struct MinimalStore {
    entries: Arc<Mutex<HashMap<SessionKey, Vec<SessionStoreEntry>>>>,
}

impl SessionStore for MinimalStore {
    async fn append(
        &self,
        key: &SessionKey,
        entries: &[SessionStoreEntry],
    ) -> Result<(), SessionStoreError> {
        self.entries
            .lock()
            .unwrap()
            .entry(key.clone())
            .or_default()
            .extend(entries.iter().cloned());
        Ok(())
    }

    async fn load(
        &self,
        key: &SessionKey,
    ) -> Result<Option<Vec<SessionStoreEntry>>, SessionStoreError> {
        Ok(self.entries.lock().unwrap().get(key).cloned())
    }
}

#[tokio::test]
async fn a_store_with_only_the_required_methods_passes_and_reports_unsupported() {
    run_session_store_conformance(|| async { MinimalStore::default() })
        .await
        .unwrap();
    let store = MinimalStore::default();
    assert_eq!(store.capabilities(), SessionStoreCapabilities::none());
    let error = store.list_sessions("p").await.unwrap_err();
    assert_eq!(
        error.downcast_ref::<UnsupportedStoreOperation>(),
        Some(&UnsupportedStoreOperation {
            operation: "list_sessions"
        })
    );
    assert!(store.delete(&SessionKey::new("p", "s")).await.is_err());
    assert!(
        store
            .list_subkeys(&SessionListSubkeysKey::new("p", "s"))
            .await
            .is_err()
    );
}

#[derive(Default, Clone)]
struct ReorderingStore(InMemorySessionStore);

impl SessionStore for ReorderingStore {
    async fn append(
        &self,
        key: &SessionKey,
        entries: &[SessionStoreEntry],
    ) -> Result<(), SessionStoreError> {
        let mut reversed = entries.to_vec();
        reversed.reverse();
        self.0.append(key, &reversed).await
    }

    async fn load(
        &self,
        key: &SessionKey,
    ) -> Result<Option<Vec<SessionStoreEntry>>, SessionStoreError> {
        self.0.load(key).await
    }
}

#[derive(Default, Clone)]
struct EmptyListStore(InMemorySessionStore);

impl SessionStore for EmptyListStore {
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

    fn capabilities(&self) -> SessionStoreCapabilities {
        SessionStoreCapabilities::none().with_list_sessions()
    }
}

#[tokio::test]
async fn broken_stores_fail_the_contract_they_break() {
    let reordering = run_session_store_conformance(|| async { ReorderingStore::default() })
        .await
        .unwrap_err();
    assert_eq!(reordering.contract, 1, "{reordering}");

    let declared_but_missing =
        run_session_store_conformance(|| async { EmptyListStore::default() })
            .await
            .unwrap_err();
    assert_eq!(declared_but_missing.contract, 6, "{declared_but_missing}");
    run_session_store_conformance_skipping(
        || async { EmptyListStore::default() },
        SessionStoreCapabilities::none().with_list_sessions(),
    )
    .await
    .unwrap();
}

#[derive(Default, Clone)]
struct NonCascadingStore(InMemorySessionStore);

impl SessionStore for NonCascadingStore {
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
        let deleted = self.0.get_entries(&key.clone().with_subpath("__deleted"));
        if !deleted.is_empty() && key.subpath.is_none() {
            return Ok(None);
        }
        self.0.load(key).await
    }

    fn capabilities(&self) -> SessionStoreCapabilities {
        SessionStoreCapabilities::none().with_delete()
    }

    async fn delete(&self, key: &SessionKey) -> Result<(), SessionStoreError> {
        self.0
            .append(
                &key.clone().with_subpath("__deleted"),
                &[entry(json!({"type": "x"}))],
            )
            .await
    }
}

#[tokio::test]
async fn a_delete_that_does_not_cascade_fails_contract_10() {
    let failure = run_session_store_conformance(|| async { NonCascadingStore::default() })
        .await
        .unwrap_err();
    assert_eq!(failure.contract, 10, "{failure}");
}

#[tokio::test]
async fn in_memory_store_semantics() {
    let store = InMemorySessionStore::new();
    let main = SessionKey::new("p", "s");
    let subagent = main.clone().with_subpath("subagents/agent-1");
    assert_eq!(store.load(&main).await.unwrap(), None);
    store
        .append(&main, &[entry(json!({"type": "user", "uuid": "u"}))])
        .await
        .unwrap();
    store.append(&main, &[]).await.unwrap();
    store
        .append(&subagent, &[entry(json!({"type": "user", "uuid": "a"}))])
        .await
        .unwrap();
    assert_eq!(store.get_entries(&main).len(), 1);
    assert_eq!(store.size(), 1);
    let listed = store.list_sessions("p").await.unwrap();
    assert_eq!(listed.len(), 1);
    let summaries = store.list_session_summaries("p").await.unwrap();
    assert_eq!(summaries[0].mtime, listed[0].mtime);
    store.delete(&main).await.unwrap();
    assert_eq!(store.load(&subagent).await.unwrap(), None);
    assert!(store.list_session_summaries("p").await.unwrap().is_empty());
    assert_eq!(store.size(), 0);
    store
        .append(&main, &[entry(json!({"type": "x"}))])
        .await
        .unwrap();
    store.clear();
    assert_eq!(store.size(), 0);
}

#[tokio::test]
async fn get_entries_returns_a_copy_of_what_was_appended_in_order() {
    let store = InMemorySessionStore::new();
    let key = SessionKey::new("proj", "sess");
    assert_eq!(store.get_entries(&key), Vec::<SessionStoreEntry>::new());
    let appended = [entry(json!({"n": 1})), entry(json!({"n": 2}))];
    store.append(&key, &appended).await.unwrap();
    assert_eq!(store.get_entries(&key), appended);
    store.get_entries(&key).push(entry(json!({"n": 999})));
    assert_eq!(store.get_entries(&key), appended);
}

fn mirror_frame(config: &Path, session: &str, uuid: &str) -> serde_json::Value {
    let file = config
        .join("projects")
        .join("proj")
        .join(format!("{session}.jsonl"));
    json!({
        "type": "transcript_mirror",
        "filePath": file.to_string_lossy(),
        "entries": [{"type": "user", "uuid": uuid, "sessionId": session}],
    })
}

fn answer(request: &serde_json::Value) -> serde_json::Value {
    json!({
        "type": "control_response",
        "response": {
            "subtype": "success",
            "request_id": request["request_id"],
            "response": {},
        },
    })
}

async fn mirrored(store: &InMemorySessionStore, session: &str) -> Vec<SessionStoreEntry> {
    let key = SessionKey::new("proj", session);
    for _ in 0..400 {
        if !store.get_entries(&key).is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    store.get_entries(&key)
}

#[tokio::test]
async fn pending_mirror_entries_reach_the_store_when_the_client_or_the_stream_is_dropped() {
    let config = tempfile::tempdir().unwrap();
    let store = InMemorySessionStore::new();
    let options = || {
        ClaudeAgentOptions::builder()
            .session_store(store.clone())
            .env_var("CLAUDE_CONFIG_DIR", config.path().to_string_lossy())
            .build()
            .unwrap()
    };

    let transport = MockTransport::new();
    transport.respond_with(|frame| {
        if frame["type"] == "control_request" {
            vec![answer(frame)]
        } else {
            Vec::new()
        }
    });
    let client = ClaudeSdkClient::with_transport(options(), transport.clone());
    client.connect().await.unwrap();
    transport.push_frame(mirror_frame(config.path(), "from-client", "c1"));
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(transport.pending_frames(), 0);
    assert!(
        store
            .get_entries(&SessionKey::new("proj", "from-client"))
            .is_empty()
    );
    drop(client);
    assert_eq!(
        mirrored(&store, "from-client").await,
        [entry(
            json!({"type": "user", "uuid": "c1", "sessionId": "from-client"})
        )]
    );

    let transport = MockTransport::new();
    let directory = config.path().to_path_buf();
    transport.respond_with(move |frame| {
        if frame["type"] == "control_request" {
            vec![answer(frame)]
        } else if frame["type"] == "user" {
            vec![
                mirror_frame(&directory, "from-stream", "q1"),
                json!({
                    "type": "assistant",
                    "parent_tool_use_id": null,
                    "session_id": "from-stream",
                    "message": {
                        "role": "assistant",
                        "model": "claude-sonnet-5",
                        "content": [{"type": "text", "text": "hi"}],
                    },
                }),
            ]
        } else {
            Vec::new()
        }
    });
    let mut messages = query_with_transport("hello", options(), transport.clone());
    loop {
        let message = tokio::time::timeout(Duration::from_secs(30), messages.next())
            .await
            .expect("the assistant message arrives")
            .expect("the stream is still open")
            .unwrap();
        if matches!(message, Message::Assistant(_)) {
            break;
        }
    }
    assert!(
        store
            .get_entries(&SessionKey::new("proj", "from-stream"))
            .is_empty()
    );
    drop(messages);
    assert_eq!(
        mirrored(&store, "from-stream").await,
        [entry(
            json!({"type": "user", "uuid": "q1", "sessionId": "from-stream"})
        )]
    );
}

#[test]
fn file_paths_map_to_keys_relative_to_the_projects_directory() {
    let projects = Path::new("/home/me/.claude/projects");
    assert_eq!(
        file_path_to_session_key(&projects.join("proj").join("sess.jsonl"), projects),
        Some(SessionKey::new("proj", "sess"))
    );
    assert_eq!(
        file_path_to_session_key(
            &projects
                .join("proj")
                .join("sess")
                .join("subagents")
                .join("workflows")
                .join("run")
                .join("agent-1.jsonl"),
            projects
        ),
        Some(SessionKey::new("proj", "sess").with_subpath("subagents/workflows/run/agent-1"))
    );
    assert_eq!(
        file_path_to_session_key(
            &projects.join("proj").join("sess").join("agent-1.meta.json"),
            projects
        ),
        None
    );
    assert_eq!(
        file_path_to_session_key(Path::new("/elsewhere/proj/sess.jsonl"), projects),
        None
    );
    assert_eq!(
        file_path_to_session_key(&projects.join("proj"), projects),
        None
    );
    assert_eq!(
        file_path_to_session_key(&projects.join("proj").join("sess.txt"), projects),
        None
    );
    assert_eq!(
        file_path_to_session_key(
            &projects.join("proj").join("sess").join("x.jsonl"),
            projects
        ),
        None
    );
    assert_eq!(
        file_path_to_session_key(&projects.join("..").join("proj").join("s.jsonl"), projects),
        None
    );
}

#[test]
fn project_keys_resolve_relative_directories_and_sanitize() {
    assert_eq!(
        project_key_for_directory(None),
        project_key_for_directory(Some(&std::env::current_dir().unwrap()))
    );
    assert_eq!(
        project_key_for_directory(Some(Path::new("."))),
        project_key_for_directory(None)
    );
    let key = project_key_for_directory(Some(Path::new("/tmp/my project!")));
    assert!(!key.contains('/') && !key.contains(' ') && !key.contains('!'));
    let base = tempfile::tempdir().unwrap();
    let composed = base.path().join("caf\u{e9}");
    std::fs::create_dir(&composed).unwrap();
    assert_eq!(
        project_key_for_directory(Some(&composed)),
        project_key_for_directory(Some(&base.path().join("cafe\u{301}")))
    );
}

#[test]
fn store_options_are_validated() {
    let minimal = || ClaudeAgentOptions::builder().session_store(MinimalStore::default());
    assert_eq!(
        minimal()
            .continue_conversation(true)
            .build()
            .as_ref()
            .err()
            .map(Error::kind),
        Some(ErrorKind::Config)
    );
    minimal()
        .continue_conversation(true)
        .resume("00000000-0000-4000-8000-000000000000")
        .build()
        .unwrap();
    let full = || ClaudeAgentOptions::builder().session_store(InMemorySessionStore::new());
    full().continue_conversation(true).build().unwrap();
    assert_eq!(
        full()
            .enable_file_checkpointing(true)
            .build()
            .as_ref()
            .err()
            .map(Error::kind),
        Some(ErrorKind::Config)
    );
    let shared = SharedSessionStore::new(InMemorySessionStore::new());
    let options = ClaudeAgentOptions::builder()
        .session_store(shared.clone())
        .build()
        .unwrap();
    assert!(options.session_store.unwrap().capabilities().list_sessions);
}
