# Parity audit 2 remediation (audit of 2026-11-20, crate 0.2.0)

Source: the second parity audit of 2026-11-20 (1 blocker, 7 major, 34 minor,
30 detail actions, 5 unconfirmed points), crate 0.2.0 at `9848300`, reference
Python SDK v0.2.186 (`69846cd`), CLI 2.1.329. Branch: `parity-audit-2`, one
commit per fix, nothing pushed.

Rule: every point is checked again in both repositories before acting. A fix
comes with a test that fails before it. A kept difference is recorded in
`parity/differences.toml` with a reason a reviewer would accept. Public renames
and the prerequisites A1 to A4 are the owner's: a decision note in
`tasks/parity-audit-2-decisions.md` instead of a change.

Status values: `todo`, `doing`, `done` (fixed, with its proof), `kept`
(deliberate difference, recorded), `refuted` (the point does not hold, with the
evidence), `owner` (decision note written, waiting on the owner).

## Prerequisites and blocker

| Id | Title | Status | Proof / note |
|---|---|---|---|
| A1 | CLI must know `sdk-rs` | owner | Decision note "B1 and A1": keep `sdk-rs` or send `sdk-cli` until the CLI knows `sdk-rs`, with measured effects. |
| A2 | Crate name, copyright, license | owner | Decision note: `claude-agent-sdk` taken on crates.io by a personal account (repo link 404), `claude-agent-sdk-rs` taken, `anthropic-claude-agent-sdk` free; name, license, rights to settle before the next publication. |
| A3 | README billing statement | owner | Not pinned by `workspace.rs`; removal recommended, kept until the owner decides; cause documented in `docs/validation/entrypoint-label.md`. |
| A4 | Maintenance of the tooling, e2e cadence | owner | Decision note with measured sizes and the e2e cadence proposal. |
| B1 | CLI does not know `sdk-rs`: ledger entry wrong; never send `sdk-py` | done | Rechecked in the 2.1.329 binary: 0 occurrence of `sdk-rs`; SDK switch `sdk-ts\|sdk-py\|sdk-cli`. Measured offline (real CLI, empty config, placeholder key, closed-port base URL, nothing spent): `init.agents` gains `claude-code-guide` with `sdk-rs` only. `[wire.entrypoint-label]` reason rewritten with the effects; dated record `docs/validation/entrypoint-label.md`; e2e test `a_session_with_the_sdk_rs_entrypoint_lists_the_agent_a_python_session_does_not` (not run: real API). `sdk-py` never sent; `sdk-cli` interim in the decision note. |

## Major

