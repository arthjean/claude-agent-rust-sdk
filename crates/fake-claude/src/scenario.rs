//! Scenario format played by `fake-claude`.
//!
//! A scenario is a JSON object:
//!
//! ```json
//! {
//!   "claude_code_version": "2.1.325",
//!   "init": true,
//!   "auto_reply": true,
//!   "report_env": ["CLAUDE_CODE_ENTRYPOINT"],
//!   "steps": [
//!     {"expect": {"type": "user"}},
//!     {"emit": {"type": "result", "subtype": "success"}},
//!     {"exit": 0}
//!   ]
//! }
//! ```
//!
//! - `claude_code_version`: version reported in the init frame; `null`
//!   leaves the field out, as CLIs older than 2.0 do. Defaults to
//!   [`DEFAULT_VERSION`].
//! - `init`: whether an init frame is emitted first. Defaults to `true`.
//!   The frame carries `fake_args`, the arguments the process received,
//!   and `fake_env`, the value of each variable named in `report_env`.
//! - `auto_reply`: answers every `control_request` read from stdin with a
//!   success response, as soon as it arrives. Defaults to `false`.
//! - `read_stdin`: whether stdin is read at all. Defaults to `true`. When
//!   `false`, nothing drains stdin, so a writer eventually blocks.
//! - `steps`: actions played in order. After the last one the process waits
//!   for stdin to close and exits with code 0.
//!
//! Each step is an object with one action:
//!
//! | Action | Effect |
//! |---|---|
//! | `{"emit": frame}` | Writes the frame and a line feed to stdout. A string equal to `$hook_callback_id` is replaced by the first hook callback id of the `initialize` request. |
//! | `{"emit_raw": text}` | Writes the text to stdout as is, without a line feed. |
//! | `{"stderr": text}` | Writes the text to stderr as is. |
//! | `{"expect": pattern, "timeout_ms": n}` | Waits for a stdin frame containing the pattern: every key of a pattern object must be present with a matching value. Frames that do not match are skipped. The limit defaults to 10,000 ms. |
//! | `{"control_response": payload}` | Answers the control request matched by the last `expect` with a success response carrying the payload. |
//! | `{"control_error": text}` | Answers the control request matched by the last `expect` with an error response. |
//! | `{"session_state": state}` | Emits a `session_state_changed` frame the way the CLI does: as is when `CLAUDE_CODE_EMIT_SESSION_STATE_EVENTS` is truthy, marked `sdk_host_only` when `CLAUDE_CODE_SDK_READS_SESSION_STATE` is truthy, not at all otherwise. |
//! | `{"sleep_ms": n}` | Sleeps. |
//! | `{"spawn_descendant": {"sleep_ms": n}}` | Starts a detached copy of this program that sleeps, and emits `{"type": "fake", "subtype": "descendant", "pid": n}`. |
//! | `{"wait_for_eof": true}` | Waits for stdin to close. |
//! | `{"exit": code}` | Exits with the code. |

use std::time::Duration;

use serde_json::{Map, Value};

