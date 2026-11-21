# claude-agent-rust-sdk

A Rust port of the Python Claude Agent SDK, published as the crate `claude-agent-rust-sdk`: it runs the Claude Code CLI as a child process and speaks its JSON control protocol over stdio. The port holds behavioral parity with one pinned upstream version, vendored as the submodule `upstream/claude-agent-sdk-python`. Upstream is the reference for behavior; `parity/` records how each upstream export and test is covered.

## Stop for the owner

Edit code, run the gates, and run every offline test without asking. Four actions wait for the owner's explicit go:

- **Publishing is permanent.** A pushed `v*` tag starts `release.yml`, which publishes both crates to crates.io, where a version is never overwritten or deleted. `cargo xtask release prepare` and `cargo xtask release tag` only change the local checkout (`release tag` makes the release commit and tag); the push, `cargo publish`, and `cargo yank` are the owner's. `RELEASING.md` gives the procedure.
- **Real-CLI runs spend API credits.** `cargo xtask e2e` (about 0.80 USD per full run), the `End-to-end` workflow, the `#[ignore]`d real-CLI tests, and every example under `crates/claude-agent-rust-sdk/examples` call the real API. Prove a change with the fake CLI or the mock transport instead.
- **Anything posted on GitHub** in the owner's name: issue, pull request, comment.
- **Lowering a floor**: the 91.5% line coverage floor in `.github/workflows/ci.yml` only goes up, and the lint levels in the root `Cargo.toml` and `clippy.toml` stay as they are.

The owner is the only visible contributor: commits carry no AI attribution and no `Co-authored-by` line.

## Files changed only through their tool

| File | Change it with |
|---|---|
| `upstream/claude-agent-sdk-python/**` | Read-only. Only the submodule pin moves, through `cargo xtask upstream-sync <tag>`. |
| `.github/workflows/actions.lock` | `gh actions-lock` (see GitHub Actions below) |
| `Cargo.lock` | cargo |
| The table between the `parity:table` markers in `docs/migration.md` | `cargo xtask migration-table` |
| The region between the `parity:page` markers in `docs/parity.md` | `cargo xtask parity-page` (rerun after editing `parity/*.toml`, `parity/cases`, or moving the submodule pin) |
| The workspace version, its copies (the `=` requirement on the macros crate, the README snippets), and the released sections of `CHANGELOG.md` | `cargo xtask release prepare`; write entries by hand under `## Unreleased` only |
| The copies of `CLI_VERSION`, `UPSTREAM_SDK_VERSION`, and the upstream commit | `cargo xtask upstream-sync <tag>`; a new copy goes in `UPSTREAM_COPIES` of `xtask/src/release/sync.rs` |
| `docs/research/*.md`, `docs/validation/project-dir-naming.md` | Dated records: write new findings to a new file. |
| `tasks/prd-claude-agent-rust-sdk.md` and its status file | Certified and closed: leave as is. |

## Validation

`cargo xtask gates` runs what the CI `gates` job runs: format, clippy, example build, tests, and rustdoc with warnings denied. Run it before reporting any code change as done.

Always pass `--all-features` to `cargo test` and `cargo clippy`. Most integration test files open with `#![cfg(feature = "testing")]` (some also need `macros`, `rmcp`, or `otel`), so a plain `cargo test` compiles them to nothing and passes. For one file: `cargo test -p claude-agent-rust-sdk --all-features --test <file>`.

The gates leave out checks that CI still enforces. Run the one that matches the change:

