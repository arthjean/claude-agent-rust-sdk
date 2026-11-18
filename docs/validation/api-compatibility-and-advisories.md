# API compatibility and advisory checks: probes of 2026-11-18

Probes run on Fedora (Linux x86_64) while implementing EP-015 of
`tasks/prd-distribution-release.md`, against `claude-agent-rust-sdk` 0.1.0 as
published on crates.io (registry versions: `0.1.0-rc.1`, `0.1.0-rc.2`,
`0.1.0`, none yanked).

## cargo-semver-checks and the toolchain pin

The PRD recorded that `cargo-semver-checks` 0.47.0 rejects the rustdoc JSON of
Rust 1.100.0. Newer releases existed on 2026-11-18: 0.48.0, 0.49.0, 0.50.0,
and 0.51.0 (published 2026-10-03, minimum Rust 1.93).

| Tool | Toolchain | Result |
|---|---|---|
| 0.47.0 | 1.100.0 | exit 1, `unsupported rustdoc format v61 ... (supported formats are v56, v57)` |
| 0.51.0 | 1.100.0 | exit 0, `202 checks: 202 pass, 58 skip`, `Summary no semver update required` |
| 0.51.0 | nightly 1.102.0 (2026-11-16) | exit 0, same summary |

Pinned: tool 0.51.0, toolchain 1.100.0 (`xtask/src/semver.rs`, `semver` job
of `.github/workflows/ci.yml`).

Behavior read in the 0.51.0 source (`src/rustdoc_gen.rs`,
`choose_baseline_version`; `src/check_release.rs`; `src/main.rs`):

- Without `--baseline-version`, the baseline is the highest non-yanked,
  non-pre-release version not above the workspace version, else the highest
  version not above it.
- An unchanged version is checked as the smallest next release ("no change;
  assume minor" for `0.1.0`), so a break fails while an addition passes.
- Exit codes: 0 compatible, 100 lint failure, 101 error. The tool runs the
  `cargo` found on `PATH`, so `RUSTUP_TOOLCHAIN` chooses the toolchain.
- `cargo-semver-checks --version` printed `cargo 0.47.0` for 0.47.0;
  `cargo-semver-checks semver-checks --version` prints
  `cargo-semver-checks <version>` for both releases, so `xtask` asks that way.

## Through `cargo xtask semver-check`

| Case | How | Result |
|---|---|---|
| Unchanged API | `cargo xtask semver-check --features jsonschema,macros,otel,rmcp` | exit 0, "keeps the API its version promises" |
| Break | `pub const VERSION` renamed, then reverted | exit 1, names `pub_module_level_const_missing: VERSION in file .../claude-agent-rust-sdk-0.1.0/src/lib.rs:220` and says to use `release prepare minor` |
| Registry unreachable | `HTTPS_PROXY` to a closed port, empty `CARGO_HOME` | exit 1, exit 101 from the tool, "crates.io could not be read: no pin is at fault" |
| Rustdoc format rejected | the real 0.47.0 behind a wrapper reporting 0.51.0, on 1.100.0 | exit 1, "cannot read the rustdoc JSON of Rust 1.100.0: update the pins together", naming both pins and both files |
| Wrong tool installed | 0.47.0 on `PATH` | exit 1, "cargo-semver-checks 0.51.0 is pinned, `cargo-semver-checks 0.47.0` is installed", with the install command |

`cargo xtask release prepare patch --dry-run`, in a scratch clone with the
EP-015 changes committed on `main` after `v0.1.0`:

- after a commit `fix: rename a constant` (the same rename): exit 1,
  "breaking change since v0.1.0: use minor (cargo-semver-checks reports:" and
  the item; no file changed;
- after reverting it: exit 0, "would move 0.1.0 -> 0.1.1".

## cargo-deny advisories

The `Advisories` workflow uses `EmbarkStudios/cargo-deny-action@v2.1.1`,
whose image installs cargo-deny 0.20.2; the local tool was 0.19.8. Both gave
the same results.

| Case | Result |
|---|---|
| `Cargo.lock` of 2026-11-18, no `ignore` | exit 1, `error[unmaintained]: chrono is unmaintained`, `ID: RUSTSEC-2026-0543`, `advisories FAILED` |
| Same, with `RUSTSEC-2026-0543` ignored with its reason | exit 0, `advisories ok` |
| Advisory database unreachable (`HTTPS_PROXY` to a closed port, empty `db-path`) | exit 1, no `advisories ok` line |
| `cargo deny check licenses` with the new section (0.20.2) | `licenses ok` |

`chrono` 0.4.45 reaches the workspace only through `rmcp` 3.10.0 and
`schemars` 1.2.3; the advisory states that no safe upgrade exists. It is the
only entry of `ignore`.
