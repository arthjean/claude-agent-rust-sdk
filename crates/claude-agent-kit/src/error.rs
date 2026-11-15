//! Error model of the crate.
//!
//! Every fallible operation returns [`Error`], a single enum whose variants
//! name the failure cause, so callers match on a variant instead of parsing
//! text.
//!
//! Message texts follow the upstream Python SDK, with one deliberate
//! difference: they name the executable "Claude CLI".

use std::fmt;
use std::ops::Deref;

use serde_json::{Map, Value};

/// Result alias whose error type defaults to [`Error`].
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Boxed error returned by application callbacks (hooks, tool permission).
pub type BoxError = Box<dyn std::error::Error + Send + Sync + 'static>;

/// Every failure this crate reports.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The CLI executable was not found in any searched location.
    #[error("{message}")]
    CliNotFound {
        /// Human-readable description, including the searched path when known.
        message: String,
    },

    /// The CLI could not be reached, or the session is not connected.
    #[error("{message}")]
    CliConnection {
        /// Human-readable description of the connection failure.
        message: String,
    },

    /// The CLI process failed.
    #[error("{}", process_text(message, *exit_code, stderr.as_deref()))]
    Process {
        /// Description of the failure, without the exit code or stderr.
        message: String,
        /// Exit code of the process, when it exited.
        exit_code: Option<i32>,
        /// Captured error output, when available.
        stderr: Option<String>,
    },

    /// The CLI reported a terminal error result and then exited.
    ///
    /// The CLI ends a failed run by emitting a `result` message with
    /// `is_error: true` and exiting with a non-zero code. This variant
    /// replaces the bare [`Error::Process`] for that case and carries the
    /// payload of the result, so callers branch on why the run failed.
    #[error("{}", process_text(message, *exit_code, None))]
    ResultError {
        /// Description of the failure, without the exit code.
        message: String,
        /// Raw `result` payload emitted by the CLI, with typed accessors.
        data: ResultErrorData,
        /// Exit code of the process, when it exited.
        exit_code: Option<i32>,
    },

    /// A line of CLI output started like a JSON object but did not parse.
    #[error(
        "Failed to decode JSON: {}...",
        leading_chars(line, JSON_DECODE_PREVIEW_CHARS)
    )]
    JsonDecode {
        /// The offending line, in full.
        line: String,
        /// The underlying JSON error.
        source: serde_json::Error,
    },

    /// A JSON message from the CLI did not have the expected shape.
    #[error("{message}")]
    MessageParse {
        /// Description of the missing or malformed part.
        message: String,
        /// The message that failed to parse, when available.
        data: Option<Value>,
    },

    /// A control request received no response in time.
    #[error("Control request timeout: {subtype}")]
    ControlTimeout {
        /// Subtype of the request that timed out.
        subtype: String,
    },

    /// The CLI answered a control request with an error.
    #[error("{message}")]
    Control {
        /// Error text reported by the CLI.
        message: String,
    },

    /// The options are invalid or mutually exclusive.
    #[error("{message}")]
    Config {
        /// Description of the invalid configuration.
        message: String,
    },
}

const JSON_DECODE_PREVIEW_CHARS: usize = 100;
const DEFAULT_CLI_NOT_FOUND_MESSAGE: &str = "Claude CLI not found";
const ERROR_RESULT_PREFIX: &str = "Claude CLI returned an error result: ";

impl Error {
    /// Builds [`Error::CliNotFound`], appending the searched path when given.
    #[must_use]
    pub fn cli_not_found(message: Option<&str>, cli_path: Option<&str>) -> Self {
        let message = message.unwrap_or(DEFAULT_CLI_NOT_FOUND_MESSAGE);
        let message = match cli_path {
            Some(path) if !path.is_empty() => format!("{message}: {path}"),
            _ => message.to_owned(),
        };
        Self::CliNotFound { message }
    }

