#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use claude_agent_rust_sdk::ErrorKind;
use claude_agent_rust_sdk::{
    ClaudeAgentOptionsBuilder, ClaudeSdkClient, Error, HookCallback, HookInput, HookMatcher,
    HookOutput, Message, query,
};
use futures_util::StreamExt;
use serde_json::{Value, json};

use common::{Fake, STEP_LIMIT, options_for, scenarios_dir, wait_until_gone};

const EMIT_VARIABLE: &str = "CLAUDE_CODE_EMIT_SESSION_STATE_EVENTS";
const READS_VARIABLE: &str = "CLAUDE_CODE_SDK_READS_SESSION_STATE";
const CEILING_VARIABLE: &str = "CLAUDE_CODE_PRINT_BG_WAIT_CEILING_MS";

const AMBIENT_VARIABLES: [&str; 3] = [EMIT_VARIABLE, READS_VARIABLE, CEILING_VARIABLE];

/// Whether the test `name` can run in this process: true when none of
/// [`AMBIENT_VARIABLES`] is set here.
///
/// Otherwise the SDK and the fake CLI would read the caller's values, so
/// the test reruns itself in a child of this test binary with the variables
/// removed, fails when the child fails or ran nothing, and returns false.
/// The variables are removed on the child only, never on this process,
/// where other tests run on other threads.
fn runs_in_a_clean_environment(name: &str) -> bool {
    if AMBIENT_VARIABLES
        .iter()
        .all(|variable| std::env::var_os(variable).is_none())
    {
        return true;
    }
    let mut command = std::process::Command::new(std::env::current_exe().unwrap());
    command.args([name, "--exact", "--nocapture"]);
    for variable in AMBIENT_VARIABLES {
        command.env_remove(variable);
    }
    let output = command.output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "{name} failed once {AMBIENT_VARIABLES:?} were removed:\n{stdout}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        stdout.contains("1 passed"),
        "the rerun of {name} without {AMBIENT_VARIABLES:?} ran no test:\n{stdout}"
    );
    false
}

type Asked = Arc<Mutex<Vec<String>>>;

fn recording_hook(asked: &Asked) -> HookCallback {
    let asked = Arc::clone(asked);
    HookCallback::new(move |input, _tool_use_id, _context| {
        let tool_name = match &input {
            HookInput::PreToolUse(input) => input.tool_name.clone(),
            HookInput::Unknown(raw) => raw["tool_name"].as_str().unwrap_or_default().to_owned(),
            other => other.hook_event_name().to_owned(),
        };
        asked.lock().unwrap().push(tool_name);
        async { Ok(HookOutput::default()) }
    })
}

async fn run(options: ClaudeAgentOptionsBuilder) -> (Vec<Message>, Vec<String>) {
    let asked = Asked::default();
    let options = options
        .hook(
            "PreToolUse",
            HookMatcher::new().hook(recording_hook(&asked)),
        )
        .build()
        .unwrap();
    let messages = collect(query("go", options)).await;
    let asked = asked.lock().unwrap().clone();
    (messages, asked)
}

async fn collect(stream: impl futures_core::Stream<Item = Result<Message, Error>>) -> Vec<Message> {
    let items: Vec<Result<Message, Error>> = tokio::time::timeout(STEP_LIMIT, stream.collect())
        .await
        .expect("the query stalled");
    items
        .into_iter()
        .map(|item| item.expect("the query yielded an error"))
        .collect()
}

fn results(messages: &[Message]) -> Vec<String> {
    messages
        .iter()
        .filter_map(|message| match message {
            Message::Result(result) => Some(result.result.clone().unwrap_or_default()),
            _ => None,
        })
        .collect()
}

fn states(messages: &[Message]) -> Vec<String> {
    messages
        .iter()
        .filter_map(|message| match message {
            Message::System(system) if system.subtype() == "session_state_changed" => {
                Some(system.data()["state"].as_str().unwrap().to_owned())
            }
            _ => None,
        })
        .collect()
}

fn init_data(messages: &[Message]) -> Value {
    messages
        .iter()
        .find_map(|message| match message {
            Message::System(system) if system.subtype() == "init" => Some(system.data().clone()),
            _ => None,
        })
        .expect("no init message")
}

fn result_frame(text: &str, is_error: bool, extra: Value) -> Value {
    let mut frame = json!({
        "type": "result",
        "subtype": "success",
        "duration_ms": 1,
        "duration_api_ms": 1,
        "is_error": is_error,
        "num_turns": 1,
        "session_id": "s",
        "result": text,
    });
    frame
        .as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    frame
}

#[tokio::test]
async fn follow_up_turn_is_served_and_sdk_frames_stay_hidden() {
    if !runs_in_a_clean_environment("follow_up_turn_is_served_and_sdk_frames_stay_hidden") {
        return;
    }
    let (messages, asked) = run(options_for(&scenarios_dir().join("run_end_wake.json"))).await;

    assert_eq!(
        init_data(&messages)["fake_env"],
        json!({READS_VARIABLE: "1", EMIT_VARIABLE: null})
    );
    assert_eq!(results(&messages), ["LAUNCHED", "FINISHED"]);
    assert_eq!(asked, ["Write"]);
    assert!(states(&messages).is_empty());
}

#[tokio::test]
async fn caller_who_opted_in_sees_the_frames() {
    if !runs_in_a_clean_environment("caller_who_opted_in_sees_the_frames") {
        return;
    }
    let options =
        options_for(&scenarios_dir().join("run_end_wake.json")).env_var(EMIT_VARIABLE, "1");
    let (messages, asked) = run(options).await;

    assert_eq!(results(&messages), ["LAUNCHED", "FINISHED"]);
    assert_eq!(asked, ["Write"]);
    assert_eq!(states(&messages), ["running", "idle"]);
}

