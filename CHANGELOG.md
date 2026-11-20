# Changelog

## Unreleased

### Added

- Every pull request compares the public API with the newest release on crates.io through `cargo-semver-checks`, so a patch release cannot break a build against the previous release.
- A scheduled check of `Cargo.lock` against the RustSec advisory database.
- `Error::is_retryable()` says whether trying again can succeed: a control timeout, a closed connection, an API error result with status 429, 529, or 5xx, a process killed by a signal, an interrupted or timed-out I/O call, or a store that took too long.
- `Error::stderr()` holds the last 8 KiB of the CLI's error output when a `stderr` callback captured it, where it used to hold a placeholder. The text of the error leaves it out, so logging an error never logs what the CLI wrote to stderr.
- A warning is logged when the CLI reports a version older or newer than `CLI_VERSION`, the one release the crate is tested against: an older CLI exits with an error on an option it does not know.

### Changed

- Upstream moved to Python SDK 0.2.186 and CLI 2.1.329.
- **Breaking:** `Error` is now an opaque, cheaply cloned type. `Error::kind()` returns an `ErrorKind` (`#[non_exhaustive]`), and the data of a cause is read through accessors: `exit_code()`, `stderr()`, `result()`, `line()`, `data()`, `control_subtype()`, `unsupported_operation()`, `io_kind()`. The former `Error::CliConnection` is split by cause into `ErrorKind::Spawn`, `NotConnected`, `ConnectionClosed`, and `Unsupported`, and a failing prompt stream is `ErrorKind::PromptStream`. `Error::new(kind, message)` and `with_source` build an error for a custom transport or store. `is_process()` and `is_cli_connection()` keep the meaning of the Python `except` clauses. Migrate `match error { Error::Process { exit_code, .. } => .. }` to `match error.kind() { ErrorKind::Process => error.exit_code(), .. }`.
- **Breaking:** the text of an `Io` or `SessionStore` error no longer repeats its source, which `std::error::Error::source` returns with its type (an `io::Error` stays an `io::Error`).
- Messages name the product "Claude Code", as upstream does: "Claude Code not found", "Failed to start Claude Code".
- **Breaking:** `PermissionUpdate::to_wire` returns a `Result`. It sends the shape the CLI accepts (upstream #1502): a rule with no `rule_content` leaves `ruleContent` out instead of sending `null`, an unset `destination` is sent as `"session"`, and a missing required field or an `Unknown` kind is an `ErrorKind::Config` error. The CLI used to reject the whole `updatedPermissions` list, so the call was allowed but nothing was remembered.

### Removed

- **Breaking:** `cli::managed_install_dir` is private. The crate installs no CLI, so the directory is only searched, before `PATH`, as before.

### Fixed

- A control request sent after the CLI died fails with `ErrorKind::ConnectionClosed` (retryable, the process error as its source), as upstream raises `CLIConnectionError`, instead of repeating the process error.
- JSON keeps its key order end to end, as the Python SDK and the CLI do: the properties of an output schema, the order of MCP servers and subagents, the keys of settings, and every line a fork or a resume from a store writes. The crate now enables the `preserve_order` feature of `serde_json`; since Cargo features add up, a `serde_json::Map` elsewhere in a program that uses this crate also keeps insertion order.
- Dropping the future of `ClaudeSdkClient::query`, `interrupt`, `set_model` or any other call, for example under `tokio::time::timeout`, no longer cuts a frame on the standard input of the CLI. Every frame now goes through one writer task per session, which finishes each frame it starts, also with a custom transport.
- A line of CLI output that escapes a lone UTF-16 surrogate, which JavaScript writes for a string cut inside an emoji, no longer ends the session: the surrogate becomes U+FFFD. Lines nested up to 512 levels deep are read too, instead of 128.
- A `can_use_tool` allow decision whose permission updates cannot all be sent now sends none of them and logs a warning; the tool call is still allowed.
- `SdkMcpTool::raw` lists every schema with `"type": "object"`: a schema with no `type`, such as `{}`, gets it, and a schema with another `type` is an `ErrorKind::Config` error. CLI 2.1.329 drops every tool of a server when one tool lists a schema without it, and reports the server connected with no error.
- An MCP server status written back with `serde` keeps the `"type": "stdio"` key the CLI sent.
- A session store whose `append` panics no longer stops the session: the panic counts as a failed append, so it is retried and then reported as a `mirror_error` message, and the result is still delivered. A panic anywhere in the task that reads the CLI output now fails the pending control requests at once and ends the message stream with an `ErrorKind::ConnectionClosed` error, where the stream used to end with no result and the requests waited for their timeout.
- When a program returns from `main` with a session still open, the CLI process group gets `SIGTERM` on Unix, as upstream sends at interpreter exit, instead of `SIGKILL`: the CLI writes the end of its transcript before it exits.

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
