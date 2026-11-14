//! Rust SDK for building agents on top of the Claude CLI.
//!
//! The crate spawns the CLI as a child process and exchanges
//! newline-delimited JSON with it. This release provides the foundation:
//! the error model, the typed data model, the message parser, and the
//! session options.
//!
//! - [`Error`] is the single error type of the crate.
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

pub mod error;
pub mod message_parser;
pub mod session_store;
pub mod types;

pub use error::{BoxError, Error, Result, ResultErrorData};
pub use message_parser::parse_message;
pub use session_store::{
    SessionKey, SessionListSubkeysKey, SessionStore, SessionStoreCapabilities, SessionStoreEntry,
    SessionStoreError, SessionStoreFlushMode, SessionStoreListEntry, SessionSummaryEntry,
    SharedSessionStore, UnsupportedStoreOperation,
};
pub use types::*;

/// Version of this crate, reported to the CLI.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Version of the CLI this release is developed and tested against.
pub const CLI_VERSION: &str = "2.1.325";

/// Version of the upstream Python SDK this release is at parity with.
pub const UPSTREAM_SDK_VERSION: &str = "0.2.182";
