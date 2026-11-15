#![cfg(feature = "testing")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::path::PathBuf;

use claude_agent_kit::{
    ClaudeAgentOptions, ClaudeSdkClient, InMemorySessionStore, Message, SessionKey, SessionStore,
    SessionStoreEntry, SessionStoreError, SessionStoreFlushMode, SystemMessage,
    query_with_transport,
};
use futures_util::StreamExt;
use serde_json::{Value, json};

use common::mock::{assistant, cli_answering, result, silent_cli, success};

fn config_dir() -> PathBuf {
    std::env::temp_dir().join("claude-agent-kit-mirror-test-config")
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
