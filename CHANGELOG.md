# Changelog

## Unreleased

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