pub(crate) const DEFAULT_VERSION: &str = "2.1.325";
pub(crate) const DEFAULT_EXPECT_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Scenario {
    pub(crate) claude_code_version: Option<String>,
    pub(crate) init: bool,
    pub(crate) auto_reply: bool,
    pub(crate) read_stdin: bool,
    pub(crate) report_env: Vec<String>,
    pub(crate) steps: Vec<Step>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Step {
    Emit(Value),
    EmitRaw(String),
    Stderr(String),
    Expect { pattern: Value, timeout: Duration },
    ControlResponse(Value),
    ControlError(String),
    SessionState(String),
    Sleep(Duration),
    SpawnDescendant { sleep: Duration },
    WaitForEof,
    Exit(u8),
}

impl Scenario {
    pub(crate) fn stand_in() -> Self {
        Self {
            claude_code_version: Some(DEFAULT_VERSION.to_owned()),
            init: true,
            auto_reply: true,
            read_stdin: true,
            report_env: Vec::new(),
            steps: Vec::new(),
        }
    }

    pub(crate) fn parse(text: &str) -> Result<Self, String> {
        let value: Value =
            serde_json::from_str(text).map_err(|error| format!("invalid JSON: {error}"))?;
        let Value::Object(scenario) = value else {
            return Err("a scenario must be a JSON object".to_owned());
        };

        let claude_code_version = match scenario.get("claude_code_version") {
            None => Some(DEFAULT_VERSION.to_owned()),
            Some(Value::Null) => None,
            Some(Value::String(version)) => Some(version.clone()),
            Some(_) => return Err("`claude_code_version` must be a string or null".to_owned()),
        };
        let report_env = match scenario.get("report_env") {
            None => Vec::new(),
            Some(Value::Array(names)) => names
                .iter()
                .map(|name| name.as_str().map(str::to_owned))
                .collect::<Option<Vec<_>>>()
                .ok_or("`report_env` must be a list of strings")?,
            Some(_) => return Err("`report_env` must be a list of strings".to_owned()),
        };
        let steps = match scenario.get("steps") {
            None => Vec::new(),
            Some(Value::Array(steps)) => steps
                .iter()
                .enumerate()
                .map(|(index, step)| {
                    Step::parse(step).map_err(|error| format!("step {index}: {error}"))
                })
                .collect::<Result<Vec<_>, _>>()?,
            Some(_) => return Err("`steps` must be a list".to_owned()),
        };

        Ok(Self {
            claude_code_version,
            init: flag(&scenario, "init", true)?,
            auto_reply: flag(&scenario, "auto_reply", false)?,
            read_stdin: flag(&scenario, "read_stdin", true)?,
            report_env,
            steps,
        })
    }
}

fn flag(object: &Map<String, Value>, name: &str, default: bool) -> Result<bool, String> {
    match object.get(name) {
        None => Ok(default),
        Some(Value::Bool(value)) => Ok(*value),
        Some(_) => Err(format!("`{name}` must be a boolean")),
    }
}

fn milliseconds(value: &Value, name: &str) -> Result<Duration, String> {
    value
        .as_u64()
        .map(Duration::from_millis)
        .ok_or_else(|| format!("`{name}` must be a number of milliseconds"))
}

fn text(value: &Value, name: &str) -> Result<String, String> {
    value
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| format!("`{name}` must be a string"))
}

impl Step {
    fn parse(step: &Value) -> Result<Self, String> {
        let Value::Object(step) = step else {
            return Err("a step must be a JSON object".to_owned());
        };
        if let Some(pattern) = step.get("expect") {
            let timeout = match step.get("timeout_ms") {
                Some(limit) => milliseconds(limit, "timeout_ms")?,
                None => DEFAULT_EXPECT_TIMEOUT,
            };
            return Ok(Self::Expect {
                pattern: pattern.clone(),
                timeout,
            });
        }
        let mut actions = step.iter();
        let (Some((action, argument)), None) = (actions.next(), actions.next()) else {
            return Err("a step must hold exactly one action".to_owned());
        };
        match action.as_str() {
            "emit" => Ok(Self::Emit(argument.clone())),
            "emit_raw" => Ok(Self::EmitRaw(text(argument, action)?)),
            "stderr" => Ok(Self::Stderr(text(argument, action)?)),
            "control_response" => Ok(Self::ControlResponse(argument.clone())),
            "control_error" => Ok(Self::ControlError(text(argument, action)?)),
            "session_state" => Ok(Self::SessionState(text(argument, action)?)),
            "sleep_ms" => Ok(Self::Sleep(milliseconds(argument, action)?)),
            "spawn_descendant" => {
                let sleep = argument
                    .get("sleep_ms")
                    .ok_or("`spawn_descendant` needs `sleep_ms`".to_owned())
                    .and_then(|limit| milliseconds(limit, "sleep_ms"))?;
                Ok(Self::SpawnDescendant { sleep })
            }
            "wait_for_eof" => Ok(Self::WaitForEof),
            "exit" => argument
                .as_u64()
                .and_then(|code| u8::try_from(code).ok())
                .map(Self::Exit)
                .ok_or_else(|| "`exit` must be a code between 0 and 255".to_owned()),
            unknown => Err(format!("unknown action `{unknown}`")),
        }
    }
}

