//! Commands that run cargo over the workspace: the quality gates and the
//! end-to-end suite.

use std::ffi::OsString;
use std::process::{Command, ExitCode};

struct Gate {
    name: &'static str,
    args: &'static [&'static str],
    env: &'static [(&'static str, &'static str)],
}

const GATES: &[Gate] = &[
    Gate {
        name: "format",
        args: &["fmt", "--all", "--check"],
        env: &[],
    },
    Gate {
        name: "lint",
        args: &[
            "clippy",
            "--workspace",
            "--all-targets",
            "--all-features",
            "--no-deps",
        ],
        env: &[],
    },
    Gate {
        name: "test",
        args: &["test", "--workspace", "--all-features"],
        env: &[],
    },
    Gate {
        name: "doc",
        args: &["doc", "--workspace", "--all-features", "--no-deps"],
        env: &[("RUSTDOCFLAGS", "-D warnings")],
    },
];

const E2E: &[&str] = &[
    "test",
    "--package",
    "claude-agent-kit",
    "--all-features",
    "--test",
    "e2e",
    "--",
    "--ignored",
];

const E2E_THREADS_FLAG: &str = "--test-threads";
const E2E_DEFAULT_THREADS: &str = "--test-threads=4";

fn cargo() -> OsString {
    std::env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo"))
}

/// `cargo xtask gates`.
pub fn run() -> ExitCode {
    for gate in GATES {
        eprintln!("xtask: gate `{}`: cargo {}", gate.name, gate.args.join(" "));
        let mut command = Command::new(cargo());
        command.args(gate.args);
        for (key, value) in gate.env {
            command.env(key, value);
        }
        match command.status() {
            Ok(status) if status.success() => {}
            Ok(status) => {
                eprintln!("xtask: gate `{}` failed with {status}", gate.name);
                return ExitCode::FAILURE;
            }
            Err(error) => {
                eprintln!("xtask: gate `{}` could not start: {error}", gate.name);
                return ExitCode::FAILURE;
            }
        }
    }
    eprintln!("xtask: all gates passed");
    ExitCode::SUCCESS
}

/// `cargo xtask e2e`: runs the ignored tests of the `e2e` target against
/// the real CLI, four sessions at a time unless the caller passes its own
/// `--test-threads`. Extra arguments go to the test binary.
pub fn run_e2e(arguments: &[String]) -> ExitCode {
    let mut tail: Vec<&str> = arguments.iter().map(String::as_str).collect();
    if !tail
        .iter()
        .any(|argument| argument.starts_with(E2E_THREADS_FLAG))
    {
        tail.insert(0, E2E_DEFAULT_THREADS);
    }
    eprintln!("xtask: cargo {} {}", E2E.join(" "), tail.join(" "));
    match Command::new(cargo()).args(E2E).args(&tail).status() {
        Ok(status) if status.success() => ExitCode::SUCCESS,
        Ok(status) => {
            eprintln!("xtask: the end-to-end suite failed with {status}");
            ExitCode::FAILURE
        }
        Err(error) => {
            eprintln!("xtask: cargo could not start: {error}");
            ExitCode::FAILURE
        }
    }
}
