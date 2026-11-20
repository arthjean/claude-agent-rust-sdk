# Changelog

## Unreleased

### Added

- Every pull request compares the public API with the newest release on crates.io through `cargo-semver-checks`, so a patch release cannot break a build against the previous release.
- A scheduled check of `Cargo.lock` against the RustSec advisory database.
- `Error::is_retryable()` says whether trying again can succeed: a control timeout, a closed connection, an API error result with status 429, 529, or 5xx, a process killed by a signal, an interrupted or timed-out I/O call, or a store that took too long.
- `Error::stderr()` holds the last 8 KiB of the CLI's error output when a `stderr` callback captured it, where it used to hold a placeholder. The text of the error leaves it out, so logging an error never logs what the CLI wrote to stderr.
- A warning is logged when the CLI reports a version older or newer than `CLI_VERSION`, the one release the crate is tested against: an older CLI exits with an error on an option it does not know.
- The types an application sends keep keys the crate does not model, as upstream passes its dictionaries through: every hook-specific output, `AsyncHookOutput`, `SandboxSettings`, `SandboxNetworkConfig`, and the stdio and remote MCP server configurations have a flattened `extra` map and an `extra_field(key, value)` builder, so a key a newer CLI reads, such as `classifierContext` or `timeout`, can be sent today.
- `HookSpecificOutput::Unknown` holds the output of an event the crate does not model, such as `Setup`, and sends it as written; reading one no longer fails.
- `cargo xtask upstream-diff` lists the upstream tests rewritten in place, which keep their names, with the Rust tests to review, and every `ref` of `parity/differences.toml` carries an `anchor` the cited lines must hold, so a ref that drifts when the pin moves fails `parity-check`. Five refs that had drifted since the sync to 0.2.186 now point at the code they describe.
- `SdkMcpTool::call` runs a tool as the in-process server does and returns the result the model would see, so a tool can be tested without a session; `#[tool]` functions take an optional state before their arguments (a `&S` shared through an `Arc`, or a `Clone` value).
- `ToolAnnotations::extra_field` lists an annotation key the crate does not model, and `McpServerConfig::Raw` passes a server configuration of a kind the crate does not model to the CLI as written, next to in-process servers in the same map. **Breaking:** `McpServerConfig::type_name` returns `&str`.
- The crate re-exports `indexmap`, `schemars`, and, with the `rmcp` feature, `rmcp`, the three crates whose types appear in its public API, and its docs state that a major release of one of them is a major release of this crate.
- `MockTransport::end_input_count` returns how many times the input was ended.
- `PermissionUpdate` and the content blocks the crate models (`TextBlock`, `ThinkingBlock`, `ToolUseBlock`, `ToolResultBlock`, `ServerToolUseBlock`, `ServerToolResultBlock`) keep the keys they do not model in an `extra` field, such as the `citations` of a text block, and a permission suggestion sent back to the CLI keeps them.
- `SessionStore` is implemented for an `Arc` of a store, so a program can keep a handle on the store it passes to the options.

### Changed

