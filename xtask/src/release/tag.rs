//! `cargo xtask release tag`: checks the release `release prepare` left in
//! the working tree, runs the gates and the package check, and creates the
//! release commit and its annotated tag with the git identity and signing
//! settings of the machine. It pushes nothing.
//!
//! The commit is made before the package check, because cargo refuses to
//! package a tree with uncommitted changes and the check never passes
//! `--allow-dirty`. When the check fails, the commit is undone with
//! `git reset --soft`, which leaves the prepared files as they were.

use super::changelog::{self, Changelog};
use super::repo::{self, Repo};

struct Arguments {
    gates: bool,
    root: std::path::PathBuf,
}

const USAGE: &str = "usage: cargo xtask release tag [--no-gates]";

fn parse(arguments: &[String]) -> Result<Arguments, String> {
    let mut parsed = Arguments {
        gates: true,
        root: repo::workspace_root(),
    };
    let mut rest = arguments.iter();
    while let Some(argument) = rest.next() {
        match argument.as_str() {
            "--no-gates" => parsed.gates = false,
            "--root" => {
                parsed.root = rest
                    .next()
                    .map(Into::into)
                    .ok_or_else(|| "--root needs a path".to_owned())?;
            }
            other => return Err(format!("unexpected argument `{other}`\n{USAGE}")),
        }
    }
    Ok(parsed)
}

/// The checks that create nothing: the version, its tag, its changelog
/// section, and the changed files. Returns the version and the files.
fn check(repo: &Repo) -> Result<(String, Vec<String>), String> {
    repo.require_main()?;
    let version = repo.workspace_version()?.to_string();
    let tag = format!("v{version}");
    if repo.tag_exists(&tag) {
        return Err(format!("the tag {tag} already exists"));
    }
    let log = Changelog::parse(&repo.read("CHANGELOG.md")?);
    let problems = log.problems();
    if !problems.is_empty() {
        return Err(format!("CHANGELOG.md:\n  {}", problems.join("\n  ")));
    }
    if !log.section_of(&version).is_some_and(changelog::has_content) {
        return Err(format!(
            "CHANGELOG.md has no section for {version}; run `cargo xtask release prepare` first"
        ));
    }
    let changed = repo.changed_files()?;
    let outside: Vec<&String> = changed
        .iter()
        .filter(|file| !repo::RELEASE_FILES.contains(&file.as_str()))
        .collect();
    if !outside.is_empty() {
        let listed: Vec<&str> = outside.iter().map(|file| file.as_str()).collect();
        return Err(format!(
            "these changes are not part of a release; commit or revert them first:\n  {}\nA release commit holds only: {}",
            listed.join("\n  "),
            repo::RELEASE_FILES.join(", ")
        ));
    }
    if changed.is_empty() {
        return Err(
            "nothing to release: the working tree is clean; run `cargo xtask release prepare` first"
                .to_owned(),
        );
    }
    Ok((version, changed))
}

fn create(repo: &Repo, version: &str, files: &[String], gates: bool) -> Result<(), String> {
    if gates {
        repo.cargo(&["xtask", "gates"])?;
    } else {
        eprintln!("xtask: --no-gates: skipping the gates");
    }
    let mut add = vec!["add", "--"];
    add.extend(files.iter().map(String::as_str));
    repo.git(&add)?;
    let message = format!("chore: release v{version}");
    repo.git(&["commit", "--quiet", "--message", &message])?;
    let undo = |error: String| match repo.git(&["reset", "--quiet", "--soft", "HEAD~1"]) {
        Ok(_) => format!("{error}\nthe release commit was undone; the prepared files are kept"),
        Err(reset) => format!("{error}\nand the release commit could not be undone: {reset}"),
    };
    if let Err(error) = repo.cargo(&["xtask", "package-check"]) {
        return Err(undo(error));
    }
    let tag = format!("v{version}");
    if let Err(error) = repo.git(&["tag", "--annotate", &tag, "--message", &tag]) {
        return Err(undo(error));
    }
    Ok(())
}

/// Runs the command.
pub fn run(arguments: &[String]) -> Result<(), String> {
    let arguments = parse(arguments)?;
    let repo = Repo {
        root: arguments.root.clone(),
    };
    let (version, files) = check(&repo)?;
    create(&repo, &version, &files, arguments.gates)?;
    println!("release tag: created the commit `chore: release v{version}` and the tag v{version}");
    println!(
        "\nPush both together to publish:\n\n  git push --atomic origin {} v{version}",
        repo::MAIN
    );
    Ok(())
}
