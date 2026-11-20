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
| A01 | blocker | Port upstream #1502 (permission updates), sync to v0.2.186 | done | Pin at v0.2.186 (`upstream-sync`); `to_wire` returns `Result`, 8 upstream tests ported (`tests/types.rs`, `tests/control_protocol.rs`); parity-check and parity-diff pass. Real-CLI proof: pending. |
| A02 | blocker | Keep JSON key order (`preserve_order`) | done | `preserve_order` in the library, `shift_remove` (clippy `disallowed-methods` on `Map::remove`), fork/resume lines keep entry order; harness compares key order (`compare.rs` tests), case `key-order`; tests `fork_keeps_the_key_order_of_each_entry_as_upstream_does`, `json_values_reach_the_cli_with_their_keys_in_the_order_written`. Hook output order recorded as `wire.hook-output-key-order*`. |
| A03 | blocker | Cancellation-safe writes to the CLI | todo | |
| A04 | blocker | Survive a lone UTF-16 surrogate escape or deep nesting | todo | |
| A05 | major | Differential harness measures what the SDK returns to the app | todo | |
| A06 | major | Detect upstream tests changed in place, drifting refs | todo | |
| A07 | major | Force `"type": "object"` on `SdkMcpTool::raw` schemas | todo | |
| A08 | major | Isolate SessionStore panics, always signal reader end | todo | |
| A09 | major | Keep unknown data on types the user sends | todo | |
| A10 | major | CLI version warning, private `managed_install_dir` | todo | |
| A11 | major | Evolvable public surface before 1.0 (error design: owner approval) | blocked | Error design proposed below, waiting for the owner; the non-error items go ahead. |
| A12 | major | Let the CLI stop cleanly when the program ends | todo | |
| A13 | major | Align public claims with what is measured | todo | |
| A14 | minor | Propagate the tracing span and OTel context | todo | |
| A15 | minor | Connection error after the CLI died | todo | |
| A16 | minor | Typed hook input does not silently fall back to Unknown | todo | |
| A17 | minor | `receive_messages` after an unparseable message | todo | |
| A18 | minor | Native tool cancellation | todo | |
| A19 | minor | No `format` validation for draft-04/06/07 | todo | |
| A20 | minor | Typed tool schemas: enums, integral floats, shape | todo | |
| A21 | minor | Tool ergonomics: direct call, stateful `#[tool]` | todo | |
| A22 | minor | Optional store methods are not silently skipped | todo | |
| A23 | minor | Empty project paths or paths with `..` | todo | |
| A24 | minor | Import fails on an unreadable sidecar | todo | |
| A25 | minor | macOS keychain account when USER is unset | todo | |
| A26 | minor | Record Rust-only session behaviors | todo | |
| A27 | minor | Reconnect with a supplied SubprocessTransport | todo | |
| A28 | minor | Control responses stuck behind unread messages | todo | |
| A29 | minor | Errors: repeated source, placeholder stderr | todo | |
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
| A40 | cosmetic | Size measures, Unicode version, subpaths | todo | |
| A41 | cosmetic | Ledger inconsistencies | todo | |
| A42 | cosmetic | Idiomatic naming | todo | |
| A43 | cosmetic | Cargo warning on every build (`homepage`) | done | `homepage` removed from the manifests; `published_manifests_carry_the_registry_metadata` now forbids it. |

## Product choices taken without the owner

Each choice the audit did not settle, with the cautious option taken.

## Real-CLI runs (budget: 10 USD)

| Date | What | Cost (USD) | Result |
|---|---|---|---|

Total spent: 0.00 USD.

## A11 proposal (error design, waiting for the owner)

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