| Change | Check |
|---|---|
| Renamed, moved, or removed a public item or a test; edited `parity/`; moved the submodule pin | `cargo xtask parity-check --deny-pending` (needs `git submodule update --init`; builds every test, so it is slow) |
| Changed what reaches the CLI (command line, environment, control frames, MCP answers), `crates/fake-claude`, `crates/parity-driver`, or `parity/cases`; moved the submodule pin | `cargo xtask parity-diff` and `cargo xtask parity-diff --coverage` (needs Python 3.10+ with `parity/requirements.txt`, pointed to by `PARITY_PYTHON`; see `parity/README.md`) |
| A manifest of a published crate, a file at the root of `crates/claude-agent-rust-sdk` or `crates/claude-agent-rust-sdk-macros`, or `xtask/src/package.rs` | `cargo xtask package-check` (packages without `--allow-dirty`, so commit first; CI runs it in the `package` job, with the docs.rs-style build: `RUSTDOCFLAGS="--cfg docsrs -D warnings" cargo +nightly doc --package claude-agent-rust-sdk --all-features --no-deps`) |
| Changed, renamed, or removed a public item of the library | `cargo xtask semver-check` (needs `cargo-semver-checks` and the toolchain pinned in `xtask/src/semver.rs`; reads crates.io). CI runs it in the `semver` job |
| New or bumped dependency | `cargo deny check licenses advisories`. The version declared in `Cargo.toml` must be a true minimum: CI resolves direct dependencies to their declared minimums and runs the tests. |
| Dependency change, or a language or std feature that may be newer than Rust 1.88 | `cargo +1.88.0 check --workspace --all-features --locked` |
| `.github/workflows/*.yml` | `gh actions-lock --verify` (extension: `gh extension install github/gh-actions-lock --pin v0.4.1`) |
| `.github/scripts/actions-freshness.sh` | `bash .github/scripts/actions-freshness.test.sh` (offline, needs `jq`) |
| `.github/scripts/upstream-watch.sh` | `bash .github/scripts/upstream-watch.test.sh` (offline, needs `git`) |
| `.github/scripts/release.sh` | `bash .github/scripts/release.test.sh` (offline, needs `git` and `jq`) |
| New code paths | Cover them with offline tests: CI fails under 91.5% line coverage, measured on Linux with `fake-claude` and `xtask` left out. |

CI runs the gates on Linux, macOS, and Windows. Gate platform-specific code and tests with `cfg`, and keep both the Unix and the Windows branch compiling.

## Repository policy tests

`crates/claude-agent-rust-sdk/tests/workspace.rs` pins repository policy and fails on edits that look unrelated. Read the failing test's name before changing either side. The rules it enforces that a plausible edit breaks:

- **Doc comments only** in `crates/claude-agent-rust-sdk/src` and `crates/fake-claude/src`: a line starting with `//` fails. Put the reason for a choice in a `///` or `//!` comment, as the surrounding code does.
- **`unsafe` lives only in `src/process/sys.rs`**. An operating-system call that needs it goes there, with a doc comment that justifies the block.
- **Fifteen examples**: the README Examples table and the example files match one to one. Adding an example means adding its row and raising the count in the test.
- **Every Cargo feature** is described in the crate docs of `src/lib.rs` as ``//! - `name`:``.
- **Pinned versions**: the tested CLI version (`CLI_VERSION`), the upstream SDK version (`UPSTREAM_SDK_VERSION`), and the upstream commit are repeated across `src/lib.rs`, `README.md`, `docs/migration.md`, `THIRD_PARTY_NOTICES`, `.github/workflows/e2e.yml`, `crates/fake-claude`, and the test itself. `cargo xtask upstream-sync` moves every copy together.
- **README statements**: the authentication, non-affiliation, and license sentences are matched word for word.
- **The former crate name and the former license** appear in no tracked file outside `docs/research/`, `docs/validation/`, `tasks/`, and `upstream/`. A test that needs either string builds it from parts.
- **Published packages**: each published crate directory holds copies of `LICENSE-MIT` and `LICENSE-APACHE` (the library also `THIRD_PARTY_NOTICES`) identical to the files at the repository root, so edit the root file and copy it. The `include` lists, the registry metadata, and the exact `=<version>` requirement on the macros crate are pinned, and no file under `src/` loads a file outside its package.
- **README links** are absolute `https://` URLs, because crates.io has no repository to resolve them against, and the `Cargo.toml` snippet carries the `major.minor` of the workspace version.
- **CI shape**: the gates, the package job, the parity job, and the manual-only, Linux-only, never-on-forks end-to-end workflow.

## Parity ledgers

`parity/surface.toml` (upstream exports) and `parity/tests.toml` (upstream tests) are edited by hand. Keys are upstream names, so a Rust-only item or test needs no entry. Each entry holds exactly one of `rust` (the Rust path or paths), `adapted` (Rust paths of a different kind of item, such as an enum variant for a class; requires a `note`), `excluded` (a key of `[exclusions]`), or `pending`. The rule between `rust` and `adapted` is written at the top of `parity/surface.toml`.

