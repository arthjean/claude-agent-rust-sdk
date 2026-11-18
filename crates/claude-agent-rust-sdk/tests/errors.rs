#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use claude_agent_rust_sdk::{Error, ResultErrorData};
use serde_json::{Value, json};

fn result_error_data(error: &Error) -> &ResultErrorData {
    match error {
        Error::ResultError { data, .. } => data,
        other => panic!("expected ResultError, got {other:?}"),
    }
}

fn invalid_json_error(line: &str) -> Error {
    let source = serde_json::from_str::<Value>(line).unwrap_err();
    Error::json_decode(line, source)
}

#[test]
fn config_error_is_neither_a_process_nor_a_connection_failure() {
    let error = Error::config("bad option");
    assert!(!error.is_process());
    assert!(!error.is_cli_connection());
}

#[test]
fn result_error_is_a_process_failure_and_cli_not_found_a_connection_failure() {
    let result = Error::result_error(json!({"subtype": "error_max_turns"}), Some(1));
    assert!(result.is_process());
    assert!(!result.is_cli_connection());
    let missing = Error::cli_not_found(None, Some("/nowhere/claude"));
    assert!(missing.is_cli_connection());
    assert!(!missing.is_process());
}

#[test]
fn error_is_send_sync_and_static() {
    fn assert_bounds<T: Send + Sync + 'static + std::error::Error>() {}
    assert_bounds::<Error>();
}

#[test]
fn every_specified_variant_exists_and_displays_its_message() {
    let errors = [
        Error::CliNotFound {
            message: "not found".to_owned(),
        },
        Error::CliConnection {
            message: "no connection".to_owned(),
        },
        Error::Process {
            message: "failed".to_owned(),
            exit_code: None,
            stderr: None,
        },
        Error::ResultError {
            message: "result".to_owned(),
            data: ResultErrorData::default(),
            exit_code: None,
        },
        invalid_json_error("{bad"),
        Error::MessageParse {
            message: "parse".to_owned(),
            data: None,
        },
        Error::ControlTimeout {
            subtype: "initialize".to_owned(),
        },
        Error::Control {
            message: "control".to_owned(),
        },
        Error::Config {
            message: "config".to_owned(),
        },
    ];
    let texts: Vec<String> = errors.iter().map(ToString::to_string).collect();
    assert_eq!(
        texts,
        [
            "not found",
            "no connection",
            "failed",
            "result",
            "Failed to decode JSON: {bad...",
            "parse",
            "Control request timeout: initialize",
            "control",
            "config",
        ]
    );
}

#[test]
fn cli_not_found_error() {
    let error = Error::cli_not_found(None, None);
    assert!(matches!(error, Error::CliNotFound { .. }));
    assert_eq!(error.to_string(), "Claude CLI not found");

    let error = Error::cli_not_found(Some("Claude CLI not found"), Some("/usr/bin/claude"));
    assert_eq!(error.to_string(), "Claude CLI not found: /usr/bin/claude");
}

#[test]
fn connection_error() {
    let error = Error::cli_connection("Failed to connect to CLI");
    assert!(matches!(error, Error::CliConnection { .. }));
    assert!(error.to_string().contains("Failed to connect to CLI"));
}

#[test]
fn process_error_with_exit_code_and_stderr() {
    let error = Error::process(
        "Process failed",
        Some(1),
        Some("Command not found".to_owned()),
    );
    match &error {
        Error::Process {
            exit_code, stderr, ..
        } => {
            assert_eq!(*exit_code, Some(1));
            assert_eq!(stderr.as_deref(), Some("Command not found"));
        }
        other => panic!("expected Process, got {other:?}"),
    }
    assert_eq!(error.exit_code(), Some(1));
    assert_eq!(
        error.to_string(),
        "Process failed (exit code: 1)\nError output: Command not found"
    );
}

#[test]
fn process_error_without_details_displays_only_the_message() {
    let error = Error::process("Process failed", None, Some(String::new()));
    assert_eq!(error.to_string(), "Process failed");
}

#[test]
fn result_error_carries_payload() {
    let data = json!({
        "type": "result",
        "subtype": "success",
        "is_error": true,
        "errors": [],
        "result": "API Error: Stream idle timeout - no chunks received",
        "api_error_status": null,
        "terminal_reason": "api_error",
        "session_id": "s-1",
    });
    let error = Error::result_error(data.clone(), Some(1));
    assert_eq!(error.exit_code(), Some(1));
    let payload = result_error_data(&error);
    assert_eq!(Value::Object(payload.clone().into_inner()), data);
    assert_eq!(payload.subtype(), Some("success"));
    assert!(payload.errors().is_empty());
    assert_eq!(
        payload.result(),
        Some("API Error: Stream idle timeout - no chunks received")
    );
    assert_eq!(payload.api_error_status(), None);
    assert_eq!(payload.terminal_reason(), Some("api_error"));
    assert_eq!(payload.session_id(), Some("s-1"));
    assert_eq!(payload["terminal_reason"], "api_error");
    assert_eq!(
        error.to_string(),
        "Claude Code returned an error result: API Error: Stream idle timeout - no chunks \
         received (exit code: 1)"
    );
}

