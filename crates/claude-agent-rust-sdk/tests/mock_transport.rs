#![cfg(feature = "testing")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::time::Duration;

use claude_agent_kit::testing::MockTransport;
use claude_agent_kit::{Error, SharedTransport, Transport};
use futures_util::{FutureExt, StreamExt};
use serde_json::{Value, json};

use common::{collect, next, next_message};

#[tokio::test]
async fn written_frames_are_recorded() {
    let transport = MockTransport::new();
    let unready = transport.write("{}\n").await.unwrap_err();
    assert!(matches!(unready, Error::CliConnection { .. }));

    transport.connect().await.unwrap();
    assert!(transport.is_ready());
    transport
        .write("{\"type\":\"control_request\",\"request_id\":\"r1\"}\n")
        .await
        .unwrap();
    transport
        .write("{\"type\":\"user\"}\n\nnot json\n")
        .await
        .unwrap();

    assert_eq!(
        transport.written(),
        [
            "{\"type\":\"control_request\",\"request_id\":\"r1\"}\n",
            "{\"type\":\"user\"}\n\nnot json\n"
        ]
    );
    assert_eq!(
        transport.written_frames(),
        [
            json!({"type": "control_request", "request_id": "r1"}),
            json!({"type": "user"}),
        ]
    );
    assert_eq!(transport.connect_count(), 1);
}

#[tokio::test]
async fn scripted_frames_are_played_in_order_then_the_stream_ends() {
    let frames = [
        json!({"type": "system", "subtype": "init"}),
        json!({"type": "assistant", "message": {"content": []}}),
        json!({"type": "result", "subtype": "success"}),
    ];
    let transport = MockTransport::with_frames(frames.clone());
    transport.connect().await.unwrap();
    let played: Vec<Value> = collect(transport.read_messages())
        .await
        .into_iter()
        .map(Result::unwrap)
        .collect();
    assert_eq!(played, frames);
}

#[tokio::test]
async fn frames_pushed_after_the_stream_started_wake_the_reader() {
    let transport = MockTransport::new();
    transport.connect().await.unwrap();
    let mut stream = transport.read_messages();
    assert!(stream.next().now_or_never().is_none());

    let script = transport.clone();
    let pusher = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(30)).await;
        script.push_frame(json!({"type": "assistant"}));
        tokio::time::sleep(Duration::from_millis(30)).await;
        script.finish();
    });
    assert_eq!(next_message(&mut stream).await["type"], "assistant");
    assert!(next(&mut stream).await.is_none());
    pusher.await.unwrap();
}

#[tokio::test]
async fn a_scripted_error_ends_the_stream() {
    let transport = MockTransport::new();
    transport.push_frame(json!({"type": "system"}));
    transport.push_error(Error::process(
        "Command failed with exit code 1",
        Some(1),
        None,
    ));
    transport.push_frame(json!({"type": "never seen"}));
    transport.connect().await.unwrap();
    let items = collect(transport.read_messages()).await;
    assert_eq!(items.len(), 2);
    assert_eq!(items[1].as_ref().unwrap_err().exit_code(), Some(1));
}

#[tokio::test]
async fn a_responder_plays_replies_to_written_frames() {
    let transport = MockTransport::new();
    transport.respond_with(|frame| {
        if frame["type"] == "control_request" {
            vec![json!({
                "type": "control_response",
                "response": {"subtype": "success", "request_id": frame["request_id"],
                             "response": {}},
            })]
        } else {
            Vec::new()
        }
    });
    transport.connect().await.unwrap();
    let mut stream = transport.read_messages();

    transport.write("{\"type\":\"user\"}\n").await.unwrap();
    assert!(stream.next().now_or_never().is_none());
    transport
        .write("{\"type\":\"control_request\",\"request_id\":\"req_7\"}\n")
        .await
        .unwrap();
    let response = next_message(&mut stream).await;
    assert_eq!(response["response"]["request_id"], "req_7");
}

#[tokio::test]
async fn lifecycle_calls_are_observable() {
    let transport = MockTransport::new();
    transport.connect().await.unwrap();
    let mut stream = transport.read_messages();
    assert!(!transport.input_ended());
    transport.end_input().await.unwrap();
    assert!(transport.input_ended());

    transport.close().await.unwrap();
    assert!(transport.is_closed());
    assert!(!transport.is_ready());
    assert_eq!(transport.close_count(), 1);
    assert!(next(&mut stream).await.is_none());
    assert!(transport.write("{}\n").await.is_err());
}

#[tokio::test]
async fn failures_can_be_scripted() {
    let refusing = MockTransport::new();
    refusing.fail_connect("Failed to start Claude CLI: denied");
    match refusing.connect().await.unwrap_err() {
        Error::CliConnection { message } => {
            assert_eq!(message, "Failed to start Claude CLI: denied");
        }
        other => panic!("unexpected error: {other:?}"),
    }
    assert!(!refusing.is_ready());

    let broken = MockTransport::new();
    broken.connect().await.unwrap();
    broken.write("{\"ok\":true}\n").await.unwrap();
    broken.fail_writes("Failed to write to process stdin: broken pipe");
    assert!(broken.write("{\"ok\":false}\n").await.is_err());
    assert_eq!(broken.written_frames(), [json!({"ok": true})]);
}

#[tokio::test]
async fn the_mock_is_injectable_as_a_shared_transport() {
    let mock = MockTransport::with_frames([json!({"type": "result"})]);
    let shared: SharedTransport = mock.clone().into();
    shared.connect().await.unwrap();
    shared.write("{\"type\":\"user\"}\n").await.unwrap();
    let played = collect(shared.read_messages()).await;
    assert_eq!(played.len(), 1);
    shared.close().await.unwrap();

    assert_eq!(mock.written_frames(), [json!({"type": "user"})]);
    assert!(mock.is_closed());
}
