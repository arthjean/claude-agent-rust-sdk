#![cfg(feature = "testing")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

common::transcripts::shared_config_dir_before_main!();

use std::path::PathBuf;
use std::time::Duration;

use claude_agent_rust_sdk::testing::MockTransport;
use claude_agent_rust_sdk::{
    ClaudeAgentOptions, ClaudeSdkClient, InMemorySessionStore, Message, SessionKey, SessionStore,
    SessionStoreEntry, SessionStoreError, SessionStoreFlushMode, SystemMessage,
    file_path_to_session_key, query_with_transport,
};
use futures_util::StreamExt;
use serde_json::{Value, json};

use common::mock::{Recorder, assistant, cli_answering, result, silent_cli, success};

fn config_dir() -> PathBuf {
    std::env::temp_dir().join("claude-agent-rust-sdk-mirror-test-config")
}

fn transcript_path(session: &str) -> String {
    config_dir()
        .join("projects")
        .join("proj")
        .join(format!("{session}.jsonl"))
        .to_string_lossy()
        .into_owned()
}

fn mirror_frame(session: &str, uuids: &[&str]) -> Value {
    let entries: Vec<Value> = uuids
        .iter()
        .map(|uuid| json!({"type": "user", "uuid": uuid, "sessionId": session}))
        .collect();
    json!({"type": "transcript_mirror", "filePath": transcript_path(session), "entries": entries})
}

fn options(store: impl SessionStore) -> ClaudeAgentOptions {
    ClaudeAgentOptions::builder()
        .session_store(store)
        .env([(
            "CLAUDE_CONFIG_DIR",
            config_dir().to_string_lossy().into_owned(),
        )])
        .build()
        .unwrap()
}

fn uuids(store: &InMemorySessionStore, session: &str) -> Vec<String> {
    store
        .get_entries(&SessionKey::new("proj", session))
        .iter()
        .map(|entry| entry["uuid"].as_str().unwrap_or_default().to_owned())
        .collect()
}

#[tokio::test]
async fn the_store_holds_the_turn_before_its_result_is_delivered() {
    let store = InMemorySessionStore::new();
    let transport = cli_answering(|frame| {
        if frame["type"] == "user" {
            vec![
                mirror_frame("s1", &["u1"]),
                assistant("hi"),
                mirror_frame("s1", &["a1", "a2"]),
                mirror_frame("s2", &["x1"]),
                result("hi"),
            ]
        } else {
            Vec::new()
        }
    });
    let mut messages = query_with_transport("hello", options(store.clone()), transport.clone());
    let mut seen_result = false;
    while let Some(message) = messages.next().await {
        match message.unwrap() {
            Message::Result(_) => {
                assert_eq!(uuids(&store, "s1"), ["u1", "a1", "a2"]);
                assert_eq!(uuids(&store, "s2"), ["x1"]);
                seen_result = true;
            }
            Message::System(system) => {
                panic!("no mirror frame reaches the application: {system:?}")
            }
            _ => {}
        }
    }
    assert!(seen_result);
}

#[tokio::test]
async fn pending_entries_are_flushed_when_the_output_ends() {
    let store = InMemorySessionStore::new();
    let transport = cli_answering(|frame| {
        if frame["type"] == "user" {
            vec![mirror_frame("s3", &["late"])]
        } else {
            Vec::new()
        }
    });
    let messages = query_with_transport("hello", options(store.clone()), transport.clone());
    let collected: Vec<_> = messages.collect().await;
    assert!(collected.iter().all(Result::is_ok));
    assert_eq!(uuids(&store, "s3"), ["late"]);
}

#[tokio::test]
async fn pending_entries_are_flushed_on_disconnect() {
    let store = InMemorySessionStore::new();
    let transport = silent_cli();
    transport.respond_with(|frame| {
        if frame["type"] == "control_request" {
            vec![success(frame, json!({}))]
        } else {
            Vec::new()
        }
    });
    let client = ClaudeSdkClient::with_transport(options(store.clone()), transport.clone());
    client.connect().await.unwrap();
    transport.push_frame(mirror_frame("s4", &["c1"]));
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    assert!(uuids(&store, "s4").is_empty());
    client.disconnect().await.unwrap();
    assert_eq!(uuids(&store, "s4"), ["c1"]);
}

