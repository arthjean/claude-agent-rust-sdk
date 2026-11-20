use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::task::Poll;
use std::time::Duration;

use futures_util::{FutureExt, stream};
use serde_json::{Value, json};
use tokio::sync::Notify;

use super::{ControlSession, Inbox, SessionConfig, TranscriptMirror};
use crate::error::{BoxError, Error};
use crate::prompt::InputStream;
use crate::session_store::SessionKey;
use crate::testing::MockTransport;
use crate::transport::SharedTransport;
use crate::types::{BoxFuture, ClaudeAgentOptions, HookCallback, HookMatcher, HookOutput};

fn answering_control_requests() -> MockTransport {
    let transport = MockTransport::new();
    transport.respond_with(|frame| {
        if frame["type"] == "control_request" {
            vec![json!({
                "type": "control_response",
                "response": {
                    "subtype": "success",
                    "request_id": frame["request_id"],
                    "response": {},
                },
            })]
        } else {
            Vec::new()
        }
    });
    transport
}

async fn settle() {
    tokio::time::sleep(Duration::from_millis(1)).await;
}

fn result(text: &str) -> Value {
    json!({
        "type": "result",
        "subtype": "success",
        "duration_ms": 1,
        "duration_api_ms": 1,
        "is_error": false,
        "num_turns": 1,
        "session_id": "s",
        "result": text,
    })
}

fn hook_request(request_id: &str, callback_id: &str) -> Value {
    json!({
        "type": "control_request",
        "request_id": request_id,
        "request": {"subtype": "hook_callback", "callback_id": callback_id, "input": {}},
    })
}

#[derive(Default)]
struct RecordingMirror {
    events: Mutex<Vec<String>>,
}

impl RecordingMirror {
    fn record(&self, event: String) {
        self.events.lock().unwrap().push(event);
    }

    fn events(&self) -> Vec<String> {
        self.events.lock().unwrap().clone()
    }
}

impl TranscriptMirror for RecordingMirror {
    fn enqueue(&self, file_path: String, entries: Vec<Value>) {
        self.record(format!("enqueue {file_path} {}", entries.len()));
    }

    fn flush(&self) -> BoxFuture<'_, ()> {
        Box::pin(async {
            tokio::task::yield_now().await;
            self.record("flush".to_owned());
        })
    }

    fn close(&self) -> BoxFuture<'_, ()> {
        Box::pin(async { self.record("close".to_owned()) })
    }
}

async fn open(options: &ClaudeAgentOptions) -> (MockTransport, ControlSession, Inbox) {
    let transport = answering_control_requests();
    let (session, inbox) = ControlSession::new(
        SharedTransport::new(transport.clone()),
        SessionConfig::from_options(options).unwrap(),
    );
    session.open().await.unwrap();
    (transport, session, inbox)
}

