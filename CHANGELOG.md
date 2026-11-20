# Changelog

## Unreleased

### Added

- Every pull request compares the public API with the newest release on crates.io through `cargo-semver-checks`, so a patch release cannot break a build against the previous release.
- A scheduled check of `Cargo.lock` against the RustSec advisory database.

### Changed

- Upstream moved to Python SDK 0.2.186 and CLI 2.1.329.
- **Breaking:** `PermissionUpdate::to_wire` returns a `Result`. It sends the shape the CLI accepts (upstream #1502): a rule with no `rule_content` leaves `ruleContent` out instead of sending `null`, an unset `destination` is sent as `"session"`, and a missing required field or an `Unknown` kind is an `Error::Config`. The CLI used to reject the whole `updatedPermissions` list, so the call was allowed but nothing was remembered.

### Fixed

- JSON keeps its key order end to end, as the Python SDK and the CLI do: the properties of an output schema, the order of MCP servers and subagents, the keys of settings, and every line a fork or a resume from a store writes. The crate now enables the `preserve_order` feature of `serde_json`; since Cargo features add up, a `serde_json::Map` elsewhere in a program that uses this crate also keeps insertion order.
- Dropping the future of `ClaudeSdkClient::query`, `interrupt`, `set_model` or any other call, for example under `tokio::time::timeout`, no longer cuts a frame on the standard input of the CLI. Every frame now goes through one writer task per session, which finishes each frame it starts, also with a custom transport.
- A line of CLI output that escapes a lone UTF-16 surrogate, which JavaScript writes for a string cut inside an emoji, no longer ends the session: the surrogate becomes U+FFFD. Lines nested up to 512 levels deep are read too, instead of 128.
- A `can_use_tool` allow decision whose permission updates cannot all be sent now sends none of them and logs a warning; the tool call is still allowed.

## 0.1.0 - 2026-11-18

Upstream: Python SDK 0.2.182, CLI 2.1.325

### Added

- The first release of `claude-agent-rust-sdk`, a Rust port of the Python Claude Agent SDK that runs the Claude Code CLI as a child process and speaks its JSON control protocol over stdio.
- One-shot queries with `query`, and multi-turn sessions with `ClaudeSdkClient`: interrupts, prompt streams, model and permission changes, and typed messages that keep unknown wire data instead of dropping it.
- Hooks, tool-permission callbacks, subagent definitions, system prompt variants, budget limits, partial message streaming, and the CLI's error output through a callback.
- Tools served from your process, written by hand, with the `#[tool]` attribute (feature `macros`), or with an `rmcp` server (feature `rmcp`), with tool arguments validated against their JSON Schema (feature `jsonschema`, on by default).
- Session listing, reading, renaming, tagging, forking, and deletion on the CLI's own disk format, and session stores that resume a conversation from your own storage.
- OpenTelemetry trace propagation into the CLI (feature `otel`), and a scriptable transport for your own tests (feature `testing`, outside the compatibility promise).
- Linux, macOS, and Windows support, with the child process placed in its own process group or Job Object.
- Requirements: Rust 1.88 or newer with tokio, and the Claude Code CLI 2.0.0 or newer installed on the machine (the crate does not bundle it). This release is tested against CLI 2.1.325.
- Dual license `MIT OR Apache-2.0`. The `#[tool]` attribute ships in `claude-agent-rust-sdk-macros` at the same version; enable it through the `macros` feature rather than depending on that crate directly.
- Published by the release workflow through crates.io trusted publishing, with no stored registry token. The `.crate` archives attached to the GitHub Release carry a build attestation that `gh attestation verify` checks, as `RELEASING.md` shows.

## 0.1.0-rc.2 - 2026-11-18

Upstream: Python SDK 0.2.182, CLI 2.1.325

### Changed

- Published by the release workflow through crates.io trusted publishing, with no stored registry token. The `.crate` archives attached to the GitHub Release carry a build attestation that `gh attestation verify` checks, as `RELEASING.md` shows.

## 0.1.0-rc.1 - 2026-11-18

Upstream: Python SDK 0.2.182, CLI 2.1.325

### Added

- The first release of `claude-agent-rust-sdk`, a Rust port of the Python Claude Agent SDK that runs the Claude Code CLI as a child process and speaks its JSON control protocol over stdio.
- One-shot queries with `query`, and multi-turn sessions with `ClaudeSdkClient`: interrupts, prompt streams, model and permission changes, and typed messages that keep unknown wire data instead of dropping it.
- Hooks, tool-permission callbacks, subagent definitions, system prompt variants, budget limits, partial message streaming, and the CLI's error output through a callback.
- Tools served from your process, written by hand, with the `#[tool]` attribute (feature `macros`), or with an `rmcp` server (feature `rmcp`), with tool arguments validated against their JSON Schema (feature `jsonschema`, on by default).
- Session listing, reading, renaming, tagging, forking, and deletion on the CLI's own disk format, and session stores that resume a conversation from your own storage.
- OpenTelemetry trace propagation into the CLI (feature `otel`), and a scriptable transport for your own tests (feature `testing`, outside the compatibility promise).
- Linux, macOS, and Windows support, with the child process placed in its own process group or Job Object.
- Requirements: Rust 1.88 or newer with tokio, and the Claude Code CLI 2.0.0 or newer installed on the machine (the crate does not bundle it). This release is tested against CLI 2.1.325.
- Dual license `MIT OR Apache-2.0`. The `#[tool]` attribute ships in `claude-agent-rust-sdk-macros` at the same version; enable it through the `macros` feature rather than depending on that crate directly.
- This is a release candidate. Add it with an explicit version: `cargo add claude-agent-rust-sdk@0.1.0-rc.1`.
