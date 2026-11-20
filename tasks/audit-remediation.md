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
| A05 | major | Differential harness measures what the SDK returns to the app | done | Journal of what each SDK hands the application (messages, method returns and errors, callback arguments, store writes), compared by `xtask/src/differential/journal.rs`; new `[observed]` table; `can_use_tool` returns suggestions in `permission-allow-suggestions` (red with the pre-#1502 `to_wire`), plus cancel, session state, mirror cases; full environment recorded; `--coverage` tells set from observable fields (46 of 47 observable, `load_timeout_ms` listed unobservable). 43 cases match. |
| A06 | major | Detect upstream tests changed in place, drifting refs | done | `python::test_sources` and the `Unit tests changed in place` section of `upstream-diff` with the ledger entry to review; `upstream-diff 4720d36` lists exactly `test_from_dict_to_dict_roundtrip_add_rules` (the #1502 rewrite). `anchor` required on every `ref`, checked against the cited lines; five drifted refs fixed (shadow warning, stdio routing, control frames, mirror frames, pending requests). `tests/fixtures/fold_session_summary.py` regenerates the fixture from the submodule (`--check` passes, byte-identical). Tests in `xtask` (`a_test_rewritten_in_place_is_listed_with_the_rust_tests_to_review`, `a_test_source_holds_its_decorators_and_body_but_no_comment`, anchor cases). |
| A07 | major | Force `"type": "object"` on `SdkMcpTool::raw` schemas | done | `object_schema` in `src/mcp/tool.rs` adds `"type": "object"` or refuses another type; tests `sdk_mcp.rs::raw_schema_without_a_type_is_listed_as_an_object_schema`, `raw_schema_whose_type_is_not_object_is_a_config_error`; real CLI red then green (`e2e/sdk_mcp_tools.rs::a_raw_tool_with_an_empty_schema_is_listed_next_to_the_others`); `[behavior.raw-tool-schema-type]`. |
| A08 | major | Isolate SessionStore panics, always signal reader end | done | `append_with_retry` catches a panic of the store (`src/mirror.rs`); `ReaderFinish` records the end, fails pending requests and sends an error when the reader stops without one (`src/control/mod.rs`). Tests `transcript_mirror.rs::a_store_that_panics_is_reported_and_the_result_is_still_delivered`, `control_protocol.rs::a_panic_in_the_reader_fails_pending_requests_at_once_and_ends_the_stream_with_an_error`, both red on the old code; exclusion `python-exception-guard` removed, the upstream test now maps to them. |
| A09 | major | Keep unknown data on types the user sends | done | `extra` + `extra_field` on every hook-specific output, `AsyncHookOutput`, `SyncHookOutput` (builder), sandbox, stdio/remote MCP configs, and `ToolAnnotations`; `HookSpecificOutput::Unknown`; `McpServerConfig::Raw` so an unmodeled server sits next to SDK servers. Tests `types.rs::a_hook_output_for_an_event_the_crate_does_not_model_is_sent_as_written`, `a_hook_output_keeps_the_keys_of_a_known_event_the_crate_does_not_model`, `command.rs::keys_the_crate_does_not_model_reach_the_cli_in_sandbox_and_mcp_settings`, `command.rs::a_server_config_of_a_kind_the_crate_does_not_model_sits_next_to_an_in_process_server`, `sdk_mcp.rs::an_annotation_key_the_crate_does_not_model_is_listed_and_a_modeled_one_wins`. |
| A10 | major | CLI version warning, private `managed_install_dir` | done | No installer (owner). `warn_if_untested` in `src/transport/subprocess.rs` warns when the init frame reports a version other than `CLI_VERSION` (tests `transport.rs::the_tested_cli_version_does_not_warn`, `a_cli_older_or_newer_than_the_tested_version_warns_once`; `[behavior.untested-cli-version]`); `managed_install_dir` private, still searched before `PATH` (see Product choices); messages say "Claude Code"; `packaging` exclusion reason updated. |
| A11 | major | Evolvable public surface before 1.0 (error design: owner approval) | done | Opaque `Error` + `ErrorKind` (17 kinds), accessors, `is_retryable()` (owner chose this design on 2026-11-20); `#[non_exhaustive]` on every field-carrying variant with constructors; `wire_enum` `Unknown(UnknownValue)` built only by the crate (test `types.rs::a_wire_enum_value_equals_another_exactly_when_their_spellings_do`); `McpSdkServerInstance` holds a private enum of the two server kinds, so the refused-at-connect case is gone. semver-check: see the checks section. |
| A12 | major | Let the CLI stop cleanly when the program ends | done | `Drop for ProcessTree` sends `SIGTERM` to the group and the child is no longer killed on drop (Unix); test `shutdown.rs::dropping_the_runtime_lets_the_process_handle_termination`, red on the old code. The group policy stays (deliberate): its effect on a background server, and `setsid` to escape it, are now written in `src/lib.rs` and `[behavior.process-tree-ended]`. Windows keeps the Job Object hard stop (no console to send a break to). |
| A13 | major | Align public claims with what is measured | done | README, migration guide, parity page reworded (port of 0.2.186, reviewed and unclassified differences, unit tests only, e2e not listed, what parity-diff compares including the application journal); claim check extended to README and migration guide (`xtask/tests/parity.rs::a_claim_of_parity_in_the_migration_guide_fails_the_check_too`); `src/control/mod.rs` lists only real differences; "Claude CLI" ledger notes removed; ResultError chains the process error as source (as `__cause__`). |
| A14 | minor | Propagate the tracing span and OTel context | done | New `src/caller_context.rs`: spawned tasks enter the caller span and, with `otel`, the OTel context at connect; without a global propagator the W3C context and baggage are written per `OTEL_PROPAGATORS` (`[behavior.otel-propagators-names]`). Tests in `tests/caller_context.rs` and `tests/otel.rs`, red before. |
| A15 | minor | Connection error after the CLI died | done | A request after the CLI died is `ErrorKind::ConnectionClosed` with the process error as source; test `control_protocol.rs::a_control_request_after_the_cli_died_is_a_connection_error`. |
| A16 | minor | Typed hook input does not silently fall back to Unknown | done | `tool_response` defaults to null, `duration_ms` accepts any number, `effort` accepts a bare string; a mismatch is logged at debug level. `[behavior.hook-input-shape]`; tests `types.rs::a_hook_input_the_cli_schema_allows_stays_typed` (Unknown on the old code) and `a_hook_input_that_does_not_match_its_event_is_unknown_and_logged`. |
| A17 | minor | `receive_messages` after an unparseable message | done | Aligned with upstream: the client streams end after an error; a new call resumes. Test `client.rs::receive_messages_ends_at_a_message_that_does_not_parse_and_a_new_call_resumes` (ledger entries updated). |
| A18 | minor | Native tool cancellation | done | Aligned: a native tool keeps running after `control_cancel_request`, its id reserved until it answers; `notifications/cancelled` and session close still stop it (shared `SHUTDOWN_GRACE`). Test `sdk_mcp.rs::control_cancel_request_lets_the_tool_finish_and_keeps_its_id_reserved_until_then` (red before). |
| A19 | minor | No `format` validation for draft-04/06/07 | done | `should_validate_formats(false)` for every draft. Test `format_is_an_annotation_only_whatever_the_draft_of_the_schema` (red before on draft-04). |
| A20 | minor | Typed tool schemas: enums, integral floats, shape | done | Enum and flatten variants merged into one root object (no top-level `oneOf`, which the API refuses); integral floats read as integers; combinator visits bounded (100k). `[behavior.typed-tool-schema-shape]`. Tests red before. |
| A21 | minor | Tool ergonomics: direct call, stateful `#[tool]` | done | `SdkMcpTool::call` and `#[tool]` with state (`&S` via `Arc<S>`, or a `Clone` value); UI tests for refused forms. |
| A22 | minor | Optional store methods are not silently skipped | done | Every optional store method is called; `UnsupportedStoreOperation` means absent; `capabilities()` is only read by the build-time `continue_conversation` check (`[behavior.store-declaration-for-continue]`). Four tests red on the old code (`delete_via_store_deletes_from_a_store_that_implements_delete_without_declaring_it`, ...). |
| A23 | minor | Empty project paths or paths with `..` | done | `canonicalize_path` treats `""` as `.` and resolves `.`/`..` segment by segment like `realpath`; probed against Python on five cases. Tests `an_empty_path_is_the_current_directory`, `dot_segments_in_a_missing_tail_are_resolved` (red before). |
| A24 | minor | Import fails on an unreadable sidecar | done | Strict sidecar read on import (`ErrorKind::Io` on a read error, absent on missing/invalid); reader stays tolerant as upstream. Test `an_unreadable_sidecar_fails_the_import` (red before). |
| A25 | minor | macOS keychain account when USER is unset | done | `sys::current_user_name()` (`getpwuid_r`) and the upstream lookup order USER, LOGNAME, LNAME, USERNAME, user database, `claude-code-user`. Not runnable on macOS here; unit tests on Linux. |
| A26 | minor | Record Rust-only session behaviors | done | Seven `[behavior]` entries with tests (invalid UTF-8, fork line separators, non-object import lines, agent id segment, canonical projects dir, bare `.jsonl` name, mirror timeout message), each probed against Python. |
| A27 | minor | Reconnect with a supplied SubprocessTransport | done | `SubprocessTransport` reconnects after `close` with a new process; `[behavior.read-messages-once-per-connection]` replaces the wrong `transport-single-use`. Tests red before. |
| A28 | minor | Control responses stuck behind unread messages | done | Reader keeps routing control frames past a full app queue, buffering up to 16 MiB of unread frames, then stops reading (`[behavior.control-routed-past-unread-messages]`); mirror flush moved off the reader. Four tests red before. |
| A29 | minor | Errors: repeated source, placeholder stderr | done | Display no longer repeats the source (`error.rs` test `the_text_does_not_repeat_the_source...`); sources shared by `Arc`, no `duplicate()`; `Error::stderr()` holds an 8 KiB tail when captured (`transport.rs::a_failed_process_keeps_the_last_lines_of_its_captured_error_output`, `[behavior.process-error-stderr]`). |
| A30 | minor | Blocking calls on the async runtime | done | `src/blocking.rs` (`spawn_blocking`, keeps the span, resumes panics) used by connect, resume, and import. Test `connect_reads_the_settings_without_stalling_a_single_thread_runtime` (FIFO settings), red before. |
| A31 | minor | Dependency weight, third-party types in the API | done | Typed `Usage` (holds the raw JSON, typed accessors, lenient counts); re-exports `indexmap`, `schemars`, `rmcp` with a "Public dependencies" section stating the semver coupling and a doctest deriving `JsonSchema` through the re-export. `jsonschema` stays on by default for parity (upstream validates tool arguments); the docs give the 109 against 57 crate counts. Test `types.rs::usage_keeps_every_key_and_reads_the_documented_counts`. |
| A32 | minor | Tests that pass silently or cannot fail | done | Early returns replaced: `run_end_subprocess.rs` reruns itself in a child with the variables removed; `/usr/local/bin/claude` is a locator field; keychain test fails with a reason; uid test targets `nobody` (root checks uid, gid, no supplementary groups; non-root expects a refused spawn); temp scan filtered by project key; shared `CLAUDE_CONFIG_DIR` set before `main`; e2e harness refuses an empty transcript. Left: four test binaries still set env vars behind a lock or alone. |
| A33 | minor | Wrong or partial test-ledger mappings | done | Four wrong mappings get new tests; about 50 partials strengthened (exact concurrency 16, one append per batch, every sanitizer class, whole `origin`). One real bug found and fixed: the resume config dir is now 0700 (test red before). `:473`/`:534` handled under A18/A20. |
| A34 | minor | Harden parity-check | done | `rust::Tests { listed, ignored }` (`--list --ignored`); an ignored test, a wrong platform suffix, and a non-test function in the textual fallback fail (`xtask/tests/parity.rs::a_ledger_naming_an_ignored_test_fails_the_check`, `a_test_of_another_platform_is_checked_against_the_sources`, `rust::tests::a_declaration_counts_only_under_a_test_attribute`). Parity job is a 3-OS matrix (CI only; not run here). New tests for `can-use-tool-with-stdio` and `timestamp-without-zone`; `user-option-group` handed to the A32 agent with the uid test. |
| A35 | cosmetic | Parse `CLAUDE_CODE_STREAM_CLOSE_TIMEOUT` like `int()` | done | `parse_python_int` shared; a value `int()` rejects keeps 60 s with a warning (`[behavior.malformed-stream-close-timeout]`). Test red before. |
| A36 | cosmetic | Error texts and product name | done | "Claude Code" everywhere (docs, logs, crate descriptions, AGENTS.md); the two on_failure errors now use the upstream text verbatim; Python-vocabulary messages kept on purpose (verbatim upstream), stated in the `error` module docs. |
| A37 | cosmetic | MCP wire and macro details | done | Wire matches upstream (description always, `annotations: {}`), `-32602` for non-object args and incomplete `initialize` (probed against Python), macro keeps relative doc indentation and takes `crate = "..."`. |
| A38 | cosmetic | Extra blocks parsed in user messages | done | User messages read only text, tool_use, tool_result; the rest is `ContentBlock::Unknown`. Test red before. |
| A39 | cosmetic | Empty `cwd` | done | Empty `cwd` is unset; empty path canonicalizes to `.`. Tests red before. |
| A40 | cosmetic | Size measures, Unicode version, subpaths | done | Trailing-separator subkey fixed (red before); sizes in UTF-8 bytes, lite serialization, and Unicode version recorded as four `[behavior]` entries with tests. |
| A41 | cosmetic | Ledger inconsistencies | done | Case `budget-as-integer` and `[wire.budget-integer-spelling]` (replaces the unclassified entry that said no case could show it); exponent entry reworded; `test_option_is_captured_at_connect` moved to `python-aliasing`; the Rust driver builds agents with the setters a program calls; the tools-object translation is kept and explained (Python writes `--tools default` for any object). 43 cases match. |
| A42 | cosmetic | Idiomatic naming | done | `get_` prefix dropped (`server_info`, `mcp_status`, `context_usage`, with `doc(alias)`), `ToolResult::with_error_flag`; option structs `ListSessionsOptions`, `SessionMessagesOptions`, `ForkSessionOptions`, `ImportSessionOptions` for the nine session functions with optional parameters, defaults tested against upstream keyword defaults; migration guide maps keyword arguments to setters. |
| A43 | cosmetic | Cargo warning on every build (`homepage`) | done | `homepage` removed from the manifests; `published_manifests_carry_the_registry_metadata` now forbids it. |

## Product choices taken without the owner

Each choice the audit did not settle, with the cautious option taken.

- A14: names in `OTEL_PROPAGATORS` other than `tracecontext`, `baggage`, and `none` (b3, jaeger) are ignored when no global propagator is installed (`[behavior.otel-propagators-names]`).
- A20: the variants of an enum are merged into one object schema instead of a top-level `oneOf`, which the Claude API refuses; the schemars shape (nullable types, bounds, title) is kept (`[behavior.typed-tool-schema-shape]`).
- A21: no form of `#[tool]` on an `impl` block; state is passed as the first argument instead.
- A22: a store that implements `list_sessions` without declaring it is still refused for `continue_conversation`, the one check that runs before the store can be called (`[behavior.store-declaration-for-continue]`).
- A28: up to 16 MiB of unread messages are buffered while control traffic keeps flowing; past it the reader stops, as upstream's pipe does (`[behavior.control-routed-past-unread-messages]`).
- A35: a `CLAUDE_CODE_STREAM_CLOSE_TIMEOUT` that Python's `int()` rejects keeps 60 s with a warning, where upstream fails the connect (`[behavior.malformed-stream-close-timeout]`).
- A40: sizes stay in UTF-8 bytes and the tag sanitizer uses the Unicode version of the Rust crates, matching what the CLI writes (four `[behavior]` entries).
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
| 2026-11-20 | Full `cargo xtask e2e` against CLI 2.1.329 on HEAD `9036340` (55 tests, `--test-threads=4`) | about 0.80 (the documented cost of a run; the suite does not print its total) | 54 pass. `session_store_resume_settings::a_store_resume_applies_the_user_settings` fails by design here: it runs the CLI under an empty config directory and needs `ANTHROPIC_API_KEY` or `CLAUDE_CODE_OAUTH_TOKEN`, which a claude.ai login does not provide (before A32 it returned early and passed without checking anything). |
| 2026-11-20 | A04 probe: a `user` frame whose text holds the escape `\ud800`, with `--replay-user-messages` and `--model haiku`, process killed after the echo | under 0.01 (killed before the answer) | The CLI echoes the escape as written (`"content":"Reply with one word: \\ud800 ok"`): a lone surrogate escape does reach the SDK, which before A04 failed the whole session on it. |
| 2026-11-20 | Counter-audit probe: a `user` frame holding `café`, same flags as the A04 probe, killed after the echo | under 0.01 (killed before the answer) | The transcript the CLI wrote holds the raw bytes `caf\xc3\xa9` and no `\u00e9`: the CLI writes raw UTF-8, which settles `[unclassified.raw-utf8-in-transcripts]` as `[behavior.transcript-raw-utf8]`. |

Total spent: about 0.84 USD (the CLI runs on a claude.ai login; the amounts are the `total_cost_usd` the CLI reports).

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

Five fresh agents, one per audit scope (protocol and process, types and parser, MCP, sessions and stores, public surface with docs, parity tooling, and CI), each with the brief, the code, and upstream, none with these notes. Every finding below was checked again before it was accepted: by reading both sides and, where it changes behavior, by a test that fails before the fix.

| Scope | Finding | Severity | Outcome |
|---|---|---|---|
| protocol | `Error::is_retryable` was false for a CLI killed by a signal (the transport records `-signal`, never no code) | major | Fixed (`99d18fc`), `shutdown.rs::a_cli_killed_by_a_signal_gives_a_retryable_process_error`, red before |
| protocol | Invalid UTF-8 on the CLI output is replaced, upstream ends the session; not in the ledger | minor | Recorded `[behavior.cli-output-invalid-utf8]` with a fake-CLI test (`emit_raw` takes bytes) |
| protocol | A control request without `request` or `subtype` is answered, upstream sends nothing; the field-error texts differ; not in the ledger | minor | Recorded `[behavior.malformed-control-request-body]`, and the `error.rs` docs name the exception to "upstream's text, word for word" |
| protocol | A non-string `callback_id` lost its value in the error; the framing `trim` kept U+001C to U+001F | cosmetic | Fixed, with tests |
| types | Two control requests sharing an id: neither was answered | minor | Fixed (`2d16385`), red before |
| types | An optional field of the wrong type reads as absent; not in the ledger | minor | Recorded `[behavior.optional-field-wrong-type]` |
| types | Unmodeled keys lost on `PermissionUpdate`, content blocks, and the user, assistant, result, and stream messages | minor | Fixed (`a45f9e7`, `8ae3424`): `extra` fields, and `message_extra` for the nested `message` |
| MCP | `#[tool]` with a state argument named like the function did not compile | major | Fixed (`1cff073`), compile error before |
| MCP | Integral-float coercion chose the wrong variant of an untagged enum and lost fields | major | Fixed (`7acc33a`): the arguments are read as sent first; red before |
| MCP | A cancelled call answered `-32800`, the Python `mcp` package answers `0` | minor | Aligned (`79a59b2`) |
| MCP | An `rmcp` request with a float id was never answered | minor | Fixed: acknowledged as a notification, as upstream; red before |
| MCP | Doc comments of enum variants dropped by the schema merge | minor | Fixed: listed in the tag's description |
| MCP | `#[tool]` named `fn r#type` tool `r#type` | minor | Fixed: `unraw` |
| MCP | `#[doc = include_str!(..)]` gave a misleading error; envelope (`jsonrpc`, `params`), integer `protocolVersion`, `-32602` text and `data` | cosmetic | Fixed and probed against upstream; the unknown-method code stays the recorded `[wire]` difference |
| sessions | A transcript line with a lone surrogate or nesting over 128 levels cut the history before it | major | Fixed (`5f2b6c1`), red before; `[behavior.transcript-line-json-reading]` |
| sessions | `fork_session` looped forever on a cycle of `progress` parents (upstream too) | minor | Fixed, `[behavior.fork-progress-parent-cycle]` (`python-defect`) |
| sessions | `trim` kept U+001C to U+001F that `str.strip()` removes | minor | Aligned |
| sessions | The 5 s limit of `git worktree list` did not hold when a grandchild kept stdout open | minor | Fixed, test at 1 s |
| sessions | A dropped `close` lost the batch the mirror was sending | minor | Fixed: sent from a task of its own, as upstream shields it; red before |
| sessions | Import read the whole transcript into memory | minor | Streams 1 MiB blocks |
| sessions | `\ud83d` left escaped in a title; no `SessionStore` for `Arc` | cosmetic | Fixed |
| sessions | `[unclassified]` entries left | minor | All four settled: `PATH` unset and dotted store paths aligned, duplicate summaries counted once as upstream, raw UTF-8 recorded as `rust-follows-cli` after a CLI probe |
| surface | Stale ledger note, README snippet missing `serde` and `schemars`, two ledger tests that did not show their entry, page grammar | minor | Fixed |
| surface | Clippy warnings not denied, MSRV on Linux only, e2e checkout with persisted credentials and no check of the CLI version | minor | Fixed (`1569299`) |
| surface | `ResultErrorData: Deref<Target = Map>` | cosmetic | Replaced by `get` and `as_map` |

Listed, not changed, with the reason:

- `parity-check` proves that each ledger entry names an existing item or test, not that the test asserts the upstream behavior; the 943 mappings were reviewed by hand in A33, and the 3 largest fan-in groups were spot-checked again.
- The journal of `parity-diff` leaves out null-valued keys and maps each type by hand; the page now says so, and the frames sent to the CLI are compared whole.
- The advisory check runs weekly and not on pull requests, by a policy the repository tests pin, so that a new advisory does not block unrelated work.
- `parity-diff` runs on Linux only: the Python driver is not set up on the macOS and Windows runners.
- `e2e.yml` installs the CLI with the official `install.sh`, which publishes no checksum to pin; the job now fails when the installed version is not the pinned one.
- `the_process_runs_as_the_requested_user` checks the group switch only when run as root, which GitHub runners are not.
- `init_data()` is all-or-nothing (documented); a direct `serde` serialization of a hook output whose `extra` repeats a modeled key duplicates it, which the wire path never does.
- `SystemInitMcpServer` and `TaskUsage` keep no `extra`: upstream drops those keys too, and `SystemMessage::data()` keeps the whole frame.
- A `#[tool]` function that does not return a `Result` gets the compiler's error, not a dedicated one: a syntactic check would refuse result aliases.
- `ResultErrorData::errors()` allocates: it trims and drops blank entries.
- Not covered by the counter-audit: the Windows branches, a `CLAUDE_CONFIG_DIR` that is not UTF-8, JSON integers above `u64` in a fork or import.

## Upstream releases during the run

None: `v0.2.186` (2026-11-19) is still the newest release of the Python SDK (`gh release list`, checked on 2026-11-20 at the end of the run).
