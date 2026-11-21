# Changelog

## Unreleased

### Added

- `Message`, every message type, `ContentBlock`, and every block type implement `Serialize` and `Deserialize` in the form of the CLI frame (`MessageOrigin` and `ToolResultContent` too; `UserContent`, `RateLimitInfo`, and `DeferredToolUse` implement `Serialize`). A message read from the CLI serializes back to a value equal to its frame, and deserializing goes through `parse_message`.
- `HookInput::tool_name()` and `HookInput::tool_input()` read the tool of a `PreToolUse`, `PostToolUse`, `PostToolUseFailure`, or `PermissionRequest` input, typed or `Unknown`, so a hook that guards tool calls keeps checking an input whose shape changed. The `hooks` example uses them and denies a call it cannot check.

### Changed

- `create_sdk_mcp_server` refuses with `Error::Config` a typed tool whose argument type cannot be read from a JSON object (`String`, a `Vec`, an enum of unit variants), which failed every call before.
- The `Debug` output of `ClaudeAgentOptions`, `McpStdioServerConfig`, and `McpRemoteServerConfig` prints the keys of `env`, `extra_args`, and `headers` with a placeholder for each value, and inline JSON `settings` as a placeholder, so logging options with `{:?}` no longer leaks API keys or tokens.
- The warning that the CLI is older or newer than the tested version is logged once per process for each CLI path and version, instead of once per session.
- `serde_json/preserve_order` is turned on by the new `preserve-order` feature, on by default, instead of unconditionally: a program that needs sorted `serde_json` maps can turn it off with `default-features = false`. Without it the crate works the same, and the keys of the JSON it writes come out sorted. A build that already set `default-features = false` loses key order until it adds `preserve-order`.
- In-process MCP servers answer as the Python SDK does under `mcp` 2.x, the version `pip install` resolves: the `initialize` result declares no `experimental` capability, an integer `protocolVersion` and a request other than `initialize` or `ping` before the handshake are refused with `-32602`, an unknown method gets `-32601 Method not found` with the method as `data`, a cancelled call gets `-32800`, and results are written with the keys of the MCP schema first, in byte order, without the keys it does not define (annotation keys set with `ToolAnnotations::extra` stay). The differential harness now runs against `mcp` 2.3.0; `docs/parity.md` lists what differs under `mcp` 1.x.
- A hook input with a missing or `null` `session_id`, `transcript_path`, `cwd`, or `tool_use_id` stays typed, with an empty string, instead of becoming `HookInput::Unknown`; an input that still does not match its event is logged at warn level instead of debug.

### Fixed

