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
//! and answer the CLI when it invokes them.
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
pub mod message_parser;
mod process;
mod prompt;
mod query;
pub mod session_store;
#[cfg(any(test, feature = "testing"))]
pub mod testing;
pub mod transport;
pub mod types;

pub use client::{ClaudeSdkClient, Messages};
pub use error::{BoxError, Error, Result, ResultErrorData};
pub use message_parser::parse_message;
pub use prompt::{InputMessage, InputStream, Prompt};
pub use query::{QueryStream, query, query_with_transport};
pub use session_store::{
    SessionKey, SessionListSubkeysKey, SessionStore, SessionStoreCapabilities, SessionStoreEntry,
    SessionStoreError, SessionStoreFlushMode, SessionStoreListEntry, SessionSummaryEntry,
    SharedSessionStore, UnsupportedStoreOperation,
};
pub use transport::{MessageStream, SharedTransport, SubprocessTransport, Transport};
pub use types::*;

/// Version of this crate, reported to the CLI.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Version of the CLI this release is developed and tested against.
pub const CLI_VERSION: &str = "2.1.325";

/// Oldest CLI version the SDK supports. An older CLI still runs, with a
/// logged warning.
pub const MINIMUM_CLI_VERSION: &str = "2.0.0";

/// Version of the upstream Python SDK this release is at parity with.
pub const UPSTREAM_SDK_VERSION: &str = "0.2.182";