#[tokio::test(start_paused = true)]
async fn each_control_request_runs_one_handler_tracked_until_it_answers() {
    let release = Arc::new(Notify::new());
    let waiting = {
        let release = Arc::clone(&release);
        HookCallback::new(move |_input, _tool_use_id, _context| {
            let release = Arc::clone(&release);
            async move {
                release.notified().await;
                Ok(HookOutput::default())
            }
        })
    };
    let options = ClaudeAgentOptions::builder()
        .hook("PreToolUse", HookMatcher::new().hook(waiting))
        .build()
        .unwrap();
    let (transport, session, _inbox) = open(&options).await;
    assert_eq!(session.handlers_in_flight(), 0);

    transport.push_frame(hook_request("hook-1", "hook_0"));
    transport.push_frame(hook_request("hook-2", "hook_0"));
    transport
        .push_frame(json!({"type": "control_request", "request": {"subtype": "hook_callback"}}));
    settle().await;
    assert_eq!(session.handlers_in_flight(), 2);

    transport.push_frame(json!({"type": "control_cancel_request", "request_id": "hook-2"}));
    settle().await;
    assert_eq!(session.handlers_in_flight(), 1);

    release.notify_waiters();
    settle().await;
    assert_eq!(session.handlers_in_flight(), 0);
    let answered: Vec<Value> = transport
        .written_frames()
        .into_iter()
        .filter(|frame| frame["type"] == "control_response")
        .map(|frame| frame["response"]["request_id"].clone())
        .collect();
    assert_eq!(answered, [json!("hook-1")]);
    session.close().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn closing_the_session_stops_handlers_and_fails_pending_requests() {
    let never = HookCallback::new(async |_input, _tool_use_id, _context| {
        std::future::pending::<()>().await;
        Ok(HookOutput::default())
    });
    let options = ClaudeAgentOptions::builder()
        .hook("PreToolUse", HookMatcher::new().hook(never))
        .build()
        .unwrap();
    let transport = MockTransport::new();
    transport.respond_with(|frame| {
        if frame["request"]["subtype"] == "initialize" {
            vec![json!({
                "type": "control_response",
                "response": {"subtype": "success", "request_id": frame["request_id"], "response": {}},
            })]
        } else {
            Vec::new()
        }
    });
    let (session, mut inbox) = ControlSession::new(
        SharedTransport::new(transport.clone()),
        SessionConfig::from_options(&options).unwrap(),
    );
    session.open().await.unwrap();
    transport.push_frame(hook_request("hook-1", "hook_0"));
    transport.push_frame(json!({"type": "system", "subtype": "status"}));
    settle().await;
    assert_eq!(session.handlers_in_flight(), 1);

    let pending = {
        let session = session.clone();
        tokio::spawn(async move {
            session
                .send_control_request(json!({"subtype": "interrupt"}), Duration::from_secs(60))
                .await
        })
    };
    settle().await;
    session.close().await.unwrap();
    session.close().await.unwrap();

    assert!(
        pending
            .await
            .unwrap()
            .as_ref()
            .err()
            .is_some_and(Error::is_cli_connection)
    );
    assert_eq!(session.handlers_in_flight(), 0);
    assert_eq!(transport.close_count(), 1);
    assert!(session.run().has_ended());
    assert_eq!(inbox.recv().await.unwrap().unwrap()["subtype"], "status");
    assert!(inbox.recv().await.is_none());
    assert!(
        session
            .send_control_request(json!({"subtype": "interrupt"}), Duration::from_secs(60))
            .await
            .as_ref()
            .err()
            .is_some_and(Error::is_cli_connection)
    );
}

struct StallingMirror {
    closes: AtomicUsize,
}

impl TranscriptMirror for StallingMirror {
    fn enqueue(&self, _file_path: String, _entries: Vec<Value>) {}

    fn flush(&self) -> BoxFuture<'_, ()> {
        Box::pin(async {})
    }

    fn close(&self) -> BoxFuture<'_, ()> {
        let first = self.closes.fetch_add(1, Ordering::SeqCst) == 0;
        Box::pin(async move {
            if first {
                std::future::pending::<()>().await;
            }
        })
    }
}

struct SetOnDrop(Arc<AtomicBool>);

impl Drop for SetOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

#[tokio::test(start_paused = true)]
async fn close_abandoned_during_the_mirror_flush_is_completed_by_the_next_close() {
    let mirror = Arc::new(StallingMirror {
        closes: AtomicUsize::new(0),
    });
    let transport = answering_control_requests();
    let (session, _inbox) = ControlSession::with_mirror(
        SharedTransport::new(transport.clone()),
        SessionConfig::from_options(&ClaudeAgentOptions::default()).unwrap(),
        Arc::clone(&mirror) as Arc<dyn TranscriptMirror>,
    );
    session.open().await.unwrap();

    let input_dropped = Arc::new(AtomicBool::new(false));
    let held = SetOnDrop(Arc::clone(&input_dropped));
    let silent: InputStream = Box::pin(stream::poll_fn(move |_context| {
        let _held = &held;
        Poll::<Option<Result<Value, BoxError>>>::Pending
    }));
    session.stream_input(silent);
    settle().await;

    assert!(session.close().now_or_never().is_none());
    assert!(!input_dropped.load(Ordering::SeqCst));
    assert_eq!(transport.close_count(), 0);

    session.close().await.unwrap();
    settle().await;
    assert_eq!(mirror.closes.load(Ordering::SeqCst), 2);
    assert!(input_dropped.load(Ordering::SeqCst));
    assert_eq!(transport.close_count(), 1);
    assert!(session.run().has_ended());
}

#[tokio::test(start_paused = true)]
async fn mirror_frames_go_to_the_attached_mirror_and_results_wait_for_its_flush() {
    let mirror = Arc::new(RecordingMirror::default());
    let transport = answering_control_requests();
    let (session, mut inbox) = ControlSession::with_mirror(
        SharedTransport::new(transport.clone()),
        SessionConfig::from_options(&ClaudeAgentOptions::default()).unwrap(),
        Arc::clone(&mirror) as Arc<dyn TranscriptMirror>,
    );
    session.open().await.unwrap();

    transport.push_frame(json!({
        "type": "transcript_mirror",
        "filePath": "/projects/p/s.jsonl",
        "entries": [{"uuid": "a"}, {"uuid": "b"}],
    }));
    transport.push_frame(json!({"type": "transcript_mirror", "filePath": 7}));
    transport.push_frame(result("first"));
    assert_eq!(inbox.recv().await.unwrap().unwrap()["result"], "first");
    assert_eq!(mirror.events(), ["enqueue /projects/p/s.jsonl 2", "flush"]);

    transport.push_frame(json!({
        "type": "transcript_mirror",
        "filePath": "/projects/p/s.jsonl",
        "entries": [{"uuid": "c"}],
    }));
    transport.finish();
    assert!(inbox.recv().await.is_none());
    settle().await;
    session.close().await.unwrap();
    assert_eq!(
        mirror.events(),
        [
            "enqueue /projects/p/s.jsonl 2",
            "flush",
            "enqueue /projects/p/s.jsonl 1",
            "flush",
            "close",
        ]
    );
}