- On Unix, a program that leaves through `exit` (returning from `main`, `std::process::exit`) with a session still open gives the process group of each CLI `SIGTERM` from an exit hook, as the Python SDK does from `atexit`, instead of leaving it running (on macOS, which has no parent-death signal) or leaving its background processes behind (on Linux).
- A CLI started with the `user` option holds the supplementary groups of that user, from the group database, instead of none when the parent is privileged.
- A `task_progress` message whose `usage` is not an object fails to parse with `Invalid field in system message: 'usage'`, as any required field of the wrong type does, instead of reading as zero counts.
- An optional message or block field whose value the typed field cannot hold (`api_error_status: "529"`, `errors: ["boom", {"code": 529}]`) keeps the value as sent in `extra` (or `message_extra`), where it was dropped.
- On Unix, the processes left in the group of the CLI when it exits get `SIGTERM`, and `SIGKILL` only 5 s later if they are still running, instead of `SIGKILL` at once, so a server the agent started in the background can clean up.
- In-process MCP tools run in the `tracing` span and the OpenTelemetry context of the caller that connected, as the other callbacks do.
- A prompt stream that panics closes stdin, so the query ends, as upstream closes it when the iterable raises.
- `Error::is_retryable()` is true for an API error result with no HTTP status (`terminal_reason` is `api_error`, such as a stream idle timeout), the case upstream's `ResultError` docs give for a retry.
- A store subpath whose second character is `:` (`1:foo`, not only a letter drive) is refused, as `ntpath.splitdrive` reads a drive, so it cannot name an alternate data stream on Windows.
- The transcript mirror matches frames under the real path of a linked config directory even when its `projects` directory does not exist yet at connect, as `[behavior.mirror-canonical-projects-dir]` states.
- A `CLAUDE_CONFIG_DIR` that is not UTF-8 is used byte for byte, as Python reads it, instead of falling back to `~/.claude`. Without the variable and a home directory, no path relative to the working directory is read: the session functions find nothing, and a connect with a session store or a resume from a store fails with `ErrorKind::Spawn` (`[behavior.no-home-directory]`).
- A resume from a store reads the macOS Keychain only when `CLAUDE_CONFIG_DIR` is unset, as upstream: an empty value set in the process keeps the Keychain out.
- `get_subagent_messages_from_store` checks the agent id only on the direct path, when the store does not list its subkeys, and only for a separator or NUL, as `[behavior.store-agent-id-segment]` states: `.` and `..` name one segment once prefixed with `agent-`.
- A control request whose write never completes (a CLI that stopped reading its stdin) fails with `ErrorKind::ControlTimeout` after its limit instead of waiting forever: the timeout covers the write and the wait (`[behavior.control-timeout-covers-write]`).
- `SubprocessTransport::close` closes stdin even when a write is still blocked on a full pipe after the 5 s it waits for the write lock, as upstream does, so a CLI that reads again within its grace period gets its end of input instead of `SIGTERM`.

## 0.2.0 - 2026-11-20

Upstream: Python SDK 0.2.183 to 0.2.186, CLI 2.1.329

0.2.0 brings the crate to the behavior of the Python Claude Agent SDK 0.2.186 and Claude Code 2.1.329, the versions it is tested against, and reshapes the public API into the one meant to carry on to 1.0. Most programs need a few mechanical edits: read "Breaking changes" first, each entry gives the old code, the new code, and the Python it corresponds to.

### Breaking changes

- **`Error` is an opaque type with a kind.** In 0.1, `Error` was an enum matched by variant. It is now a cheaply cloned struct: `error.kind()` returns an `ErrorKind` (`#[non_exhaustive]`), and the data of a cause is read through accessors: `exit_code()`, `stderr()`, `result()`, `line()`, `data()`, `control_subtype()`, `unsupported_operation()`, `io_kind()`. The former `CliConnection` variant is split by cause into `ErrorKind::Spawn`, `NotConnected`, `ConnectionClosed`, and `Unsupported`, and a failing prompt stream is `ErrorKind::PromptStream`. `is_process()` and `is_cli_connection()` keep the meaning of the Python `except ProcessError` and `except CLIConnectionError` clauses, and the new `is_retryable()` says whether trying again can succeed. Migrate a match on variants to a match on the kind:

  ```rust
  // 0.1
  match error {
      Error::Process { exit_code, .. } => eprintln!("exit {exit_code:?}"),
      Error::CliConnection { .. } => reconnect(),
      other => return Err(other),
  }
  // 0.2
  match error.kind() {
      ErrorKind::Process => eprintln!("exit {:?}", error.exit_code()),
      _ if error.is_cli_connection() => reconnect(),
      _ => return Err(error),
  }
  ```

