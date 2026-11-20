//! Error model of the crate.
//!
//! Every fallible operation returns [`Error`], an opaque value whose
//! [`kind`](Error::kind) names the cause, the way [`std::io::Error`] does.
//! The data of a cause is read through accessors ([`Error::exit_code`],
//! [`Error::result`], ...), so a later release can add data without
//! breaking a caller, and [`ErrorKind`] can gain kinds for the same reason.
//! [`Error::is_retryable`] says whether trying again can succeed.
//!
//! An error is cheap to clone: one failure reaches every waiter of a
//! session as clones of one value, with its typed source.
//!
//! The error model differs from upstream in deliberate ways:
//!
//! - Python has a class hierarchy (`ResultError` is a `ProcessError`,
//!   `CLINotFoundError` is a `CLIConnectionError`), and this crate has one
//!   kind per cause. [`Error::is_process`] and [`Error::is_cli_connection`]
//!   answer the questions an `except` on the parent class asks.
//! - Python raises `CLIConnectionError` for about a dozen causes; here they
//!   are told apart: [`ErrorKind::Spawn`], [`ErrorKind::NotConnected`],
//!   [`ErrorKind::ConnectionClosed`], and [`ErrorKind::Unsupported`].
//! - The text of an error does not repeat its [`source`](std::error::Error::source),
//!   which carries the underlying error instead.
//! - [`Error::stderr`] holds the last lines the CLI wrote to its error
//!   output when they were captured (a [`StderrCallback`](crate::StderrCallback)
//!   was set), where Python always says "Check stderr output for details".

use std::fmt;
use std::ops::Deref;
use std::sync::Arc;

use serde_json::{Map, Value};

use crate::session_store::UnsupportedStoreOperation;

/// Result alias whose error type defaults to [`Error`].
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Boxed error returned by application callbacks (hooks, tool permission).
pub type BoxError = Box<dyn std::error::Error + Send + Sync + 'static>;

type SharedSource = Arc<dyn std::error::Error + Send + Sync + 'static>;

/// What went wrong, as [`Error::kind`] reports it.
///
/// New kinds may be added in a minor release, so a `match` needs a
/// wildcard arm.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ErrorKind {
    /// The CLI executable was not found in any searched location.
    CliNotFound,
    /// The CLI process could not be started: the executable failed to run,
    /// the working directory is missing, the settings could not be read, or
    /// the executable is a Windows batch script, which is refused.
    Spawn,
    /// The call needs a connected session or transport, and there is none:
    /// it was never connected, or it was disconnected.
    NotConnected,
    /// The CLI exited or closed its pipes before the call completed: a write
    /// to a process that has ended, output that ended before a response, or
    /// a session closed while a request waited.
    ConnectionClosed,
    /// The CLI cannot do what the options require, such as honoring
    /// `on_failure` on hook matchers.
    Unsupported,
    /// The CLI process exited with an error code.
    Process,
    /// The CLI reported a terminal error result and then exited.
    ///
    /// The CLI ends a failed run by emitting a `result` message with
    /// `is_error: true` and exiting with a non-zero code. This kind replaces
    /// the bare [`ErrorKind::Process`] for that case; [`Error::result`]
    /// holds the payload of the result, so callers branch on why the run
    /// failed.
    ResultError,
    /// A line of CLI output started like a JSON object but did not parse.
    /// [`Error::line`] holds it.
    JsonDecode,
    /// A JSON message from the CLI did not have the expected shape.
    /// [`Error::data`] holds it when available.
    MessageParse,
    /// A control request received no response in time.
    /// [`Error::control_subtype`] names it.
    ControlTimeout,
    /// The CLI answered a control request with an error.
    Control,
    /// The options or arguments are invalid or contradictory.
    Config,
    /// A prompt stream supplied by the application yielded an error, which
    /// is the [`source`](std::error::Error::source).
    PromptStream,
    /// A session transcript was not found.
    SessionNotFound,
    /// A filesystem operation on a transcript or a temporary directory
    /// failed; the [`source`](std::error::Error::source) is the
    /// [`std::io::Error`].
    Io,
    /// A session store failed or took too long; the
    /// [`source`](std::error::Error::source) is the error of the store, when
    /// it returned one.
    SessionStore,
    /// A session store does not implement an optional method the operation
    /// needs. [`Error::unsupported_operation`] names it.
    StoreUnsupported,
}

