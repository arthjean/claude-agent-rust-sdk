//! Maintenance tasks for the workspace, run as `cargo xtask <command>`.

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

fn main() -> ExitCode {
    match std::env::args().nth(1).as_deref() {
        Some("gates") => run_gates(),
        _ => {
            eprintln!("usage: cargo xtask gates");
            ExitCode::from(2)
        }
    }
}

fn run_gates() -> ExitCode {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo"));
    for gate in GATES {
        eprintln!("xtask: gate `{}`: cargo {}", gate.name, gate.args.join(" "));
        let mut command = Command::new(&cargo);
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