| Id | Title | Status | Proof / note |
|---|---|---|---|
| M1 | In-process MCP server follows `mcp` 1.x, upstream installs 2.x | done | Reference moved to `mcp` 2.x (what `pip install` resolves, what upstream main CI requires): no `experimental`, string `protocolVersion`, `-32602` before initialize, `-32601`+`data`, `-32800`, results shaped by `src/mcp/surface.rs` (schema keys first in byte order, unknown keys dropped) for native and rmcp. Measured with probes of the Python SDK under mcp 1.30.0 and 2.3.0. Harness pinned to mcp 2.3.0 (`requirements.in`, `HELD_MAJOR_LINES`); cases `mcp-protocol-edges`, `mcp-tool-content`, `mcp-cancel`, red before; 46 cases match under 2.3.0. Kept: annotation extras (`[behavior.tool-annotation-extra-keys]`). `docs/parity.md` lists the mcp 1.x answers. Entries `mcp-unknown-method`, `mcp-before-initialize` removed (aligned). |
| M2 | Incomplete hook input becomes `Unknown` silently; example fails open | done | Base fields and `tool_use_id` lenient (absent or null is empty), warn log, `HookInput::tool_name()`/`tool_input()` read typed and `Unknown`, `hooks` example denies what it cannot check. `types.rs::a_hook_input_missing_metadata_the_callback_does_not_need_stays_typed` (Unknown before, as the audit probe showed), `the_tool_of_a_hook_input_is_read_even_when_the_input_is_unknown`; `[behavior.hook-input-shape]` rewritten. |
| M3 | Message types do not implement `Serialize` | done | New `src/message_writer.rs`: `Serialize`/`Deserialize` on `Message`, the 7 message types, `ContentBlock`, the 6 block types, `MessageOrigin`, `ToolResultContent` (Serialize only on `UserContent`, `RateLimitInfo`, `DeferredToolUse`), in the frame form, inverse of the parser; system messages and rate limits overlay changed typed fields on the kept frame. `message_serde.rs` (6 tests: round trip of 16 frames of every type, alone and in `Message`; unreadable values written back until changed; built messages; errors). `[behavior.message-serialization]` (asdict gives fields, Rust gives the frame), migration guide section. |
| M4 | In-process MCP tools run outside the caller context | done | MCP calls spawned through `caller_context::spawn` (`src/mcp/server.rs`); `caller_context.rs::in_process_tools_run_in_the_span_of_the_caller_that_connected`, red before. |
| M5 | Process group gets SIGKILL with no grace at normal exit | done | Group policy kept (owner decision A12 of the first audit); Unix sweep is now SIGTERM, 50 ms polls with `killpg(group, 0)`, SIGKILL after 5 s only if a member is left (never once empty); Windows unchanged (no signal), documented. `[behavior.process-tree-ended]` and crate docs rewritten. `shutdown.rs::descendants_left_at_exit_get_sigterm_before_sigkill` (background server with a TERM trap), red before. Opt-out option left to the decision note. |
| M6 | No pinned CLI; reserved dir never filled; "newer" warning per session | owner | Warning once per process per path and version (`transport.rs::an_untested_cli_warns_once_per_process_not_once_per_session`, red before: 3 warnings); reserved dir documented in README, migration guide, and `find_cli` as the pin location; `[behavior.untested-cli-version]` updated. Installer (download protocol of install.sh measured: manifest.json + SHA-256) designed in the decision note, waits on A1. |
| M7 | `serde_json/preserve_order` forced on the whole dependency graph | done | Measured first: without the feature 9 tests break (tool schema and transcript key order, which the harness compares at every depth). So `serde_json/preserve_order` moved to a library feature `preserve-order`, on by default, which dependents can turn off; the crate compiles and passes `--no-default-features --features testing` (removal through `json_map::RemoveKey`, `retain`-based, order kept in both representations; order tests gated). `workspace.rs::serde_json_key_order_is_a_feature_of_the_library_its_dependents_can_turn_off`, red on the old manifest. MSRV 1.88 both ways, package-check ok, actions-lock ok (semver job lists the feature). Own ordered JSON type left to the owner (decision note). |

## Minor

