//! Rust SDK for building agents on top of the Claude CLI.
//!
//! The crate spawns the CLI as a child process and exchanges
//! newline-delimited JSON with it. It has two entry points:
//!
//! - [`query()`] runs one prompt and returns the stream of its messages.
//! - [`ClaudeSdkClient`] keeps a session open for a multi-turn conversation
//!   and changes settings mid-session.
//!
//! Both register the hooks and the tool-permission callback of the options
//! and answer the CLI when it invokes them. Both also serve the tools
//! defined in this process: [`create_sdk_mcp_server`] turns a set of
//! [`SdkMcpTool`]s into an MCP server the CLI calls over the control
//! channel, with no separate server process.
//!
//! # Cargo features
//!
//! `jsonschema` is the one feature on by default; the others are off:
//!
//! - `jsonschema`: on by default. Validation of tool arguments against the
//!   JSON Schema of the tool, for tools made with [`SdkMcpTool::raw`], as the
//!   Python SDK always does. A call whose arguments break the schema gets an
//!   error result and the handler does not run. Remote `$ref` targets are
//!   never fetched. Turn it off with `default-features = false`: arguments
//!   then reach the handler as the model sent them.
//! - `macros`: the `#[tool]` attribute, which turns an async function into
//!   a tool.
//! - `rmcp`: `McpServerConfig::sdk_rmcp`, which serves a server built with
//!   the `rmcp` crate inside this process.
//! - `otel`: OpenTelemetry trace propagation. When a span is active as the
//!   CLI starts, its context is handed to the CLI through the `TRACEPARENT`
//!   and `TRACESTATE` environment variables, so the spans of the CLI join
//!   the trace of the caller. Variables set in the `env` option are never
//!   overwritten.
//! - `testing`: the `testing` module, with a scriptable transport for tests.
//!
//! # Platform support
//!
//! Linux, macOS, and Windows are supported and tested on every change.
//! tokio is the only async runtime supported. The CLI runs as the root of
//! its own process tree, which the crate ends as a whole: through a process
//! group on Unix and a Job Object on Windows.
//!
//! # Claude Code CLI
//!
//! The crate does not bundle the CLI: it runs the `claude` executable
//! already installed on the machine (see [`cli::find_cli`]), with the
//! credentials of the person running the program. It is tested against CLI
//! 2.1.325 ([`CLI_VERSION`]), and warns when the installed CLI is older than
//! 2.0.0 ([`MINIMUM_CLI_VERSION`]).
//!
//! # Example
//!
//! ```no_run
//! use claude_agent_kit::{ClaudeAgentOptions, Message, query};
//! use futures_util::StreamExt;
//!
//! # async fn demo() -> Result<(), claude_agent_kit::Error> {
//! let mut messages = query("What is 2 + 2?", ClaudeAgentOptions::default());
//! while let Some(message) = messages.next().await {
//!     if let Message::Assistant(assistant) = message? {
//!         println!("{:?}", assistant.content);
//!     }
//! }
//! # Ok(())
//! # }
//! ```
//!
//! The building blocks are public too:
//!
//! - [`Error`] is the single error type of the crate.
//! - [`SubprocessTransport`] spawns the CLI, frames its output into JSON
//!   messages, and ends the process and its descendants within a bounded
//!   time. [`Transport`] is the interface a custom transport implements.
//! - [`cli::find_cli`] locates the CLI executable.
//! - [`parse_message`] turns one JSON message of CLI output into a typed
//!   [`Message`]. Messages and content blocks unknown to this version of
//!   the crate are kept as raw JSON instead of being dropped.
//! - [`ClaudeAgentOptions`] configures a session and is built with
//!   [`ClaudeAgentOptions::builder`].
//!
//! ```
//! use claude_agent_kit::{ClaudeAgentOptions, Message, PermissionMode, parse_message};
//! use serde_json::json;
//!
//! # fn main() -> Result<(), claude_agent_kit::Error> {
//! let options = ClaudeAgentOptions::builder()
//!     .model("claude-sonnet-5")
//!     .permission_mode(PermissionMode::AcceptEdits)
//!     .allowed_tools(["Read", "Grep"])
//!     .build()?;
//! assert_eq!(options.allowed_tools, ["Read", "Grep"]);
//!
//! let message = parse_message(json!({
//!     "type": "assistant",
//!     "message": {
//!         "model": "claude-sonnet-5",
//!         "content": [{"type": "text", "text": "Hello"}],
//!     },
//! }))?;
//! if let Message::Assistant(assistant) = message {
//!     assert_eq!(assistant.content[0].as_text(), Some("Hello"));
//! }
//! # Ok(())
//! # }
//! ```

