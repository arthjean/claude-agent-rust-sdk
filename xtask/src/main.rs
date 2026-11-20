//! Maintenance tasks for the workspace, run as `cargo xtask <command>`.

mod differential;
mod gates;
mod package;
mod parity;
mod release;
mod semver;

use std::process::ExitCode;

const USAGE: &str = "\
usage: cargo xtask <command>

  gates                 run the format, lint, example build, test, and
                        documentation checks
  package-check         package both published crates with the lockfile, let
                        cargo build each archive, and fail on a file outside
                        the allowed set, a missing license file, a missing
                        manifest field, or an archive over 1 MiB
  parity-check          fail when a ledger misses an upstream export or test,
                        or names a Rust path that does not exist
                        (--deny-pending also fails on pending entries)
  parity-report         print the share of upstream exports and tests
                        implemented, adapted, excluded, and pending
  parity-tests          print the Rust test paths parity/tests.toml may name
  migration-table       regenerate the table of docs/migration.md from
                        parity/surface.toml (--check only compares)
  parity-page           regenerate the measured region of docs/parity.md
                        from the ledgers and the cases (--check only
                        compares)
  upstream-diff <tag>   list what upstream changed between the pinned commit
                        and <tag>, and whether only versions moved
                        (verdict: cli-only) or the change needs a port
                        (verdict: needs-port)
  upstream-sync <tag>   move the upstream pin to <tag> and every copy of the
                        pinned SDK version, CLI version, and commit; print
                        the verdict
  release prepare <patch|minor|X.Y.Z[-pre]>
                        move the crate version everywhere, regenerate the
                        generated pages, and turn `## Unreleased` of
                        CHANGELOG.md into the new section (--dry-run only
                        prints the plan); commits nothing
  release tag           run the gates and the package check, then create the
                        commit `chore: release v<version>` and the tag
                        v<version> (--no-gates skips the gates); pushes
                        nothing
  changelog-section <version>
                        print the CHANGELOG.md section of <version>
  semver-check          compare the public API of the library with its
                        newest release on crates.io through the pinned
                        cargo-semver-checks, and fail on a break the
                        workspace version does not allow (--features a,b
                        names the features; by default every feature but
                        `default` and `testing`)
  parity-diff [cases]   play every case of parity/cases through both SDKs
                        against fake-claude and fail on any difference not
                        in parity/differences.toml, in what the CLI receives
                        or in what the application observes (--coverage
                        lists the option fields the cases set; --check-requirements
                        checks parity/requirements.txt against upstream)
  e2e [test arguments]  run the end-to-end suite against the real CLI";

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let rest = arguments.get(1..).unwrap_or_default();
    match arguments.first().map(String::as_str) {
        Some("gates") => gates::run(),
        Some("package-check") => package::run(rest),
        Some("parity-check") => parity::run_check(rest),
        Some("parity-report") => parity::run_report(rest),
        Some("parity-tests") => parity::run_tests(rest),
        Some("migration-table") => parity::run_migration(rest),
        Some("parity-page") => parity::run_page(rest),
        Some("upstream-diff") => parity::run_diff(rest),
        Some("upstream-sync") => release::run_sync(rest),
        Some("release") => release::run(rest),
        Some("changelog-section") => release::run_section(rest),
        Some("semver-check") => semver::run(rest),
        Some("parity-diff") => differential::run(rest),
        Some("e2e") => gates::run_e2e(rest),
        _ => {
            eprintln!("{USAGE}");
            ExitCode::from(2)
        }
    }
}