/// Every failure this crate reports.
///
/// Branch on [`kind`](Self::kind) and read the data of a cause through the
/// accessors:
///
/// ```
/// use claude_agent_rust_sdk::{Error, ErrorKind};
///
/// fn handle(error: &Error) -> String {
///     match error.kind() {
///         ErrorKind::ConnectionClosed => "reconnect".to_owned(),
///         ErrorKind::Process => format!("exit code {:?}", error.exit_code()),
///         _ if error.is_retryable() => "retry".to_owned(),
///         _ => error.to_string(),
///     }
/// }
///
/// let error = Error::process("Command failed with exit code 2", Some(2), None);
/// assert_eq!(handle(&error), "exit code Some(2)");
/// ```
///
/// The predicates keep the meaning of the Python exception hierarchy:
///
/// ```python
/// try:
///     ...
/// except ProcessError as error:        # also catches ResultError
///     report(error.exit_code)
/// except CLIConnectionError:           # also catches CLINotFoundError
///     reconnect()
/// ```
///
/// ```
/// use claude_agent_rust_sdk::{Error, ErrorKind};
///
/// fn handle(error: &Error) -> &'static str {
///     if error.is_process() {
///         "process failed"
///     } else if error.is_cli_connection() {
///         "reconnect"
///     } else {
///         "other"
///     }
/// }
///
/// assert_eq!(handle(&Error::result_error(serde_json::json!({}), Some(1))), "process failed");
/// assert_eq!(handle(&Error::cli_not_found(None, None)), "reconnect");
/// assert_eq!(handle(&Error::new(ErrorKind::Config, "bad option")), "other");
/// ```
#[derive(Clone)]
pub struct Error(Arc<Inner>);

struct Inner {
    kind: ErrorKind,
    message: String,
    source: Option<SharedSource>,
    detail: Detail,
}

#[derive(Default)]
struct Detail {
    exit_code: Option<i32>,
    stderr: Option<String>,
    result: Option<ResultErrorData>,
    line: Option<String>,
    data: Option<Value>,
    control_subtype: Option<String>,
    timed_out: bool,
    unsupported: Option<UnsupportedStoreOperation>,
}

const JSON_DECODE_PREVIEW_CHARS: usize = 100;
const DEFAULT_CLI_NOT_FOUND_MESSAGE: &str = "Claude Code not found";
const ERROR_RESULT_PREFIX: &str = "Claude Code returned an error result: ";
const RETRYABLE_HTTP_STATUSES: [i64; 2] = [429, 529];