    /// Builds [`Error::CliConnection`].
    #[must_use]
    pub fn cli_connection(message: impl Into<String>) -> Self {
        Self::CliConnection {
            message: message.into(),
        }
    }

    /// Builds [`Error::Process`].
    #[must_use]
    pub fn process(
        message: impl Into<String>,
        exit_code: Option<i32>,
        stderr: Option<String>,
    ) -> Self {
        Self::Process {
            message: message.into(),
            exit_code,
            stderr,
        }
    }

    /// Builds [`Error::ResultError`] from the raw `result` payload.
    ///
    /// The message is the upstream error-result prefix followed by
    /// [`ResultErrorData::summary`].
    #[must_use]
    pub fn result_error(data: Value, exit_code: Option<i32>) -> Self {
        let data = ResultErrorData::from(data);
        Self::ResultError {
            message: format!("{ERROR_RESULT_PREFIX}{}", data.summary()),
            data,
            exit_code,
        }
    }

    /// Builds [`Error::JsonDecode`].
    #[must_use]
    pub fn json_decode(line: impl Into<String>, source: serde_json::Error) -> Self {
        Self::JsonDecode {
            line: line.into(),
            source,
        }
    }

    /// Builds [`Error::MessageParse`].
    #[must_use]
    pub fn message_parse(message: impl Into<String>, data: Option<Value>) -> Self {
        Self::MessageParse {
            message: message.into(),
            data,
        }
    }

    /// Builds [`Error::Config`].
    #[must_use]
    pub fn config(message: impl Into<String>) -> Self {
        Self::Config {
            message: message.into(),
        }
    }

    /// Builds [`Error::Control`].
    #[must_use]
    pub fn control(message: impl Into<String>) -> Self {
        Self::Control {
            message: message.into(),
        }
    }

    /// Builds [`Error::ControlTimeout`].
    #[must_use]
    pub fn control_timeout(subtype: impl Into<String>) -> Self {
        Self::ControlTimeout {
            subtype: subtype.into(),
        }
    }

    /// Copies the error so one failure can be reported to several waiters.
    /// The JSON error of [`Error::JsonDecode`] is rebuilt from its text.
    pub(crate) fn duplicate(&self) -> Self {
        match self {
            Self::CliNotFound { message } => Self::CliNotFound {
                message: message.clone(),
            },
            Self::CliConnection { message } => Self::CliConnection {
                message: message.clone(),
            },
            Self::Process {
                message,
                exit_code,
                stderr,
            } => Self::Process {
                message: message.clone(),
                exit_code: *exit_code,
                stderr: stderr.clone(),
            },
            Self::ResultError {
                message,
                data,
                exit_code,
            } => Self::ResultError {
                message: message.clone(),
                data: data.clone(),
                exit_code: *exit_code,
            },
            Self::JsonDecode { line, source } => Self::JsonDecode {
                line: line.clone(),
                source: <serde_json::Error as serde::de::Error>::custom(source.to_string()),
            },
            Self::MessageParse { message, data } => Self::MessageParse {
                message: message.clone(),
                data: data.clone(),
            },
            Self::ControlTimeout { subtype } => Self::ControlTimeout {
                subtype: subtype.clone(),
            },
            Self::Control { message } => Self::Control {
                message: message.clone(),
            },
            Self::Config { message } => Self::Config {
                message: message.clone(),
            },
        }
    }

    /// Exit code of the CLI process, for the variants that carry one.
    #[must_use]
    pub fn exit_code(&self) -> Option<i32> {
        match self {
            Self::Process { exit_code, .. } | Self::ResultError { exit_code, .. } => *exit_code,
            _ => None,
        }
    }
}

/// Raw `result` payload carried by [`Error::ResultError`].
///
/// Dereferences to the JSON object emitted by the CLI. A payload that is not
/// a JSON object is replaced by an empty one. The accessors return `None`
/// for a missing or mistyped field.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ResultErrorData(Map<String, Value>);

