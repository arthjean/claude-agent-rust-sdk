//! Rust driver of the differential parity harness.
//!
//! `parity-driver <case.json> --cli <path>` plays one case of `parity/cases`
//! through `claude-agent-rust-sdk`, with `cli_path` set to `<path>`. The Python
//! driver, `parity/drivers/python_driver.py`, plays the same case through
//! the Python SDK, and `cargo xtask parity-diff` compares what the CLI
//! received from each. The case format is described by
//! `parity/case.schema.json` and `parity/README.md`; the translation of
//! options is documented in the `options` module.
//!
//! A case without `steps` runs the one-shot `query` with the case's prompt
//! and succeeds when the stream holds a result. A case with `steps`
//! drives a `ClaudeSdkClient` through them in order.
//!
//! When `PARITY_JOURNAL` names a file, the driver writes there what the
//! application observed: each message, the outcome of each call, and the
//! arguments of each callback (see the `journal` module). An error of the
//! SDK is journaled and ends the case, which then fails unless it sets
//! `"expect_error": true`.
//!
//! Exit status: 0 when the case ran, 1 when it failed, 2 on a usage error,
//! and 3 when the case uses an option, a step, or a value this driver does
//! not support; the unsupported item is printed on stderr.

mod journal;
mod options;
mod store;

use std::path::PathBuf;
use std::process::ExitCode;

use claude_agent_rust_sdk::{ClaudeSdkClient, Message, PermissionMode, Prompt, query};
use futures_util::StreamExt;
use serde_json::{Map, Value};

const EXIT_FAILED: u8 = 1;
const EXIT_USAGE: u8 = 2;
const EXIT_UNSUPPORTED: u8 = 3;

/// Why a case did not run.
pub(crate) enum Failure {
    /// The case needs something this driver does not implement.
    Unsupported(String),
    /// The case ran and failed.
    Failed(String),
    /// A call of the SDK failed; the error is journaled.
    Sdk(claude_agent_rust_sdk::Error),
}

impl From<claude_agent_rust_sdk::Error> for Failure {
    fn from(error: claude_agent_rust_sdk::Error) -> Self {
        Self::Sdk(error)
    }
}

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let (Some(case), Some("--cli"), Some(cli), None) = (
        arguments.first(),
        arguments.get(1).map(String::as_str),
        arguments.get(2),
        arguments.get(3),
    ) else {
        eprintln!("usage: parity-driver <case.json> --cli <path>");
        return ExitCode::from(EXIT_USAGE);
    };
    let text = match std::fs::read_to_string(case) {
        Ok(text) => text,
        Err(error) => {
            eprintln!("parity-driver: cannot read the case: {error}");
            return ExitCode::from(EXIT_USAGE);
        }
    };
    let case = match serde_json::from_str::<Value>(&text) {
        Ok(Value::Object(case)) => case,
        Ok(_) => {
            eprintln!("parity-driver: a case must be a JSON object");
            return ExitCode::from(EXIT_USAGE);
        }
        Err(error) => {
            eprintln!("parity-driver: cannot read the case: {error}");
            return ExitCode::from(EXIT_USAGE);
        }
    };
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("parity-driver: cannot start the runtime: {error}");
            return ExitCode::from(EXIT_FAILED);
        }
    };
    match runtime.block_on(run(&case, &text, PathBuf::from(cli))) {
        Ok(()) => ExitCode::SUCCESS,
        Err(Failure::Unsupported(item)) => {
            eprintln!("parity-driver: unsupported {item}");
            ExitCode::from(EXIT_UNSUPPORTED)
        }
        Err(Failure::Failed(reason)) => {
            eprintln!("parity-driver: {reason}");
            ExitCode::from(EXIT_FAILED)
        }
        Err(Failure::Sdk(error)) => {
            eprintln!("parity-driver: {error}");
            ExitCode::from(EXIT_FAILED)
        }
    }
}

async fn run(case: &Map<String, Value>, text: &str, cli: PathBuf) -> Result<(), Failure> {
    let options = options::build(text, cli)?;
    let expect_error = case
        .get("expect_error")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    match case.get("steps") {
        None => one_shot(case.get("prompt"), options, expect_error).await,
        Some(Value::Array(steps)) => client(steps, options, expect_error).await,
        Some(_) => Err(Failure::Failed("`steps` must be a list".to_owned())),
    }
}

fn prompt(value: Option<&Value>) -> Result<Prompt, Failure> {
    match value {
        Some(Value::String(text)) => Ok(Prompt::from(text.as_str())),
        Some(Value::Array(messages)) => {
            Ok(Prompt::stream(futures_util::stream::iter(messages.clone())))
        }
        _ => Err(Failure::Failed(
            "a prompt must be a string or a list of messages".to_owned(),
        )),
    }
}