pub(crate) fn matches(pattern: &Value, frame: &Value) -> bool {
    match (pattern, frame) {
        (Value::Object(pattern), Value::Object(frame)) => pattern.iter().all(|(key, expected)| {
            frame
                .get(key)
                .is_some_and(|actual| matches(expected, actual))
        }),
        _ => pattern == frame,
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use serde_json::json;

    use super::{DEFAULT_EXPECT_TIMEOUT, Scenario, Step, matches};

    #[test]
    fn defaults_follow_the_documented_format() {
        let scenario = Scenario::parse("{}").unwrap();
        assert_eq!(scenario.claude_code_version.as_deref(), Some("2.1.325"));
        assert!(scenario.init);
        assert!(!scenario.auto_reply);
        assert!(scenario.read_stdin);
        assert!(scenario.steps.is_empty());
        assert_eq!(DEFAULT_EXPECT_TIMEOUT, Duration::from_secs(10));
    }

    #[test]
    fn every_action_parses() {
        let scenario = Scenario::parse(
            &json!({
                "claude_code_version": null,
                "init": false,
                "auto_reply": true,
                "report_env": ["A"],
                "steps": [
                    {"emit": {"type": "result"}},
                    {"emit_raw": "partial"},
                    {"stderr": "warning\n"},
                    {"expect": {"type": "user"}},
                    {"expect": {"type": "user"}, "timeout_ms": 250},
                    {"control_response": {"ok": true}},
                    {"control_error": "refused"},
                    {"session_state": "idle"},
                    {"sleep_ms": 5},
                    {"spawn_descendant": {"sleep_ms": 1000}},
                    {"wait_for_eof": true},
                    {"exit": 3},
                ],
            })
            .to_string(),
        )
        .unwrap();
        assert_eq!(scenario.claude_code_version, None);
        assert!(!scenario.init);
        assert_eq!(scenario.report_env, ["A"]);
        assert_eq!(
            scenario.steps,
            [
                Step::Emit(json!({"type": "result"})),
                Step::EmitRaw("partial".to_owned()),
                Step::Stderr("warning\n".to_owned()),
                Step::Expect {
                    pattern: json!({"type": "user"}),
                    timeout: DEFAULT_EXPECT_TIMEOUT,
                },
                Step::Expect {
                    pattern: json!({"type": "user"}),
                    timeout: Duration::from_millis(250),
                },
                Step::ControlResponse(json!({"ok": true})),
                Step::ControlError("refused".to_owned()),
                Step::SessionState("idle".to_owned()),
                Step::Sleep(Duration::from_millis(5)),
                Step::SpawnDescendant {
                    sleep: Duration::from_secs(1),
                },
                Step::WaitForEof,
                Step::Exit(3),
            ]
        );
    }

    #[test]
    fn malformed_scenarios_name_the_problem() {
        assert!(Scenario::parse("[").unwrap_err().contains("invalid JSON"));
        assert!(Scenario::parse("[]").unwrap_err().contains("JSON object"));
        let error = Scenario::parse(r#"{"steps": [{"emit": 1}, {"dance": 1}]}"#).unwrap_err();
        assert_eq!(error, "step 1: unknown action `dance`");
        let error = Scenario::parse(r#"{"steps": [{"exit": 300}]}"#).unwrap_err();
        assert!(error.contains("between 0 and 255"));
        let error = Scenario::parse(r#"{"steps": [{"emit": 1, "sleep_ms": 2}]}"#).unwrap_err();
        assert!(error.contains("exactly one action"));
    }

    #[test]
    fn patterns_match_a_subset_of_the_frame() {
        let frame = json!({
            "type": "control_response",
            "response": {"request_id": "hook-1", "subtype": "success", "response": {}},
        });
        assert!(matches(&json!({}), &frame));
        assert!(matches(&json!({"type": "control_response"}), &frame));
        assert!(matches(
            &json!({"response": {"request_id": "hook-1"}}),
            &frame
        ));
        assert!(!matches(
            &json!({"response": {"request_id": "hook-2"}}),
            &frame
        ));
        assert!(!matches(&json!({"missing": null}), &frame));
        assert!(!matches(&json!({"response": {"response": []}}), &frame));
    }
}