#[tokio::test(start_paused = true)]
async fn mirror_error_is_delivered_without_blocking_and_dropped_when_the_queue_is_full() {
    let (transport, session, mut inbox) = open(&ClaudeAgentOptions::default()).await;
    let key = SessionKey::new("project", "session-1");
    session.report_mirror_error(Some(&key), "the store is down");
    session.report_mirror_error(None, "no key");

    let first = inbox.recv().await.unwrap().unwrap();
    assert_eq!(first["type"], "system");
    assert_eq!(first["subtype"], "mirror_error");
    assert_eq!(first["error"], "the store is down");
    assert_eq!(
        first["key"],
        json!({"project_key": "project", "session_id": "session-1"})
    );
    assert_eq!(first["session_id"], "session-1");
    assert_eq!(first["uuid"].as_str().unwrap().len(), 36);
    let parsed = crate::parse_message(first).unwrap();
    assert!(matches!(
        parsed,
        crate::Message::System(crate::SystemMessage::MirrorError(ref message))
            if message.error == "the store is down" && message.key.as_ref() == Some(&key)
    ));
    let second = inbox.recv().await.unwrap().unwrap();
    assert_eq!(second["key"], Value::Null);
    assert_eq!(second["session_id"], "");

    for index in 0..100 {
        transport.push_frame(json!({"type": "system", "subtype": "status", "index": index}));
    }
    settle().await;
    session.report_mirror_error(Some(&key), "dropped");
    for index in 0..100 {
        assert_eq!(inbox.recv().await.unwrap().unwrap()["index"], index);
    }
    settle().await;
    assert!(inbox.try_recv().is_err());

    session.report_mirror_error(Some(&key), "delivered again");
    assert_eq!(
        inbox.recv().await.unwrap().unwrap()["error"],
        "delivered again"
    );
    session.close().await.unwrap();
    session.report_mirror_error(Some(&key), "after close");
}

/// A mirror whose flush waits until the test releases it.
#[derive(Default)]
struct GatedMirror {
    release: Notify,
    flushes: AtomicUsize,
}

impl TranscriptMirror for GatedMirror {
    fn enqueue(&self, _file_path: String, _entries: Vec<Value>) {}

    fn flush(&self) -> BoxFuture<'_, ()> {
        Box::pin(async {
            self.release.notified().await;
            self.flushes.fetch_add(1, Ordering::SeqCst);
        })
    }

    fn close(&self) -> BoxFuture<'_, ()> {
        Box::pin(async {})
    }
}

#[tokio::test(start_paused = true)]
async fn control_traffic_is_routed_while_a_result_waits_for_the_mirror_flush() {
    let mirror = Arc::new(GatedMirror::default());
    let options = ClaudeAgentOptions::builder()
        .hook(
            "PreToolUse",
            HookMatcher::new().hook(HookCallback::new(async |_input, _tool_use_id, _context| {
                Ok(HookOutput::default())
            })),
        )
        .build()
        .unwrap();
    let transport = answering_control_requests();
    let (session, mut inbox) = ControlSession::with_mirror(
        SharedTransport::new(transport.clone()),
        SessionConfig::from_options(&options).unwrap(),
        Arc::clone(&mirror) as Arc<dyn TranscriptMirror>,
    );
    session.open().await.unwrap();

    transport.push_frame(result("first"));
    transport.push_frame(json!({"type": "system", "subtype": "status"}));
    transport.push_frame(hook_request("hook-1", "hook_0"));
    settle().await;

    let answered: Vec<Value> = transport
        .written_frames()
        .into_iter()
        .filter(|frame| frame["type"] == "control_response")
        .map(|frame| frame["response"]["request_id"].clone())
        .collect();
    assert_eq!(answered, [json!("hook-1")]);
    assert!(
        inbox.try_recv().is_err(),
        "the result came before its flush"
    );

    mirror.release.notify_one();
    assert_eq!(inbox.recv().await.unwrap().unwrap()["result"], "first");
    assert_eq!(mirror.flushes.load(Ordering::SeqCst), 1);
    assert_eq!(inbox.recv().await.unwrap().unwrap()["subtype"], "status");
    session.close().await.unwrap();
}