#[test]
fn result_error_exposes_subtype_terminal_reason_and_api_error_status() {
    let error = Error::result_error(
        json!({
            "subtype": "error_max_turns",
            "terminal_reason": "max_turns",
            "api_error_status": 529,
        }),
        Some(1),
    );
    let payload = result_error_data(&error);
    assert_eq!(payload.subtype(), Some("error_max_turns"));
    assert_eq!(payload.terminal_reason(), Some("max_turns"));
    assert_eq!(payload.api_error_status(), Some(529));
}

#[test]
fn result_error_tolerates_missing_or_malformed_fields() {
    let error = Error::result_error(json!({"errors": 42, "api_error_status": "500"}), None);
    let payload = result_error_data(&error);
    assert_eq!(payload.subtype(), None);
    assert!(payload.errors().is_empty());
    assert_eq!(payload.result(), None);
    assert_eq!(payload.api_error_status(), None);
    assert_eq!(payload.terminal_reason(), None);
    assert_eq!(payload.session_id(), None);
    assert_eq!(error.exit_code(), None);

    let error = Error::result_error(json!("not an object"), None);
    assert!(result_error_data(&error).is_empty());
}

#[test]
fn result_error_normalizes_errors_like_the_message_text() {
    let bare = Error::result_error(json!({"errors": "boom"}), None);
    assert_eq!(result_error_data(&bare).errors(), ["boom"]);

    let mixed = Error::result_error(json!({"errors": [" ", "x ", 3]}), None);
    assert_eq!(result_error_data(&mixed).errors(), ["x"]);
}

#[test]
fn result_error_text_prefers_errors_then_result_then_subtype_then_status() {
    let text = |data: Value| ResultErrorData::from(data).summary();
    assert_eq!(
        text(json!({"errors": ["a", " b "], "result": "ignored", "subtype": "error_x"})),
        "a; b"
    );
    assert_eq!(
        text(json!({"errors": [], "result": "  API Error: overloaded  ", "subtype": "success"})),
        "API Error: overloaded"
    );
    assert_eq!(
        text(json!({"result": "   ", "subtype": "error_max_turns"})),
        "error_max_turns"
    );
    assert_eq!(
        text(json!({"subtype": "success", "api_error_status": 529})),
        "API error (HTTP 529)"
    );
    assert_eq!(text(json!({"subtype": "success"})), "unknown error");
    assert_eq!(text(json!({})), "unknown error");
}

#[test]
fn json_decode_error_keeps_the_line_and_the_source() {
    let error = invalid_json_error("{invalid json}");
    match &error {
        Error::JsonDecode { line, source } => {
            assert_eq!(line, "{invalid json}");
            assert!(!source.to_string().is_empty());
        }
        other => panic!("expected JsonDecode, got {other:?}"),
    }
    assert!(error.to_string().contains("Failed to decode JSON"));
    assert!(std::error::Error::source(&error).is_some());
}

#[test]
fn json_decode_error_displays_at_most_100_characters_of_the_line() {
    let line = format!("{{{}", "é".repeat(300));
    let error = invalid_json_error(&line);
    let text = error.to_string();
    let preview = text
        .strip_prefix("Failed to decode JSON: ")
        .and_then(|rest| rest.strip_suffix("..."))
        .unwrap();
    assert_eq!(preview.chars().count(), 100);
    assert!(line.starts_with(preview));

    let short = invalid_json_error("{short");
    assert_eq!(short.to_string(), "Failed to decode JSON: {short...");
}

#[test]
fn message_parse_error_carries_the_data() {
    let data = json!({"type": "assistant"});
    let error = Error::message_parse("Missing required field", Some(data.clone()));
    match error {
        Error::MessageParse {
            message,
            data: carried,
        } => {
            assert_eq!(message, "Missing required field");
            assert_eq!(carried, Some(data));
        }
        other => panic!("expected MessageParse, got {other:?}"),
    }
}

#[test]
fn no_public_signature_mentions_anyhow() {
    let source_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let manifest = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml"),
    )
    .unwrap();
    assert!(!manifest.contains("anyhow"));

    let mut pending = vec![source_dir];
    let mut checked = 0;
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(directory).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                let source = std::fs::read_to_string(&path).unwrap();
                assert!(
                    !source.contains("anyhow"),
                    "{} mentions anyhow",
                    path.display()
                );
                checked += 1;
            }
        }
    }
    assert!(checked > 5);
}