/// Journals the outcome of one call: `step` is its index among the steps
/// of the case, or `null` for the one-shot query. An error of the SDK ends
/// the case: it passes when the case expects an error, and fails
/// otherwise.
fn journal_call(
    step: Value,
    method: &str,
    outcome: Result<Value, Failure>,
    expect_error: bool,
) -> Result<bool, Failure> {
    match outcome {
        Ok(result) => {
            journal::write(
                "call",
                journal::fields([
                    ("step", step),
                    ("method", Value::String(method.to_owned())),
                    ("result", result),
                ]),
            );
            Ok(true)
        }
        Err(Failure::Sdk(error)) => {
            journal::write(
                "call",
                journal::fields([
                    ("step", step),
                    ("method", Value::String(method.to_owned())),
                    ("error", journal::error(&error)),
                ]),
            );
            if expect_error {
                Ok(false)
            } else {
                Err(Failure::Failed(error.to_string()))
            }
        }
        Err(failure) => Err(failure),
    }
}

/// Reads messages until the stream ends, journaling each one, and requires
/// a result among them (a session state message may follow it).
async fn messages<S>(mut stream: S, what: &str) -> Result<Value, Failure>
where
    S: futures_util::Stream<Item = claude_agent_rust_sdk::Result<Message>> + Unpin,
{
    let mut result = false;
    while let Some(message) = stream.next().await {
        let message = message?;
        let is_result = matches!(message, Message::Result(_));
        let appends = is_result.then(store::appends).flatten();
        journal::write(
            "message",
            journal::fields([
                ("message", journal::message(&message)),
                ("store_appends", appends.map_or(Value::Null, Value::from)),
            ]),
        );
        result |= is_result;
    }
    if result {
        Ok(Value::Null)
    } else {
        Err(Failure::Failed(format!("{what} held no result")))
    }
}

async fn one_shot(
    value: Option<&Value>,
    options: claude_agent_rust_sdk::ClaudeAgentOptions,
    expect_error: bool,
) -> Result<(), Failure> {
    let stream = query(prompt(value)?, options);
    let outcome = messages(stream, "the stream").await;
    journal_call(Value::Null, "query", outcome, expect_error).map(|_| ())
}

async fn client(
    steps: &[Value],
    options: claude_agent_rust_sdk::ClaudeAgentOptions,
    expect_error: bool,
) -> Result<(), Failure> {
    let client = ClaudeSdkClient::new(options);
    for (index, step) in steps.iter().enumerate() {
        let Some((action, argument)) = step.as_object().and_then(|step| {
            let mut actions = step.iter();
            match (actions.next(), actions.next()) {
                (Some(action), None) => Some(action),
                _ => None,
            }
        }) else {
            return Err(Failure::Failed(format!(
                "step {index} must hold exactly one action"
            )));
        };
        let outcome = play(&client, action, argument).await;
        let go_on =
            journal_call(Value::from(index), action, outcome, expect_error).map_err(|failure| {
                match failure {
                    Failure::Failed(reason) => {
                        Failure::Failed(format!("step {index} ({action}): {reason}"))
                    }
                    other => other,
                }
            })?;
        if !go_on {
            let _ = client.disconnect().await;
            break;
        }
    }
    Ok(())
}

fn text(argument: &Value) -> Result<&str, Failure> {
    argument
        .as_str()
        .ok_or_else(|| Failure::Failed(format!("expected a string, got {argument}")))
}

/// Plays one step and returns the canonical form of what the method
/// returned, `null` for a method that returns nothing.
async fn play(client: &ClaudeSdkClient, action: &str, argument: &Value) -> Result<Value, Failure> {
    match action {
        "connect" => match argument.get("prompt") {
            Some(value) => client.connect_with_prompt(prompt(Some(value))?).await?,
            None => client.connect().await?,
        },
        "query" => {
            let value = argument.get("prompt");
            match argument.get("session_id").and_then(Value::as_str) {
                Some(session_id) => {
                    client
                        .query_with_session_id(prompt(value)?, session_id)
                        .await?
                }
                None => client.query(prompt(value)?).await?,
            }
        }
        "receive_response" => return messages(client.receive_response(), "the response").await,
        "interrupt" => client.interrupt().await?,
        "set_permission_mode" => {
            client
                .set_permission_mode(PermissionMode::from(text(argument)?))
                .await?
        }
        "set_model" => client.set_model(argument.as_str()).await?,
        "rewind_files" => client.rewind_files(text(argument)?).await?,
        "reconnect_mcp_server" => client.reconnect_mcp_server(text(argument)?).await?,
        "toggle_mcp_server" => {
            let name = argument
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let enabled = argument
                .get("enabled")
                .and_then(Value::as_bool)
                .unwrap_or(true);
            client.toggle_mcp_server(name, enabled).await?;
        }
        "stop_task" => client.stop_task(text(argument)?).await?,
        "get_mcp_status" => return Ok(journal::response(&client.mcp_status().await?)),
        "get_context_usage" => {
            return Ok(journal::response(&client.context_usage().await?));
        }
        "get_server_info" => return Ok(client.server_info()?.unwrap_or(Value::Null)),
        "disconnect" => client.disconnect().await?,
        other => return Err(Failure::Unsupported(format!("step {other}"))),
    }
    Ok(Value::Null)
}
