[PRD]
# PRD: Claude Agent SDK for Rust (`claude-agent-kit`)

## Changelog

| Version | Date | Author | Summary |
|---------|------|--------|---------|
| 1.0 | 2026-11-14 | Arthur Jean | Initial draft |
| 1.1 | 2026-11-14 | Arthur Jean | Repository named `claude-agent-sdk-rust`; license set to GPL-3.0-or-later |
| 1.2 | 2026-11-14 | Arthur Jean | Python source references added to every epic and story |

## Problem Statement

1. **Rust has no maintained Agent SDK.** Anthropic ships the Agent SDK for Python and TypeScript only; its docs tell other languages to "run the CLI as a subprocess" (code.claude.com/docs/en/agent-sdk/overview). About ten community crates exist, and the most downloaded one (`anthropic-agent-sdk`, 88,153 downloads) has had no release since 2025-12-23 while the Python SDK reached v0.2.182.
2. **Existing crates are incomplete.** Only one of the four most used crates covers session helpers. `claude-agent-sdk-rs` has open parity issues (`agent_id` missing from hook inputs, #32) and shipped Windows process bugs (#26, #18). None publishes a way to prove parity.
3. **The protocol is undocumented and changes with every CLI release.** The control protocol has no official specification or stability promise. 11 of the last 18 Python releases are CLI version bumps. A port without a drift-detection mechanism goes stale within months.
4. **Process handling is where ports fail.** Orphaned CLI descendants, console windows on Windows, and lost final messages when the child is killed before it flushes its session file are recurring defects in this class of library.

**Why now:** the feasibility study of 2026-11-14 (`docs/research/rust-sdk-feasibility.md`) verified that the Python SDK has no native binding to Claude Code: a 50-line script without the SDK completed the `initialize` handshake against CLI 2.1.325, including with `CLAUDE_CODE_ENTRYPOINT=sdk-rs`. The whole behavior is specified by 13,707 lines of MIT-licensed Python and 1,171 unit tests, so a complete port is a bounded task today.

## Overview

`claude-agent-kit` is a Rust crate at full behavioral parity with `claude-agent-sdk` (Python) v0.2.182, commit `4720d36`. It spawns the Claude Code CLI as a child process and exchanges newline-delimited JSON (NDJSON, one JSON object per line) over stdin and stdout. It covers the transport, the bidirectional control protocol (hooks, tool-permission callback, dynamic control), the typed data model, in-process MCP tool servers (MCP is the Model Context Protocol through which tools are exposed to the model), the session helpers and stores, and CLI discovery and installation.

The Python source is the behavioral reference and is vendored as a git submodule at `upstream/claude-agent-sdk-python`, pinned to a commit. The same tree is available locally at `C:/dev/claude-agent-sdk-python`, at that commit. Every epic and story carries a `Python reference` block listing the source and test files it ports, and `ref:` notes inside acceptance criteria point at the exact rule. All these paths are relative to that root, and line numbers are valid for commit `4720d36` only. Two ledgers make parity measurable: a surface ledger mapping each of the 159 Python exports to a Rust item, and a test ledger mapping each Python unit test to a Rust test or a written reason for exclusion. A drift tool compares a new upstream tag against the pinned commit and lists changed flags, types, and tests.

Key decisions: tokio as the only async runtime; manual dispatch on the wire `type` field so unknown messages and content blocks are kept as raw JSON instead of dropped; an explicit `close()` with a bounded escalation because Rust has no asynchronous destructor; a native JSON-RPC dispatcher for in-process tools with `rmcp` support behind a feature; no bundled CLI binary (crates.io caps a crate at 10 MB), replaced by discovery plus an opt-in installer that verifies checksums. Delivery is phased in three releases: Release 1 (EP-001 to EP-003, core), Release 2 (EP-004 and EP-005, tools and sessions), Release 3 (EP-006, distribution and parity tooling).

The project lives in the repository `claude-agent-sdk-rust` (local path `C:/dev/claude-agent-sdk-rust`). The repository name and the crate name differ on purpose: the crate name `claude-agent-sdk-rust` is already taken on crates.io, so the crate keeps the working name `claude-agent-kit` until the owner confirms a final name. The project is licensed GPL-3.0-or-later.

## Goals

| Goal | Month-1 Target | Month-6 Target |
|------|---------------|----------------|
| Python public surface mapped in the surface ledger (159 exports) | 65% implemented (Release 1) | 100% implemented or excluded with a written reason |
| Python unit tests dispositioned in the test ledger (1,171 tests) | 50% | 100% |
| Lag between a Python SDK release that changes the wire or options and the matching crate release | Not measured | 7 days or less |
| Lag between a CLI version bump upstream and the pinned-version bump here | Not measured | 48 hours or less, automated |
| Orphaned CLI processes after the stress test (200 spawn/close cycles, half cancelled) | 0 on Linux | 0 on Linux, macOS, Windows |

## Target Users

### Rust application developer
- **Role:** builds a Rust service, CLI, or desktop app that needs an agent loop with tools.
- **Behaviors:** uses tokio, serde, and typed APIs; reads docs.rs; expects `Stream`-based APIs and `Result` errors.
- **Pain points:** community crates lag the CLI by months, expose untyped JSON for tools, and leak processes on Windows.
- **Current workaround:** shells out to `claude -p --output-format json`, or calls the Python SDK from a sidecar process.
- **Success looks like:** `cargo add claude-agent-kit`, a typed message stream in under 20 lines, and no orphan process after cancellation.

### Agent platform engineer
- **Role:** runs many agent sessions in one process and stores transcripts outside the local disk.
- **Behaviors:** needs hooks, permission callbacks, session stores, resume, and observability.
- **Pain points:** no Rust crate offers a session store contract with a conformance suite; only one offers session listing at all.
- **Current workaround:** runs the Python or TypeScript SDK behind an RPC boundary.
- **Success looks like:** a `SessionStore` trait with a published conformance suite, and resume from a store working against the real CLI.

### SDK maintainer
- **Role:** keeps the crate aligned with upstream.
- **Behaviors:** reviews upstream releases, bumps the pinned CLI version.
- **Pain points:** no specification to diff against; drift is discovered by users.
- **Current workaround:** reads upstream commits by hand.
- **Success looks like:** one command lists every upstream change that needs porting.

## Research Findings

Key findings that informed this PRD:

### Competitive Context
- `anthropic-agent-sdk` 0.2.75 (2025-12-23): `query()` stream, hooks, permission callback, in-process MCP behind an `rmcp` feature, environment allowlist. No sessions, no release for 11 months.
- `claude-agent-sdk-rs` 0.6.4 (2026-02-09): builder options, `tool!` macro over untyped `serde_json::Value`, `anyhow` in the public API. Open parity gaps and fixed Windows process bugs.
- `claude-code-sdk-rust` 0.4.1 (2026-07-21): sessions and `SessionStore` through `async_trait`. Published under one name and imported under another.
- **Market gap:** a complete port that stays current. Freshness and proven parity are the differentiators, not existence.

### Best Practices Applied
- `#[non_exhaustive]` on every public enum and struct fed by the wire, so upstream additions are not breaking changes.
- One `thiserror` error enum; no `anyhow` in the public API.
- A single reader task owns stdout and feeds a bounded channel: tokio documents `read_line` as not cancel-safe.
- Additive Cargo features only.
- Feature detection through the `capabilities` array of the init event, which the official docs recommend over version comparison.

*Full research sources available in `docs/research/rust-sdk-feasibility.md`.*

## Assumptions & Constraints

### Assumptions (to validate)
- The CLI names project directories by counting UTF-16 code units, as JavaScript does, while the Python reference counts Unicode code points (`ref: src/claude_agent_sdk/_internal/sessions.py:84-114`). Validated by US-023.
- The CLI treats `CLAUDE_CODE_ENTRYPOINT=sdk-rs` like the Python and TypeScript labels beyond the handshake. The handshake was probed successfully on 2026-11-14; later behavior is covered by US-033.
- A native MCP dispatcher that implements `initialize`, `ping`, `tools/list`, and `tools/call` satisfies the CLI's MCP client. Covered by US-033.
- The performance targets in Non-Functional Requirements are reachable; they are targets, not measurements.
- Licensing a port of MIT-licensed code under GPL-3.0-or-later is permitted when the upstream MIT notice is preserved, and the licenses of the planned dependencies (MIT, Apache-2.0) are compatible with GPL version 3. Not legal advice.

### Hard Constraints
- The Claude Code binary must not be modified or redistributed inside the crate (Anthropic legal page; crates.io 10 MB limit).
- The crate name and docs must not use "Claude Code" as a product name; "Claude Agent" is permitted.
- No feature may offer claude.ai login on behalf of end users; authentication stays whatever the CLI resolves.
- Minimum supported Rust version 1.88, edition 2024, tokio 1.54 or later.
- License GPL-3.0-or-later for every workspace member; no dependency with a license incompatible with GPL version 3.
- Code carries no implementation comments (`//`); rustdoc on public items is documentation and is required.
- The upstream submodule is read-only.

## Quality Gates

These commands must pass for every user story:
- `cargo fmt --all --check` - formatting
- `cargo clippy --workspace --all-targets --all-features --no-deps` - lint policy (`panic`, `unimplemented`, `dbg_macro` denied)
- `cargo test --workspace --all-features` - unit and integration tests, including the fake-CLI tests
- `cargo doc --workspace --all-features --no-deps` - documentation builds with `RUSTDOCFLAGS="-D warnings"`

## Epics & User Stories

Reference root for every `Python reference` block and `ref:` note: `C:/dev/claude-agent-sdk-python` at commit `4720d36` (the same tree as the submodule `upstream/claude-agent-sdk-python`). A reference names what to port; the acceptance criteria state what must hold.

### EP-001: Foundation and data model

The workspace, the error model, and every typed structure that crosses the wire. Release 1.

**Definition of Done:** every Python type exported from `types.py` that is not session-related has a Rust counterpart, and the parser reproduces the required/optional rules of `message_parser.py` on the ported test payloads.

**Python reference** (root `C:/dev/claude-agent-sdk-python`, commit `4720d36`):
- `pyproject.toml`: 1-45 (package metadata and dependencies)
- `src/claude_agent_sdk/_errors.py`: 1-134 (error hierarchy)
- `src/claude_agent_sdk/types.py`: 25-1926 and 2233-2986 (every type that is not session-related)
- `src/claude_agent_sdk/_internal/message_parser.py`: 37-401 (parser)
- `src/claude_agent_sdk/__init__.py`: 648-824 (`__all__`, the 159 exported names)
- `tests/test_errors.py`: tests
- `tests/test_types.py`: tests
- `tests/test_message_parser.py`: tests
- `tests/test_rate_limit_event_repro.py`: tests
- `tests/test_option_warnings.py`: tests

#### US-001: Workspace scaffold, lint policy and CI
**Description:** As an SDK maintainer, I want a Cargo workspace with the lint policy and a three-OS CI so that every later story is validated the same way.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** None

**Python reference** (root `C:/dev/claude-agent-sdk-python`, commit `4720d36`):
- `pyproject.toml`: 1-45 (metadata, runtime and optional dependencies), 87-143 (lint and type-check settings)
- `CLAUDE.md`: workflow commands and CI hardening rules
- `.github/workflows/test.yml`: 11-22 (unit test matrix on three operating systems)
- `.github/workflows/lint.yml`: lint and type-check job
- `LICENSE`: MIT notice to preserve in `THIRD_PARTY_NOTICES`
- `src/claude_agent_sdk/_version.py`: 1-3 (SDK version constant)
- `src/claude_agent_sdk/_cli_version.py`: 1-3 (pinned CLI version constant)

**Acceptance Criteria:**
- [ ] The repository `claude-agent-sdk-rust` holds a workspace with members `claude-agent-kit` (library), `claude-agent-kit-macros` (proc-macro, empty), `fake-claude` (binary, `publish = false`), and `xtask` (binary, `publish = false`)
- [ ] A `LICENSE` file holds the GPL version 3 text and CI fails when a dependency carries a license outside an allow list compatible with GPL-3.0-or-later
- [ ] The root manifest sets `rust-version = "1.88"`, `edition = "2024"`, `license = "GPL-3.0-or-later"`, and the `[workspace.lints.clippy]` block with `panic`, `unimplemented`, `dbg_macro` at `deny` and `todo`, `unwrap_used`, `expect_used` at `warn`
- [ ] `clippy.toml` sets `allow-unwrap-in-tests`, `allow-expect-in-tests`, `allow-panic-in-tests` to `true`
- [ ] The library declares `#![deny(missing_docs)]` and `#![deny(unsafe_code)]`
- [ ] The Python SDK is a git submodule at `upstream/claude-agent-sdk-python` pinned to commit `4720d36`, and `THIRD_PARTY_NOTICES` carries its MIT notice
- [ ] CI runs the four quality gates on `ubuntu-latest`, `macos-latest`, and `windows-latest`
- [ ] Given a pull request that introduces `panic!` in library code, when CI runs, then the clippy job fails

#### US-002: Error model
**Description:** As a Rust application developer, I want one typed error enum so that I can match on failure causes without string parsing.

**Priority:** P0
**Size:** S (2 pts)
**Dependencies:** Blocked by US-001

**Python reference** (root `C:/dev/claude-agent-sdk-python`, commit `4720d36`):
- `src/claude_agent_sdk/_errors.py`: 6-7 (`ClaudeSDKError`), 10-11 (`CLIConnectionError`), 14-22 (`CLINotFoundError`), 25-39 (`ProcessError`), 42-53 (`_normalize_result_errors`), 56-117 (`ResultError`), 120-126 (`CLIJSONDecodeError`), 129-134 (`MessageParseError`)
- `src/claude_agent_sdk/_internal/query.py`: 90-115 (`_error_result_text`, source of the `ResultError` message)
- `README.md`: 262-284 (documented error handling)
- `tests/test_errors.py`: 13-112 (tests)

**Acceptance Criteria:**
- [ ] A `#[non_exhaustive]` enum `Error` built with `thiserror` has variants `CliNotFound`, `CliConnection`, `Process { exit_code, stderr }`, `ResultError { message, data, exit_code }`, `JsonDecode { line, source }`, `MessageParse { message, data }`, `ControlTimeout { subtype }`, `Control { message }`, and `Config { message }` (ref: `src/claude_agent_sdk/_errors.py`)
- [ ] `ResultError` exposes `subtype`, `terminal_reason`, and `api_error_status` read from its `data`
- [ ] `Error` is `Send + Sync + 'static`
- [ ] Given a `JsonDecode` error, when it is displayed, then the message includes at most the first 100 characters of the offending line
- [ ] No public signature in the crate mentions `anyhow`

#### US-003: Content blocks and conversation messages
**Description:** As a Rust application developer, I want typed user, assistant, and result messages so that I can read model output without touching raw JSON.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-002

**Python reference** (root `C:/dev/claude-agent-sdk-python`, commit `4720d36`):
- `src/claude_agent_sdk/types.py`: 1207-1286 (content blocks, `ContentBlock`), 1290-1352 (`AssistantMessageError`, `MessageOrigin`), 1386-1422 (`UserMessage`, `AssistantMessage`), 1689-1786 (`DeferredToolUse`, `ModelUsage`, `ResultMessage`), 1918-1926 (`Message`)
- `src/claude_agent_sdk/_internal/message_parser.py`: 37-47 (`_parse_origin`), 50-94 (entry checks), 96-149 (`user`), 151-226 (`assistant`), 310-345 (`result`)
- `tests/test_message_parser.py`: 31-1437 (tests)
- `tests/test_types.py`: 94-150 (tests)

**Acceptance Criteria:**
- [ ] `ContentBlock` has variants `Text`, `Thinking`, `ToolUse`, `ToolResult`, `ServerToolUse`, `ServerToolResult`, and `Unknown(serde_json::Value)`
- [ ] `Message` has variants `User`, `Assistant`, `System`, `Result`, `StreamEvent`, `RateLimit`, `ConversationReset`, and `Unknown(serde_json::Value)`, and is `#[non_exhaustive]`
- [ ] `parse_message(Value)` dispatches on the `type` field by hand, not through a serde tagged enum
- [ ] A user message with string content yields string content; with a list it yields blocks (ref: `src/claude_agent_sdk/_internal/message_parser.py:96-139`)
- [ ] An assistant message maps `message.id` to `message_id` and reads `usage`, `stop_reason`, `error`, `session_id`, `uuid`, `parent_tool_use_id` as optional
- [ ] A result message requires `subtype`, `duration_ms`, `duration_api_ms`, `is_error`, `num_turns`, `session_id` and maps `modelUsage` to `model_usage`
- [ ] Given an assistant message whose `message.content` is not a list, when parsed, then the result is `Error::MessageParse`
- [ ] Given a content block of an unrecognized `type`, when parsed, then it is kept as `ContentBlock::Unknown` with its raw JSON (deliberate improvement over Python, which drops it)

#### US-004: System, task, stream and rate-limit messages with forward compatibility
**Description:** As a Rust application developer, I want every remaining message family typed and unknown ones preserved so that a newer CLI never breaks my program.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-003

**Python reference** (root `C:/dev/claude-agent-sdk-python`, commit `4720d36`):
- `src/claude_agent_sdk/types.py`: 1426-1434 (`SystemMessage`), 1437-1546 (`SystemInitData` and parts), 1549-1578 (`TaskUsage`, statuses, `TERMINAL_TASK_STATUSES`), 1582-1685 (task messages, `MirrorErrorMessage`), 1790-1806 (`StreamEvent`), 1810-1854 (rate limit), 1858-1881 (`ConversationResetMessage`), 1885-1915 (`HookEventMessage`)
- `src/claude_agent_sdk/_internal/message_parser.py`: 69-89 (hook events), 228-308 (`system` and its subtypes), 347-360 (`stream_event`), 362-382 (`rate_limit_event`), 384-395 (`conversation_reset`), 397-401 (unknown type)
- `tests/test_message_parser.py`: 31-1437 (tests)
- `tests/test_rate_limit_event_repro.py`: 16-108 (tests)
- `tests/test_types.py`: 690-777 (tests)

**Acceptance Criteria:**
- [ ] System subtypes `task_started`, `task_progress`, `task_notification`, `task_updated`, `mirror_error`, `hook_started`, and `hook_response` each produce a dedicated typed value with the required fields of `message_parser.py:230-310`
- [ ] Any other system subtype produces a generic system message holding `subtype` and the raw object
- [ ] `task_updated` never fails: a missing `task_id` becomes an empty string and a non-object `patch` becomes an empty object
- [ ] Hook event messages take `hook_event_name` from `hook_event`, then `hook_name`, then `hook_event_name`, then an empty string
- [ ] `rate_limit_event` converts `resetsAt`, `rateLimitType`, `overageStatus`, `overageResetsAt`, `overageDisabledReason` to snake_case fields and keeps the raw info object
- [ ] `stream_event` requires `uuid`, `session_id`, `event`; `conversation_reset` requires `new_conversation_id`, `uuid`, `session_id`
- [ ] Given a message whose `type` is unrecognized, when parsed, then the result is `Message::Unknown` with the raw JSON and no error
- [ ] Given a message with no `type` field, when parsed, then the result is `Error::MessageParse`
- [ ] `TERMINAL_TASK_STATUSES` equals `completed`, `failed`, `stopped`, `killed`

#### US-005: Permission and hook types
**Description:** As a Rust application developer, I want typed hook inputs, hook outputs, and permission results so that my callbacks are checked by the compiler.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-001

**Python reference** (root `C:/dev/claude-agent-sdk-python`, commit `4720d36`):
- `src/claude_agent_sdk/types.py`: 25-27 (`PermissionMode`), 130-282 (permission types, `CanUseTool`), 286-297 (`HookEvent`), 301-509 (hook inputs), 513-752 (hook-specific outputs), 762-817 (async and sync hook outputs), 820-866 (`HookContext`, `HookCallback`, `HookMatcher`), 2371-2388 (`_hooks_to_internal_format`)
- `src/claude_agent_sdk/_internal/query.py`: 118-134 (`async_` and `continue_` renamed on the wire)
- `tests/test_types.py`: 53-91 and 296-687 (tests)
- `tests/test_tool_callbacks.py`: 391-678 (tests)

**Acceptance Criteria:**
- [ ] `PermissionMode` has the six values `default`, `acceptEdits`, `plan`, `bypassPermissions`, `dontAsk`, `auto`
- [ ] `PermissionUpdate` serializes `tool_name` and `rule_content` as `toolName` and `ruleContent` and round-trips through its wire form (ref: `src/claude_agent_sdk/types.py:160-215`)
- [ ] `PermissionResult` is an enum `Allow { updated_input, updated_permissions }` and `Deny { message, interrupt }`
- [ ] `HookEvent` has the ten values `PreToolUse`, `PostToolUse`, `PostToolUseFailure`, `UserPromptSubmit`, `Stop`, `SubagentStop`, `PreCompact`, `Notification`, `SubagentStart`, `PermissionRequest`
- [ ] `HookInput` is an enum selected by `hook_event_name` with one variant per event, each carrying `agent_id` and the other subagent context fields, plus `Unknown(serde_json::Value)`
- [ ] `HookOutput` models the synchronous form and the asynchronous form, serializes `continue_` as `continue` and `async_` as `async`, and offers a typed variant for each of the 21 hook-specific outputs (ref: `types.py:513-728`)
- [ ] `HookMatcher` holds `matcher`, `hooks`, `timeout`, and `on_failure` (`continue` or `block`)
- [ ] Given a hook input with an unrecognized `hook_event_name`, when deserialized, then it is `HookInput::Unknown` and no error is raised

#### US-006: Configuration types and options builder
**Description:** As a Rust application developer, I want a builder for the 50 options so that I can configure a session with typed values.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-005

**Python reference** (root `C:/dev/claude-agent-sdk-python`, commit `4720d36`):
- `src/claude_agent_sdk/types.py`: 30-37 (`SdkBeta`, `SettingSource`, `EffortLevel`), 40-126 (system prompt forms, `TaskBudget`, `ToolsPreset`, `AgentDefinition`), 870-915 (MCP server configs), 924-1099 (MCP status and context usage), 1102-1202 (plugin and sandbox), 2233-2251 (thinking), 2254-2368 (shadow warning, `_configure_can_use_tool`), 2392-2878 (`ClaudeAgentOptions`)
- `src/claude_agent_sdk/_internal/session_store_validation.py`: 18-45 (option combinations refused)
- `src/claude_agent_sdk/_internal/client.py`: 127 (agents serialized with absent fields dropped)
- `tests/test_types.py`: 153-293 and 780-1044 (tests)
- `tests/test_option_warnings.py`: 36-429 (tests)

**Acceptance Criteria:**
- [ ] `ClaudeAgentOptions` exposes all 50 fields of `types.py:2395-2878` except `debug_stderr` (unused upstream), with the same defaults, through `ClaudeAgentOptions::builder()`
- [ ] Unions that accept a string or a structure are enums: `SystemPrompt` (text, preset, custom, file), `Tools` (list or preset), `McpServers` (map, inline JSON, path), `Skills` (list or all)
- [ ] `AgentDefinition`, `McpServerConfig` (stdio, sse, http, sdk), `SandboxSettings`, `ThinkingConfig`, `TaskBudget`, `SdkPluginConfig`, `EffortLevel`, `SettingSource`, and `SdkBeta` exist with the wire names of upstream
- [ ] `AgentDefinition` serializes with absent fields omitted and wire names `disallowedTools`, `mcpServers`, `initialPrompt`, `maxTurns`, `permissionMode`
- [ ] Callback options (`can_use_tool`, `hooks`, `stderr`) are stored as `Arc<dyn Fn ... + Send + Sync>` returning boxed futures, with constructors that accept async closures
- [ ] Setting `can_use_tool` forces `permission_prompt_tool_name` to `stdio`
- [ ] Given both `can_use_tool` and `permission_prompt_tool_name` set, when the options are built, then the result is `Error::Config` (ref: `types.py:2359-2368`)
- [ ] Given `session_store` and `enable_file_checkpointing` both set, when the options are built, then the result is `Error::Config`

---

### EP-002: Transport and process lifecycle

Spawning the CLI, framing the stream, and ending the process without leaks. Release 1.

**Definition of Done:** the stress test of 200 spawn/close cycles, half of them cancelled, leaves zero `fake-claude` processes on Linux, macOS, and Windows, and the command-line tests ported from `tests/test_transport.py` pass.

**Python reference** (root `C:/dev/claude-agent-sdk-python`, commit `4720d36`):
- `src/claude_agent_sdk/_internal/transport/__init__.py`: 8-84 (`Transport` interface)
- `src/claude_agent_sdk/_internal/transport/subprocess_cli.py`: 1-1193 (whole subprocess transport)
- `src/claude_agent_sdk/_internal/_client_app.py`: 18-19 (environment variable names only)
- `tests/test_transport.py`: tests
- `tests/test_subprocess_buffering.py`: tests
- `tests/test_close_cancellation.py`: tests
- `tests/test_run_end_subprocess.py`: 37-118 (protocol stand-in CLI)
- `tests/test_mcp_large_output.py`: 95-167 (environment passthrough tests)

#### US-007: Command-line construction
**Description:** As an SDK maintainer, I want the argument vector built exactly as upstream builds it so that every option reaches the CLI with the same meaning.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-006

**Python reference** (root `C:/dev/claude-agent-sdk-python`, commit `4720d36`):
- `src/claude_agent_sdk/_internal/transport/subprocess_cli.py`: 89-174 (skill name validation), 451-474 (`_build_settings_value`), 476-517 (`_apply_skills_defaults`), 519-744 (`_build_command`)
- `tests/test_transport.py`: 29-2511 (tests), 2803-2827 (`extra_args` tests)
- `tests/test_transcript_mirror.py`: 584-601 (`--session-mirror` flag tests)

**Acceptance Criteria:**
- [ ] The command always starts with `--output-format stream-json --verbose` and ends with `--input-format stream-json`
- [ ] Every flag of `_build_command` (ref: `src/claude_agent_sdk/_internal/transport/subprocess_cli.py:519-744`) is emitted under the same condition and in the same order
- [ ] `--resume`, `--session-id`, `--resume-session-at`, `--resume-drops-turn`, and `--setting-sources` use the `--flag=value` form
- [ ] `--mcp-config` carries `{"mcpServers": {...}}` with the in-process server instance removed from `sdk` entries
- [ ] `--settings` merges the `sandbox` option into the settings object, reading a settings file relative to the working directory and tolerating a byte order mark
- [ ] `extra_args` emits a bare flag for an absent value, `--flag=value` when the value starts with `-`, and two tokens otherwise
- [ ] `thinking` takes precedence over `max_thinking_tokens`
- [ ] The prompt and `agents` never appear in the argument vector
- [ ] Given a skill name containing `(`, `)`, `,`, a control character, or a leading `/`, when the command is built, then the result is `Error::Config` (ref: `subprocess_cli.py:89-174`)
- [ ] Given a plugin whose type is not `local`, when the command is built, then the result is `Error::Config`

#### US-008: CLI discovery, environment and Windows guards
**Description:** As a Rust application developer, I want the SDK to find the CLI and build a safe child environment so that it works after a standard Claude Code install.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-006

**Python reference** (root `C:/dev/claude-agent-sdk-python`, commit `4720d36`):
- `src/claude_agent_sdk/_internal/transport/subprocess_cli.py`: 42-66 (constants), 241-270 (constructor), 272-343 (`_find_cli`, `_find_bundled_cli`), 346-449 (Windows guards), 746-823 (`connect`: environment)
- `src/claude_agent_sdk/_internal/_client_app.py`: 18-19 (client-app variable names)
- `tests/test_transport.py`: 2556-2719, 2722-2800, 2830-2900 (Windows tests)
- `tests/test_client_app.py`: 320-431 (precedence tests)
- `tests/test_mcp_large_output.py`: 95-167 (tests)

**Acceptance Criteria:**
- [ ] Discovery order is: `cli_path` option, the managed install directory of this crate, `PATH`, then the known locations of `subprocess_cli.py:280-301`
- [ ] On Windows only absolute `PATH` entries are searched and the file name is `claude.exe`
- [ ] The child environment is built in this order: inherited variables minus `CLAUDECODE`, `CLAUDE_CODE_ENTRYPOINT=sdk-rs`, `options.env`, then `CLAUDE_AGENT_SDK_VERSION` set to the crate version and not overridable
- [ ] `CLAUDE_CODE_SDK_READS_SESSION_STATE=1` is set unless a variable of that name exists in any letter case
- [ ] `CLAUDE_CODE_ENABLE_SDK_FILE_CHECKPOINTING=true` is set when `enable_file_checkpointing` is on, `PWD` is set when `cwd` is set, and `CLAUDE_AGENT_SDK_CLIENT_APP` is set when `client_app` is given
- [ ] Given no CLI in any location, when discovery runs, then the result is `Error::CliNotFound` with the install instruction text
- [ ] Given a resolved path with a segment ending in `.bat` or `.cmd` on Windows, when connecting, then the result is `Error::CliConnection` and nothing is spawned (ref: `subprocess_cli.py:346-426`)
- [ ] Given `resume` containing any of `& | < > ^ % ! "` or a line break on Windows, when connecting, then the result is `Error::Config`

#### US-009: Process spawn, NDJSON framing and stderr
**Description:** As a Rust application developer, I want a transport that reads and writes whole JSON messages so that upper layers never see partial data.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-007, US-008

**Python reference** (root `C:/dev/claude-agent-sdk-python`, commit `4720d36`):
- `src/claude_agent_sdk/_internal/transport/__init__.py`: 8-84 (`Transport`)
- `src/claude_agent_sdk/_internal/transport/subprocess_cli.py`: 177-235 (`_LineFramer`, `_parse_stdout_line`), 825-870 (spawn and spawn errors), 872-915 (`_handle_stderr`), 1018-1118 (`write`, `end_input`, read loop), 1153-1189 (`_check_claude_version`), 1191-1193 (`is_ready`)
- `tests/test_subprocess_buffering.py`: 28-545 (tests)
- `tests/test_transport.py`: 29-2511 (tests)
- `tests/test_verbatim_prompts.py`: 215-245 (old-CLI warning tests)
- `e2e-tests/test_stderr_callback.py`: end-to-end test

**Acceptance Criteria:**
- [ ] A public `Transport` trait has `connect`, `write`, `read_messages`, `close`, `is_ready`, and `end_input`, and a custom implementation can be injected into `query` and the client
- [ ] `SubprocessTransport` spawns with an argument vector and no shell, piping stdin and stdout, and piping stderr only when a `stderr` callback is set
- [ ] One reader task owns stdout; framing splits on `\n` only and reassembles messages split across reads
- [ ] Blank lines and lines not starting with `{` are skipped
- [ ] Writes are serialized by a lock, and `write` checks readiness, exit status, and the stored exit error inside that lock
- [ ] The stderr callback receives each non-empty line with trailing whitespace removed, and a failing callback does not stop reading
- [ ] The version is read from `claude_code_version` of the first `system/init` message, once, and a missing value logs a warning citing minimum version 2.0.0
- [ ] Given a line starting with `{` that is not valid JSON, when read, then the stream yields `Error::JsonDecode`
- [ ] Given a message longer than `max_buffer_size` bytes (default 1,048,576), when read, then the stream yields `Error::JsonDecode` and stops (the limit counts bytes, a documented divergence from Python)
- [ ] Given a final line without a newline that does not parse, when the stream ends, then it is dropped without error
- [ ] Given a non-zero exit code, when the stream ends, then it yields `Error::Process` with that code
- [ ] Given a working directory that does not exist, when connecting, then the result is `Error::CliConnection` with text `Working directory does not exist: {cwd}`

#### US-010: Bounded shutdown and orphan prevention
**Description:** As an agent platform engineer, I want every CLI process and its descendants to end with my session so that long-running services do not leak processes.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-009

**Python reference** (root `C:/dev/claude-agent-sdk-python`, commit `4720d36`):
- `src/claude_agent_sdk/_internal/transport/subprocess_cli.py`: 68-82 (live children and exit hook), 917-1016 (`close`)
- `src/claude_agent_sdk/_internal/query.py`: 1178-1246 (close ordering of the session)
- `tests/test_close_cancellation.py`: 72-211 (tests)
- `tests/test_transport.py`: 2514-2542 (tests)
- `tests/test_client.py`: 427-464 (tests)

**Acceptance Criteria:**
- [ ] `close()` closes stdin, waits up to 5 s for exit, then terminates and waits up to 5 s, then kills and waits up to 5 s (ref: `subprocess_cli.py:917-1016`)
- [ ] `close()` acquires the write lock with a 5 s limit so a blocked writer cannot hold shutdown
- [ ] `close()` runs to completion when the calling future is dropped, by executing on a detached task
- [ ] The child is spawned with `kill_on_drop(true)`
- [ ] On Unix the child runs in its own process group and escalation signals the whole group
- [ ] On Windows the child runs in a Job Object with kill-on-close, with `CREATE_NO_WINDOW` and `CREATE_NEW_PROCESS_GROUP`
- [ ] On Linux the child receives a parent-death signal
- [ ] All `unsafe` code lives in one module `process::sys` with `#[allow(unsafe_code)]`
- [ ] Given 200 spawn/close cycles where half the callers drop the future mid-run, when the test ends, then zero `fake-claude` processes remain on each CI operating system
- [ ] Given a child that ignores termination, when `close()` runs, then it returns within 20 s and the child is dead

#### US-011: Scriptable fake CLI and mock transport
**Description:** As an SDK maintainer, I want a fake CLI binary and an in-memory transport so that protocol tests run without network on every operating system.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-009

**Python reference** (root `C:/dev/claude-agent-sdk-python`, commit `4720d36`):
- `tests/test_run_end_subprocess.py`: 37-118 (`FAKE_CLI`), 121-162 (helpers)
- `tests/test_close_cancellation.py`: 83-99 and 192-211 (second fake CLI)
- `tests/test_query.py`: 195-264 (`_make_mock_transport` and scripted payloads)
- `tests/test_streaming_client.py`: 26-90 (reactive mock transport)
- `tests/test_tool_callbacks.py`: 22-50 (`MockTransport`)
- `tests/conftest.py`: 7-9 (every async test runs on two backends)

**Acceptance Criteria:**
- [ ] `fake-claude` reads a scenario file given by an environment variable and plays it: emit frames, wait for a stdin frame matching a pattern, reply to control requests, sleep, exit with a code
- [ ] A scenario equivalent to `FAKE_CLI` (ref: `tests/test_run_end_subprocess.py:37-118`) exists and runs on Windows, where the Python version is skipped
- [ ] A public `testing::MockTransport` records written frames and plays scripted frames, behind a `testing` feature
- [ ] `fake-claude` reports a configurable `claude_code_version` in its init frame
- [ ] Given a scenario that waits for a frame that never arrives, when 10 s pass, then `fake-claude` exits with a distinct non-zero code and prints the unmet expectation to stderr

---

### EP-003: Control protocol, query and client

Routing, the control channel in both directions, and the two public entry points. Release 1.

**Definition of Done:** the ported tests of `tests/test_query.py`, `tests/test_streaming_client.py`, `tests/test_tool_callbacks.py`, and `tests/test_run_end_subprocess.py` pass against `MockTransport` and `fake-claude`.

**Python reference** (root `C:/dev/claude-agent-sdk-python`, commit `4720d36`):
- `src/claude_agent_sdk/_internal/query.py`: 1-1257 (router, control protocol, run-end tracking)
- `src/claude_agent_sdk/_internal/client.py`: 28-211 (one-shot wiring)
- `src/claude_agent_sdk/client.py`: 26-631 (`ClaudeSDKClient`)
- `src/claude_agent_sdk/query.py`: 11-132 (`query`)
- `src/claude_agent_sdk/types.py`: 2882-2986 (control request and response types)
- `src/claude_agent_sdk/_internal/_task_compat.py`: 1-188 (asyncio and trio layer, not ported)
- `tests/test_query.py`: tests
- `tests/test_streaming_client.py`: tests
- `tests/test_tool_callbacks.py`: tests
- `tests/test_client.py`: tests
- `tests/test_integration.py`: tests
- `tests/test_verbatim_prompts.py`: tests
- `tests/test_run_end_subprocess.py`: tests

#### US-012: Message router and control request core
**Description:** As an SDK maintainer, I want one router that classifies every incoming frame so that control traffic never reaches the application stream.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-004, US-011

**Python reference** (root `C:/dev/claude-agent-sdk-python`, commit `4720d36`):
- `src/claude_agent_sdk/_internal/query.py`: 151-281 (state), 394-415 (`start`, task spawning), 417-604 (`_read_messages`), 606-733 (`_handle_control_request`), 735-780 (`_send_control_request`), 1164-1176 (`receive_messages`)
- `src/claude_agent_sdk/types.py`: 2955-2986 (control envelopes)
- `tests/test_query.py`: 1830-1876 and 2101-2219 (cleanup and cancel tests)
- `tests/test_transcript_mirror.py`: 654-938 (mirror frames kept out of the stream)

**Acceptance Criteria:**
- [ ] `control_response` frames complete the pending request with the same `request_id`; unknown ids are ignored
- [ ] `control_request` frames each start one handler task, tracked by `request_id`
- [ ] `control_cancel_request` aborts the matching handler, and an aborted handler writes no response
- [ ] `transcript_mirror` frames go to the mirror batcher when one is attached and are never yielded
- [ ] All other frames enter a bounded queue of capacity 100, and a full queue blocks the reader
- [ ] Outgoing control requests use the envelope `{"type":"control_request","request_id":"req_{n}_{8 hex}","request":{...}}` (ref: `src/claude_agent_sdk/_internal/query.py:735-780`)
- [ ] Replies to the CLI use `{"type":"control_response","response":{"subtype":"success","request_id","response"}}` or `{"subtype":"error","request_id","error"}`
- [ ] Given no response within 60 s, when a control request is pending, then it fails with `Error::ControlTimeout` naming the subtype
- [ ] Given an error response, when a control request is pending, then it fails with `Error::Control` carrying the CLI's text
- [ ] Given a control request with an unsupported subtype, when handled, then the reply is an error response and the session continues
- [ ] Given the reader ends with an error, when requests are pending, then every one fails with that error

#### US-013: Initialize handshake and hook dispatch
**Description:** As a Rust application developer, I want my hooks registered at startup and invoked with typed inputs so that I can observe and steer tool use.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-012, US-005

**Python reference** (root `C:/dev/claude-agent-sdk-python`, commit `4720d36`):
- `src/claude_agent_sdk/_internal/query.py`: 118-134 (output renaming), 314-392 (`initialize`), 669-683 (`hook_callback`)
- `src/claude_agent_sdk/_internal/client.py`: 132-135 (initialize timeout)
- `src/claude_agent_sdk/client.py`: 178-181 (initialize timeout), 540-564 (`get_server_info`)
- `src/claude_agent_sdk/types.py`: 2900-2907 and 2915-2919 (request shapes)
- `tests/test_tool_callbacks.py`: 391-678 and 712-992 (tests)
- `tests/test_query.py`: 1543-1631 (tests)
- `e2e-tests/test_hooks.py`: end-to-end test
- `e2e-tests/test_hook_events.py`: end-to-end test

**Acceptance Criteria:**
- [ ] `initialize` is sent before any prompt, with `hooks`, and with `agents`, `excludeDynamicSections`, `systemPromptSnapshot`, `skills` (list only), `forwardSubagentText` (only when true) when set (ref: `query.py:314-392`)
- [ ] Hook callbacks get ids `hook_0`, `hook_1`, ... from one counter per session, and matchers are sent as `{matcher, hookCallbackIds, timeout?, onFailure?}`
- [ ] The initialize timeout is the larger of 60 s and `CLAUDE_CODE_STREAM_CLOSE_TIMEOUT` in milliseconds
- [ ] A `hook_callback` request invokes the callback with the typed input and `tool_use_id`, and its output is sent back with `continue` and `async` wire names
- [ ] The initialize response is stored and returned by `get_server_info`
- [ ] Given a matcher with `on_failure = block` and a response lacking `hook_on_failure_supported`, when initializing, then the result is `Error::CliConnection`
- [ ] Given a matcher with `on_failure = block` and `hooks_applied` false, when initializing, then the result is `Error::CliConnection`
- [ ] Given a hook callback that returns an error, when invoked, then the CLI receives an error response and the session continues
- [ ] Given a `hook_callback` with an unknown `callback_id`, when handled, then the reply is an error response

#### US-014: Tool-permission callback
**Description:** As a Rust application developer, I want to approve, modify, or deny each tool call from code so that my application enforces its own policy.

**Priority:** P0
**Size:** S (2 pts)
**Dependencies:** Blocked by US-012

**Python reference** (root `C:/dev/claude-agent-sdk-python`, commit `4720d36`):
- `src/claude_agent_sdk/_internal/query.py`: 615-667 (`can_use_tool`)
- `src/claude_agent_sdk/types.py`: 225-282 (context and results), 2345-2368 (`_configure_can_use_tool`), 2886-2897 (request shape)
- `tests/test_tool_callbacks.py`: 53-388 (tests)
- `tests/test_query.py`: 1696-1827 (tests)
- `e2e-tests/test_tool_permissions.py`: end-to-end test
- `examples/tool_permission_callback.py`: usage example

**Acceptance Criteria:**
- [ ] A `can_use_tool` request invokes the callback with the tool name, the input, and a context holding `suggestions`, `tool_use_id`, `agent_id`, `blocked_path`, `decision_reason`, `title`, `display_name`, `description`
- [ ] `Allow` replies `{"behavior":"allow","updatedInput": ...}` using the original input when none is supplied, plus `updatedPermissions` when supplied
- [ ] `Deny` replies `{"behavior":"deny","message": ...}` plus `interrupt` when true
- [ ] Given a `can_use_tool` request and no callback configured, when handled, then the reply is an error response with text `canUseTool callback is not provided`
- [ ] Given a callback that returns an error, when handled, then the reply is an error response

#### US-015: Run-end tracking and stdin closing
**Description:** As an agent platform engineer, I want stdin to stay open exactly as long as callbacks may be needed so that background agents keep their hooks and tools.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-013

**Python reference** (root `C:/dev/claude-agent-sdk-python`, commit `4720d36`):
- `src/claude_agent_sdk/_internal/query.py`: 57-87 (deferring task types, ceiling), 466-532 (system, result and turn handling in the read loop), 899-1120 (task ledger, session state, ceiling, `wait_for_result_and_end_input`)
- `tests/test_query.py`: 559-813, 922-1148, 1151-1431 (tests)
- `tests/test_run_end_subprocess.py`: 164-210 (tests against the stand-in CLI)
- `e2e-tests/test_run_end.py`: end-to-end test

**Acceptance Criteria:**
- [ ] With no hooks, no `can_use_tool`, and no in-process MCP server, stdin closes as soon as the input is written
- [ ] Otherwise stdin closes when the run has ended (ref: `query.py:899-1120`)
- [ ] The run ends on a `result` while the session state is unknown or `idle`, or on `idle` after a `result`
- [ ] The run does not end while a task of type `local_agent` or `local_workflow` is started and not in a terminal status
- [ ] `session_state_changed` frames marked `sdk_host_only` update the state and are not yielded
- [ ] An `assistant` or `stream_event` frame with no `parent_tool_use_id` after a result reopens the run
- [ ] A ceiling timer starts after a result when no turn is in progress; it defaults to 600,000 ms, reads `CLAUDE_CODE_PRINT_BG_WAIT_CEILING_MS` from `options.env` first then the process environment, treats `0` as no limit, and caps at 2,147,483,647 ms
- [ ] Given a CLI that sends no session-state frames, when a `result` arrives with no tracked task, then the run ends
- [ ] Given a tracked background task that never reaches a terminal status, when the ceiling expires, then the run ends and stdin closes
- [ ] Given the ceiling variable set to a non-integer, when read, then the default applies

#### US-016: One-shot `query()` and error-result mapping
**Description:** As a Rust application developer, I want a single function returning a stream of messages so that a one-turn task takes under 20 lines of code.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-015

**Python reference** (root `C:/dev/claude-agent-sdk-python`, commit `4720d36`):
- `src/claude_agent_sdk/query.py`: 11-132 (`query`)
- `src/claude_agent_sdk/_internal/client.py`: 34-211 (`process_query`)
- `src/claude_agent_sdk/_internal/query.py`: 90-115 (`_error_result_text`), 137-148 (`stamp_user_message`), 292-312 (`report_mirror_error`), 534-604 (error propagation and end of stream), 1122-1176 (`stream_input`, `receive_messages`)
- `tests/test_query.py`: 331-502, 1434-1540, 2222-2715 (tests)
- `tests/test_client.py`: 13-289 (tests)
- `tests/test_integration.py`: 22-309 (tests)
- `tests/test_verbatim_prompts.py`: 113-155 (tests)
- `e2e-tests/test_error_results.py`: end-to-end test
- `examples/quick_start.py`: usage example

**Acceptance Criteria:**
- [ ] `query(prompt, options)` returns `impl Stream<Item = Result<Message, Error>>`, accepting a string or a stream of user inputs
- [ ] A string prompt is written as `{"type":"user","session_id":"","message":{"role":"user","content"},"parent_tool_use_id":null}`
- [ ] With `verbatim_prompts`, every user frame carries `client_composed: true`
- [ ] Dropping the stream closes the session through the bounded shutdown
- [ ] A synthetic `mirror_error` system message is delivered without blocking and dropped when the queue is full
- [ ] Given an error result followed by a non-zero exit, when the stream ends, then the last item is `Error::ResultError` whose text comes from `errors[]`, then `result`, then a non-success `subtype`, then `API error (HTTP n)`, then `unknown error` (ref: `query.py:90-115`)
- [ ] Given an empty input stream, when `query` runs, then stdin closes immediately
- [ ] Given an input stream that fails, when `query` runs, then stdin is still closed

#### US-017: Interactive client and dynamic control
**Description:** As a Rust application developer, I want a long-lived client so that I can hold a multi-turn conversation and change settings mid-session.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-015

**Python reference** (root `C:/dev/claude-agent-sdk-python`, commit `4720d36`):
- `src/claude_agent_sdk/client.py`: 67-91 (constructor), 93-258 (`connect`), 260-270 (`receive_messages`), 272-306 (`query`), 308-564 (control methods), 566-605 (`receive_response`), 607-621 (`disconnect`)
- `src/claude_agent_sdk/_internal/query.py`: 813-897 (control requests sent)
- `tests/test_streaming_client.py`: 158-1274 (tests)
- `tests/test_client.py`: 292-464 (tests)
- `tests/test_verbatim_prompts.py`: 158-212 (tests)
- `e2e-tests/test_dynamic_control.py`: end-to-end test
- `examples/streaming_mode.py`: usage example

**Acceptance Criteria:**
- [ ] `ClaudeSdkClient` offers `new`, `connect`, `query`, `receive_messages`, `receive_response`, `interrupt`, `set_permission_mode`, `set_model`, `rewind_files`, `reconnect_mcp_server`, `toggle_mcp_server`, `stop_task`, `get_mcp_status`, `get_context_usage`, `get_server_info`, and `disconnect`
- [ ] `receive_response` ends after the first result message
- [ ] `query` uses session id `default` unless one is given and never closes stdin itself
- [ ] Control methods send the subtypes and field names of `query.py:813-897`, including `serverName` for the MCP calls and `user_message_id` for `rewind_files`
- [ ] `disconnect` is safe when never connected and idempotent
- [ ] Given any method other than `connect` or `disconnect` before connecting, when called, then the result is `Error::CliConnection` with text beginning `Not connected`
- [ ] Given `connect` called on a connected client, when called, then the previous session is closed first (deliberate fix: upstream replaces it without closing)

---

### EP-004: In-process MCP tools

Tools defined in Rust and served to the CLI through the control channel. Release 2.

**Definition of Done:** a typed tool defined in an example is called by the real CLI end to end, and the ported tests of `tests/test_sdk_mcp_integration.py` that apply pass.

**Python reference** (root `C:/dev/claude-agent-sdk-python`, commit `4720d36`):
- `src/claude_agent_sdk/__init__.py`: 200-645 (tool definition and server factory)
- `src/claude_agent_sdk/_internal/sdk_mcp_bridge.py`: 1-384 (bridge)
- `src/claude_agent_sdk/_internal/_mcp_compat.py`: 36-154 (mcp 1.x and 2.x differences)
- `src/claude_agent_sdk/_internal/query.py`: 685-703 and 782-811 (`mcp_message` routing)
- `src/claude_agent_sdk/types.py`: 870-915 (MCP server configs)
- `src/claude_agent_sdk/_internal/transport/subprocess_cli.py`: 616-641 (`--mcp-config` serialization)
- `tests/test_sdk_mcp_integration.py`: tests
- `tests/test_mcp_large_output.py`: tests
- `e2e-tests/test_sdk_mcp_tools.py`: end-to-end test
- `examples/mcp_calculator.py`: usage example

#### US-018: Tool definition and schema generation
**Description:** As a Rust application developer, I want to declare a tool from a typed argument struct so that the schema and validation come from my types.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-006

**Python reference** (root `C:/dev/claude-agent-sdk-python`, commit `4720d36`):
- `src/claude_agent_sdk/__init__.py`: 200-274 (`ToolAnnotations`, `SdkMcpTool`), 277-361 (`tool`), 364-456 (schema conversion), 459-468 (`_build_meta`), 517-645 (`create_sdk_mcp_server`)
- `src/claude_agent_sdk/types.py`: 895-910 (`McpSdkServerConfig`)
- `tests/test_sdk_mcp_integration.py`: 1846-2197 (schema tests)
- `examples/mcp_calculator.py`: usage example

**Acceptance Criteria:**
- [ ] `SdkMcpTool::typed::<Args>(name, description, handler)` derives the input schema from `Args: JsonSchema + DeserializeOwned` with `schemars` 1.x
- [ ] `SdkMcpTool::raw(name, description, schema, handler)` accepts a JSON Schema value and a handler taking `serde_json::Value`
- [ ] `ToolAnnotations` accepts the standard hints plus `max_result_size_chars`, sent as `_meta["anthropic/maxResultSizeChars"]` only when set
- [ ] `create_sdk_mcp_server(name, version, tools)` returns an `sdk` server config with version defaulting to `1.0.0`
- [ ] The wire tool is `{name, description, inputSchema, annotations, _meta}` with empty fields omitted
- [ ] Given two tools with the same name in one server, when the server is created, then the result is `Error::Config`
- [ ] Given a raw schema that is not a JSON object, when the tool is created, then the result is `Error::Config`

#### US-019: Native JSON-RPC server and `mcp_message` bridge
**Description:** As a Rust application developer, I want my tools served without any external MCP library so that tool calls work with zero extra dependencies.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-018, US-012

**Python reference** (root `C:/dev/claude-agent-sdk-python`, commit `4720d36`):
- `src/claude_agent_sdk/_internal/query.py`: 685-703 (`mcp_message`), 782-811 (`_handle_sdk_mcp_request`), 1070-1080 (`_has_bidirectional_needs`)
- `src/claude_agent_sdk/_internal/sdk_mcp_bridge.py`: 62-100 (pending response and id reservation), 343-384 (`SdkMcpBridge`)
- `src/claude_agent_sdk/__init__.py`: 471-514 (result conversion), 620-643 (`run_tool`)
- `src/claude_agent_sdk/_internal/_mcp_compat.py`: 112-154 (`build_tool_server`)
- `src/claude_agent_sdk/_internal/transport/subprocess_cli.py`: 616-641 (`--mcp-config`)
- `src/claude_agent_sdk/_internal/client.py`: 101-107 (in-process servers extracted from options)
- `tests/test_sdk_mcp_integration.py`: 52-1845 (tests)
- `tests/test_mcp_large_output.py`: 175-329 (tests)
- `tests/test_query.py`: 331-502 (tests)
- `e2e-tests/test_sdk_mcp_tools.py`: end-to-end test

**Acceptance Criteria:**
- [ ] An `mcp_message` request is routed by `server_name` to the server registered under that options key
- [ ] The dispatcher answers `initialize` (server info, `tools` capability, negotiated protocol version), `ping`, `tools/list`, and `tools/call`
- [ ] A notification is acknowledged with `{"jsonrpc":"2.0","result":{}}`
- [ ] Responses are wrapped as `{"mcp_response": ...}` in a success control response
- [ ] Tool results convert `text` and `image` as is, flatten `resource_link` and text `resource` to text, and drop binary resources with a warning (ref: `src/claude_agent_sdk/__init__.py:477-514`)
- [ ] `is_error` is sent as `isError`
- [ ] Concurrent `tools/call` requests on one server run in parallel
- [ ] Stdin stays open while at least one in-process server is configured
- [ ] Given an unknown tool name, when called, then the result is a tool result with `isError: true` and text `Tool '{name}' not found`
- [ ] Given arguments that fail validation, when called, then the result is a tool result with `isError: true` and text beginning `Input validation error:`
- [ ] Given a handler that returns an error, when called, then the result is a tool result with `isError: true` carrying the error text
- [ ] Given an unknown `server_name`, when routed, then the JSON-RPC reply has error code `-32601`
- [ ] Given an unsupported method, when dispatched, then the JSON-RPC reply has error code `-32601`
- [ ] Given a request id already in flight, when received, then the JSON-RPC reply has error code `-32603`

#### US-020: `rmcp` server bridge
**Description:** As a Rust application developer, I want to plug an existing `rmcp` server in process so that I can reuse servers with resources and prompts.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** Blocked by US-019

**Python reference** (root `C:/dev/claude-agent-sdk-python`, commit `4720d36`):
- `src/claude_agent_sdk/_internal/sdk_mcp_bridge.py`: 1-22 (design notes), 103-340 (`_Session`), 343-384 (`SdkMcpBridge`)
- `src/claude_agent_sdk/_internal/_mcp_compat.py`: 36-109 (message parsing and cancellation)
- `src/claude_agent_sdk/__init__.py`: 580-587 (bring-your-own server)
- `tests/test_sdk_mcp_integration.py`: 1451-1459 (`_CrashingServer` used by the failure tests)

**Acceptance Criteria:**
- [ ] Behind the `rmcp` feature, `McpServerConfig::sdk_rmcp(name, service)` accepts any `rmcp` server handler
- [ ] The bridge serves the handler over an in-memory message channel pair, one session per configured server per query, started by the first message
- [ ] Responses are matched to requests by JSON-RPC id, and late responses are dropped
- [ ] Server-to-client notifications are dropped; a server `ping` is answered; other server requests are refused with `-32601`
- [ ] Closing the query cancels pending requests and waits at most 5 s for the server to stop
- [ ] Given a server that fails after a good handshake, when a later message arrives, then the reply is error `-32603` naming the server and the cause, until a new `initialize` restarts it
- [ ] The crate builds and tests pass with the `rmcp` feature off

#### US-021: `#[tool]` attribute macro
**Description:** As a Rust application developer, I want an attribute that turns an async function into a tool so that declaring a tool takes one line.

**Priority:** P2
**Size:** M (3 pts)
**Dependencies:** Blocked by US-018

**Python reference** (root `C:/dev/claude-agent-sdk-python`, commit `4720d36`):
- `src/claude_agent_sdk/__init__.py`: 277-361 (`tool` decorator, the Python counterpart)
- `examples/mcp_calculator.py`: usage example

**Acceptance Criteria:**
- [ ] Behind the `macros` feature, `#[tool(name = "...", description = "...")]` on an `async fn(args: Args) -> Result<ToolResult, E>` generates a function returning the `SdkMcpTool`
- [ ] The doc comment of the function is used as the description when none is given
- [ ] Annotation hints are accepted as attribute arguments
- [ ] Given the attribute on a non-async function, when compiled, then a compile error names the function and states that it must be async
- [ ] Given a function with more than one argument, when compiled, then a compile error states that exactly one argument struct is expected

---

### EP-005: Sessions

Reading, editing, storing, and resuming conversation transcripts. Release 2.

**Definition of Done:** every session function exported by Python has a Rust counterpart in the surface ledger, the store conformance suite passes on the in-memory store, and resume from a store works against the real CLI.

**Python reference** (root `C:/dev/claude-agent-sdk-python`, commit `4720d36`):
- `src/claude_agent_sdk/_internal/sessions.py`: 1-2252
- `src/claude_agent_sdk/_internal/session_mutations.py`: 1-962
- `src/claude_agent_sdk/_internal/session_resume.py`: 1-623
- `src/claude_agent_sdk/_internal/session_store.py`: 1-194
- `src/claude_agent_sdk/_internal/session_store_validation.py`: 1-45
- `src/claude_agent_sdk/_internal/session_summary.py`: 1-232
- `src/claude_agent_sdk/_internal/session_import.py`: 1-162
- `src/claude_agent_sdk/_internal/transcript_mirror_batcher.py`: 1-219
- `src/claude_agent_sdk/testing/session_store_conformance.py`: 1-327
- `src/claude_agent_sdk/types.py`: 1934-2228 (session types)
- `examples/session_stores/README.md`: store adapter guide
- `tests/test_sessions.py`: tests
- `tests/test_session_mutations.py`: tests
- `tests/test_session_resume.py`: tests
- `tests/test_session_summary.py`: tests
- `tests/test_session_import.py`: tests
- `tests/test_session_helpers_store.py`: tests
- `tests/test_session_store_conformance.py`: tests
- `tests/test_session_store_anyio.py`: tests
- `tests/test_transcript_mirror.py`: tests

#### US-022: Session discovery and metadata
**Description:** As an agent platform engineer, I want to list sessions and read their metadata from disk so that I can build a session picker.

**Priority:** P1
**Size:** L (5 pts)
**Dependencies:** Blocked by US-002

**Python reference** (root `C:/dev/claude-agent-sdk-python`, commit `4720d36`):
- `src/claude_agent_sdk/_internal/sessions.py`: 33-61 (constants), 69-73 (UUID check), 81-184 (hash, sanitizing, config and project directories), 192-251 (field extraction), 259-333 (first prompt), 341-380 (head and tail read), 388-413 (worktrees), 421-511 (metadata rules), 519-731 (listing), 739-818 (`get_session_info`), 1741-1751 (`project_key_for_directory`)
- `src/claude_agent_sdk/types.py`: 2162-2194 (`SDKSessionInfo`)
- `tests/test_sessions.py`: 131-637 and 1548-2006 (tests)
- `tests/test_session_store_conformance.py`: 212-270 (tests)

**Acceptance Criteria:**
- [ ] The config directory is `CLAUDE_CONFIG_DIR` when set and non-empty, otherwise `~/.claude`, normalized to NFC
- [ ] `project_key_for_directory` resolves the real path, normalizes to NFC, replaces every character outside `[a-zA-Z0-9]` with `-`, and for names over 200 units appends `-` and a base-36 hash computed as `h = h*31 + c` on signed 32 bits over the unsanitized path (ref: `src/claude_agent_sdk/_internal/sessions.py:81-114`)
- [ ] Length and hashing iterate UTF-16 code units, isolated in one function
- [ ] For names over 200 units, lookup falls back to the first directory whose name starts with the 200-unit prefix followed by `-`
- [ ] `list_sessions(directory, limit, offset, include_worktrees)` reads only the first and last 65,536 bytes of each transcript and sorts by modification time, newest first
- [ ] Title, summary, tag, git branch, working directory, and creation time are derived by the rules of `sessions.py:421-511`
- [ ] The first prompt is truncated to 200 characters plus an ellipsis and skips the patterns of `sessions.py:52-57`
- [ ] With more than one git worktree, listing covers each worktree's project directory, found through `git worktree list --porcelain` with a 5 s limit, and removes duplicates by session id keeping the newest
- [ ] `get_session_info(session_id, directory)` returns one entry or none
- [ ] These functions are synchronous and documented as blocking
- [ ] Given a transcript whose first line contains `"isSidechain":true`, when listing, then it is skipped
- [ ] Given a session id that is not a UUID, when calling `get_session_info`, then the result is none
- [ ] Given `git` missing or timing out, when listing, then only the directory's own project is used and no error is raised

#### US-023: Validate assumption: project directory naming matches the CLI
**Description:** As an SDK maintainer, I want the directory naming checked against the real CLI so that sessions are found for every path.

**Priority:** P1
**Size:** S (2 pts)
**Dependencies:** Blocked by US-022

**Python reference** (root `C:/dev/claude-agent-sdk-python`, commit `4720d36`):
- `src/claude_agent_sdk/_internal/sessions.py`: 81-114 (`_simple_hash`, `_sanitize_path`), 157-184 (hash mismatch note and prefix fallback), 1741-1751 (`project_key_for_directory`)
- `tests/test_session_store_conformance.py`: 212-270 (tests)
- `e2e-tests/conftest.py`: credentials fixture

**Acceptance Criteria:**
- [ ] A recorded experiment runs the real CLI in three working directories: ASCII under 200 units, a name containing a character outside the Basic Multilingual Plane, and a path over 200 units
- [ ] For each, the directory created by the CLI under `projects/` is compared with the output of `project_key_for_directory`
- [ ] The result and the CLI version are written to `docs/validation/project-dir-naming.md`
- [ ] Given a mismatch on the non-BMP case, when the experiment ends, then the unit-iteration function is corrected and the test ledger notes the divergence from Python
- [ ] Given no API credentials in the environment, when the experiment is attempted, then the story is marked BLOCKED and nothing else depends on it

#### US-024: Conversation reconstruction and subagent transcripts
**Description:** As an agent platform engineer, I want the visible conversation of a session and of its subagents so that I can render history.

**Priority:** P1
**Size:** L (5 pts)
**Dependencies:** Blocked by US-022

**Python reference** (root `C:/dev/claude-agent-sdk-python`, commit `4720d36`):
- `src/claude_agent_sdk/_internal/sessions.py`: 826-928 (reading and parsing entries), 931-1022 (`_build_conversation_chain`), 1025-1245 (parallel tool results), 1248-1359 (visible messages, `get_session_messages`), 1367-1733 (subagent transcripts and sidecars)
- `src/claude_agent_sdk/types.py`: 2198-2228 (`SessionMessage`)
- `tests/test_sessions.py`: 734-1540 and 2039-2393 (tests)
- `e2e-tests/test_parallel_tool_session_reads.py`: end-to-end test
- `e2e-tests/test_subagent_session_reads.py`: end-to-end test

**Acceptance Criteria:**
- [ ] `get_session_messages(session_id, directory, limit, offset)` keeps entries of type `user`, `assistant`, `progress`, `system`, `attachment` with a string `uuid`, and skips corrupt lines
- [ ] The chain is rebuilt by the rules of `sessions.py:931-1022`: terminal entries, nearest user or assistant leaf, preference for leaves that are not sidechain, team, or meta, latest in file wins, walk `parentUuid` to the root with cycle protection
- [ ] `logicalParentUuid` is not followed
- [ ] Assistant entries sharing a `message.id` and tool results the walk missed are reinserted after the last on-chain entry of that response, ordered by timestamp (ref: `sessions.py:1086-1245`)
- [ ] Visible messages are user or assistant entries that are not meta, sidechain, or team; compact summaries are kept
- [ ] `list_subagents` and `get_subagent_messages` read `<project>/<session>/subagents/**/agent-<id>.jsonl` and the `.meta.json` sidecar
- [ ] Given a transcript with a `parentUuid` cycle, when read, then the function returns without looping
- [ ] Given a missing session file, when read, then the result is an empty list

#### US-025: Session mutations: rename, tag, delete, fork
**Description:** As an agent platform engineer, I want to rename, tag, delete, and fork sessions so that users can organize their history.

**Priority:** P1
**Size:** L (5 pts)
**Dependencies:** Blocked by US-022

**Python reference** (root `C:/dev/claude-agent-sdk-python`, commit `4720d36`):
- `src/claude_agent_sdk/_internal/session_mutations.py`: 53-109 (`rename_session`), 112-179 (`tag_session`), 182-229 (`delete_session`), 233-345 (`fork_session`), 348-484 (`_build_fork_lines`), 492-621 (file lookup, title, fork parsing), 624-711 (append), 721-757 (`_sanitize_unicode`)
- `tests/test_session_mutations.py`: 73-817 (tests)

**Acceptance Criteria:**
- [ ] `rename_session` appends `{"type":"custom-title","customTitle","sessionId"}` and `tag_session` appends a `tag` entry, with `None` writing an empty tag
- [ ] Tags are sanitized with NFKC and removal of characters in categories Cf, Co, Cn, repeated up to 10 passes (ref: `src/claude_agent_sdk/_internal/session_mutations.py:737-757`)
- [ ] Appends open the file in append mode without creating it
- [ ] `delete_session` removes the transcript and the `<session>/` directory
- [ ] `fork_session(session_id, directory, up_to_message_id, title)` writes a new transcript created exclusively with mode `0o600`, following the rules of `session_mutations.py:348-484`: new ids for every entry, parents re-linked across skipped `progress` entries, sidechains dropped, cut inclusive at `up_to_message_id`, `forkedFrom` set, a `custom-title` entry appended
- [ ] A fork without a title is named with the derived title followed by ` (fork)`
- [ ] Given a transcript of 0 bytes, when renaming or tagging, then the result is an error and the file is unchanged
- [ ] Given an `up_to_message_id` absent from the transcript, when forking, then the result is an error and no file is created
- [ ] Given a title that is empty after trimming, when renaming, then the result is `Error::Config`

#### US-026: SessionStore trait, in-memory store and conformance suite
**Description:** As an agent platform engineer, I want a storage contract with a reusable test suite so that I can back sessions with my own database.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** Blocked by US-001

**Python reference** (root `C:/dev/claude-agent-sdk-python`, commit `4720d36`):
- `src/claude_agent_sdk/types.py`: 1934-2015 (keys, entries, flush mode), 2028-2153 (`SessionStore`)
- `src/claude_agent_sdk/_internal/session_store.py`: 27-146 (`InMemorySessionStore`)
- `src/claude_agent_sdk/_internal/session_store_validation.py`: 8-45
- `src/claude_agent_sdk/testing/session_store_conformance.py`: 28-327 (conformance suite)
- `tests/test_session_store_conformance.py`: 32-209 (tests)
- `tests/test_session_store_anyio.py`: tests
- `examples/session_stores/README.md`: store adapter guide

**Acceptance Criteria:**
- [ ] `SessionStore` has required async methods `append(key, entries)` and `load(key)`, and optional `list_sessions`, `list_session_summaries`, `delete`, `list_subkeys`
- [ ] The trait uses native async methods returning `Send` futures, with an internal erased wrapper so options hold a shared trait object; `async-trait` is not a dependency
- [ ] Optional methods are declared through `capabilities()` returning a set of flags, and default implementations return an unsupported error
- [ ] `SessionKey` is `{project_key, session_id, subpath?}` with `/`-joined subpaths
- [ ] `InMemorySessionStore` implements every method
- [ ] `testing::run_session_store_conformance(make_store)` checks the 14 contracts of `src/claude_agent_sdk/testing/session_store_conformance.py:83-318`, skipping those for capabilities the store does not declare
- [ ] Given `load` on a key never written, when called, then the result is `None`
- [ ] Given `append` with an empty list, when called, then the stored content is unchanged
- [ ] Given a store that declares `delete`, when a main key is deleted, then its subkeys are deleted too

#### US-027: Store-backed helpers and import
**Description:** As an agent platform engineer, I want the session helpers to work against a store so that history is available without the local disk.

**Priority:** P1
**Size:** L (5 pts)
**Dependencies:** Blocked by US-024, US-025, US-026

**Python reference** (root `C:/dev/claude-agent-sdk-python`, commit `4720d36`):
- `src/claude_agent_sdk/_internal/sessions.py`: 38 (load concurrency), 1754-1897 (store helpers), 1900-2054 (`list_sessions_from_store`), 2057-2252 (other store readers)
- `src/claude_agent_sdk/_internal/session_mutations.py`: 765-962 (`*_via_store`)
- `src/claude_agent_sdk/_internal/session_summary.py`: 30-232
- `src/claude_agent_sdk/_internal/session_import.py`: 29-162
- `tests/test_session_helpers_store.py`: 82-924 (tests)
- `tests/test_session_summary.py`: 57-885 (tests)
- `tests/test_session_import.py`: 59-365 (tests)

**Acceptance Criteria:**
- [ ] Async functions `list_sessions_from_store`, `get_session_info_from_store`, `get_session_messages_from_store`, `list_subagents_from_store`, `get_subagent_messages_from_store` return the same results as their disk counterparts for the same content
- [ ] Async functions `rename_session_via_store`, `tag_session_via_store`, `delete_session_via_store`, `fork_session_via_store` exist, and rename and tag entries carry `uuid` and `timestamp`
- [ ] Store listing loads at most 16 sessions concurrently
- [ ] Listing uses a stored summary only when its time is not older than the session's listing time; stale or missing summaries are refilled after pagination
- [ ] `fold_session_summary` reproduces `src/claude_agent_sdk/_internal/session_summary.py:112`
- [ ] `import_session_to_store(session_id, store, directory, include_subagents, batch_size)` copies a disk session in batches of 500 by default, storing each sidecar as an `agent_metadata` entry
- [ ] Given one session whose load fails during listing, when listing, then that row has an empty summary and the others are returned
- [ ] Given a store without `list_sessions`, when listing from the store, then the result is an unsupported error

#### US-028: Transcript mirroring
**Description:** As an agent platform engineer, I want transcripts copied to my store as the session runs so that no history depends on the local disk.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** Blocked by US-026, US-012

**Python reference** (root `C:/dev/claude-agent-sdk-python`, commit `4720d36`):
- `src/claude_agent_sdk/_internal/transcript_mirror_batcher.py`: 28-35 (limits), 46-219 (`TranscriptMirrorBatcher`)
- `src/claude_agent_sdk/_internal/session_store.py`: 149-194 (`file_path_to_session_key`)
- `src/claude_agent_sdk/_internal/session_resume.py`: 97-127 (`build_mirror_batcher`)
- `src/claude_agent_sdk/_internal/query.py`: 283-312 (batcher attach, `report_mirror_error`), 457-464 (mirror frames), 484-489 (flush before a result), 584-604 (flush at end of stream)
- `src/claude_agent_sdk/_internal/transport/subprocess_cli.py`: 672-673 (`--session-mirror`)
- `src/claude_agent_sdk/_internal/sessions.py`: 130-141 (projects directory with environment override)
- `src/claude_agent_sdk/_internal/client.py`: 158-169 (batcher wiring)
- `tests/test_transcript_mirror.py`: 50-954 (tests)

**Acceptance Criteria:**
- [ ] `--session-mirror` is passed when a store is set
- [ ] Entries from `transcript_mirror` frames are grouped per file path in first-seen order
- [ ] A flush happens before each result message is yielded, when the reader ends, on close, and in the background above 500 pending entries or 1,048,576 pending bytes
- [ ] With flush mode `eager`, both thresholds are 0
- [ ] Flushes are serialized so append order per key is preserved
- [ ] A file path maps to a key relative to the projects directory, honoring `CLAUDE_CONFIG_DIR` from `options.env`
- [ ] Given an append that fails, when flushing, then it is retried up to 3 attempts with waits of 0.2 s then 0.8 s
- [ ] Given an append that exceeds 60 s, when flushing, then it is not retried
- [ ] Given a batch dropped after retries, when it is dropped, then a `mirror_error` system message is delivered
- [ ] Given a file path outside the projects directory, when received, then it is dropped with a warning

#### US-029: Resume from a store
**Description:** As an agent platform engineer, I want to resume a session held only in my store so that conversations survive a change of machine.

**Priority:** P1
**Size:** L (5 pts)
**Dependencies:** Blocked by US-026, US-016

**Python reference** (root `C:/dev/claude-agent-sdk-python`, commit `4720d36`):
- `src/claude_agent_sdk/_internal/session_resume.py`: 55-94 (types, `apply_materialized_options`), 130-200 (`materialize_resume_session`), 209-251 (cleanup with retries), 254-298 (candidate selection), 301-323 (timeout, transcript writing), 326-529 (auth and settings copy, Keychain), 532-623 (subagents, safe subpaths)
- `src/claude_agent_sdk/_internal/client.py`: 42-71 (materialization and cleanup around a query)
- `src/claude_agent_sdk/client.py`: 113-126 and 607-621 (same for the client)
- `src/claude_agent_sdk/_internal/session_store_validation.py`: 18-45
- `tests/test_session_resume.py`: 73-1505 (tests)
- `e2e-tests/test_session_store_resume_settings.py`: end-to-end test
- `e2e-tests/test_truncating_resume.py`: end-to-end test

**Acceptance Criteria:**
- [ ] With a store and `resume` or `continue_conversation` set, the session is written to a temporary directory `claude-resume-*` as `projects/<project_key>/<id>.jsonl` with mode `0o600`
- [ ] The child runs with `CLAUDE_CONFIG_DIR` set to that directory, `resume` set to the id, and `continue` off
- [ ] `.credentials.json` is copied with `claudeAiOauth.refreshToken` removed; on macOS it is read from the Keychain through the `security` command when no config directory, API key, or OAuth token is set (ref: `src/claude_agent_sdk/_internal/session_resume.py:326-454`)
- [ ] `.claude.json`, `settings.json`, and `cowork_settings.json` are copied, minus `enabledPlugins`, `extraKnownMarketplaces`, and `env.CLAUDE_CONFIG_DIR`
- [ ] Subagent transcripts are written when the store declares `list_subkeys`, and `agent_metadata` entries become `.meta.json` files
- [ ] `continue_conversation` picks the newest session whose first entry is not a sidechain
- [ ] The temporary directory is removed after the child exits, with 4 retries 0.1 s apart, and on any failure or cancellation
- [ ] Given a store call exceeding `load_timeout_ms` (default 60,000), when resuming, then the result is an error and the temporary directory is removed
- [ ] Given a subkey containing `..` or an absolute path, when resuming, then it is rejected and not written
- [ ] Given `continue_conversation` and a store without `list_sessions`, when building options, then the result is `Error::Config`

---

### EP-006: Distribution, parity and release

Getting the CLI, proving parity, and staying current. Release 3.

**Definition of Done:** the crate is publishable, both ledgers are at 100%, the end-to-end suite passes against the pinned CLI on Linux, and a CLI version bump produces a release without manual steps.

**Python reference** (root `C:/dev/claude-agent-sdk-python`, commit `4720d36`):
- `scripts/download_cli.py`: 1-403 (CLI download for the wheel)
- `scripts/build_wheel.py`: platform wheels
- `scripts/update_cli_version.py`: CLI version bump
- `src/claude_agent_sdk/_cli_version.py`: 1-3 (pinned CLI version)
- `RELEASING.md`: 1-86 (release flows)
- `.github/workflows/test.yml`: unit, end-to-end and example jobs
- `.github/workflows/auto-release.yml`: release on CLI bump
- `.github/workflows/build-and-publish.yml`: publishing
- `e2e-tests/README.md`: end-to-end guide
- `README.md`: public usage documentation
- `src/claude_agent_sdk/_internal/transport/subprocess_cli.py`: 782-806 (trace context propagation)
- `tests/test_download_cli.py`: tests

#### US-030: CLI installer with checksum verification
**Description:** As a Rust application developer, I want an opt-in installer that fetches a verified CLI so that my program works on a machine without Claude Code.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** Blocked by US-008

**Python reference** (root `C:/dev/claude-agent-sdk-python`, commit `4720d36`):
- `scripts/download_cli.py`: 49-92 (version and installed binary lookup), 118-160 (installer script checks), 172-305 (retry and download), 308-374 (version record and copy)
- `scripts/_cli_version_validation.py`: 30-39 (version format, channel-shaped names)
- `scripts/build_wheel.py`: 91-126 (pinned version read, unpinned versions refused), 158-256 (platform tags and wheel retagging)
- `src/claude_agent_sdk/_cli_version.py`: 1-3
- `src/claude_agent_sdk/_internal/transport/subprocess_cli.py`: 330-343 (bundled binary lookup, counterpart of the managed install directory)
- `tests/test_download_cli.py`: 29-761 (tests)

**Acceptance Criteria:**
- [ ] Behind the `download` feature, `cli::install(version)` fetches `manifest.json` for that version from `downloads.claude.ai/claude-code-releases/` and the binary for the current platform
- [ ] The SHA256 of the binary is compared with the manifest before the file is made executable or moved into the managed install directory
- [ ] Supported platforms are the eight of the official setup page, including the two musl targets
- [ ] The crate exposes `CLI_VERSION`, the version it is tested against
- [ ] The version argument must match `[0-9]+\.[0-9]+\.[0-9]+` with an optional pre-release suffix; channel names are refused
- [ ] The binary is never modified after download
- [ ] Given a checksum mismatch, when installing, then the download is deleted and the result is an error naming both hashes
- [ ] Given no network, when installing, then the result is an error and the managed directory is unchanged
- [ ] Given an unsupported platform, when installing, then the result is an error naming the platform

#### US-031: Manifest signature verification
**Description:** As an agent platform engineer, I want the manifest's signature checked so that a compromised download host cannot serve a forged binary.

**Priority:** P2
**Size:** M (3 pts)
**Dependencies:** Blocked by US-030

**Python reference** (root `C:/dev/claude-agent-sdk-python`, commit `4720d36`):
- `scripts/download_cli.py`: 221-305 (the Python build verifies no checksum or signature; this story has no Python counterpart)

**Acceptance Criteria:**
- [ ] `manifest.json.sig` is fetched and verified against Anthropic's release key, embedded in the crate with its full fingerprint taken from the official setup page
- [ ] Verification runs before any checksum is trusted
- [ ] Versions before 2.1.89, which have no signature, are installable only with an explicit `allow_unsigned` option
- [ ] Given an invalid signature, when installing, then nothing is written and the result is an error
- [ ] Given a missing signature for a version at or after 2.1.89, when installing, then the result is an error

#### US-032: Parity ledgers and upstream drift tool
**Description:** As an SDK maintainer, I want parity measured and upstream changes listed by a command so that the crate never silently falls behind.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** Blocked by US-001

**Python reference** (root `C:/dev/claude-agent-sdk-python`, commit `4720d36`):
- `src/claude_agent_sdk/__init__.py`: 648-824 (`__all__`)
- `src/claude_agent_sdk/_internal/transport/subprocess_cli.py`: 519-744 (flags to diff)
- `src/claude_agent_sdk/types.py`: 2392-2878 (option fields to diff), 2955-2969 (control subtypes to diff)
- `CHANGELOG.md`: upstream change log
- `tests/conftest.py`: 7-9 (backend doubling, which explains tests excluded as not applicable)

**Acceptance Criteria:**
- [ ] `parity/surface.toml` lists each of the 159 names of upstream `__all__` with a Rust path or an exclusion reason
- [ ] `parity/tests.toml` lists each upstream unit test with a Rust test path or an exclusion reason
- [ ] `cargo xtask parity-check` fails when an upstream name or test is absent from a ledger, or when a listed Rust path does not exist
- [ ] `cargo xtask parity-report` prints the percentage implemented, excluded, and pending for both ledgers
- [ ] `cargo xtask upstream-diff <tag>` lists, between the pinned commit and the tag: CLI flags added or removed, option fields, control subtypes, exported names, and tests added or removed
- [ ] CI runs `parity-check` on every pull request
- [ ] Given a submodule bump that adds an export, when `parity-check` runs without a ledger update, then it fails naming the export

#### US-033: End-to-end suite against the real CLI
**Description:** As an SDK maintainer, I want tests that run the real CLI so that protocol assumptions are checked against reality.

**Priority:** P1
**Size:** L (5 pts)
**Dependencies:** Blocked by US-017, US-019

**Python reference** (root `C:/dev/claude-agent-sdk-python`, commit `4720d36`):
- `e2e-tests/conftest.py`: 16-46 (model pin, credentials fixture, marker)
- `e2e-tests/README.md`: how the suite runs
- `e2e-tests/test_agents_and_settings.py`: topic
- `e2e-tests/test_conversation_reset.py`: topic
- `e2e-tests/test_dynamic_control.py`: topic
- `e2e-tests/test_error_results.py`: topic
- `e2e-tests/test_forward_subagent_text.py`: topic
- `e2e-tests/test_hook_events.py`: topic
- `e2e-tests/test_hooks.py`: topic
- `e2e-tests/test_include_partial_messages.py`: topic
- `e2e-tests/test_message_origin.py`: topic
- `e2e-tests/test_parallel_tool_session_reads.py`: topic
- `e2e-tests/test_run_end.py`: topic
- `e2e-tests/test_sdk_mcp_tools.py`: topic
- `e2e-tests/test_session_store_resume_settings.py`: topic
- `e2e-tests/test_stderr_callback.py`: topic
- `e2e-tests/test_structured_output.py`: topic
- `e2e-tests/test_subagent_session_reads.py`: topic
- `e2e-tests/test_tool_permissions.py`: topic
- `e2e-tests/test_truncating_resume.py`: topic
- `e2e-tests/test_verbatim_prompts.py`: topic
- `.github/workflows/test.yml`: 84-108 (end-to-end job gating)
- `src/claude_agent_sdk/_internal/transport/subprocess_cli.py`: 43 (minimum CLI version), 1153-1175 (version warning)

**Acceptance Criteria:**
- [ ] An `e2e` test target covers the 19 topics of upstream `e2e-tests/`, including dynamic control, hooks, tool permissions, in-process MCP tools, structured output, partial messages, and background-agent run end
- [ ] The suite pins the model through `ANTHROPIC_DEFAULT_MODEL`
- [ ] A test asserts the handshake and one full turn succeed with `CLAUDE_CODE_ENTRYPOINT=sdk-rs`
- [ ] A test asserts the native MCP dispatcher is accepted by the CLI: the tool appears as `mcp__<key>__<tool>` and a call returns its result
- [ ] CI runs the suite on Linux after unit tests pass and never on forks
- [ ] Given no credentials, when the suite starts, then it fails with a message naming the missing variable instead of skipping
- [ ] Given a CLI older than 2.0.0, when a session starts, then a warning is logged and the test asserts it

#### US-034: Examples and API documentation
**Description:** As a Rust application developer, I want runnable examples and complete docs so that I can adopt the crate without reading its source.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** Blocked by US-017, US-019

**Python reference** (root `C:/dev/claude-agent-sdk-python`, commit `4720d36`):
- `README.md`: public usage documentation
- `examples/quick_start.py`: example
- `examples/streaming_mode.py`: example
- `examples/hooks.py`: example
- `examples/tool_permission_callback.py`: example
- `examples/mcp_calculator.py`: example
- `examples/agents.py`: example
- `examples/filesystem_agents.py`: example
- `examples/system_prompt.py`: example
- `examples/include_partial_messages.py`: example
- `examples/max_budget_usd.py`: example
- `examples/stderr_callback_example.py`: example
- `examples/setting_sources.py`: example
- `examples/tools_option.py`: example
- `examples/plugin_example.py`: example
- `examples/session_stores/README.md`: store adapter guide

**Acceptance Criteria:**
- [ ] Examples exist for: quick start, interactive client, hooks, tool-permission callback, in-process MCP calculator, agents, system prompt variants, partial messages, budget cap, stderr callback, setting sources, and a session store adapter
- [ ] Every public item has rustdoc, and the crate root documents features, platform support, and the tested CLI version
- [ ] docs.rs builds with all features
- [ ] The README states that end users authenticate with their own credentials and that the crate is not an Anthropic product
- [ ] The README states the license (GPL-3.0-or-later) and that a distributed program linking the crate must be released under a compatible license
- [ ] A migration table maps each Python name to its Rust name
- [ ] Given an example compiled with default features only, when built, then it compiles or is gated by `required-features`

#### US-035: OpenTelemetry trace propagation
**Description:** As an agent platform engineer, I want the CLI's spans to join my trace so that agent work appears in my observability stack.

**Priority:** P2
**Size:** S (2 pts)
**Dependencies:** Blocked by US-009

**Python reference** (root `C:/dev/claude-agent-sdk-python`, commit `4720d36`):
- `src/claude_agent_sdk/_internal/transport/subprocess_cli.py`: 782-806 (trace context injection)
- `pyproject.toml`: 43-45 (optional `otel` dependency)
- `tests/test_transport.py`: 29-2511 (tests, including the trace propagation cases)

**Acceptance Criteria:**
- [ ] Behind the `otel` feature, the active context is injected into the child environment with upper-case keys when a `traceparent` is present (ref: `subprocess_cli.py:785-806`)
- [ ] Inherited `TRACEPARENT` and `TRACESTATE` are removed before the fresh values are written
- [ ] Keys present in `options.env` are never overwritten
- [ ] Given no active span, when connecting, then inherited trace variables are left untouched
- [ ] Given an injection failure, when connecting, then the connection proceeds and a debug log is written

#### US-036: Release pipeline and CLI version tracking
**Description:** As an SDK maintainer, I want releases produced automatically when the CLI moves so that users always have a crate tested against a recent CLI.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** Blocked by US-032, US-033

**Python reference** (root `C:/dev/claude-agent-sdk-python`, commit `4720d36`):
- `RELEASING.md`: 1-86 (automatic and manual release flows)
- `.github/workflows/auto-release.yml`: release triggered by a CLI bump
- `.github/workflows/build-and-publish.yml`: build and publish
- `.github/workflows/publish.yml`: manual release
- `.github/workflows/test.yml`: test jobs that gate a release
- `scripts/update_cli_version.py`: CLI version bump
- `scripts/update_version.py`: SDK version bump
- `src/claude_agent_sdk/_cli_version.py`: 1-3
- `CLAUDE.md`: separation of publishing credentials from jobs that call a model

**Acceptance Criteria:**
- [ ] A scheduled workflow detects a new CLI version, opens a pull request bumping `CLI_VERSION`, and runs unit and end-to-end tests against it
- [ ] A merged bump publishes a patch release through crates.io trusted publishing, with no long-lived registry token stored
- [ ] The publishing job holds no model credentials, and jobs that call a model hold no publishing rights
- [ ] A scheduled workflow runs `upstream-diff` against the newest Python SDK tag and opens an issue listing the changes when any exist
- [ ] The first publish to crates.io happens only after the crate name is confirmed by the owner
- [ ] Given end-to-end tests failing on a new CLI version, when the bump workflow runs, then no release is published and the pull request stays open with the failure summary

## Functional Requirements

- FR-01: The system must spawn the Claude Code CLI with an argument vector and never through a shell.
- FR-02: The system must exchange newline-delimited JSON over the child's stdin and stdout and expose typed messages as a `Stream`.
- FR-03: The system must send `initialize` before any prompt and register hook callbacks in it.
- FR-04: When the CLI sends `can_use_tool`, `hook_callback`, or `mcp_message`, the system must invoke the matching application code and reply on the control channel.
- FR-05: When the CLI sends `control_cancel_request`, the system must abort the matching handler and send no response for it.
- FR-06: The system must keep stdin open while callbacks may still be needed and close it when the run has ended or the ceiling expires.
- FR-07: The system must preserve unknown message types and unknown content blocks as raw JSON.
- FR-08: The system must NOT drop a child process without attempting the bounded shutdown.
- FR-09: The system must NOT execute `.bat` or `.cmd` files on Windows.
- FR-10: The system must serve in-process tools through a native JSON-RPC dispatcher and report tool failures as `isError` results, never as protocol errors.
- FR-11: The system must read, list, rename, tag, delete, fork, import, mirror, and resume session transcripts with the rules of the Python reference.
- FR-12: The system must NOT bundle, modify, or redistribute the CLI binary; the installer must verify a SHA256 checksum before making a download executable.
- FR-13: The system must NOT write an OAuth refresh token to a temporary resume directory.
- FR-14: The system must fail the build of its own CI when an upstream export or test is missing from the parity ledgers.

## Non-Functional Requirements

- **Performance:** parsing and routing at least 50,000 messages of 1 KiB per second on one core in the benchmark; added latency per message under 1 ms at P99; `list_sessions` over 1,000 transcripts of 1 MiB each under 300 ms on a warm cache; message queue capacity 100; default maximum message size 1,048,576 bytes.
- **Security:** 0 shell invocations; `unsafe` confined to 1 module; 100% of installer downloads checksum-verified before use; temporary transcript and credential files created with mode `0o600`; 0 refresh tokens written to temporary directories; 0 long-lived registry tokens in CI.
- **Accessibility:** not applicable to a library with no user interface; the equivalent requirement is 100% of public items documented, enforced by `deny(missing_docs)`.
- **Scalability:** 64 concurrent sessions in one process with no cross-talk between their control channels, verified by test; store listing limited to 16 concurrent loads.
- **Reliability:** `close()` returns within 20 s in the worst case; 0 orphan processes after 200 spawn/close cycles on each of 3 operating systems; control request timeout 60 s; initialize timeout at least 60 s; mirror appends retried up to 3 attempts; resume cleanup retried 4 times.
- **Compatibility:** minimum supported Rust version 1.88; tested on 3 operating systems in CI; tested against the pinned CLI version and warns below CLI 2.0.0.

## Edge Cases & Error States

Systematic coverage of unhappy paths. Evidence shows earlier defect discovery significantly reduces cost (Boehm 1981, NIST 2002).

| # | Scenario | Trigger | Expected Behavior | User Message |
|---|----------|---------|-------------------|--------------|
| 1 | Empty state: no CLI installed | Discovery finds nothing | `Error::CliNotFound`, nothing spawned | "Claude Code not found. Install it or set cli_path." |
| 2 | Empty state: no sessions | Project directory absent | Empty list, no error | None |
| 3 | Loading: slow MCP startup | Initialize exceeds 60 s | Timeout raised from `CLAUDE_CODE_STREAM_CLOSE_TIMEOUT`, else `Error::ControlTimeout` | "Control request timeout: initialize" |
| 4 | Error: CLI exits non-zero after an error result | API failure reported in `result` | Last stream item is `Error::ResultError` with the CLI's text | "Claude Code returned an error result: {text}" |
| 5 | Error: invalid JSON line | Line starts with `{` and does not parse | `Error::JsonDecode` | "Failed to decode JSON: {first 100 chars}" |
| 6 | Error: write after exit | Process already terminated | `Error::CliConnection` | "Cannot write to terminated process (exit code: {n})" |
| 7 | Error: working directory missing | `cwd` does not exist | `Error::CliConnection` | "Working directory does not exist: {cwd}" |
| 8 | Boundary: oversize message | Message exceeds `max_buffer_size` | `Error::JsonDecode`, stream stops | "JSON message exceeded maximum buffer size of {n} bytes" |
| 9 | Boundary: path over 200 units | Long working directory | Hash suffix, then prefix fallback lookup | None |
| 10 | Boundary: full message queue | Consumer slower than CLI | Reader blocks; `mirror_error` dropped with a warning | None |
| 11 | Interrupted flow: consumer drops the stream | Future dropped mid-run | Bounded shutdown runs on a detached task | None |
| 12 | Interrupted flow: CLI cancels a callback | `control_cancel_request` | Handler aborted, no response written | None |
| 13 | Interrupted flow: background task never ends | Task stays non-terminal | Ceiling expires, stdin closes | None |
| 14 | Concurrent modification: two appends to one transcript | Rename and tag at once | Both entries present, append mode, no truncation | None |
| 15 | Concurrent modification: duplicate JSON-RPC id | Id reused while in flight | Second request refused with `-32603` | "Request id {id} is already in flight" |
| 16 | Permission change: unsupported hook guard | `on_failure = block` on an old CLI | `Error::CliConnection` at initialize | "The process answering initialize does not honor on_failure on hook matchers." |
| 17 | External dependency: store outage | `append` fails 3 times | Batch dropped, `mirror_error` message | "{store error text}" |
| 18 | External dependency: store too slow on resume | Load exceeds `load_timeout_ms` | Error, temporary directory removed | "Session store load timed out after {n} ms" |
| 19 | External dependency: download host unreachable | Installer offline | Error, managed directory unchanged | "Could not download Claude Code {version}: {cause}" |
| 20 | Security: checksum mismatch | Tampered or truncated download | File deleted, error | "Checksum mismatch: expected {a}, got {b}" |
| 21 | Security: batch-file CLI on Windows | Path ends in `.cmd` | `Error::CliConnection`, nothing spawned | "Refusing to run a batch script as the Claude Code CLI: {path}" |
| 22 | Forward compatibility: unknown message | Newer CLI | `Message::Unknown` with raw JSON | None |

## Risks & Mitigations

| # | Risk | Probability | Impact | Mitigation |
|---|------|------------|--------|------------|
| 1 | The undocumented protocol changes and breaks the crate | High | High | Unknowns preserved as raw JSON, `capabilities` feature detection, end-to-end suite on every CLI bump (US-033, US-036) |
| 2 | The crate falls behind upstream like existing crates | High | High | Parity ledgers and drift tool with scheduled issues (US-032, US-036) |
| 3 | Orphan processes on Windows or under cancellation | Med | High | Job Object, process groups, detached shutdown, stress test in CI (US-010) |
| 4 | Stdin closed too early or never | Med | High | Dedicated story with the upstream rules and a `fake-claude` scenario (US-015) |
| 5 | Directory naming diverges from the CLI for unusual paths | Med | Med | Validation spike against the real CLI, one isolated function (US-023) |
| 6 | Anthropic ships an official Rust SDK | Low | High | Parity naming and a migration table keep switching cost low; scope is not reduced on speculation |
| 7 | Trademark objection to the crate name | Low | Med | "Claude Agent" is permitted wording; first publish gated on owner confirmation (US-036) |
| 8 | Native MCP dispatcher rejected by a future CLI | Low | Med | `rmcp` bridge as an alternative path (US-020), end-to-end test (US-033) |
| 9 | Scope of 36 stories stalls before a usable release | Med | Med | Three releases; Release 1 alone is a usable SDK without sessions |
| 10 | Signature verification pulls a heavy dependency | Med | Low | Separate P2 story behind the `download` feature (US-031) |
| 11 | GPL-3.0-or-later keeps the crate out of closed-source applications that are distributed, since Rust links statically | High | Med | Accepted owner decision; the README states the licensing consequence so adopters know before they depend on it (US-034) |

## Non-Goals

What this version explicitly does NOT include:

- Features documented only for the TypeScript SDK (`startup`, `prewarm`, `setMcpServers`, `reloadPlugins`, `supportedModels`, `onElicitation`, and similar). Parity target is Python v0.2.182; revisit after Release 3.
- Support for async runtimes other than tokio. The Python asyncio/trio layer has no counterpart.
- Bundling the CLI binary in the crate. Ruled out by the 10 MB crates.io limit and by the redistribution terms.
- A derived client-application label. Upstream disables it in practice (it requires CLI 999.0.0); only the explicit `client_app` option is ported.
- Reference store adapters for Postgres, Redis, and S3 as supported crates. One adapter appears as an example only.
- Forwarding MCP server-to-client requests (sampling, elicitation, roots). Upstream refuses them too.
- Bindings for other languages (C ABI, WebAssembly).
- Any feature offering claude.ai login on behalf of end users.

## Files NOT to Modify

- `upstream/claude-agent-sdk-python/**`: read-only behavioral reference, changed only by moving the submodule pin.
- `C:/dev/claude-agent-sdk-python/**`: the original checkout of the Python SDK stays untouched; work happens only in `claude-agent-sdk-rust`.

## Technical Considerations

Frame as questions for engineering input, not mandates:

- **Architecture:** one library crate with features (`rmcp`, `macros`, `download`, `otel`, `testing`) versus several crates. Recommended: one library plus a proc-macro crate. Engineering to confirm compile-time impact of the default feature set.
- **Process isolation:** running the child in its own process group changes Ctrl+C behavior: the child no longer receives the terminal's interrupt directly, unlike Python. Recommended: isolate and own the lifecycle. Is an opt-out option needed?
- **Data Model:** manual dispatch on `type` then `serde_json::from_value` per variant, versus serde internally tagged enums with an untagged fallback. Trade-off: manual dispatch reproduces the upstream required/optional rules exactly and avoids serde's flatten limitations, at the cost of one intermediate `Value`. Does the benchmark target hold with the intermediate value?
- **API Design:** should callbacks accept `AsyncFn` closures directly or only through constructor helpers that box them? Recommended: helpers, since stored trait objects need boxed futures.
- **Dependencies:** tokio 1.54, serde 1.0.229, serde_json 1.0.154, schemars 1.2, thiserror 2.0, futures 0.3, tracing 0.1, unicode-normalization 0.1, tempfile 3, uuid 1; optional rmcp 3.10, jsonschema 0.61, reqwest, sha2 0.11, opentelemetry 0.33, windows-sys 0.61. Should raw-schema argument validation (`jsonschema`) be a default feature?
- **Blocking I/O:** disk session functions are synchronous like upstream. Should async wrappers using `spawn_blocking` be offered?
- **Migration:** none; new crate. Backward compatibility policy: semantic versioning with `#[non_exhaustive]`, minor bumps for MSRV raises. Rollback plan: yank a broken release and pin `CLI_VERSION` back.

## Success Metrics

| Metric | Baseline (current) | Target | Timeframe | How Measured |
|--------|-------------------|--------|-----------|-------------|
| Python exports covered (of 159) | 0 | 100% implemented or excluded with reason | Month-6 | `cargo xtask parity-report` |
| Python unit tests dispositioned (of 1,171) | 0 | 100% | Month-6 | `cargo xtask parity-report` |
| Release lag behind a wire-changing Python release | 11 months for the most used community crate | 7 days or less | Month-6 | Dates of upstream tag and crate release |
| CLI bump lag | N/A (new) | 48 hours or less | Month-6 | Dates of CLI release and bump merge |
| Orphan processes in the stress test | N/A (new) | 0 on 3 operating systems | Month-1 on Linux, Month-6 on all | CI stress job |
| End-to-end topics passing against the pinned CLI (of 19) | 0 | 19 | Month-6 | CI `e2e` job |
| Public items documented | N/A (new) | 100% | Month-1 | `deny(missing_docs)` |

## Open Questions

- Crate name: the repository is `claude-agent-sdk-rust`, but that crate name is taken on crates.io, as are `claude-agent`, `claude-agent-rs`, and `claude-agent-sdk`. `claude-agent-kit` is a working name. Owner to confirm before US-036 publishes; a crate name cannot be reused once published.
- Does the "never comment code" rule extend to rustdoc on public items? This PRD assumes rustdoc is documentation and requires it. Owner to confirm before US-001.
- License: decided by the owner on 2026-11-14 as GPL-3.0-or-later, with the upstream MIT notice preserved. Remaining question for the owner before the first publish: is keeping closed-source distributed applications out an intended effect, or would a file-level copyleft (MPL-2.0) fit the goal better?
- May a third party's installer download the official binary on the user's machine? No source forbids it and the binary is not redistributed, but no source allows it explicitly. Owner to decide whether to ask Anthropic before US-030 ships.
- Should the child's process-group isolation be the default (US-010), given that it diverges from Python? Engineering to answer during EP-002.
- Should Release 4 target the TypeScript-only features, or wait for Python to adopt them? Owner to decide after Release 3.
[/PRD]