#[tokio::test]
async fn eager_mode_mirrors_without_waiting_for_the_result() {
    let store = InMemorySessionStore::new();
    let transport = silent_cli();
    transport.respond_with(|frame| {
        if frame["type"] == "control_request" {
            vec![success(frame, json!({}))]
        } else {
            Vec::new()
        }
    });
    let mut options = options(store.clone());
    options.session_store_flush = SessionStoreFlushMode::Eager;
    let client = ClaudeSdkClient::with_transport(options, transport.clone());
    client.connect().await.unwrap();
    transport.push_frame(mirror_frame("s5", &["e1"]));
    for _ in 0..100 {
        if !uuids(&store, "s5").is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    assert_eq!(uuids(&store, "s5"), ["e1"]);
    client.disconnect().await.unwrap();
}

struct DownStore;

impl SessionStore for DownStore {
    async fn append(
        &self,
        _key: &SessionKey,
        _entries: &[SessionStoreEntry],
    ) -> Result<(), SessionStoreError> {
        Err("the store is down".into())
    }

    async fn load(
        &self,
        _key: &SessionKey,
    ) -> Result<Option<Vec<SessionStoreEntry>>, SessionStoreError> {
        Ok(None)
    }
}

#[tokio::test(start_paused = true)]
async fn a_dropped_batch_is_delivered_as_a_mirror_error_message() {
    let transport = cli_answering(|frame| {
        if frame["type"] == "user" {
            vec![mirror_frame("s6", &["d1"]), result("done")]
        } else {
            Vec::new()
        }
    });
    let messages: Vec<Message> =
        query_with_transport("hello", options(DownStore), transport.clone())
            .map(Result::unwrap)
            .collect()
            .await;
    let mirror_error = messages
        .iter()
        .find_map(|message| match message {
            Message::System(SystemMessage::MirrorError(error)) => Some(error.clone()),
            _ => None,
        })
        .expect("a mirror_error message");
    assert_eq!(mirror_error.error, "the store is down");
    assert_eq!(mirror_error.key, Some(SessionKey::new("proj", "s6")));
    assert!(matches!(messages.last(), Some(Message::Result(_))));
}

struct PanickingStore;

impl SessionStore for PanickingStore {
    async fn append(
        &self,
        _key: &SessionKey,
        _entries: &[SessionStoreEntry],
    ) -> Result<(), SessionStoreError> {
        panic!("the adapter has a bug");
    }

    async fn load(
        &self,
        _key: &SessionKey,
    ) -> Result<Option<Vec<SessionStoreEntry>>, SessionStoreError> {
        Ok(None)
    }
}

#[tokio::test(start_paused = true)]
async fn a_store_that_panics_is_reported_and_the_result_is_still_delivered() {
    let transport = cli_answering(|frame| {
        if frame["type"] == "user" {
            vec![mirror_frame("s7", &["p1"]), result("done")]
        } else {
            Vec::new()
        }
    });
    let messages: Vec<Message> =
        query_with_transport("hello", options(PanickingStore), transport.clone())
            .map(Result::unwrap)
            .collect()
            .await;
    let mirror_error = messages
        .iter()
        .find_map(|message| match message {
            Message::System(SystemMessage::MirrorError(error)) => Some(error.clone()),
            _ => None,
        })
        .expect("a mirror_error message");
    assert_eq!(mirror_error.error, "Session store append panicked");
    assert!(matches!(messages.last(), Some(Message::Result(_))));
}

#[tokio::test]
async fn frames_outside_the_projects_directory_are_dropped() {
    let store = InMemorySessionStore::new();
    let transport = cli_answering(|frame| {
        if frame["type"] == "user" {
            vec![
                json!({"type": "transcript_mirror", "filePath": "/somewhere/else/proj/s7.jsonl", "entries": [{"type": "user", "uuid": "o"}]}),
                result("done"),
            ]
        } else {
            Vec::new()
        }
    });
    let collected: Vec<_> = query_with_transport("hello", options(store.clone()), transport)
        .collect()
        .await;
    assert!(collected.iter().all(Result::is_ok));
    assert_eq!(store.size(), 0);
}

#[derive(Clone)]
struct RecordingStore {
    inner: InMemorySessionStore,
    appends: Recorder<(SessionKey, Vec<String>)>,
}

impl SessionStore for RecordingStore {
    async fn append(
        &self,
        key: &SessionKey,
        entries: &[SessionStoreEntry],
    ) -> Result<(), SessionStoreError> {
        let uuids = entries
            .iter()
            .map(|entry| entry["uuid"].as_str().unwrap_or_default().to_owned())
            .collect();
        self.appends.push((key.clone(), uuids));
        self.inner.append(key, entries).await
    }

    async fn load(
        &self,
        key: &SessionKey,
    ) -> Result<Option<Vec<SessionStoreEntry>>, SessionStoreError> {
        self.inner.load(key).await
    }
}

#[tokio::test]
async fn a_flush_appends_once_per_file_in_first_seen_order() {
    let store = RecordingStore {
        inner: InMemorySessionStore::new(),
        appends: Recorder::new(),
    };
    let transport = cli_answering(|frame| {
        if frame["type"] == "user" {
            vec![
                mirror_frame("s8", &["n1"]),
                mirror_frame("s9", &["n2"]),
                mirror_frame("s8", &["n3"]),
                result("done"),
            ]
        } else {
            Vec::new()
        }
    });
    let collected: Vec<_> = query_with_transport("hello", options(store.clone()), transport)
        .collect()
        .await;
    assert!(collected.iter().all(Result::is_ok));
    assert_eq!(
        store.appends.all(),
        [
            (
                SessionKey::new("proj", "s8"),
                vec!["n1".to_owned(), "n3".to_owned()]
            ),
            (SessionKey::new("proj", "s9"), vec!["n2".to_owned()]),
        ]
    );
}

fn idle_cli() -> MockTransport {
    let transport = silent_cli();
    transport.respond_with(|frame| {
        if frame["type"] == "control_request" {
            vec![success(frame, json!({}))]
        } else {
            Vec::new()
        }
    });
    transport
}

fn blob_frame(session: &str, uuid: &str, size: usize) -> Value {
    json!({
        "type": "transcript_mirror",
        "filePath": transcript_path(session),
        "entries": [{"type": "user", "uuid": uuid, "blob": "x".repeat(size)}],
    })
}

async fn mirrored(store: &InMemorySessionStore, session: &str, count: usize) -> Vec<String> {
    for _ in 0..400 {
        if uuids(store, session).len() >= count {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    uuids(store, session)
}

#[tokio::test]
async fn pending_entries_are_mirrored_early_only_above_500_entries_or_1_mib() {
    const MEBIBYTE: usize = 1 << 20;
    let store = InMemorySessionStore::new();
    let transport = idle_cli();
    let client = ClaudeSdkClient::with_transport(options(store.clone()), transport.clone());
    client.connect().await.unwrap();

    let names: Vec<String> = (0..500).map(|index| format!("e{index}")).collect();
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    transport.push_frame(mirror_frame("s10", &names));
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        uuids(&store, "s10").is_empty(),
        "500 pending entries are not above the threshold"
    );
    transport.push_frame(mirror_frame("s10", &["e500"]));
    assert_eq!(mirrored(&store, "s10", 501).await.len(), 501);

    transport.push_frame(blob_frame("s11", "under", MEBIBYTE - 1024));
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        uuids(&store, "s11").is_empty(),
        "just under 1 MiB pending is not above the threshold"
    );
    transport.push_frame(blob_frame("s11", "over", 2048));
    assert_eq!(mirrored(&store, "s11", 2).await, ["under", "over"]);

    client.disconnect().await.unwrap();
}

#[tokio::test]
async fn an_empty_config_dir_in_the_options_falls_back_to_the_process_environment() {
    let file_path = common::transcripts::projects_dir()
        .join("proj")
        .join("s12.jsonl")
        .to_string_lossy()
        .into_owned();
    let store = InMemorySessionStore::new();
    let options = ClaudeAgentOptions::builder()
        .session_store(store.clone())
        .env([("CLAUDE_CONFIG_DIR", String::new())])
        .build()
        .unwrap();
    let transport = cli_answering(move |frame| {
        if frame["type"] == "user" {
            vec![
                json!({
                    "type": "transcript_mirror",
                    "filePath": file_path,
                    "entries": [{"type": "user", "uuid": "ambient"}],
                }),
                result("done"),
            ]
        } else {
            Vec::new()
        }
    });
    let collected: Vec<_> = query_with_transport("hello", options, transport)
        .collect()
        .await;
    assert!(collected.iter().all(Result::is_ok));
    assert_eq!(uuids(&store, "s12"), ["ambient"]);
}

#[test]
fn a_trailing_separator_on_the_projects_directory_gives_the_same_keys() {
    let projects = config_dir().join("projects");
    let with_separator = PathBuf::from(format!(
        "{}{}",
        projects.display(),
        std::path::MAIN_SEPARATOR
    ));
    assert_eq!(
        file_path_to_session_key(
            &projects.join("-home-user-repo").join("abc-123.jsonl"),
            &with_separator
        ),
        Some(SessionKey::new("-home-user-repo", "abc-123"))
    );
    assert_eq!(
        file_path_to_session_key(
            &projects
                .join("-home-user-repo")
                .join("abc-123")
                .join("subagents")
                .join("agent-xyz.jsonl"),
            &with_separator
        ),
        Some(SessionKey::new("-home-user-repo", "abc-123").with_subpath("subagents/agent-xyz"))
    );
}

#[cfg(windows)]
#[test]
fn a_transcript_on_another_drive_than_the_projects_directory_maps_to_no_key() {
    use std::path::Path;

    assert_eq!(
        file_path_to_session_key(Path::new(r"D:\cfg\p\s.jsonl"), Path::new(r"C:\home")),
        None
    );
    assert_eq!(
        file_path_to_session_key(
            Path::new(r"D:\cfg\projects\p\s.jsonl"),
            Path::new(r"C:\cfg\projects")
        ),
        None
    );
    assert_eq!(
        file_path_to_session_key(
            Path::new(r"C:\cfg\projects\p\s.jsonl"),
            Path::new(r"C:\cfg\projects")
        ),
        Some(SessionKey::new("p", "s"))
    );
}