impl Error {
    /// Builds an error of a kind with its text, like
    /// `std::io::Error::new`. A custom
    /// [`Transport`](crate::Transport) or
    /// [`SessionStore`](crate::SessionStore) uses it for the failures it
    /// reports.
    #[must_use]
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self::with_detail(kind, message.into(), None, Detail::default())
    }

    /// Returns the error with `source` as its
    /// [`source`](std::error::Error::source).
    #[must_use]
    pub fn with_source(self, source: impl Into<BoxError>) -> Self {
        let source: SharedSource = Arc::from(source.into());
        match Arc::try_unwrap(self.0) {
            Ok(mut inner) => {
                inner.source = Some(source);
                Self(Arc::new(inner))
            }
            Err(shared) => Self(Arc::new(Inner {
                kind: shared.kind,
                message: shared.message.clone(),
                source: Some(source),
                detail: Detail {
                    exit_code: shared.detail.exit_code,
                    stderr: shared.detail.stderr.clone(),
                    result: shared.detail.result.clone(),
                    line: shared.detail.line.clone(),
                    data: shared.detail.data.clone(),
                    control_subtype: shared.detail.control_subtype.clone(),
                    timed_out: shared.detail.timed_out,
                    unsupported: shared.detail.unsupported,
                },
            })),
        }
    }

    fn with_detail(
        kind: ErrorKind,
        message: String,
        source: Option<SharedSource>,
        detail: Detail,
    ) -> Self {
        Self(Arc::new(Inner {
            kind,
            message,
            source,
            detail,
        }))
    }

    /// Builds an [`ErrorKind::CliNotFound`] error, appending the searched
    /// path when given.
    #[must_use]
    pub fn cli_not_found(message: Option<&str>, cli_path: Option<&str>) -> Self {
        let message = message.unwrap_or(DEFAULT_CLI_NOT_FOUND_MESSAGE);
        let message = match cli_path {
            Some(path) if !path.is_empty() => format!("{message}: {path}"),
            _ => message.to_owned(),
        };
        Self::new(ErrorKind::CliNotFound, message)
    }

    /// Builds an [`ErrorKind::Process`] error. The text appends the exit
    /// code and the error output to `message`.
    #[must_use]
    pub fn process(
        message: impl Into<String>,
        exit_code: Option<i32>,
        stderr: Option<String>,
    ) -> Self {
        let message = process_text(&message.into(), exit_code, stderr.as_deref());
        Self::with_detail(
            ErrorKind::Process,
            message,
            None,
            Detail {
                exit_code,
                stderr,
                ..Detail::default()
            },
        )
    }

    /// Builds an [`ErrorKind::ResultError`] error from the raw `result`
    /// payload.
    ///
    /// The text is the upstream error-result prefix followed by
    /// [`ResultErrorData::summary`] and the exit code.
    #[must_use]
    pub fn result_error(data: Value, exit_code: Option<i32>) -> Self {
        let data = ResultErrorData::from(data);
        let message = process_text(
            &format!("{ERROR_RESULT_PREFIX}{}", data.summary()),
            exit_code,
            None,
        );
        Self::with_detail(
            ErrorKind::ResultError,
            message,
            None,
            Detail {
                exit_code,
                result: Some(data),
                ..Detail::default()
            },
        )
    }

    /// Builds an [`ErrorKind::JsonDecode`] error for a line of output.
    #[must_use]
    pub fn json_decode(line: impl Into<String>, source: serde_json::Error) -> Self {
        let line = line.into();
        let message = format!(
            "Failed to decode JSON: {}...",
            leading_chars(&line, JSON_DECODE_PREVIEW_CHARS)
        );
        Self::with_detail(
            ErrorKind::JsonDecode,
            message,
            Some(Arc::new(source)),
            Detail {
                line: Some(line),
                ..Detail::default()
            },
        )
    }

    /// Builds an [`ErrorKind::MessageParse`] error, with the message that
    /// failed to parse when available.
    #[must_use]
    pub fn message_parse(message: impl Into<String>, data: Option<Value>) -> Self {
        Self::with_detail(
            ErrorKind::MessageParse,
            message.into(),
            None,
            Detail {
                data,
                ..Detail::default()
            },
        )
    }

    /// Builds an [`ErrorKind::ControlTimeout`] error for a request subtype.
    #[must_use]
    pub fn control_timeout(subtype: impl Into<String>) -> Self {
        let subtype = subtype.into();
        Self::with_detail(
            ErrorKind::ControlTimeout,
            format!("Control request timeout: {subtype}"),
            None,
            Detail {
                control_subtype: Some(subtype),
                ..Detail::default()
            },
        )
    }

    /// Builds an [`ErrorKind::Io`] error: what was being done, and the
    /// error, which becomes the source.
    #[must_use]
    pub fn io(message: impl Into<String>, source: std::io::Error) -> Self {
        Self::with_detail(
            ErrorKind::Io,
            message.into(),
            Some(Arc::new(source)),
            Detail::default(),
        )
    }

    /// Builds an [`ErrorKind::SessionStore`] error from the error a store
    /// returned, which becomes the source.
    #[must_use]
    pub fn session_store(context: impl Into<String>, source: BoxError) -> Self {
        Self::with_detail(
            ErrorKind::SessionStore,
            context.into(),
            Some(Arc::from(source)),
            Detail::default(),
        )
    }

    /// Builds an [`ErrorKind::SessionStore`] error for a store call that
    /// took too long. It is retryable.
    #[must_use]
    pub fn session_store_timeout(message: impl Into<String>) -> Self {
        Self::with_detail(
            ErrorKind::SessionStore,
            message.into(),
            None,
            Detail {
                timed_out: true,
                ..Detail::default()
            },
        )
    }

    pub(crate) fn config(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Config, message)
    }

    pub(crate) fn control(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Control, message)
    }

    pub(crate) fn session_not_found(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::SessionNotFound, message)
    }

    pub(crate) fn not_connected(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::NotConnected, message)
    }

    pub(crate) fn connection_closed(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::ConnectionClosed, message)
    }

    pub(crate) fn spawn(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Spawn, message)
    }

    pub(crate) fn unsupported(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Unsupported, message)
    }

    /// What went wrong.
    #[must_use]
    pub fn kind(&self) -> ErrorKind {
        self.0.kind
    }

    /// Whether trying again, with the same call or a new session, can
    /// succeed without a change of code or options.
    ///
    /// | Kind | Retryable |
    /// |---|---|
    /// | [`ControlTimeout`](ErrorKind::ControlTimeout), [`ConnectionClosed`](ErrorKind::ConnectionClosed) | always |
    /// | [`ResultError`](ErrorKind::ResultError) | when the failing API call answered 429, 529, or a 5xx status |
    /// | [`Process`](ErrorKind::Process) | when the process ended without an exit code (killed by a signal) |
    /// | [`Io`](ErrorKind::Io), [`Spawn`](ErrorKind::Spawn) | when the I/O error is `Interrupted`, `TimedOut`, or `WouldBlock` |
    /// | [`SessionStore`](ErrorKind::SessionStore) | when the store took too long |
    /// | every other kind | never |
    #[must_use]
    pub fn is_retryable(&self) -> bool {
        match self.0.kind {
            ErrorKind::ControlTimeout | ErrorKind::ConnectionClosed => true,
            ErrorKind::ResultError => self
                .result()
                .and_then(ResultErrorData::api_error_status)
                .is_some_and(|status| {
                    RETRYABLE_HTTP_STATUSES.contains(&status) || (500..600).contains(&status)
                }),
            ErrorKind::Process => self.0.detail.exit_code.is_none(),
            ErrorKind::Io | ErrorKind::Spawn => self.io_kind().is_some_and(|kind| {
                matches!(
                    kind,
                    std::io::ErrorKind::Interrupted
                        | std::io::ErrorKind::TimedOut
                        | std::io::ErrorKind::WouldBlock
                )
            }),
            ErrorKind::SessionStore => self.0.detail.timed_out,
            _ => false,
        }
    }

    /// Whether the CLI process failed: true for [`ErrorKind::Process`] and
    /// [`ErrorKind::ResultError`].
    ///
    /// This is the Rust form of `except ProcessError`, which in Python also
    /// catches `ResultError`.
    #[must_use]
    pub fn is_process(&self) -> bool {
        matches!(self.0.kind, ErrorKind::Process | ErrorKind::ResultError)
    }

    /// Whether the CLI could not be reached: true for
    /// [`ErrorKind::CliNotFound`], [`ErrorKind::Spawn`],
    /// [`ErrorKind::NotConnected`], [`ErrorKind::ConnectionClosed`], and
    /// [`ErrorKind::Unsupported`].
    ///
    /// This is the Rust form of `except CLIConnectionError`, which in
    /// Python also catches `CLINotFoundError`.
    #[must_use]
    pub fn is_cli_connection(&self) -> bool {
        matches!(
            self.0.kind,
            ErrorKind::CliNotFound
                | ErrorKind::Spawn
                | ErrorKind::NotConnected
                | ErrorKind::ConnectionClosed
                | ErrorKind::Unsupported
        )
    }

    /// Exit code of the CLI process, for [`ErrorKind::Process`] and
    /// [`ErrorKind::ResultError`].
    #[must_use]
    pub fn exit_code(&self) -> Option<i32> {
        self.0.detail.exit_code
    }

    /// The last lines the CLI wrote to its error output, for
    /// [`ErrorKind::Process`], when they were captured.
    #[must_use]
    pub fn stderr(&self) -> Option<&str> {
        self.0.detail.stderr.as_deref()
    }

    /// Payload of the error result, for [`ErrorKind::ResultError`].
    #[must_use]
    pub fn result(&self) -> Option<&ResultErrorData> {
        self.0.detail.result.as_ref()
    }

    /// The line that did not parse, in full, for [`ErrorKind::JsonDecode`].
    #[must_use]
    pub fn line(&self) -> Option<&str> {
        self.0.detail.line.as_deref()
    }

    /// The message that failed to parse, for [`ErrorKind::MessageParse`],
    /// when available.
    #[must_use]
    pub fn data(&self) -> Option<&Value> {
        self.0.detail.data.as_ref()
    }

    /// Subtype of the request that timed out, for
    /// [`ErrorKind::ControlTimeout`].
    #[must_use]
    pub fn control_subtype(&self) -> Option<&str> {
        self.0.detail.control_subtype.as_deref()
    }

    /// The store method that is not implemented, for
    /// [`ErrorKind::StoreUnsupported`].
    #[must_use]
    pub fn unsupported_operation(&self) -> Option<UnsupportedStoreOperation> {
        self.0.detail.unsupported
    }

    /// Kind of the underlying I/O error, when the source is one.
    #[must_use]
    pub fn io_kind(&self) -> Option<std::io::ErrorKind> {
        self.0
            .source
            .as_deref()
            .and_then(|source| source.downcast_ref::<std::io::Error>())
            .map(std::io::Error::kind)
    }
}