- A Rust test's name is its ledger key. Before renaming or moving a test, search `parity/tests.toml` for it and update the entry. `cargo xtask parity-tests` prints the exact paths the ledger accepts.
- A test compiled on some platforms only takes a ` @windows`, ` @unix`, ` @linux`, or ` @macos` suffix in the ledger, or several joined by `+` (` @linux+windows`). An `#[ignore]`d test passes `parity-check` only when a `cargo test ... --test <file> <filter> -- --ignored` step of `ci.yml` runs it.
- After editing `parity/surface.toml`, run `cargo xtask migration-table`.
- `parity/differences.toml` also holds `[behavior]` (divergences off the wire, each naming the Rust test that shows it) and `[unclassified]` (audit divergences not yet reviewed). After editing any ledger or case, run `cargo xtask parity-page`; `parity-check` fails on a stale `docs/parity.md`, and on hand-written text there that claims "100%" or "full parity".
- The daily `Upstream watch` workflow opens one issue per upstream release newer than the pin, titled `Port upstream release <tag>`, with the verdict and the sync command. Run it by hand with `dry-run` to print the issue instead.
- To move to a newer upstream: `cargo xtask upstream-sync <tag>` moves the pin and every pinned copy and prints `verdict: cli-only` (nothing left to port) or `verdict: needs-port` with what `upstream-diff` lists; then port the changes and add or remove ledger entries until `parity-check --deny-pending` passes.
- `parity/differences.toml` writes moving versions as the tokens `{crate_version}` and `{upstream_version}`, so no release edits it.

## Design invariants

- **Layers, top to bottom**: public API (`query`, `ClaudeSdkClient`) → `ControlSession` (`src/control`) → transport (`src/transport`) → process supervision (`src/process`). Each module's `//!` docs state its contract.
- **Unknown wire data is kept, never dropped or rejected.** An unknown message or content block becomes an `Unknown` variant holding the raw JSON, string enums go through `wire_enum!` (which adds `Unknown(String)`), and structs keep extra keys in a flattened `extra` field. New wire types follow the same shape.
- **Public types are `#[non_exhaustive]`**, built with `new()` and setters. All failures go through the one `Error` enum in `src/error.rs`.
- **Deliberate differences from upstream are written down** where they happen: a bullet in the module docs ("differs from upstream in ... deliberate ways") and a `note` on the ledger entry. Anything else that differs from upstream is a bug.
- **The CLI outranks the Python SDK on disk formats.** `src/sessions` reproduces what the CLI writes (project directory names in UTF-16 units, JSONL key order, timestamps, tag normalization), validated against the real CLI in `docs/validation/`. Where the two disagree, match the CLI.

## Tests

- **Mock transport** (`tests/common/mock.rs`, feature `testing`): protocol logic with no process. Timing-sensitive tests use `#[tokio::test(start_paused = true)]`.
- **Fake CLI** (`crates/fake-claude`): a scripted stand-in binary for anything that needs a real child process. Build a scenario inline with `Fake::new(json!({...}))` from `tests/common/mod.rs`; the format is documented at the top of `crates/fake-claude/src/scenario.rs`. Existing tests depend on its exit codes (64, 65, 66, 67) and init-frame keys (`fake_args`, `fake_env`, `fake_pid`): add to them, keep the existing ones. With `FAKE_CLAUDE_RECORD` set it records what it received (`crates/fake-claude/src/record.rs`), which the differential harness compares.
- **End-to-end** (`tests/e2e/`): `#[ignore]`d, real CLI. Without credentials they fail instead of skipping, by design.
- Name a test as a sentence stating the behavior, for example `follow_up_turn_is_served_while_stdin_stays_open`.

## GitHub Actions

Workflows reference actions by tag, and `.github/workflows/actions.lock` fixes each tag to a commit. To bump actions, run `bash .github/scripts/actions-freshness.sh --apply`, which adopts only releases at least 14 days old. To add one, reference it by tag and run `gh actions-lock`. Either way, review the diff and commit the workflows together with the lockfile.

## Where to look

- `tasks/prd-*.md` with a `-status.json` tracker each: scope, non-goals, hard constraints, and story status. `tasks/prd-parity-hardening.md` is the open one.
- `docs/research/python-sdk-parity.md`: the measured gaps with upstream that the open PRD closes.
- `docs/migration.md`: the user-facing Python-to-Rust mapping.
- `README.md`, Development section: the `cargo xtask` commands.
- `RELEASING.md`: the release commands, what each job of `release.yml` does, the settings outside the repository, and the recovery from each failure.

## Reporting

End a run with the checks you ran and their result, then any CI-only check from the Validation table that applied and that you could not run locally.