- **The constructors `Error::cli_connection`, `Error::config`, `Error::control`, and `Error::session_not_found` are gone.** Build the same error with `Error::new(ErrorKind::Config, "..")` and attach a cause with `with_source`; a custom transport or store keeps `Error::io`, `Error::process`, and `Error::session_store`.
- **The text of an `Io` or `SessionStore` error no longer repeats its source.** Read the cause with `std::error::Error::source`, which returns it with its type (an `io::Error` stays an `io::Error`). Code that searched the message for the text of the underlying error should walk the source chain instead.
- **Client methods lost their `get_` prefix.** `ClaudeSdkClient::get_server_info`, `get_mcp_status`, and `get_context_usage` are `server_info`, `mcp_status`, and `context_usage` (Python: `get_server_info()`, `get_mcp_status()`, `get_context_usage()`). `ToolResult::is_error(bool)` is `with_error_flag(bool)`.
- **The session functions take an options struct.** `list_sessions`, `list_sessions_from_store`, `get_session_messages`, `get_subagent_messages`, `get_session_messages_from_store`, `get_subagent_messages_from_store`, `fork_session`, `fork_session_via_store`, and `import_session_to_store` take `ListSessionsOptions`, `SessionMessagesOptions`, `ForkSessionOptions`, or `ImportSessionOptions` in place of their positional optional parameters. The defaults are the Python keyword defaults, so pass only what a Python call passes by keyword:

  ```rust
  // Python: list_sessions(directory=project, limit=10)
  // 0.1
  let sessions = list_sessions(Some(project), Some(10), 0, false);
  // 0.2
  let sessions = list_sessions(ListSessionsOptions::new().directory(project).limit(10));
  ```