impl From<UnsupportedStoreOperation> for Error {
    fn from(unsupported: UnsupportedStoreOperation) -> Self {
        Self::with_detail(
            ErrorKind::StoreUnsupported,
            unsupported.to_string(),
            None,
            Detail {
                unsupported: Some(unsupported),
                ..Detail::default()
            },
        )
    }
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0.message)
    }
}

impl fmt::Debug for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let detail = &self.0.detail;
        let mut debug = formatter.debug_struct("Error");
        debug
            .field("kind", &self.0.kind)
            .field("message", &self.0.message);
        if let Some(code) = detail.exit_code {
            debug.field("exit_code", &code);
        }
        if let Some(stderr) = &detail.stderr {
            debug.field("stderr", stderr);
        }
        if let Some(result) = &detail.result {
            debug.field("result", result);
        }
        if let Some(subtype) = &detail.control_subtype {
            debug.field("control_subtype", subtype);
        }
        if let Some(source) = &self.0.source {
            debug.field("source", source);
        }
        debug.finish_non_exhaustive()
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.0
            .source
            .as_deref()
            .map(|source| source as &(dyn std::error::Error + 'static))
    }
}

/// Raw `result` payload carried by [`ErrorKind::ResultError`].
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

fn process_text(message: &str, exit_code: Option<i32>, stderr: Option<&str>) -> String {
    let mut text = message.to_owned();
    if let Some(code) = exit_code {
        text.push_str(&format!(" (exit code: {code})"));
    }
    if let Some(stderr) = stderr
        && !stderr.is_empty()
    {
        text.push_str(&format!("\nError output: {stderr}"));
    }
    text
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{Error, ErrorKind};
    use crate::session_store::UnsupportedStoreOperation;

    /// One value of every kind. The match in [`expected`] has no wildcard,
    /// so a new kind does not compile until it is listed there with a
    /// decision, and [`one_of_each`] must then build it for the test to
    /// keep its count.
    fn one_of_each() -> Vec<Error> {
        vec![
            Error::cli_not_found(None, None),
            Error::spawn("cannot start"),
            Error::not_connected("not connected"),
            Error::connection_closed("closed"),
            Error::unsupported("no on_failure"),
            Error::process("failed", Some(1), None),
            Error::result_error(json!({"subtype": "error_max_turns"}), Some(1)),
            Error::json_decode(
                "{",
                serde_json::from_str::<serde_json::Value>("{").unwrap_err(),
            ),
            Error::message_parse("bad", None),
            Error::control_timeout("initialize"),
            Error::control("refused"),
            Error::config("bad option"),
            Error::new(ErrorKind::PromptStream, "the stream failed"),
            Error::session_not_found("missing"),
            Error::io("write", std::io::Error::other("disk")),
            Error::session_store("append", "down".into()),
            Error::from(UnsupportedStoreOperation { operation: "list" }),
        ]
    }

    const KINDS: usize = 17;

    /// `(is_process, is_cli_connection)` as the Python hierarchy decides it.
    fn expected(kind: ErrorKind) -> (bool, bool) {
        match kind {
            ErrorKind::Process | ErrorKind::ResultError => (true, false),
            ErrorKind::CliNotFound
            | ErrorKind::Spawn
            | ErrorKind::NotConnected
            | ErrorKind::ConnectionClosed
            | ErrorKind::Unsupported => (false, true),
            ErrorKind::JsonDecode
            | ErrorKind::MessageParse
            | ErrorKind::ControlTimeout
            | ErrorKind::Control
            | ErrorKind::Config
            | ErrorKind::PromptStream
            | ErrorKind::SessionNotFound
            | ErrorKind::Io
            | ErrorKind::SessionStore
            | ErrorKind::StoreUnsupported => (false, false),
        }
    }

    #[test]
    fn predicates_follow_the_python_exception_hierarchy_for_every_kind() {
        let errors = one_of_each();
        let distinct: std::collections::HashSet<_> = errors.iter().map(Error::kind).collect();
        assert_eq!(distinct.len(), KINDS);
        assert_eq!(errors.len(), KINDS);
        for error in &errors {
            assert_eq!(
                (error.is_process(), error.is_cli_connection()),
                expected(error.kind()),
                "{error:?}"
            );
        }
        let processes = errors.iter().filter(|error| error.is_process()).count();
        let connections = errors
            .iter()
            .filter(|error| error.is_cli_connection())
            .count();
        assert_eq!((processes, connections), (2, 5));
    }

    #[test]
    fn retryable_errors_are_the_documented_ones() {
        let retryable: Vec<ErrorKind> = one_of_each()
            .iter()
            .filter(|error| error.is_retryable())
            .map(Error::kind)
            .collect();
        assert_eq!(
            retryable,
            [ErrorKind::ConnectionClosed, ErrorKind::ControlTimeout]
        );
        for status in [429, 500, 503, 529] {
            let error = Error::result_error(json!({"api_error_status": status}), Some(1));
            assert!(error.is_retryable(), "{status}");
        }
        for status in [400, 401, 404] {
            let error = Error::result_error(json!({"api_error_status": status}), Some(1));
            assert!(!error.is_retryable(), "{status}");
        }
        assert!(Error::process("killed", None, None).is_retryable());
        let interrupted = std::io::Error::from(std::io::ErrorKind::Interrupted);
        assert!(Error::io("read", interrupted).is_retryable());
        let missing = std::io::Error::from(std::io::ErrorKind::NotFound);
        assert!(!Error::io("read", missing).is_retryable());
        assert!(Error::session_store_timeout("load timed out").is_retryable());
    }

    #[test]
    fn the_text_does_not_repeat_the_source_and_the_source_keeps_its_type() {
        let error = Error::io("Cannot write /tmp/x", std::io::Error::other("disk full"));
        assert_eq!(error.to_string(), "Cannot write /tmp/x");
        let source = std::error::Error::source(&error).unwrap();
        assert_eq!(source.to_string(), "disk full");
        assert!(source.downcast_ref::<std::io::Error>().is_some());
        assert_eq!(error.io_kind(), Some(std::io::ErrorKind::Other));

        let store = Error::session_store("Session store append failed", "backend down".into());
        assert_eq!(store.to_string(), "Session store append failed");
        assert_eq!(
            std::error::Error::source(&store).unwrap().to_string(),
            "backend down"
        );
    }

    #[test]
    fn a_clone_shares_the_data_and_the_source() {
        let error = Error::process(
            "Command failed with exit code 3",
            Some(3),
            Some("boom".to_owned()),
        );
        let copy = error.clone();
        assert_eq!(copy.kind(), ErrorKind::Process);
        assert_eq!(copy.exit_code(), Some(3));
        assert_eq!(copy.stderr(), Some("boom"));
        assert_eq!(
            copy.to_string(),
            "Command failed with exit code 3 (exit code: 3)\nError output: boom"
        );
        let wrapped = copy.with_source(std::io::Error::other("pipe"));
        assert_eq!(wrapped.exit_code(), Some(3));
        assert!(std::error::Error::source(&wrapped).is_some());
        assert!(std::error::Error::source(&error).is_none());
    }

    #[test]
    fn accessors_return_the_data_of_their_kind_only() {
        let decode = Error::json_decode(
            "{\"a\": ",
            serde_json::from_str::<serde_json::Value>("{").unwrap_err(),
        );
        assert_eq!(decode.line(), Some("{\"a\": "));
        assert_eq!(decode.exit_code(), None);
        let timeout = Error::control_timeout("interrupt");
        assert_eq!(timeout.control_subtype(), Some("interrupt"));
        assert_eq!(timeout.to_string(), "Control request timeout: interrupt");
        let parse = Error::message_parse("bad", Some(json!({"type": "x"})));
        assert_eq!(parse.data(), Some(&json!({"type": "x"})));
        let result = Error::result_error(json!({"subtype": "error_max_turns"}), Some(1));
        assert_eq!(
            result.result().and_then(|data| data.subtype()),
            Some("error_max_turns")
        );
        let unsupported = Error::from(UnsupportedStoreOperation {
            operation: "delete",
        });
        assert_eq!(
            unsupported.unsupported_operation().map(|op| op.operation),
            Some("delete")
        );
    }
}