#[tokio::test]
async fn ceiling_closes_stdin_when_idle_never_comes() {
    if !runs_in_a_clean_environment("ceiling_closes_stdin_when_idle_never_comes") {
        return;
    }
    let started = Instant::now();
    let options =
        options_for(&scenarios_dir().join("run_end_stuck.json")).env_var(CEILING_VARIABLE, "300");
    let (messages, asked) = run(options).await;

    assert_eq!(results(&messages), ["LAUNCHED"]);
    assert!(asked.is_empty());
    assert!(started.elapsed() >= Duration::from_millis(300));
}

#[tokio::test]
async fn without_callbacks_stdin_closes_right_after_the_prompt() {
    let fake = Fake::new(json!({
        "auto_reply": true,
        "steps": [
            {"expect": {"type": "user", "message": {"content": "go"}}},
            {"wait_for_eof": true},
            {"emit": result_frame("stdin closed", false, json!({}))},
        ],
    }));
    let messages = collect(query("go", fake.build())).await;
    assert_eq!(results(&messages), ["stdin closed"]);
}

#[tokio::test]
async fn error_result_then_failing_exit_ends_the_stream_with_a_result_error() {
    let fake = Fake::new(json!({
        "auto_reply": true,
        "steps": [
            {"expect": {"type": "user"}},
            {"emit": result_frame(
                "",
                true,
                json!({"subtype": "error_max_turns", "errors": ["Reached maximum number of turns (1)"]}),
            )},
            {"exit": 1},
        ],
    }));
    let items: Vec<Result<Message, Error>> =
        tokio::time::timeout(STEP_LIMIT, query("go", fake.build()).collect())
            .await
            .unwrap();

    let (last, earlier) = items.split_last().unwrap();
    assert!(earlier.iter().all(Result::is_ok));
    assert!(matches!(earlier.last(), Some(Ok(Message::Result(result))) if result.is_error));
    let error = last.as_ref().unwrap_err();
    assert!(error.is_process(), "{error:?}");
    assert!(!error.is_cli_connection(), "{error:?}");
    assert_eq!(error.kind(), ErrorKind::ResultError);
    assert_eq!(
        error.to_string(),
        "Claude Code returned an error result: Reached maximum number of turns (1) (exit code: 1)"
    );
    assert_eq!(
        error.result().and_then(|data| data.subtype()),
        Some("error_max_turns")
    );
    assert_eq!(error.exit_code(), Some(1));
    let cause = std::error::Error::source(error)
        .and_then(|source| source.downcast_ref::<Error>())
        .expect("the process error is the source, as upstream's __cause__");
    assert_eq!(cause.kind(), ErrorKind::Process);
    assert_eq!(cause.exit_code(), Some(1));
}

#[tokio::test]
async fn failing_exit_without_an_error_result_stays_a_process_error() {
    let fake = Fake::new(json!({
        "auto_reply": true,
        "steps": [{"expect": {"type": "user"}}, {"exit": 7}],
    }));
    let items: Vec<Result<Message, Error>> =
        tokio::time::timeout(STEP_LIMIT, query("go", fake.build()).collect())
            .await
            .unwrap();
    let Some(Err(error)) = items.last() else {
        panic!("expected a process error, got {items:?}");
    };
    assert_eq!(error.kind(), ErrorKind::Process);
    assert_eq!(error.exit_code(), Some(7));
}

#[tokio::test]
async fn dropping_the_stream_ends_the_cli_process() {
    let fake = Fake::new(json!({
        "auto_reply": true,
        "steps": [{"expect": {"type": "user"}}, {"sleep_ms": 600_000}],
    }));
    let asked = Asked::default();
    let options = fake
        .options()
        .hook(
            "PreToolUse",
            HookMatcher::new().hook(recording_hook(&asked)),
        )
        .build()
        .unwrap();
    let mut stream = query("go", options);
    let first = tokio::time::timeout(STEP_LIMIT, stream.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let Message::System(init) = first else {
        panic!("expected the init message");
    };
    let pid = u32::try_from(init.data()["fake_pid"].as_u64().unwrap()).unwrap();
    assert!(common::process_exists(pid));

    drop(stream);
    assert!(wait_until_gone(pid, STEP_LIMIT).await);
}

#[tokio::test]
async fn client_holds_a_conversation_with_the_cli_process() {
    let fake = Fake::new(json!({
        "auto_reply": true,
        "steps": [
            {"expect": {"type": "user", "session_id": "default", "message": {"content": "first"}}},
            {"emit": result_frame("one", false, json!({}))},
            {"expect": {"type": "control_request", "request": {"subtype": "set_model", "model": "claude-opus-5"}}},
            {"expect": {"type": "user", "session_id": "side", "message": {"content": "second"}}},
            {"emit": result_frame("two", false, json!({}))},
        ],
    }));
    let client = ClaudeSdkClient::new(fake.build());
    client.connect().await.unwrap();
    assert!(client.server_info().unwrap().is_some());

    client.query("first").await.unwrap();
    let first = collect(client.receive_response()).await;
    assert_eq!(results(&first), ["one"]);
    let pid = u32::try_from(init_data(&first)["fake_pid"].as_u64().unwrap()).unwrap();

    client.set_model(Some("claude-opus-5")).await.unwrap();
    client
        .query_with_session_id("second", "side")
        .await
        .unwrap();
    let second = collect(client.receive_response()).await;
    assert_eq!(results(&second), ["two"]);

    client.disconnect().await.unwrap();
    assert!(!common::process_exists(pid));
    client.disconnect().await.unwrap();
}
