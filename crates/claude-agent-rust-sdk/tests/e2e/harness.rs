//! Rules of the suite itself. These tests never call the API, so they run
//! with every `cargo test`.

use std::process::Command;

use claude_agent_rust_sdk::{ClaudeAgentOptions, ClaudeSdkClient, MINIMUM_CLI_VERSION};
use futures_util::StreamExt;
use serde_json::json;

use crate::common::Fake;
use crate::common::capture::Warnings;
use crate::suite::{
    self, CREDENTIALS_VARIABLE, LOGIN_VALUE, LOGIN_VARIABLE, MODEL_VARIABLE, PINNED_MODEL,
    SESSION_MODEL_VARIABLE, credentials_error, pin_model,
};

const REAL_CLI_TEST: &str =
    "handshake::the_handshake_and_one_turn_succeed_with_the_sdk_rs_entrypoint";

#[test]
fn missing_credentials_are_reported_by_naming_the_variable() {
    let message = credentials_error(|_| None).unwrap();
    assert!(message.starts_with("ANTHROPIC_API_KEY environment variable is required"));
    assert!(message.contains("CLAUDE_AGENT_RUST_SDK_E2E_AUTH=cli"));

    let empty = |name: &str| (name == CREDENTIALS_VARIABLE).then(String::new);
    assert!(credentials_error(empty).is_some());
    let wrong_login = |name: &str| (name == LOGIN_VARIABLE).then(|| "yes".to_owned());
    assert!(credentials_error(wrong_login).is_some());

    let key = |name: &str| (name == CREDENTIALS_VARIABLE).then(|| "sk-test".to_owned());
    assert_eq!(credentials_error(key), None);
    let login = |name: &str| (name == LOGIN_VARIABLE).then(|| LOGIN_VALUE.to_owned());
    assert_eq!(credentials_error(login), None);
}

const STORE_RESUME_TEST: &str =
    "store_resume::a_session_held_only_in_a_store_is_resumed_by_the_cli";

#[test]
fn without_credentials_a_real_cli_test_fails_instead_of_being_skipped() {
    for test in [REAL_CLI_TEST, STORE_RESUME_TEST] {
        fails_without_credentials(test);
    }
}

fn fails_without_credentials(test: &str) {
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["--ignored", "--exact", test])
        .env_remove(CREDENTIALS_VARIABLE)
        .env_remove(LOGIN_VARIABLE)
        .output()
        .unwrap();
    let report = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    assert!(!output.status.success(), "{report}");
    assert!(
        report.contains("ANTHROPIC_API_KEY environment variable is required"),
        "{report}"
    );
    assert!(report.contains("0 passed; 1 failed; 0 ignored"), "{report}");
}

#[test]
fn the_default_model_is_pinned_through_the_environment_of_each_session() {
    let pinned = pin_model(ClaudeAgentOptions::builder(), |_| None)
        .build()
        .unwrap();
    assert_eq!(pinned.env[MODEL_VARIABLE], PINNED_MODEL);
    assert_eq!(pinned.env[SESSION_MODEL_VARIABLE], PINNED_MODEL);

    let empty = pin_model(ClaudeAgentOptions::builder(), |_| Some(String::new()))
        .build()
        .unwrap();
    assert_eq!(empty.env[MODEL_VARIABLE], PINNED_MODEL);
    assert_eq!(empty.env[SESSION_MODEL_VARIABLE], PINNED_MODEL);

    let default_kept = pin_model(ClaudeAgentOptions::builder(), |name| {
        (name == MODEL_VARIABLE).then(|| "claude-opus-5".to_owned())
    })
    .build()
    .unwrap();
    assert!(!default_kept.env.contains_key(MODEL_VARIABLE));
    assert_eq!(default_kept.env[SESSION_MODEL_VARIABLE], "claude-opus-5");

    let both_kept = pin_model(ClaudeAgentOptions::builder(), |_| {
        Some("claude-opus-5".to_owned())
    })
    .build()
    .unwrap();
    assert!(!both_kept.env.contains_key(MODEL_VARIABLE));
    assert!(!both_kept.env.contains_key(SESSION_MODEL_VARIABLE));
}

#[tokio::test]
#[ignore = "runs the real CLI; run with `cargo xtask e2e`"]
async fn a_session_runs_on_the_pinned_model() {
    for variable in [MODEL_VARIABLE, SESSION_MODEL_VARIABLE] {
        assert!(
            std::env::var_os(variable).is_none(),
            "unset {variable}: this test checks the model the suite pins by itself"
        );
    }
    let options = suite::options().max_turns(1).build().unwrap();

    let messages = suite::run("Reply with the single word OK.", options).await;

    assert_eq!(suite::init(&messages)["model"], PINNED_MODEL);
    let result = suite::result(&messages);
    assert!(!result.is_error, "{result:#?}");
    let models: Vec<&String> = result
        .model_usage
        .iter()
        .flat_map(|usage| usage.keys())
        .collect();
    assert_eq!(models, [PINNED_MODEL], "{result:#?}");
}

#[tokio::test]
async fn a_cli_older_than_2_0_0_logs_a_warning_when_a_session_starts() {
    let (warnings, _guard) = Warnings::install();
    let fake = Fake::new(json!({
        "claude_code_version": null,
        "auto_reply": true,
        "steps": [
            {"expect": {"type": "user"}},
            {"emit": {"type": "result", "subtype": "success", "duration_ms": 1, "duration_api_ms": 1,
                      "is_error": false, "num_turns": 1, "session_id": "s", "result": "ok"}},
        ],
    }));

    let client = ClaudeSdkClient::new(fake.build());
    client.connect().await.unwrap();
    client.query("Hello").await.unwrap();
    let response: Vec<_> = client.receive_response().collect().await;
    client.disconnect().await.unwrap();

    assert!(response.iter().all(Result::is_ok), "{response:?}");
    let messages = warnings.about_the_cli_version();
    assert_eq!(messages.len(), 1, "{messages:?}");
    assert!(messages[0].contains("too old to report its version"));
    assert!(messages[0].contains(&format!(
        "Minimum required version is {MINIMUM_CLI_VERSION}"
    )));
    assert_eq!(MINIMUM_CLI_VERSION, "2.0.0");
}