impl ResultErrorData {
    /// Result subtype, such as `error_max_turns` or `error_during_execution`.
    ///
    /// The value is `success` when the agent loop completed but its last turn
    /// was an API error.
    #[must_use]
    pub fn subtype(&self) -> Option<&str> {
        self.string("subtype")
    }

    /// Error strings reported by the CLI, trimmed, with blank entries dropped.
    ///
    /// A bare string is accepted in place of a list.
    #[must_use]
    pub fn errors(&self) -> Vec<String> {
        normalize_result_errors(self.0.get("errors"))
    }

    /// Result text, when any. For API failures it holds the error prose.
    #[must_use]
    pub fn result(&self) -> Option<&str> {
        self.string("result")
    }

    /// HTTP status of the failing API call, when any.
    #[must_use]
    pub fn api_error_status(&self) -> Option<i64> {
        self.0.get("api_error_status").and_then(Value::as_i64)
    }

    /// Why the run ended, such as `api_error` or `max_turns`, when reported.
    #[must_use]
    pub fn terminal_reason(&self) -> Option<&str> {
        self.string("terminal_reason")
    }

    /// Session the result belongs to, when reported.
    #[must_use]
    pub fn session_id(&self) -> Option<&str> {
        self.string("session_id")
    }

    /// Most informative text of the payload.
    ///
    /// The order of preference is the joined `errors`, then `result`, then a
    /// `subtype` other than `success`, then `API error (HTTP n)`, then
    /// `unknown error`.
    #[must_use]
    pub fn summary(&self) -> String {
        let errors = self.errors();
        if !errors.is_empty() {
            return errors.join("; ");
        }
        if let Some(result) = self.result().map(str::trim)
            && !result.is_empty()
        {
            return result.to_owned();
        }
        if let Some(subtype) = self.subtype()
            && !subtype.is_empty()
            && subtype != "success"
        {
            return subtype.to_owned();
        }
        match self.0.get("api_error_status") {
            Some(status) if !status.is_null() => format!("API error (HTTP {})", plain(status)),
            _ => "unknown error".to_owned(),
        }
    }

    /// Consumes the wrapper and returns the JSON object.
    #[must_use]
    pub fn into_inner(self) -> Map<String, Value> {
        self.0
    }

    fn string(&self, key: &str) -> Option<&str> {
        self.0.get(key).and_then(Value::as_str)
    }
}

impl Deref for ResultErrorData {
    type Target = Map<String, Value>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl From<Value> for ResultErrorData {
    fn from(value: Value) -> Self {
        match value {
            Value::Object(map) => Self(map),
            _ => Self::default(),
        }
    }
}

impl From<Map<String, Value>> for ResultErrorData {
    fn from(map: Map<String, Value>) -> Self {
        Self(map)
    }
}

fn normalize_result_errors(raw: Option<&Value>) -> Vec<String> {
    let clean = |text: &str| {
        let trimmed = text.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_owned())
    };
    match raw {
        Some(Value::String(text)) => clean(text).into_iter().collect(),
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(Value::as_str)
            .filter_map(clean)
            .collect(),
        _ => Vec::new(),
    }
}

fn plain(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

fn leading_chars(text: &str, count: usize) -> &str {
    match text.char_indices().nth(count) {
        Some((index, _)) => &text[..index],
        None => text,
    }
}

fn process_text<'a>(
    message: &'a str,
    exit_code: Option<i32>,
    stderr: Option<&'a str>,
) -> ProcessText<'a> {
    ProcessText {
        message,
        exit_code,
        stderr,
    }
}

struct ProcessText<'a> {
    message: &'a str,
    exit_code: Option<i32>,
    stderr: Option<&'a str>,
}

impl fmt::Display for ProcessText<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.message)?;
        if let Some(code) = self.exit_code {
            write!(formatter, " (exit code: {code})")?;
        }
        if let Some(stderr) = self.stderr
            && !stderr.is_empty()
        {
            write!(formatter, "\nError output: {stderr}")?;
        }
        Ok(())
    }
}