#![deny(missing_docs)]
#![deny(unsafe_code)]

pub mod cli;
mod client;
mod control;
pub mod error;
pub mod mcp;
pub mod message_parser;
mod mirror;
mod process;
mod prompt;
mod query;
mod resume;
pub mod session_store;
pub mod sessions;
/// The `tracing` capture helper of the integration tests, shared with the
/// unit tests so both assert logged warnings the same way.
#[cfg(test)]
#[allow(dead_code)]
#[path = "../tests/common/capture.rs"]
mod test_capture;
#[cfg(any(test, feature = "testing"))]
pub mod testing;
pub mod transport;
pub mod types;

pub use client::{ClaudeSdkClient, Messages};
pub use error::{BoxError, Error, Result, ResultErrorData};
pub use mcp::{
    ResourceContents, SdkMcpTool, ToolAnnotations, ToolContent, ToolHandler, ToolResult,
    create_sdk_mcp_server,
};
pub use message_parser::parse_message;

/// Turns an async function into a function returning the [`SdkMcpTool`] it
/// defines.
///
/// The function takes exactly one argument, whose type gives the input
/// schema of the tool as in [`SdkMcpTool::typed`], and returns
/// `Result<ToolResult, E>` where `E` converts into [`BoxError`]. The
/// attribute replaces it with a function of the same name and visibility
/// that takes no argument and returns the tool.
///
/// Arguments of the attribute, all optional:
///
/// - `name`: name of the tool. Defaults to the name of the function.
/// - `description`: description shown to the model. Defaults to the doc
///   comment of the function. One of the two is required.
/// - `title`, `read_only_hint`, `destructive_hint`, `idempotent_hint`,
///   `open_world_hint`, `max_result_size_chars`: the fields of
///   [`ToolAnnotations`].
///
/// ```
/// use claude_agent_kit::{ToolResult, create_sdk_mcp_server, tool};
/// use schemars::JsonSchema;
/// use serde::Deserialize;
///
/// #[derive(Deserialize, JsonSchema)]
/// struct Divide {
///     dividend: f64,
///     divisor: f64,
/// }
///
/// /// Divide one number by another.
/// #[tool(read_only_hint = true)]
/// async fn divide(args: Divide) -> Result<ToolResult, String> {
///     if args.divisor == 0.0 {
///         return Err("Division by zero".to_owned());
///     }
///     Ok(ToolResult::text((args.dividend / args.divisor).to_string()))
/// }
///
/// # fn main() -> Result<(), claude_agent_kit::Error> {
/// let tool = divide();
/// assert_eq!(tool.name(), "divide");
/// assert_eq!(tool.description(), "Divide one number by another.");
/// let calculator = create_sdk_mcp_server("calculator", None, [tool])?;
/// # let _ = calculator;
/// # Ok(())
/// # }
/// ```
#[cfg(feature = "macros")]
pub use claude_agent_kit_macros::tool;
pub use prompt::{InputMessage, InputStream, Prompt};
pub use query::{QueryStream, query, query_with_transport};
pub use session_store::{
    InMemorySessionStore, SessionKey, SessionListSubkeysKey, SessionStore,
    SessionStoreCapabilities, SessionStoreEntry, SessionStoreError, SessionStoreFlushMode,
    SessionStoreListEntry, SessionSummaryEntry, SharedSessionStore, UnsupportedStoreOperation,
    file_path_to_session_key,
};
pub use sessions::{
    ForkSessionResult, SdkSessionInfo, SessionMessage, SessionMessageType, delete_session,
    delete_session_via_store, fold_session_summary, fork_session, fork_session_via_store,
    get_session_info, get_session_info_from_store, get_session_messages,
    get_session_messages_from_store, get_subagent_messages, get_subagent_messages_from_store,
    import_session_to_store, list_sessions, list_sessions_from_store, list_subagents,
    list_subagents_from_store, project_key_for_directory, rename_session, rename_session_via_store,
    tag_session, tag_session_via_store,
};
pub use transport::{MessageStream, SharedTransport, SubprocessTransport, Transport};
pub use types::*;

/// The code blocks of the README, compiled as documentation tests.
#[cfg(doctest)]
#[doc = include_str!("../../../README.md")]
struct ReadmeExamples;

/// Version of this crate, reported to the CLI.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Version of the CLI this release is developed and tested against.
pub const CLI_VERSION: &str = "2.1.325";

/// Oldest CLI version the SDK supports. An older CLI still runs, with a
/// logged warning.
pub const MINIMUM_CLI_VERSION: &str = "2.0.0";

/// Version of the upstream Python SDK this release is at parity with.
pub const UPSTREAM_SDK_VERSION: &str = "0.2.182";
