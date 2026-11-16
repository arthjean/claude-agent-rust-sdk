# claude-agent-sdk-rust

A Rust port of the Python Claude Agent SDK, published as the crate `claude-agent-kit`: it runs the Claude CLI as a child process and speaks its JSON control protocol over stdio. The port holds behavioral parity with one pinned upstream version, vendored as the submodule `upstream/claude-agent-sdk-python`. Upstream is the reference for behavior; `parity/` records how each upstream export and test is covered.

## Stop for the owner

Edit code, run the gates, and run every offline test without asking. Three actions wait for the owner's explicit go:

- **Real-CLI runs spend API credits.** `cargo xtask e2e` (about 0.80 USD per full run), the `End-to-end` workflow, the `#[ignore]`d real-CLI tests, and every example under `crates/claude-agent-kit/examples` call the real API. Prove a change with the fake CLI or the mock transport instead.
- **Anything posted on GitHub** in the owner's name: issue, pull request, comment.
- **Lowering a floor**: the 91.5% line coverage floor in `.github/workflows/ci.yml` only goes up, and the lint levels in the root `Cargo.toml` and `clippy.toml` stay as they are.

The owner is the only visible contributor: commits carry no AI attribution and no `Co-authored-by` line.

## Files changed only through their tool

| File | Change it with |
|---|---|
| `upstream/claude-agent-sdk-python/**` | Read-only. Only the submodule pin moves. |
| `.github/workflows/actions.lock` | `gh actions-lock` (see GitHub Actions below) |
| `Cargo.lock` | cargo |
| The table between the `parity:table` markers in `docs/migration.md` | `cargo xtask migration-table` |
| `docs/research/*.md`, `docs/validation/project-dir-naming.md` | Dated records: write new findings to a new file. |
| `tasks/prd-claude-agent-rust-sdk.md` and its status file | Certified and closed: leave as is. |

## Validation

`cargo xtask gates` runs what the CI `gates` job runs: format, clippy, example build, tests, and rustdoc with warnings denied. Run it before reporting any code change as done.

Always pass `--all-features` to `cargo test` and `cargo clippy`. Most integration test files open with `#![cfg(feature = "testing")]` (some also need `macros`, `rmcp`, or `otel`), so a plain `cargo test` compiles them to nothing and passes. For one file: `cargo test -p claude-agent-kit --all-features --test <file>`.

The gates leave out checks that CI still enforces. Run the one that matches the change:

| Change | Check |
|---|---|
| Renamed, moved, or removed a public item or a test; edited `parity/`; moved the submodule pin | `cargo xtask parity-check --deny-pending` (needs `git submodule update --init`; builds every test, so it is slow) |
| New or bumped dependency | `cargo deny check licenses`. The version declared in `Cargo.toml` must be a true minimum: CI resolves direct dependencies to their declared minimums and runs the tests. |
| Dependency change, or a language or std feature that may be newer than Rust 1.88 | `cargo +1.88.0 check --workspace --all-features --locked` |
| `.github/workflows/*.yml` | `gh actions-lock --verify` (extension: `gh extension install github/gh-actions-lock --pin v0.4.1`) |
| `.github/scripts/actions-freshness.sh` | `bash .github/scripts/actions-freshness.test.sh` (offline, needs `jq`) |
| New code paths | Cover them with offline tests: CI fails under 91.5% line coverage, measured on Linux with `fake-claude` and `xtask` left out. |

CI runs the gates on Linux, macOS, and Windows. Gate platform-specific code and tests with `cfg`, and keep both the Unix and the Windows branch compiling.

## Repository policy tests

`crates/claude-agent-kit/tests/workspace.rs` pins repository policy and fails on edits that look unrelated. Read the failing test's name before changing either side. The rules it enforces that a plausible edit breaks:

- **Doc comments only** in `crates/claude-agent-kit/src` and `crates/fake-claude/src`: a line starting with `//` fails. Put the reason for a choice in a `///` or `//!` comment, as the surrounding code does.
- **`unsafe` lives only in `src/process/sys.rs`**. An operating-system call that needs it goes there, with a doc comment that justifies the block.
- **Twelve examples**: the README Examples table and the example files match one to one. Adding an example means adding its row and raising the count in the test.
- **Every Cargo feature** is described in the crate docs of `src/lib.rs` as ``//! - `name`:``.
- **Pinned versions**: the tested CLI version (`CLI_VERSION`), the upstream SDK version (`UPSTREAM_SDK_VERSION`), and the upstream commit are repeated across `src/lib.rs`, `README.md`, `docs/migration.md`, `THIRD_PARTY_NOTICES`, `.github/workflows/e2e.yml`, and the test itself. Move every copy together.
- **README statements**: the authentication, non-affiliation, and license sentences are matched word for word.
- **CI shape**: the gates, the parity job, and the manual-only, Linux-only, never-on-forks end-to-end workflow.

## Parity ledgers

`parity/surface.toml` (upstream exports) and `parity/tests.toml` (upstream tests) are edited by hand. Keys are upstream names, so a Rust-only item or test needs no entry. Each entry holds exactly one of `rust` (the Rust path or paths), `excluded` (a key of `[exclusions]`), or `pending`.

- A Rust test's name is its ledger key. Before renaming or moving a test, search `parity/tests.toml` for it and update the entry. `cargo xtask parity-tests` prints the exact paths the ledger accepts.
- A test compiled on one platform only takes a ` @windows`, ` @unix`, ` @linux`, or ` @macos` suffix in the ledger.
- After editing `parity/surface.toml`, run `cargo xtask migration-table`.
- To move to a newer upstream: `cargo xtask upstream-diff <tag>` lists what changed, then move the pin, port the changes, and add or remove ledger entries until `parity-check --deny-pending` passes.

## Design invariants

- **Layers, top to bottom**: public API (`query`, `ClaudeSdkClient`) → `ControlSession` (`src/control`) → transport (`src/transport`) → process supervision (`src/process`). Each module's `//!` docs state its contract.
- **Unknown wire data is kept, never dropped or rejected.** An unknown message or content block becomes an `Unknown` variant holding the raw JSON, string enums go through `wire_enum!` (which adds `Unknown(String)`), and structs keep extra keys in a flattened `extra` field. New wire types follow the same shape.
- **Public types are `#[non_exhaustive]`**, built with `new()` and setters. All failures go through the one `Error` enum in `src/error.rs`.
- **Deliberate differences from upstream are written down** where they happen: a bullet in the module docs ("differs from upstream in ... deliberate ways") and a `note` on the ledger entry. Anything else that differs from upstream is a bug.
- **The CLI outranks the Python SDK on disk formats.** `src/sessions` reproduces what the CLI writes (project directory names in UTF-16 units, JSONL key order, timestamps, tag normalization), validated against the real CLI in `docs/validation/`. Where the two disagree, match the CLI.

## Tests

- **Mock transport** (`tests/common/mock.rs`, feature `testing`): protocol logic with no process. Timing-sensitive tests use `#[tokio::test(start_paused = true)]`.
- **Fake CLI** (`crates/fake-claude`): a scripted stand-in binary for anything that needs a real child process. Build a scenario inline with `Fake::new(json!({...}))` from `tests/common/mod.rs`; the format is documented at the top of `crates/fake-claude/src/scenario.rs`. Existing tests depend on its exit codes (64, 65, 66) and init-frame keys (`fake_args`, `fake_env`, `fake_pid`): add to them, keep the existing ones.
- **End-to-end** (`tests/e2e/`): `#[ignore]`d, real CLI. Without credentials they fail instead of skipping, by design.
- Name a test as a sentence stating the behavior, for example `follow_up_turn_is_served_while_stdin_stays_open`.

## GitHub Actions

Workflows reference actions by tag, and `.github/workflows/actions.lock` fixes each tag to a commit. To bump actions, run `bash .github/scripts/actions-freshness.sh --apply`, which adopts only releases at least 14 days old. To add one, reference it by tag and run `gh actions-lock`. Either way, review the diff and commit the workflows together with the lockfile.

## Where to look

- `tasks/prd-*.md` with a `-status.json` tracker each: scope, non-goals, hard constraints, and story status. `tasks/prd-parity-hardening.md` is the open one.
- `docs/research/python-sdk-parity.md`: the measured gaps with upstream that the open PRD closes.
- `docs/migration.md`: the user-facing Python-to-Rust mapping.
- `README.md`, Development section: the `cargo xtask` commands.

## Reporting

End a run with the checks you ran and their result, then any CI-only check from the Validation table that applied and that you could not run locally.
