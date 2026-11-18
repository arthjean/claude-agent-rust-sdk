//! The release commands: `release prepare`, `release tag`,
//! `changelog-section`, and `upstream-sync`.
//!
//! None of them commits under another identity, pushes, or publishes:
//! `release tag` makes the one release commit and tag with the local git
//! settings, and the owner pushes them. A refused command changes no file.

mod changelog;
mod prepare;
mod repo;
mod sync;
mod tag;
mod version;

use std::process::ExitCode;

pub use repo::workspace_root;

fn outcome(command: &str, result: Result<(), String>) -> ExitCode {
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("xtask: {command}: {error}");
            ExitCode::FAILURE
        }
    }
}

/// `cargo xtask release <prepare|tag> ...`.
pub fn run(arguments: &[String]) -> ExitCode {
    let rest = arguments.get(1..).unwrap_or_default();
    match arguments.first().map(String::as_str) {
        Some("prepare") => outcome("release prepare", prepare::run(rest)),
        Some("tag") => outcome("release tag", tag::run(rest)),
        _ => {
            eprintln!("usage: cargo xtask release <prepare|tag> ...");
            ExitCode::from(2)
        }
    }
}

/// `cargo xtask upstream-sync <tag>`.
pub fn run_sync(arguments: &[String]) -> ExitCode {
    outcome("upstream-sync", sync::run(arguments))
}

/// `cargo xtask changelog-section <version>`: prints the body of the
/// released section of that version, and fails when the section is missing
/// or holds nothing but its `Upstream:` line.
pub fn run_section(arguments: &[String]) -> ExitCode {
    let result = (|| {
        let mut root = repo::workspace_root();
        let mut version = None;
        let mut rest = arguments.iter();
        while let Some(argument) = rest.next() {
            match argument.as_str() {
                "--root" => {
                    root = rest
                        .next()
                        .map(Into::into)
                        .ok_or_else(|| "--root needs a path".to_owned())?;
                }
                value if version.is_none() => version = Some(value.to_owned()),
                extra => return Err(format!("unexpected argument `{extra}`")),
            }
        }
        let version =
            version.ok_or_else(|| "usage: cargo xtask changelog-section <version>".to_owned())?;
        let repo = repo::Repo { root };
        let log = changelog::Changelog::parse(&repo.read("CHANGELOG.md")?);
        let body = log
            .section_of(&version)
            .filter(|body| changelog::has_content(body))
            .ok_or_else(|| format!("CHANGELOG.md has no section for {version}"))?;
        println!("{}", body.trim());
        Ok(())
    })();
    outcome("changelog-section", result)
}
