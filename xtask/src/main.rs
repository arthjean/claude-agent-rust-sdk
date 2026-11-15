//! Maintenance tasks for the workspace, run as `cargo xtask <command>`.

mod gates;
mod parity;

use std::process::ExitCode;

const USAGE: &str = "\
usage: cargo xtask <command>

  gates                 run the format, lint, test, and documentation checks
  parity-check          fail when a ledger misses an upstream export or test,
                        or names a Rust path that does not exist
                        (--deny-pending also fails on pending entries)
  parity-report         print the share of upstream exports and tests
                        implemented, excluded, and pending
  parity-tests          print the Rust test paths parity/tests.toml may name
  migration-table       regenerate the table of docs/migration.md from
                        parity/surface.toml (--check only compares)
  upstream-diff <tag>   list what upstream changed between the pinned commit
                        and <tag>
  e2e [test arguments]  run the end-to-end suite against the real CLI";

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let rest = arguments.get(1..).unwrap_or_default();
    match arguments.first().map(String::as_str) {
        Some("gates") => gates::run(),
        Some("parity-check") => parity::run_check(rest),
        Some("parity-report") => parity::run_report(rest),
        Some("parity-tests") => parity::run_tests(rest),
        Some("migration-table") => parity::run_migration(rest),
        Some("upstream-diff") => parity::run_diff(rest),
        Some("e2e") => gates::run_e2e(rest),
        _ => {
            eprintln!("{USAGE}");
            ExitCode::from(2)
        }
    }
}