| Id | Title | Status | Proof / note |
|---|---|---|---|
| m1 | Panicking prompt stream leaves stdin open | done | `catch_unwind` around `input.next()` in `stream_input`; `query.rs::a_prompt_stream_that_panics_closes_stdin_and_the_query_ends`, red before (stream stalled). |
| m2 | `disconnect()` cannot interrupt a stuck `connect()` | todo | |
| m3 | `is_retryable()` false for `terminal_reason == "api_error"` without status | done | `terminal_reason == "api_error"` with no status is retryable; `error::tests::an_api_error_result_without_a_status_is_retryable_as_upstream_documents`, red before. |
| m4 | `continue_conversation` refused for a store that implements `list_sessions` undeclared | todo | |
| m5 | Wrongly typed optional field is lost | done | Modeled keys carry what they must hold (`Reads` tables in `message_parser.rs`); a non-null value the typed field cannot hold stays in `extra`/`message_extra` (frames, bodies, blocks). System messages and rate limits already keep the whole frame raw. `message_parser.rs::an_optional_value_the_crate_cannot_read_is_kept_in_extra`, red before; `[behavior.optional-field-wrong-type]` updated. Not covered: a `modelUsage` entry that is not an object (still skipped) and wrongly typed counters inside `ModelUsage` (read as 0). |
| m6 | `task_progress` null/non-object `usage` read as zeros | done | Required `usage` read through `required_object`, so null or a scalar is `Invalid field ... (expected dict, got ...)` under `[behavior.strict-required-fields]`; the frame stays in the error data. `message_parser.rs::a_task_progress_usage_that_is_not_an_object_is_an_error_not_zeros`, red before (zeros). |
| m7 | `close()` does not close stdin when the write lock times out | done | After the 5 s lock wait, a `Notify` makes the blocked writer drop the pipe (upstream order). Through the client the writer task is aborted on close, so only a direct `SubprocessTransport` writer was affected. `transport.rs::close_ends_stdin_even_while_a_write_is_blocked_on_a_full_pipe` (fake CLI), red before (no `stdin_eof`, 10.3 s). |
| m8 | Derived `Debug` prints secrets of options | done | Manual `Debug` (exhaustive destructuring, so a new field fails to compile until printed) for `ClaudeAgentOptions`, `McpStdioServerConfig`, `McpRemoteServerConfig`; keys kept, values `<redacted>`; inline JSON settings redacted, a path printed. `options.rs::debug_output_names_secrets_without_printing_them`, red before; `[behavior.debug-redacts-secrets]`. |
| m9 | Control request timeout does not cover the write | done | One `timeout` around write and wait; `[behavior.control-timeout-covers-write]`; `control_protocol.rs::a_control_request_whose_write_never_completes_times_out`, red before. |
| m10 | No `async with` equivalent; drop closes on a detached task | todo | |
| m11 | `user` option drops supplementary groups | done | The switch moved into one `pre_exec` closure (setgroups when privileged, setgid, setuid), groups from `getgrouplist` before the fork; registered before the parent-death closure so Linux keeps that signal. Python only does `setreuid` (child keeps root groups): `[behavior.run-as-user-groups]`. Root branch of `transport.rs::the_process_runs_as_the_requested_user` now expects `id -G nobody` (expected no group before); `sys::tests::the_groups_of_a_user_are_the_ones_the_group_database_lists_for_it`. Root path not run here (no root). |
| m12 | macOS: no parent-death signal, no exit cleanup | done | Registry of live groups (`process::sys::imp::exit_registry`) and a `libc::atexit` hook that sends SIGTERM to each (try_lock, never blocks), on every Unix. `shutdown.rs::a_program_that_exits_without_closing_gives_its_sessions_sigterm` (host leaves via `std::process::exit`, a background server traps TERM), red before even on Linux (pdeathsig reaches the direct child only); lib test of the registry. `python-atexit-registry` exclusion removed: both upstream tests now mapped; crate docs, `[behavior.process-tree-ended]` and the ledger note fixed. macOS run not available here (U5). |
| m13 | Subagent read via store: id check too broad, applied with `list_subkeys` | done | Check moved to the direct path, separators and NUL only; ledger entry rewritten; `session_store_helpers.rs::a_store_that_lists_its_subkeys_finds_any_agent_id_by_its_last_segment`, red before. |
| m14 | Non-UTF-8 `CLAUDE_CONFIG_DIR` falls back to `~/.claude` | done | `var_os`, kept byte for byte; no home gives no directory (`[behavior.no-home-directory]`); `sessions.rs::a_config_dir_whose_name_is_not_utf8_is_read_as_python_reads_it` (Linux), red before (listed `~/.claude`). |
| m15 | Mirror real-path alias only when `projects/` exists at start | done | `is_dir()` condition removed; `mirror::tests::real_path_frames_are_mirrored_before_the_projects_directory_exists`, red before. |
| m16 | Windows drive guard on subpaths accepts ASCII letters only | done | Any second character `:` is a drive, as `ntpath.splitdrive` (probed on Python 3.15: `1:foo`, `é:foo`, `::x`); `resume::tests::a_subpath_with_any_drive_prefix_is_rejected_as_ntpath_splitdrive_does`, red before. |
| m17 | `SdkMcpTool::typed` accepts argument types that fail every call | done | A typed tool is marked never readable when its schema describes no object, it takes arguments, and `{}` does not deserialize; `create_sdk_mcp_server` refuses it with `Error::Config` (signature of `typed` unchanged). `sdk_mcp.rs::a_typed_tool_whose_arguments_cannot_be_an_object_is_a_config_error` (String, Vec, unit enum refused; Value, (), HashMap accepted), red before; `[behavior.typed-tool-unreadable-arguments]`; rustdoc of `typed` and `create_sdk_mcp_server`. |
| m18 | `can_use_tool` shadowed warning not filterable, emitted before validation | done | Target `claude_agent_rust_sdk::can_use_tool_shadowed` and field `code = CLAUDE_SDK_CAN_USE_TOOL_SHADOWED` (the TypeScript code); emitted after `for_session()` passes, as upstream validates first. `transport.rs::the_shadow_warning_has_its_own_target_and_code_so_it_can_be_filtered_alone` and `options_that_fail_validation_at_connect_do_not_warn_first`, both red before; test capture records target and code; ledger entry and rustdoc updated. |
| m19 | MCP schema validation: two unrecorded differences, one wrong entry | done | Probed: Python jsonschema 4.26 downloads a remote `$ref` (DeprecationWarning, the local server got the request). New `[behavior.remote-schema-ref]` (deliberate, SSRF) and `[behavior.validation-feature]`; `full-schema-detection` rust text fixed (type added, other types refused). The cfg(not) test became `raw_tool_arguments_are_validated_only_with_the_jsonschema_feature`, compiled and run both ways (CI runs no-default-features). |
| m20 | Error texts "word for word" claim false; test mapped without note | todo | |
| m21 | Parsing: unrecorded differences, structs without `extra` marked same kind | todo | |
| m22 | Control protocol: three unrecorded Rust robustness behaviors | todo | |
| m23 | README and migration guide overstate what `surface.toml` covers | todo | |
| m24 | Four ledger entries inexact or weakly supported | todo | |
| m25 | MCP: native close path untested, weak mapping, 3 differential cases | todo | |
| m26 | Control protocol: six test mappings weaker than upstream | todo | |
| m27 | Sessions fixtures always compact JSON; upstream writes spaced | todo | |
| m28 | Client: four upstream facts not checked in Rust | todo | |
| m29 | Stores: no key-reordering store; adapter example never run through conformance | todo | |
| m30 | Three upstream examples missing, one thinner | todo | |
| m31 | Nothing runs the real CLI automatically; e2e parity not checked by a tool | todo | |
| m32 | Public types without constructors | todo | |
| m33 | `build()` rewrites a public field | todo | |
| m34 | Flat public surface; hand-written MCP dispatcher | todo | |