- Upstream moved to Python SDK 0.2.186 and CLI 2.1.329.
- **Breaking:** `Error` is now an opaque, cheaply cloned type. `Error::kind()` returns an `ErrorKind` (`#[non_exhaustive]`), and the data of a cause is read through accessors: `exit_code()`, `stderr()`, `result()`, `line()`, `data()`, `control_subtype()`, `unsupported_operation()`, `io_kind()`. The former `Error::CliConnection` is split by cause into `ErrorKind::Spawn`, `NotConnected`, `ConnectionClosed`, and `Unsupported`, and a failing prompt stream is `ErrorKind::PromptStream`. `Error::new(kind, message)` and `with_source` build an error for a custom transport or store. `is_process()` and `is_cli_connection()` keep the meaning of the Python `except` clauses. Migrate `match error { Error::Process { exit_code, .. } => .. }` to `match error.kind() { ErrorKind::Process => error.exit_code(), .. }`.
- **Breaking:** the text of an `Io` or `SessionStore` error no longer repeats its source, which `std::error::Error::source` returns with its type (an `io::Error` stays an `io::Error`).
- Messages name the product "Claude Code", as upstream does: "Claude Code not found", "Failed to start Claude Code".
- **Breaking:** `PermissionUpdate::to_wire` returns a `Result`. It sends the shape the CLI accepts (upstream #1502): a rule with no `rule_content` leaves `ruleContent` out instead of sending `null`, an unset `destination` is sent as `"session"`, and a missing required field or an `Unknown` kind is an `ErrorKind::Config` error. The CLI used to reject the whole `updatedPermissions` list, so the call was allowed but nothing was remembered.
- **Breaking:** the variants that carry fields, `PermissionResult::Allow` and `Deny`, `ThinkingConfig::Adaptive` and `Enabled`, and every `ToolContent` variant, are `#[non_exhaustive]`: build them with their constructors (`PermissionResult::allow().updated_permissions(..)`, `PermissionResult::deny_and_interrupt`, `ThinkingConfig::enabled(n).display(..)`, `ToolContent::resource_link_parts`) and match them with `..`, so a later field is not a breaking change.
- **Breaking:** the `Unknown` variant of every string enum (`HookEvent`, `PermissionMode`, and the others) holds an `UnknownValue` that only the crate builds, from a spelling no known variant has. Two values are now equal exactly when their wire spellings are: `HookEvent::from("PreToolUse")` is `HookEvent::PreToolUse`, never an `Unknown` that compares unequal to it. Read the text with `as_str()`.
- **Breaking:** `McpSdkServerInstance` can no longer be built from an arbitrary value, and `downcast_ref` is gone: only `create_sdk_mcp_server` and `McpServerConfig::sdk_rmcp` make one, so an in-process server a session cannot serve is no longer representable instead of failing at connection.
- **Breaking:** `HookSpecificOutput::hook_event_name` returns `&str` instead of `&'static str`, since an `Unknown` output carries its own name.
- An MCP server status whose configuration carries a key the crate does not model is now read as its typed configuration, with the key in `extra`, instead of as `McpServerStatusConfig::Unknown`.
- A `ResultError` that replaces the process error after a failed run carries that process error as its `source`, as upstream sets `__cause__`.
- The README, the migration guide, and the parity page state what is measured: a port of 0.2.186, the reviewed differences and the ones not yet classified, and the unit tests the ledger lists (the upstream end-to-end tests are not). `parity-check` now fails on a hand-written claim of parity (`at parity with`, `every known difference`, `full parity`, `100%`) in any of the three.
- **Breaking:** `ClaudeSdkClient::receive_messages` and `receive_response` end after yielding an error, such as a message that cannot be parsed, as upstream's iterator does; call `receive_messages` again to read the messages after it.
- `cargo xtask parity-check` fails when a ledger names an `#[ignore]`d test, a test compiled on this platform under another platform's suffix, or, for a test of another platform, a function that is not a test; CI runs it on Linux, macOS, and Windows.
- **Breaking:** `ClaudeSdkClient::get_server_info`, `get_mcp_status`, and `get_context_usage` are renamed `server_info`, `mcp_status`, and `context_usage`, and `ToolResult::is_error(bool)` is renamed `with_error_flag(bool)`.
- The product is named Claude Code throughout the docs, logs, and package descriptions, and the errors about hooks set to block on failure use upstream's words (`remove on_failure="block" from your hooks`).
- **Breaking:** `AssistantMessage::usage` and `ResultMessage::usage` are a `Usage`, which keeps the JSON the CLI sent and reads `input_tokens`, `output_tokens`, the two cache counts, and `service_tier`; `Usage::into_value` gives the JSON back.
- **Breaking:** `list_sessions`, `list_sessions_from_store`, `get_session_messages`, `get_subagent_messages`, `get_session_messages_from_store`, `get_subagent_messages_from_store`, `fork_session`, `fork_session_via_store`, and `import_session_to_store` take an options struct (`ListSessionsOptions`, `SessionMessagesOptions`, `ForkSessionOptions`, `ImportSessionOptions`) built with `new()` and chainable setters, in place of their positional optional parameters; the defaults are upstream's keyword defaults.
- `import_session_to_store` reads the transcript in blocks of 1 MiB instead of loading it whole.
- A cancelled call to an in-process MCP server is answered with the error code `0`, as the Python `mcp` package does, instead of `-32800`.
- An in-process MCP server answers invalid parameters with exactly `Invalid request parameters` and an empty `data`, as the Python `mcp` package does, accepts an integer `protocolVersion`, and refuses a message whose `jsonrpc` is not `"2.0"` or whose `params` is not an object with a `-32603` error.
- `ResultErrorData` no longer dereferences to the JSON object: read other keys with `get` and the whole object with `as_map`, as the Rust API guidelines keep `Deref` for smart pointers.

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
- A hook input without `tool_response`, with a fractional `duration_ms`, or with `effort` as a bare string stays typed instead of becoming `HookInput::Unknown`, and an input that still does not match its event is logged at debug level.
- The session store functions, resume, and the conformance suite call an optional store method a store implements even when `capabilities()` does not declare it; previously `delete_session_via_store` returned without deleting and a resume skipped subagent transcripts.
- `project_key_for_directory` and the store readers give an empty directory the key of the current directory, and resolve `.` and `..` in the part of a path that does not exist yet, as upstream.
- `import_session_to_store` fails with `ErrorKind::Io` when a subagent's `.meta.json` sidecar exists but cannot be read, instead of importing the transcript without its metadata.
- On macOS, resuming from a session store finds the Keychain credentials of a process started without `USER` or `LOGNAME`, such as a launchd daemon, by reading the user database as upstream does.
- Resuming from a store writes a subagent subkey that ends with a separator to the same transcript file as the subkey without it, as upstream.
- When the CLI cancels the control request of an in-process tool call, the tool now runs to its end and its JSON-RPC id stays reserved until it answers, as in the Python SDK; `notifications/cancelled` and closing the session still stop it.
- Tool arguments are no longer checked against `format` for JSON Schema drafts 4, 6, and 7, matching the Python SDK, which validates without a format checker.
- A typed tool whose arguments are an enum lists every field of every variant in one object schema instead of an object with no property, and a float without a fractional part is read as an integer where the schema asks for one.
- In-process tools are listed with their description even when empty and with `annotations: {}` when annotations hold no hint; `tools/call` arguments that are not an object and an incomplete `initialize` are JSON-RPC `-32602` errors; `#[tool]` keeps the relative indentation of its doc comment and takes `crate = "..."`.
- A user message no longer fails to parse on a `thinking`, `server_tool_use`, or `advisor_tool_result` block; any block other than text, tool use, and tool result is kept as `ContentBlock::Unknown`.
- A `SubprocessTransport` connects again after `close`, starting a new CLI process, so `ClaudeSdkClient::with_transport` survives a second `connect`.
- An empty `cwd` is treated as unset: the CLI runs in the current directory instead of failing with "Working directory does not exist".
- `CLAUDE_CODE_STREAM_CLOSE_TIMEOUT` is read like Python's `int()`, so `120_000` gives two minutes; a value `int()` rejects keeps the 60 s default with a warning.
- Permission callbacks, hooks, in-process MCP tools, session stores, and the stderr callback run in the `tracing` span and, with `otel`, the OpenTelemetry context current when the session connected. Without a global propagator, the `otel` feature hands the CLI the W3C trace context and baggage, following `OTEL_PROPAGATORS`, as the Python SDK does.
- `connect`, a resume from a session store, and `import_session_to_store` no longer block the async runtime: file access, the macOS Keychain read, and the user lookup run on the blocking pool, so a `current_thread` runtime keeps its other tasks and timers going.
- Control responses, hooks, and permission requests keep flowing while up to 16 MiB of messages wait unread, so `interrupt()` called from the loop that reads the messages no longer times out after 60 s; past that budget the session stops reading the CLI output until the application catches up.
- The temporary config directory of a store-backed resume is created readable by its owner only (0700) on Unix, as upstream, since it holds the credentials.
- Two control requests that share an id are both answered, as upstream; before, neither was.
- `Error::is_retryable()` is true for a CLI killed by a signal, as documented: the exit code the SDK records for it is negative on Unix (`-9` for `SIGKILL`), which the check now reads.
- `#[tool]` compiles when the state argument has the name of the function, names a tool `type` for `fn r#type`, and says when a doc attribute it cannot read (such as one built with `include_str!`) is the reason a description is missing.
- A typed tool reads its arguments as sent first, and turns integral floats into integers only when that fails, so an untagged enum keeps the variant whose field takes a float instead of losing the other fields.
- The session readers, fork, and import read a transcript line with a lone surrogate escape or a nesting deeper than 128 levels, as the live session does, instead of dropping it and every message before it.
- `fork_session` ends on a transcript whose `progress` entries name each other as parents, where it looped forever (upstream has the same defect).
- `rename_session` and `tag_session` strip what Python's `str.strip()` strips, the separators U+001C to U+001F included, and so does the reading of a first prompt.
- The 5-second limit on `git worktree list` and the macOS Keychain read holds when a process the command started keeps its output open.
- A `close` or `flush` of a client dropped while the transcript mirror appends to the store, as under a timeout, no longer loses the batch being sent: the sending runs in a task of its own, as upstream shields it.
- A request with a float id (`2.0`) sent to an `rmcp` server is acknowledged as a notification, as upstream does, where it was never answered.
- The tag of a typed tool whose arguments are a tagged enum lists the doc comment of each variant in its description; the merge of the variants dropped them.

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
