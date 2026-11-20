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
| A11 | major | Evolvable public surface before 1.0 (error design: owner approval) | todo | |
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

To be written.

## Counter-audit

To be run at the end.

## Upstream releases during the run

None seen yet (checked against the `v0.2.186` reference).
