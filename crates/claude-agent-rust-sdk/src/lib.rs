//! Rust SDK for building agents on top of the Claude Code CLI.
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
//! `jsonschema` and `preserve-order` are on by default; the others are off:
//!
//! - `jsonschema`: on by default. Validation of tool arguments against the
//!   JSON Schema of the tool, for every [`SdkMcpTool`], as the Python SDK
//!   always does. A call whose arguments break the schema gets an error
//!   result and the handler does not run. `format` is not checked, and
//!   remote `$ref` targets are never fetched. Turn it off with
//!   `default-features = false`: the arguments of a raw tool then reach the
//!   handler as the model sent them, and those of a typed tool are still
//!   checked by `serde`.
//! - `preserve-order`: on by default. Turns on `serde_json/preserve_order`,
//!   so a JSON object keeps the order its keys were written in, as a Python
//!   dictionary does: the properties of a tool schema reach the model in
//!   the order the program wrote them, and a transcript the crate copies
//!   (a fork, a resume from a store) keeps the key order the CLI wrote.
//!   Cargo turns a feature on for the whole program, so with it every
//!   `serde_json::Map` of the program keeps insertion order instead of
//!   sorting its keys. Turn it off with `default-features = false` (and
//!   `features = ["jsonschema"]` to keep validation) when another part of
//!   the program needs sorted maps: the crate works the same, except that
//!   the keys of the JSON it writes to the CLI and to transcripts come out
//!   sorted.
//! - `macros`: the `#[tool]` attribute, which turns an async function into
//!   a tool.
//! - `rmcp`: `McpServerConfig::sdk_rmcp`, which serves a server built with
//!   the `rmcp` crate inside this process.
//! - `otel`: OpenTelemetry trace propagation. When a span is active as the
//!   CLI starts, its context is handed to the CLI through the `TRACEPARENT`
//!   and `TRACESTATE` environment variables (and `BAGGAGE`), so the spans
//!   of the CLI join the trace of the caller. The global propagator of the
//!   application writes them; when none is installed, the W3C trace
//!   context and baggage are written, as the Python SDK does by default,
//!   following `OTEL_PROPAGATORS` (`tracecontext`, `baggage`, `none`).
//!   The active context is `opentelemetry::Context::current()`; a `tracing`
//!   span becomes it through `tracing-opentelemetry`, whose layer attaches
//!   the context of a span while it is entered (its default), so a
//!   `connect` instrumented with that span propagates it. Variables set in
//!   the `env` option are never overwritten. Callbacks,
//!   hooks, and in-process MCP tools run in the OpenTelemetry context that
//!   was current when the session connected.
//! - `testing`: the `testing` module, with a scriptable transport for tests.
//!   It is outside the compatibility promise: its items may change in a
//!   patch release.
//!
//! # Public dependencies
//!
//! Six crates appear in the public API. [`serde`] (`Serialize` and
//! `Deserialize` on the types) and [`serde_json`] (`Value` and `Map` for
//! the JSON the CLI writes) are the common vocabulary of Rust, at 1.x. The
//! other four are re-exported so a program uses the version this crate was
//! built with: [`futures_core`] (the `Stream` that `query`, the client, and
//! a [`Transport`] yield), [`indexmap`] (`IndexMap` keeps the order of
//! options such as `env` and `extra_args`), [`schemars`] (the `JsonSchema`
//! bound of a typed tool), and, with the `rmcp` feature, `rmcp` (the
//! `ServerHandler` that `sdk_rmcp` serves). A major release of any of the
//! six is a major release of this crate. A typed tool can derive its
//! schema without a direct dependency:
//!
//! ```
//! use claude_agent_rust_sdk::schemars::{self, JsonSchema};
//!
//! #[derive(serde::Deserialize, JsonSchema)]
//! #[schemars(crate = "claude_agent_rust_sdk::schemars")]
//! struct Divide {
//!     a: f64,
//!     b: f64,
//! }
//! # let _ = schemars::schema_for!(Divide);
//! ```
//!
//! With the default `jsonschema` feature the build compiles about twice as
//! many crates (109 against 57 without default features); turning it off
//! keeps the `serde` check of typed tools.
//!
//! # Platform support
//!
//! Linux, macOS, and Windows are supported and tested on every change.
//! tokio is the only async runtime supported. The CLI runs as the root of
//! its own process tree, which the crate ends as a whole: through a process
//! group on Unix and a Job Object on Windows. A server the agent starts in
//! the background (`nohup server &`) is part of that tree and ends with the
//! session, unless it leaves the group (`setsid` on Unix): on Unix it gets
//! `SIGTERM` when the CLI exits, and `SIGKILL` 5 s later if it is still
//! running; on Windows it is terminated at once. When the program
//! returns from `main` or calls `std::process::exit` with a session still
//! open, the process group of the CLI gets `SIGTERM` on Unix from an exit
//! hook, as from the `atexit` hook of the Python SDK, so the CLI writes the
//! end of its transcript before it exits.
//!
//! The tasks a session spawns run in the `tracing` span that is current
//! when the session connects, so what a permission callback, a hook, an
//! in-process MCP tool, or a session store logs lands under the span of
//! the caller, as Python callbacks see the `contextvars` of the caller.
//!
//! # Claude Code CLI
//!
//! The crate does not bundle the CLI: it runs the `claude` executable
//! already installed on the machine (see [`cli::find_cli`]), with the
//! credentials of the person running the program. It is tested against CLI
//! 2.1.329 ([`CLI_VERSION`]), warns when the installed CLI reports another
//! version, and warns that it may not work when it is older than
//! 2.0.0 ([`MINIMUM_CLI_VERSION`]).
//!
//! # Example
//!
//! ```no_run
//! use claude_agent_rust_sdk::{ClaudeAgentOptions, Message, query};
//! use futures_util::StreamExt;
//!
//! # async fn demo() -> Result<(), claude_agent_rust_sdk::Error> {
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
//! use claude_agent_rust_sdk::{ClaudeAgentOptions, Message, PermissionMode, parse_message};
//! use serde_json::json;
//!
//! # fn main() -> Result<(), claude_agent_rust_sdk::Error> {
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

