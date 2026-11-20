# Audit remediation (audit of 2026-11-20)

Source: the parity audit of 2026-11-20 (43 actions), crate 0.1.0 at `cc6f158`,
reference Python SDK v0.2.186 (`69846cd`), CLI 2.1.329. Branch: `audit-remediation`.
The reference stays v0.2.186 for the whole run.

Rule: align with upstream when an action leaves the choice; keep a Rust behavior
only when it is safer and a reviewer would accept it, recorded in
`parity/differences.toml` with its reason and a test that shows it.

Status values: `todo`, `doing`, `done` (fixed, with its proof), `kept` (deliberate
difference, recorded), `blocked` (waiting on the owner).

## Actions

| Id | Impact | Title | Status | Proof / note |
|---|---|---|---|---|
| A01 | blocker | Port upstream #1502 (permission updates), sync to v0.2.186 | done | Pin at v0.2.186 (`upstream-sync`); `to_wire` returns `Result`, 8 upstream tests ported (`tests/types.rs`, `tests/control_protocol.rs`); parity-check and parity-diff pass. Real CLI: `e2e/tool_permissions.rs::a_whole_tool_rule_returned_by_the_callback_is_applied_by_the_cli` fails before the fix and passes after. |
| A02 | blocker | Keep JSON key order (`preserve_order`) | done | `preserve_order` in the library, `shift_remove` (clippy `disallowed-methods` on `Map::remove`), fork/resume lines keep entry order; harness compares key order (`compare.rs` tests), case `key-order`; tests `fork_keeps_the_key_order_of_each_entry_as_upstream_does`, `json_values_reach_the_cli_with_their_keys_in_the_order_written`. Hook output order recorded as `wire.hook-output-key-order*`. |
| A03 | blocker | Cancellation-safe writes to the CLI | done | One writer task per session fed whole frames (`src/control/mod.rs`); fake-claude `stdin_pause`; test `transport.rs::a_dropped_write_never_cuts_a_frame_the_cli_reads` (fails on the old code). Real CLI 2.1.329 exits with code 1 on a torn line (probe in the runs table), so the fix prevents a session loss. |
| A04 | blocker | Survive a lone UTF-16 surrogate escape or deep nesting | done | `framing.rs`: lone surrogate escapes become U+FFFD, nesting read up to 512 levels; unit tests and `transport.rs::a_lone_surrogate_escape_from_the_cli_does_not_end_the_session`; `[behavior.lone-surrogate-escape]`, `[behavior.nesting-depth-limit]`. |
| A05 | major | Differential harness measures what the SDK returns to the app | todo | |
| A06 | major | Detect upstream tests changed in place, drifting refs | done | `python::test_sources` and the `Unit tests changed in place` section of `upstream-diff` with the ledger entry to review; `upstream-diff 4720d36` lists exactly `test_from_dict_to_dict_roundtrip_add_rules` (the #1502 rewrite). `anchor` required on every `ref`, checked against the cited lines; five drifted refs fixed (shadow warning, stdio routing, control frames, mirror frames, pending requests). `tests/fixtures/fold_session_summary.py` regenerates the fixture from the submodule (`--check` passes, byte-identical). Tests in `xtask` (`a_test_rewritten_in_place_is_listed_with_the_rust_tests_to_review`, `a_test_source_holds_its_decorators_and_body_but_no_comment`, anchor cases). |
| A07 | major | Force `"type": "object"` on `SdkMcpTool::raw` schemas | done | `object_schema` in `src/mcp/tool.rs` adds `"type": "object"` or refuses another type; tests `sdk_mcp.rs::raw_schema_without_a_type_is_listed_as_an_object_schema`, `raw_schema_whose_type_is_not_object_is_a_config_error`; real CLI red then green (`e2e/sdk_mcp_tools.rs::a_raw_tool_with_an_empty_schema_is_listed_next_to_the_others`); `[behavior.raw-tool-schema-type]`. |
| A08 | major | Isolate SessionStore panics, always signal reader end | done | `append_with_retry` catches a panic of the store (`src/mirror.rs`); `ReaderFinish` records the end, fails pending requests and sends an error when the reader stops without one (`src/control/mod.rs`). Tests `transcript_mirror.rs::a_store_that_panics_is_reported_and_the_result_is_still_delivered`, `control_protocol.rs::a_panic_in_the_reader_fails_pending_requests_at_once_and_ends_the_stream_with_an_error`, both red on the old code; exclusion `python-exception-guard` removed, the upstream test now maps to them. |
| A09 | major | Keep unknown data on types the user sends | doing | Done: `extra` + `extra_field` on every hook-specific output, `AsyncHookOutput`, `SyncHookOutput` (builder), sandbox and stdio/remote MCP configs; `HookSpecificOutput::Unknown`; tests `types.rs::a_hook_output_for_an_event_the_crate_does_not_model_is_sent_as_written`, `a_hook_output_keeps_the_keys_of_a_known_event_the_crate_does_not_model`, `command.rs::keys_the_crate_does_not_model_reach_the_cli_in_sandbox_and_mcp_settings`. Left: `ToolAnnotations` extra, after the MCP group merges. |
| A10 | major | CLI version warning, private `managed_install_dir` | done | No installer (owner). `warn_if_untested` in `src/transport/subprocess.rs` warns when the init frame reports a version other than `CLI_VERSION` (tests `transport.rs::the_tested_cli_version_does_not_warn`, `a_cli_older_or_newer_than_the_tested_version_warns_once`; `[behavior.untested-cli-version]`); `managed_install_dir` private, still searched before `PATH` (see Product choices); messages say "Claude Code"; `packaging` exclusion reason updated. |
| A11 | major | Evolvable public surface before 1.0 (error design: owner approval) | done | Opaque `Error` + `ErrorKind` (17 kinds), accessors, `is_retryable()` (owner chose this design on 2026-11-20); `#[non_exhaustive]` on every field-carrying variant with constructors; `wire_enum` `Unknown(UnknownValue)` built only by the crate (test `types.rs::a_wire_enum_value_equals_another_exactly_when_their_spellings_do`); `McpSdkServerInstance` holds a private enum of the two server kinds, so the refused-at-connect case is gone. semver-check: see the checks section. |
| A12 | major | Let the CLI stop cleanly when the program ends | done | `Drop for ProcessTree` sends `SIGTERM` to the group and the child is no longer killed on drop (Unix); test `shutdown.rs::dropping_the_runtime_lets_the_process_handle_termination`, red on the old code. The group policy stays (deliberate): its effect on a background server, and `setsid` to escape it, are now written in `src/lib.rs` and `[behavior.process-tree-ended]`. Windows keeps the Job Object hard stop (no console to send a break to). |
| A13 | major | Align public claims with what is measured | doing | Done: README, migration guide, parity page reworded (port of 0.2.186, reviewed and unclassified differences, unit tests only, e2e not listed); claim check extended to README and migration guide with new phrases (`xtask/tests/parity.rs::a_claim_of_parity_in_the_migration_guide_fails_the_check_too`); `src/control/mod.rs` lists only real differences; "Claude CLI" ledger notes removed; ResultError now chains the process error as source (as `__cause__`). Left: the README sentence on what parity-diff compares, after A05 merges. |
| A14 | minor | Propagate the tracing span and OTel context | todo | |
| A15 | minor | Connection error after the CLI died | done | A request after the CLI died is `ErrorKind::ConnectionClosed` with the process error as source; test `control_protocol.rs::a_control_request_after_the_cli_died_is_a_connection_error`. |
| A16 | minor | Typed hook input does not silently fall back to Unknown | done | `tool_response` defaults to null, `duration_ms` accepts any number, `effort` accepts a bare string; a mismatch is logged at debug level. `[behavior.hook-input-shape]`; tests `types.rs::a_hook_input_the_cli_schema_allows_stays_typed` (Unknown on the old code) and `a_hook_input_that_does_not_match_its_event_is_unknown_and_logged`. |
| A17 | minor | `receive_messages` after an unparseable message | todo | |
| A18 | minor | Native tool cancellation | todo | |
| A19 | minor | No `format` validation for draft-04/06/07 | todo | |
| A20 | minor | Typed tool schemas: enums, integral floats, shape | todo | |
| A21 | minor | Tool ergonomics: direct call, stateful `#[tool]` | todo | |
| A22 | minor | Optional store methods are not silently skipped | done | Every optional store method is called; `UnsupportedStoreOperation` means absent; `capabilities()` is only read by the build-time `continue_conversation` check (`[behavior.store-declaration-for-continue]`). Four tests red on the old code (`delete_via_store_deletes_from_a_store_that_implements_delete_without_declaring_it`, ...). |
| A23 | minor | Empty project paths or paths with `..` | done | `canonicalize_path` treats `""` as `.` and resolves `.`/`..` segment by segment like `realpath`; probed against Python on five cases. Tests `an_empty_path_is_the_current_directory`, `dot_segments_in_a_missing_tail_are_resolved` (red before). |
| A24 | minor | Import fails on an unreadable sidecar | done | Strict sidecar read on import (`ErrorKind::Io` on a read error, absent on missing/invalid); reader stays tolerant as upstream. Test `an_unreadable_sidecar_fails_the_import` (red before). |
| A25 | minor | macOS keychain account when USER is unset | done | `sys::current_user_name()` (`getpwuid_r`) and the upstream lookup order USER, LOGNAME, LNAME, USERNAME, user database, `claude-code-user`. Not runnable on macOS here; unit tests on Linux. |
| A26 | minor | Record Rust-only session behaviors | done | Seven `[behavior]` entries with tests (invalid UTF-8, fork line separators, non-object import lines, agent id segment, canonical projects dir, bare `.jsonl` name, mirror timeout message), each probed against Python. |
| A27 | minor | Reconnect with a supplied SubprocessTransport | todo | |
| A28 | minor | Control responses stuck behind unread messages | todo | |
| A29 | minor | Errors: repeated source, placeholder stderr | done | Display no longer repeats the source (`error.rs` test `the_text_does_not_repeat_the_source...`); sources shared by `Arc`, no `duplicate()`; `Error::stderr()` holds an 8 KiB tail when captured (`transport.rs::a_failed_process_keeps_the_last_lines_of_its_captured_error_output`, `[behavior.process-error-stderr]`). |
| A30 | minor | Blocking calls on the async runtime | todo | |
| A31 | minor | Dependency weight, third-party types in the API | todo | |
| A32 | minor | Tests that pass silently or cannot fail | todo | |
| A33 | minor | Wrong or partial test-ledger mappings | todo | |
| A34 | minor | Harden parity-check | todo | |
| A35 | cosmetic | Parse `CLAUDE_CODE_STREAM_CLOSE_TIMEOUT` like `int()` | todo | |
| A36 | cosmetic | Error texts and product name | todo | |
| A37 | cosmetic | MCP wire and macro details | todo | |
| A38 | cosmetic | Extra blocks parsed in user messages | todo | |
| A39 | cosmetic | Empty `cwd` | todo | |
| A40 | cosmetic | Size measures, Unicode version, subpaths | done | Trailing-separator subkey fixed (red before); sizes in UTF-8 bytes, lite serialization, and Unicode version recorded as four `[behavior]` entries with tests. |
| A41 | cosmetic | Ledger inconsistencies | todo | |
| A42 | cosmetic | Idiomatic naming | todo | |
| A43 | cosmetic | Cargo warning on every build (`homepage`) | done | `homepage` removed from the manifests; `published_manifests_carry_the_registry_metadata` now forbids it. |

## Product choices taken without the owner

Each choice the audit did not settle, with the cautious option taken.

- A10: `managed_install_dir` is private as asked, and its directory is still searched before `PATH`, so a CLI placed there by hand keeps working. Removing the lookup would be the other option; it changes behavior for no measured gain.

## Real-CLI runs (budget: 10 USD)

| Date | What | Cost (USD) | Result |
|---|---|---|---|
| 2026-11-20 | A03 probe: `initialize`, then half a `user` frame and stdin closed (CLI 2.1.329, no prompt) | 0.00 | The CLI exits with code 1 and writes `Error parsing streaming input line (type=user, 50 chars): SyntaxError`: a torn frame ends the session. |
| 2026-11-20 | A07 probe: stdio MCP server listing one valid tool and one `"inputSchema": {}`, then `mcp_status` (no prompt) | 0.00 | The server is `connected` with `tools: []` and no error: one bad schema drops every tool. With two valid tools both are listed. Captured in `tests/fixtures/cli/`. |
| 2026-11-20 | `get_context_usage` and `mcp_status` probes (no prompt) | 0.00 | Answers captured in `tests/fixtures/cli/`; `ContextUsageResponse` and `McpStatusResponse` read them; the write-back lost `"type": "stdio"` (fixed). |
| 2026-11-20 | A01: `e2e` `a_whole_tool_rule_returned_by_the_callback_is_applied_by_the_cli`, four runs on the fixed code | about 0.016 (0.0038 per run, two runs not measured) | Three passes, one failure where `first.txt` was missing (the model did not write it; the run printed nothing more). |
| 2026-11-20 | A01: the same test on `cb286bc~1`, before the fix, in a separate worktree | 0.0038 | Fails: the callback runs for both writes, so the CLI did not apply the `Write` rule sent with `ruleContent: null`. |
| 2026-11-20 | `e2e` `a_raw_tool_with_an_empty_schema_is_listed_next_to_the_others`, fixed code then the previous `tool.rs` (handshake and `mcp_status` only) | 0.00 | Passes with the fix; with the previous code the CLI lists `tools: []` for the server. |

Total spent: about 0.02 USD (the CLI runs on a claude.ai login; the amounts are the `total_cost_usd` the CLI reports).

## A11 proposal (error design, approved by the owner on 2026-11-20: opaque `Error` with `kind()`)

Problem (audit A11, A29): `Error::CliConnection { message }` covers about 18
causes, `io::Error` is flattened to text by `duplicate()`, nothing says what
is retryable, every variant exposes its fields (adding one breaks callers),
and `Display` repeats the source.

### Recommended: an opaque `Error` with a `kind()` (the `std::io::Error` shape)

```rust
#[derive(Clone)]                       // cheap: Arc inside, no more duplicate()
pub struct Error(Arc<Inner>);          // Send + Sync + 'static, std::error::Error
struct Inner {
    kind: ErrorKind,
    message: String,                   // Display shows this only
    source: Option<Arc<dyn std::error::Error + Send + Sync>>, // io::Error kept typed
    detail: Detail,                    // exit code, stderr tail, result payload, line...
}

#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ErrorKind {
    CliNotFound,      // no executable found (today CliNotFound)
    Spawn,            // the process could not start: io error, missing cwd,
                      // unreadable settings, refused .bat (today CliConnection)
    NotConnected,     // call before connect / after disconnect (today CliConnection)
    ConnectionClosed, // the CLI exited or closed its pipes before answering:
                      // write to a dead process, output ended, session closed
                      // (today CliConnection; also A15's "control request after
                      // the CLI died")
    Unsupported,      // the CLI cannot do what the options need: on_failure not
                      // honored, hooks not applied (today CliConnection)
    Process,          // the CLI exited with an error code (today Process)
    ResultError,      // the CLI reported an error result, then exited
    JsonDecode,       // a line of output is not JSON
    MessageParse,     // a JSON message lacks an expected part
    ControlTimeout,   // no answer to a control request in time
    Control,          // the CLI answered a control request with an error
    Config,           // invalid or contradictory options (incl. PermissionUpdate)
    SessionNotFound,
    Io,               // transcript or temporary directory I/O
    SessionStore,     // the store failed or timed out
    StoreUnsupported, // the store lacks an optional method the call needs
}
```

Accessors instead of public fields: `kind()`, `exit_code()`, `stderr()` (a
bounded tail of the real stderr, A29), `result()` (`&ResultErrorData`),
`line()` (JsonDecode), `data()` (MessageParse), `control_subtype()`,
`io_kind()`; `source()` returns the typed `io::Error` or store error.
The predicates stay: `is_process()` (Process | ResultError) and
`is_cli_connection()` (CliNotFound | Spawn | NotConnected | ConnectionClosed
| Unsupported), which keep the meaning of the Python `except` clauses.

`is_retryable()`: true when the same call, or a new session, can succeed
without a change of code or options:

| Kind | Retryable |
|---|---|
| ControlTimeout, ConnectionClosed | yes |
| ResultError | when `api_error_status` is 429, 529 or 5xx |
| Process | when no exit code (killed by a signal) |
| Io, Spawn | when the io kind is `Interrupted`, `TimedOut`, `WouldBlock` |
| SessionStore | when the store timed out |
| every other kind | no |

Constructors stay public for what a custom `Transport` or `SessionStore`
returns (`Error::connection_closed`, `Error::not_connected`,
`Error::process`, `Error::json_decode`, `Error::io`, `Error::config`,
`Error::session_store`); the others become `pub(crate)`.

Migration of callers (CHANGELOG, migration.md, one example):

```rust
// 0.1                                        // 0.2
match error {                                 match error.kind() {
    Error::CliConnection { message } => ..,       ErrorKind::ConnectionClosed => ..,
    Error::Process { exit_code, .. } => ..,       ErrorKind::Process => error.exit_code(),
    _ => ..,                                      _ => ..,
}                                             }
```

The other A11 items, which do not depend on this choice and go ahead now:
`#[non_exhaustive]` on every variant with fields (`PermissionResult`,
`ToolContent`, `ThinkingConfig`...), `wire_enum` equality on the wire text
(`Unknown("PreToolUse") == PreToolUse`), and a sealed `McpSdkServerInstance`.

### Alternative: keep the enum, split it

The same variants as `ErrorKind`, each `#[non_exhaustive]` with public
fields, `io::Error` in an `Arc`, `Error: Clone`. Callers keep `match error {
Error::Process { exit_code, .. } => .. }`, but every field is public API
forever, and a variant cannot gain data without a semver-major release.

## Counter-audit

To be run at the end.

## Upstream releases during the run

None seen yet (checked against the `v0.2.186` reference).