## Details

| Id | Title | Status | Proof / note |
|---|---|---|---|
| D1 | Non-string `request_id` gets no answer | todo | |
| D2 | Handler with a reused id survives `close()` | todo | |
| D3 | Callback panic text replaced by a fixed message | todo | |
| D4 | `HookOutput::to_wire` falls back to `{}` on serialization failure | todo | |
| D5 | Run end decided before the mirror flush | todo | |
| D6 | `receive_messages()` checks the connection at call time | todo | |
| D7 | `ClaudeSdkClient::query` error docs wrong | todo | |
| D8 | `server_info()` returns an `Option` never `None`; no `options()` accessor | todo | |
| D9 | `"agentId": null` and absent key differ in chain rebuild | todo | |
| D10 | `str::trim` instead of Python `strip()` in three places | todo | |
| D11 | Duplicate ids in a store listing page: one filled | todo | |
| D12 | Float `created_at` refused by the session summary | todo | |
| D13 | Mirror drops a non-object entry without a trace | todo | |
| D14 | macOS: `CLAUDE_CONFIG_DIR=""` reads the Keychain | done | Keychain read only when the variable is unset in the options and the process, as upstream `caller_config_dir is None`; `resume::tests::an_empty_config_dir_variable_of_the_process_keeps_the_keychain_out_as_upstream`. Home part of D15 done with m14. |
| D15 | Sync `remove_dir_all` in a `Drop`; no home reads `./.claude` | todo | |
| D16 | Conformance takes the first summary instead of the last | todo | |
| D17 | `ToolContent::Resource.mime_type` never settable or sent | todo | |
| D18 | CLI discovery: empty `PATH` looks for `./claude`; exec bit instead of access | todo | |
| D19 | `#[allow]` without reason; `Message` is 808 bytes | todo | |
| D20 | `syn` 2 and 3 built together with `macros` | todo | |
| D21 | `IndexMap` in public fields couples semver | todo | |
| D22 | `UnknownError`/`UnknownReason` next to `Unknown`; errors naming Python types | todo | |
| D23 | `deny.toml` without `[bans]`/`[sources]`; `curl \| bash` in the e2e job | todo | |
| D24 | Three allowed licenses no dependency uses | todo | |
| D25 | Exclusion reasons to reword (`derived-client-app`, `python-typing`) | todo | |
| D26 | Linux: cancelling `connect()` during spawn leaves an unsupervised child | todo | |
| D27 | `agent-sdk/0.2.0` overlaps Python's 0.2.x numbering | todo | |
| D28 | Module docs without "differs from upstream" bullets; `SessionStore` doc omits obligations | todo | |
| D29 | `Error::data()` only for `MessageParse` | todo | |
| D30 | Process exit error kept as text | todo | |

## Unconfirmed points

| Id | Point | Status | Proof / note |
|---|---|---|---|
| U1 | A tool reading `/dev/tty` is stopped (SIGTTIN) since the CLI has its own process group | todo | |
| U2 | OTel context carried by tracing spans not propagated to the CLI | todo | |
| U3 | `claude-code-guide` agent in the init message of a Rust session | done | Proven: offline run of CLI 2.1.329 lists `claude-code-guide` in `init.agents` with `sdk-rs`, not with `sdk-py`, `sdk-cli`, or no label (docs/validation/entrypoint-label.md). |
| U4 | A tag read after a store reorders keys is correct | todo | |
| U5 | Real orphan on macOS | todo | |

## Findings the audit missed

| Id | Finding | Status | Proof / note |
|---|---|---|---|

## Checks

