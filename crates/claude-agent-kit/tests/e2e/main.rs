//! End-to-end suite: the SDK against the real Claude Code CLI and the real
//! API.
//!
//! The tests that call the API are ignored by default, so a plain
//! `cargo test` never spends credits. Run them with:
//!
//! ```text
//! cargo xtask e2e
//! ```
//!
//! which is `cargo test -p claude-agent-kit --all-features --test e2e --
//! --ignored --test-threads=4`: four sessions at a time. Extra arguments go
//! to the test binary, so `cargo xtask e2e hooks::` runs one topic and
//! `cargo xtask e2e --test-threads=1` runs the sessions one by one.
//!
//! A few tests need more than a logged-in CLI. `session_store_resume_settings`
//! runs the CLI under an empty configuration directory, so it needs
//! credentials from the environment (`ANTHROPIC_API_KEY` or
//! `CLAUDE_CODE_OAUTH_TOKEN`) and fails, saying so, without them.
//!
//! The suite needs credentials. It reads `ANTHROPIC_API_KEY`, and fails,
//! naming that variable, when it is missing. A developer whose installed
//! CLI is already logged in sets `CLAUDE_AGENT_KIT_E2E_AUTH=cli` instead.
//!
//! Every session runs with `ANTHROPIC_DEFAULT_MODEL` pinned, and with
//! `ANTHROPIC_MODEL` set to the same model so a `model` in the settings of
//! a developer machine cannot replace it. That way a CLI
//! release that changes its default model does not change what the suite
//! tests. A value already set in the environment is kept.
//!
//! There is one module per topic of the upstream `e2e-tests/` directory,
//! plus `handshake` for the two checks specific to this crate (the
//! `sdk-rs` entrypoint and the native MCP dispatcher) and `harness` for
//! the rules of the suite itself, which run without the API.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "../common/mod.rs"]
mod common;

mod agents_and_settings;
mod conversation_reset;
mod dynamic_control;
mod error_results;
mod forward_subagent_text;
mod handshake;
mod harness;
mod hook_events;
mod hooks;
mod include_partial_messages;
mod message_origin;
mod parallel_tool_session_reads;
mod run_end;
mod sdk_mcp_tools;
mod session_store_resume_settings;
mod stderr_callback;
mod structured_output;
mod subagent_session_reads;
mod suite;
mod tool_permissions;
mod truncating_resume;
mod verbatim_prompts;
