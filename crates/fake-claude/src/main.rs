//! Scriptable stand-in for the Claude CLI.
//!
//! Scenario playback is not available yet: this build only reports its version.

use std::process::ExitCode;

fn main() -> ExitCode {
    match std::env::args().nth(1).as_deref() {
        Some("--version" | "-v") => {
            println!("{} (fake-claude)", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        _ => {
            eprintln!("fake-claude: scenario playback is not available in this build");
            ExitCode::from(2)
        }
    }
}