#![cfg_attr(docsrs, feature(doc_cfg))]
#![deny(missing_docs)]
#![deny(unsafe_code)]

mod blocking;
mod caller_context;
pub mod cli;
mod client;
mod control;
pub mod error;
mod json_map;
pub mod mcp;
pub mod message_parser;
mod message_writer;
mod mirror;
mod process;
mod prompt;
mod query;
mod resume;
pub mod session_store;
pub mod sessions;
/// The `tracing` capture helper of the unit tests. The integration tests
/// load the same file, so both assert logged warnings the same way.
#[cfg(test)]
#[allow(
    dead_code,
    reason = "shared with the integration tests, which use helpers the unit tests do not"
)]
mod test_capture;
#[cfg(any(test, feature = "testing"))]
pub mod testing;
pub mod transport;
pub mod types;

pub use futures_core;
pub use indexmap;
#[cfg(feature = "rmcp")]
pub use rmcp;
pub use schemars;

pub use client::{ClaudeSdkClient, Messages};
pub use error::{BoxError, Error, ErrorKind, Result, ResultErrorData};
pub use mcp::{
    ResourceContents, SdkMcpTool, ToolAnnotations, ToolContent, ToolHandler, ToolResult,
    create_sdk_mcp_server,
};
pub use message_parser::parse_message;

/// Turns an async function into a function returning the [`SdkMcpTool`] it
/// defines.
///
/// The function takes the arguments of the tool as its last parameter,
/// whose type gives the input schema as in [`SdkMcpTool::typed`], and
/// returns `Result<ToolResult, E>` where `E` converts into [`BoxError`].
/// The attribute replaces it with a function of the same name and
/// visibility that returns the tool. Call the tool with
/// [`SdkMcpTool::call`] to test it on its own.
///
/// A parameter before the arguments is the state of the tool, which the
/// generated function takes:
///
/// - `state: &S`: the generated function takes an `Arc<S>`, shared by every
///   call (and with the caller, who can keep a clone).
/// - `state: T`: the generated function takes a `T`, cloned for each call.
///
/// Calls may run at the same time, so `&mut` state is refused: keep what
/// changes behind a lock. A method taking `self` is refused too.
///
/// Arguments of the attribute, all optional:
///
/// - `name`: name of the tool. Defaults to the name of the function.
/// - `description`: description shown to the model. Defaults to the doc
///   comment of the function, less the indentation its lines share, so an
///   indented code block or a nested list keeps its shape. One of the two
///   is required.
/// - `crate`: the path of this crate in the generated code, as a string,
///   for a crate that depends on it under another name or re-exports it:
///   `#[tool(crate = "my_sdk")]`. Defaults to `::claude_agent_rust_sdk`.
/// - `title`, `read_only_hint`, `destructive_hint`, `idempotent_hint`,
///   `open_world_hint`, `max_result_size_chars`: the fields of
///   [`ToolAnnotations`].
///
/// ```
/// use claude_agent_rust_sdk::{ToolResult, create_sdk_mcp_server, tool};
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
/// /// Remember a note.
/// #[tool]
/// async fn remember(notes: &std::sync::Mutex<Vec<String>>, args: Note) -> Result<ToolResult, String> {
///     notes.lock().map_err(|_| "notes poisoned")?.push(args.text);
///     Ok(ToolResult::text("noted"))
/// }
///
/// #[derive(Deserialize, JsonSchema)]
/// struct Note {
///     text: String,
/// }
///
/// # #[tokio::main(flavor = "current_thread")]
/// # async fn main() -> Result<(), claude_agent_rust_sdk::Error> {
/// let tool = divide();
/// assert_eq!(tool.name(), "divide");
/// assert_eq!(tool.description(), "Divide one number by another.");
/// let quotient = tool.call(serde_json::json!({"dividend": 6, "divisor": 3})).await;
/// assert_eq!(quotient, ToolResult::text("2"));
///
/// let notes = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
/// let calculator = create_sdk_mcp_server("calculator", None, [tool, remember(notes.clone())])?;
/// # let _ = calculator;
/// # Ok(())
/// # }
/// ```
#[cfg(feature = "macros")]
pub use claude_agent_rust_sdk_macros::tool;
pub use prompt::{InputMessage, InputStream, Prompt};
pub use query::{QueryStream, query, query_with_transport};
pub use session_store::{
    InMemorySessionStore, SessionKey, SessionListSubkeysKey, SessionStore,
    SessionStoreCapabilities, SessionStoreEntry, SessionStoreError, SessionStoreFlushMode,
    SessionStoreListEntry, SessionSummaryEntry, SharedSessionStore, UnsupportedStoreOperation,
    file_path_to_session_key,
};
pub use sessions::{
    ForkSessionOptions, ForkSessionResult, ImportSessionOptions, ListSessionsOptions,
    SdkSessionInfo, SessionMessage, SessionMessageType, SessionMessagesOptions, delete_session,
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
pub const CLI_VERSION: &str = "2.1.329";

/// Oldest CLI version the SDK supports. An older CLI still runs, with a
/// logged warning.
pub const MINIMUM_CLI_VERSION: &str = "2.0.0";

/// Version of the upstream Python SDK this release is ported from.
pub const UPSTREAM_SDK_VERSION: &str = "0.2.186";
