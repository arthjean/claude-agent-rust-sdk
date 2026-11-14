#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use claude_agent_kit::{MessageStream, SubprocessTransport};
use serde_json::{Value, json};

use common::{next, next_message, options_for, scenarios_dir};

const EMIT_VARIABLE: &str = "CLAUDE_CODE_EMIT_SESSION_STATE_EVENTS";
const READS_VARIABLE: &str = "CLAUDE_CODE_SDK_READS_SESSION_STATE";

fn ambient_variables_are_unset() -> bool {
    std::env::var_os(EMIT_VARIABLE).is_none() && std::env::var_os(READS_VARIABLE).is_none()
}

async fn send(transport: &SubprocessTransport, frame: Value) {
    transport.write(&format!("{frame}\n")).await.unwrap();
}

fn text(message: &Value) -> &str {
    message["message"]["content"][0]["text"].as_str().unwrap()
}

async fn start(scenario: &str, variables: &[(&str, &str)]) -> (SubprocessTransport, MessageStream) {
    let mut options = options_for(&scenarios_dir().join(scenario));
    for (name, value) in variables {
        options = options.env_var(*name, *value);
    }
    let transport = SubprocessTransport::new(options.build().unwrap());
    transport.connect().await.unwrap();
    let stream = transport.read_messages();
    (transport, stream)
}

async fn initialize_and_prompt(transport: &SubprocessTransport, stream: &mut MessageStream) {
    send(
        transport,
        json!({
            "type": "control_request",
            "request_id": "req_1",
            "request": {
                "subtype": "initialize",
                "hooks": {"PreToolUse": [{"matcher": null, "hookCallbackIds": ["hook_0"]}]},
            },
        }),
    )
    .await;
    let response = next_message(stream).await;
    assert_eq!(response["type"], "control_response");
    assert_eq!(response["response"]["request_id"], "req_1");
    send(
        transport,
        json!({"type": "user", "message": {"role": "user", "content": "go"}}),
    )
    .await;
}

async fn answer_hook(transport: &SubprocessTransport, stream: &mut MessageStream) {
    let hook = next_message(stream).await;
    assert_eq!(hook["type"], "control_request");
    assert_eq!(hook["request_id"], "hook-1");
    assert_eq!(hook["request"]["subtype"], "hook_callback");
    assert_eq!(hook["request"]["callback_id"], "hook_0");
    assert_eq!(hook["request"]["input"]["tool_name"], "Write");
    send(
        transport,
        json!({
            "type": "control_response",
            "response": {"subtype": "success", "request_id": "hook-1", "response": {}},
        }),
    )
    .await;
}

#[tokio::test]
async fn follow_up_turn_is_served_while_stdin_stays_open() {
    if !ambient_variables_are_unset() {
        return;
    }
    let (transport, mut stream) = start("run_end_wake.json", &[]).await;
    let init = next_message(&mut stream).await;
    assert_eq!(init["claude_code_version"], "2.1.999");
    assert_eq!(
        init["fake_env"],
        json!({READS_VARIABLE: "1", EMIT_VARIABLE: null})
    );

    initialize_and_prompt(&transport, &mut stream).await;
    let running = next_message(&mut stream).await;
    assert_eq!(running["subtype"], "session_state_changed");
    assert_eq!(running["state"], "running");
    assert_eq!(running["sdk_host_only"], true);
    assert_eq!(text(&next_message(&mut stream).await), "LAUNCHED");
    assert_eq!(next_message(&mut stream).await["result"], "LAUNCHED");
    assert_eq!(text(&next_message(&mut stream).await), "writing");

    answer_hook(&transport, &mut stream).await;
    assert_eq!(text(&next_message(&mut stream).await), "FINISHED");
    assert_eq!(next_message(&mut stream).await["result"], "FINISHED");
    let idle = next_message(&mut stream).await;
    assert_eq!(idle["state"], "idle");
    assert_eq!(idle["sdk_host_only"], true);

    transport.end_input().await.unwrap();
    assert!(next(&mut stream).await.is_none());
    transport.close().await.unwrap();
}

#[tokio::test]
async fn a_caller_who_opted_in_gets_unmarked_state_frames() {
    let (transport, mut stream) = start("run_end_wake.json", &[(EMIT_VARIABLE, "1")]).await;
    assert_eq!(
        next_message(&mut stream).await["fake_env"][EMIT_VARIABLE],
        "1"
    );

    initialize_and_prompt(&transport, &mut stream).await;
    let running = next_message(&mut stream).await;
    assert_eq!(running["state"], "running");
    assert!(running.get("sdk_host_only").is_none());
    for _ in 0..3 {
        next_message(&mut stream).await;
    }
    answer_hook(&transport, &mut stream).await;
    next_message(&mut stream).await;
    next_message(&mut stream).await;
    let idle = next_message(&mut stream).await;
    assert_eq!(idle["state"], "idle");
    assert!(idle.get("sdk_host_only").is_none());

    transport.close().await.unwrap();
    assert!(next(&mut stream).await.is_none());
}

#[tokio::test]
async fn the_stuck_scenario_ends_only_when_stdin_closes() {
    let (transport, mut stream) = start("run_end_stuck.json", &[(READS_VARIABLE, "0")]).await;
    assert_eq!(next_message(&mut stream).await["subtype"], "init");
    initialize_and_prompt(&transport, &mut stream).await;
    assert_eq!(text(&next_message(&mut stream).await), "LAUNCHED");
    assert_eq!(next_message(&mut stream).await["result"], "LAUNCHED");

    let quiet = tokio::time::timeout(std::time::Duration::from_millis(300), async {
        futures_util::StreamExt::next(&mut stream).await
    })
    .await;
    assert!(quiet.is_err(), "the stuck scenario emitted {quiet:?}");

    transport.end_input().await.unwrap();
    assert!(next(&mut stream).await.is_none());
    transport.close().await.unwrap();
}