- **Variants with fields are `#[non_exhaustive]`.** `PermissionResult::Allow` and `Deny`, `ThinkingConfig::Adaptive` and `Enabled`, and every `ToolContent` variant can no longer be written as struct literals. Build them with their constructors, `PermissionResult::allow()`, `allow_with_input(..)`, `deny(..)`, `deny_and_interrupt(..)`, `.updated_permissions(..)`, `ThinkingConfig::adaptive()`, `ThinkingConfig::enabled(n).display(..)`, `ToolContent::resource_link_parts(..)`, and add `..` to the patterns that match them. A later field is then not a breaking change.
- **`PermissionUpdate::to_wire` returns a `Result`.** It now sends the shape the CLI accepts (upstream #1502): a rule with no `rule_content` leaves `ruleContent` out instead of sending `null`, an unset `destination` is sent as `"session"`, and a missing required field or an `Unknown` kind is an `ErrorKind::Config` error. In 0.1 the CLI rejected the whole `updatedPermissions` list, so the tool call was allowed but no rule was remembered. Callers of `to_wire` add `?`; a permission callback needs no change.
- **The `Unknown` variant of string enums holds an `UnknownValue`.** For `HookEvent`, `PermissionMode`, and the other string enums, `Unknown` now holds a value only the crate builds, from a spelling no known variant has, so two values are equal exactly when their wire spellings are: `HookEvent::from("PreToolUse")` is `HookEvent::PreToolUse`. Build a value from text with `From`, and read the text of an unknown one with `as_str()` instead of matching its `String`.
- **`McpSdkServerInstance` is built only by the crate.** It can no longer wrap an arbitrary value, and `downcast_ref` is gone: make one with `create_sdk_mcp_server` or `McpServerConfig::sdk_rmcp` (Python: `create_sdk_mcp_server`). An in-process server a session cannot serve now fails to build instead of failing at connection.
- **Names returned as `&'static str` are `&str`.** `HookSpecificOutput::hook_event_name` and `McpServerConfig::type_name` borrow from the value, since `HookSpecificOutput::Unknown` and `McpServerConfig::Raw` carry their own name. Copy the text with `.to_owned()` where it must outlive the value.
- **`receive_messages` and `receive_response` end after an error.** As the Python iterator does, the stream of `ClaudeSdkClient` ends after yielding an error, such as a message that cannot be parsed. To read the messages after it, call `receive_messages` again.
- **`usage` is a `Usage`.** `AssistantMessage::usage` and `ResultMessage::usage` keep the JSON the CLI sent and read `input_tokens()`, `output_tokens()`, the two cache counts, and `service_tier()`; `get(key)` reads any other key and `into_value()` gives the JSON back (Python: the `usage` dict).
- **`ResultErrorData` no longer dereferences to the JSON object.** Read a key with `get` and the whole object with `as_map`, as the Rust API guidelines keep `Deref` for smart pointers.
- **`cli::managed_install_dir` is private.** The crate installs no CLI. The directory is still searched, before `PATH`.

### Added

- `Error::is_retryable()` says whether trying again can succeed: a control timeout, a closed connection, an API error result with status 429, 529, or 5xx, a process killed by a signal, an interrupted or timed-out I/O call, or a store that took too long.
- `Error::stderr()` holds the last 8 KiB of the CLI's error output when a `stderr` callback captured it, where it used to hold a placeholder. Neither the text nor the `Debug` form of the error shows it, so logging an error with `{}`, `{:?}`, `unwrap`, or a `main` that returns a `Result` never logs what the CLI wrote to stderr.
- A warning is logged when the CLI reports a version other than `CLI_VERSION`, the one release the crate is tested against: an older CLI exits with an error on an option it does not know.
- `SdkMcpTool::call` runs a tool as the in-process server does and returns the result the model would see, so a tool can be tested without a session. `#[tool]` functions take an optional state before their arguments: a `&S` shared through an `Arc`, or a `Clone` value.
- `McpServerConfig::Raw` passes a server configuration of a kind the crate does not model to the CLI as written, next to in-process servers in the same map, and `ToolAnnotations::extra_field` adds an annotation key the crate does not model.
- The types an application sends keep keys the crate does not model, as upstream passes its dictionaries through: every hook-specific output, `AsyncHookOutput`, `SandboxSettings`, `SandboxNetworkConfig`, and the stdio and remote MCP server configurations have a flattened `extra` map and an `extra_field(key, value)` builder, so a key a newer CLI reads, such as `classifierContext` or `timeout`, can be sent today.
- The types the crate reads keep the keys they do not model in `extra`: `UserMessage`, `AssistantMessage` (and their nested `message` object, in `message_extra`), `ResultMessage`, `StreamEvent`, `RateLimitEvent`, `ConversationResetMessage`, `PermissionUpdate`, the content blocks (`TextBlock`, `ThinkingBlock`, `ToolUseBlock`, `ToolResultBlock`, `ServerToolUseBlock`, `ServerToolResultBlock`), `HookEffort`, `BackgroundTaskSummary`, and `SessionCronSummary`. A permission suggestion sent back to the CLI keeps them.
- `HookSpecificOutput::Unknown` holds the output of a hook event the crate does not model, such as `Setup`, and sends it as written; reading one no longer fails.
- `SessionStore` is implemented for an `Arc` of a store, so a program can keep a handle on the store it passes to the options.
- The crate re-exports `futures_core`, `indexmap`, `schemars`, and, with the `rmcp` feature, `rmcp`. The crate docs list the six crates whose types appear in its public API (with `serde` and `serde_json`) and state that a major release of one of them is a major release of this crate.
- `MockTransport::end_input_count` returns how many times the input was ended.

### Changed

- The pinned upstream moved to Python SDK 0.2.186 and Claude Code 2.1.329.
- JSON keeps its key order end to end, as the Python SDK and the CLI do: the properties of an output schema, the order of MCP servers and subagents, the keys of settings, and every line a fork or a resume from a store writes. The crate enables the `preserve_order` feature of `serde_json`, and since Cargo features add up, every `serde_json::Map` in a program that uses this crate now keeps insertion order too.
- Messages and docs name the product "Claude Code", as upstream does ("Claude Code not found", "Failed to start Claude Code"), and the errors about hooks set to block on failure use upstream's words (`remove on_failure="block" from your hooks`).
- A `ResultError` that replaces the process error after a failed run carries that process error as its `source`, as upstream sets `__cause__`.
- An MCP server status whose configuration carries a key the crate does not model is read as its typed configuration, with the key in `extra`, instead of as `McpServerStatusConfig::Unknown`.
- `import_session_to_store` reads the transcript in blocks of 1 MiB instead of loading it whole.
- An in-process MCP server answers as the Python `mcp` package does: a cancelled call gets the error code `0` instead of `-32800`, invalid parameters get exactly `Invalid request parameters` with an empty `data`, an integer `protocolVersion` is accepted, and a message whose `jsonrpc` is not `"2.0"` or whose `params` is not an object gets a `-32603` error.
- The README, the migration guide, and the parity page state what is measured: a port of 0.2.186, the reviewed differences with upstream, and the upstream unit tests the parity ledger maps. The README says what a resume from a session store does with credentials: it copies the access token of the installed CLI, without its refresh token, into a temporary configuration directory that is removed when the session ends.

### Fixed

- **Permission updates are remembered.** A rule returned by a `can_use_tool` callback is now applied by the CLI (see `PermissionUpdate::to_wire` above). An allow decision whose updates cannot all be sent sends none of them and logs a warning; the tool call is still allowed.
- **Sessions survive output the CLI really writes.** A line that escapes a lone UTF-16 surrogate, which JavaScript writes for a string cut inside an emoji, no longer ends the session: the surrogate becomes U+FFFD. A line nested deeper than 128 levels is read too: up to 512 levels whole, and deeper with each array or object at depth 512 kept as its JSON text. A deep line whose brackets do not pair or close is refused without overflowing the stack.
- **Session history keeps every message.** The session readers, fork, and import read a transcript line with a lone surrogate or deep nesting as the live session does, instead of dropping it and every message before it. A subagent `.meta.json` sidecar with a lone surrogate is read as `json.loads` reads it.
- **Cancelling a call no longer corrupts the session.** Dropping the future of `ClaudeSdkClient::query`, `interrupt`, `set_model`, or any other call, for example under `tokio::time::timeout`, no longer cuts a frame on the standard input of the CLI: every frame goes through one writer task per session, also with a custom transport. A `close` or `flush` dropped while the transcript mirror appends to the store no longer loses the batch being sent.
- **Control requests keep flowing.** Control responses, hooks, and permission requests are served while up to 16 MiB of messages wait unread, so `interrupt()` called from the loop that reads the messages no longer times out after 60 s; past that budget the session stops reading the CLI output until the application catches up. Two control requests that share an id are both answered, as upstream.
- **Failures are reported, not hung.** A control request sent after the CLI died fails with `ErrorKind::ConnectionClosed` (retryable, the process error as its source), as upstream raises `CLIConnectionError`. A panic in the task that reads the CLI output fails the pending control requests at once and ends the stream with `ErrorKind::ConnectionClosed`. A read error on the CLI output ends the session with `ErrorKind::Io`, as upstream raises it. `Error::is_retryable()` is true for a CLI killed by a signal (exit code `-9` for `SIGKILL` on Unix).
- **Session stores.** A store whose `append` panics no longer stops the session: the panic counts as a failed append, is retried, and is reported as a `mirror_error` message. The store functions, resume, and the conformance suite call an optional store method a store implements even when `capabilities()` does not declare it (before, `delete_session_via_store` returned without deleting and a resume skipped subagent transcripts). The conformance suite counts a summary a store lists twice once. `import_session_to_store` fails with `ErrorKind::Io` when a subagent's `.meta.json` exists but cannot be read.
- **Resume from a store.** The temporary configuration directory is created readable by its owner only (0700) on Unix, as upstream, since it holds the access token. On macOS the Keychain credentials are found for a process started without `USER` or `LOGNAME`, such as a launchd daemon. A subagent subkey that ends with a separator writes to the same transcript as the subkey without it.
- **Paths and process.** `project_key_for_directory`, `file_path_to_session_key`, and the store readers resolve `.` and `..` as upstream, give an empty directory the key of the current directory, and compare the projects directory without regard to case on Windows. With `PATH` unset, the CLI is searched in the system search path (`confstr(_CS_PATH)`, else `/bin:/usr/bin`) on Unix, as `shutil.which` does. An empty `cwd` is treated as unset. A `SubprocessTransport` connects again after `close`. When a program returns from `main` with a session open, the CLI process group gets `SIGTERM` on Unix, as upstream sends at interpreter exit, so the CLI writes the end of its transcript. The 5-second limits on `git worktree list` and the macOS Keychain read hold when the command leaves a process that keeps its output open.
- **The async runtime is never blocked.** `connect`, a resume from a store, and `import_session_to_store` run file access, the Keychain read, and the user lookup on the blocking pool, so a `current_thread` runtime keeps its other tasks and timers going.
- **In-process MCP tools.**
  - `SdkMcpTool::raw` lists every schema with `"type": "object"`: a schema with no `type`, such as `{}`, gets it, and one with another `type` is an `ErrorKind::Config` error. Claude Code 2.1.329 drops every tool of a server when one tool lists a schema without it, and reports the server connected with no error.
  - A typed tool whose arguments are an enum lists every field of every variant in one object schema, with the tag first and the doc comment of each variant in the tag's description, instead of an object with no property.
  - A typed tool reads its arguments as sent first and turns integral floats into integers only when that fails, and only through the alternatives of an `anyOf` or `oneOf` that state every key, so an untagged enum keeps the variant the arguments describe.
  - A typed tool of no arguments can take `()` or a unit struct and receives the call whatever keys the model sends, as upstream hands a tool of schema `{}` any keys.
  - When the CLI cancels the control request of a tool call, the tool runs to its end and its JSON-RPC id stays reserved until it answers, as upstream; `notifications/cancelled` and closing the session still stop it. A request with a float id (`2.0`) sent to an `rmcp` server is acknowledged as a notification.
  - Tools are listed with their description even when empty and with `annotations: {}` when no hint is set. `tools/call` arguments that are not an object and an incomplete `initialize` are `-32602` errors. Tool arguments are not checked against `format` for JSON Schema drafts 4, 6, and 7, as upstream validates without a format checker.
  - `#[tool]` compiles when the state argument has the name of the function, names a tool `type` for `fn r#type`, keeps the relative indentation of its doc comment, takes `crate = "..."`, and says when a doc attribute it cannot read (such as one built with `include_str!`) is why a description is missing.
- **Messages and hooks.**
  - A user message with a `thinking`, `server_tool_use`, or `advisor_tool_result` block no longer fails to parse; any block other than text, tool use, and tool result is kept as `ContentBlock::Unknown`.
  - A hook input without `tool_response`, with a fractional `duration_ms`, or with `effort` as a bare string stays typed instead of becoming `HookInput::Unknown`. `hook_on_failure_supported` in the answer to `initialize` is read as Python's `if` reads it.
  - A `task_notification` whose `usage` is not an object reads `usage` as absent. The `mcp_status` and `context_usage` responses read a fractional, negative, or `null` count and a `null` string instead of failing the call. An MCP server status written back with `serde` keeps the `"type": "stdio"` key the CLI sent.
  - An unknown hook callback id that is not a string is named in the error answer (`No hook callback found for ID: 7`).
- **Sessions on disk.** `fork_session` ends on a transcript whose `progress` entries name each other as parents, where it looped forever (upstream has the same defect). `rename_session`, `tag_session`, the reading of a first prompt, and the framing of CLI output strip what Python's `str.strip()` strips, the separators U+001C to U+001F included.
- **Configuration and tracing.** `CLAUDE_CODE_STREAM_CLOSE_TIMEOUT` is read like Python's `int()`, so `120_000` gives two minutes; a value `int()` rejects keeps the 60 s default with a warning. Permission callbacks, hooks, in-process MCP tools, session stores, and the stderr callback run in the `tracing` span and, with `otel`, the OpenTelemetry context current when the session connected; without a global propagator, `otel` hands the CLI the W3C trace context and baggage, following `OTEL_PROPAGATORS`, as the Python SDK does.

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
