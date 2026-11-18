//! `cargo xtask release prepare <patch|minor|X.Y.Z[-pre]>`: moves the crate
//! version everywhere it is copied, regenerates the generated pages, and
//! turns `## Unreleased` into the section of the new version. It commits,
//! tags, and pushes nothing.
//!
//! `patch` is refused after a commit marked breaking, and when the pinned
//! `cargo-semver-checks` reports a break against the newest release on
//! crates.io (`crate::semver`); either way it says to use `minor`.
//!
//! "The current version" a new one must exceed is the newest `v*` tag,
//! not the workspace version: before the first release the workspace
//! version (`0.1.0`) was never published, and a first release candidate
//! (`0.1.0-rc.1`) sorts below it. Once a release exists, the two agree.

use std::fmt::Write as _;

use super::changelog::{self, Changelog};
use super::repo::{self, Pinned, Repo, Transaction, Values};
use super::version::Version;

/// Every copy of the crate version. The two README snippets give the
/// `major.minor` and change only when the minor does.
pub const VERSION_COPIES: [Pinned; 4] = [
    Pinned {
        file: "Cargo.toml",
        template: "\nversion = \"{version}\"\n",
    },
    Pinned {
        file: "crates/claude-agent-rust-sdk/Cargo.toml",
        template: "version = \"={version}\"",
    },
    Pinned {
        file: "README.md",
        template: "\nclaude-agent-rust-sdk = \"{minor}\"\n",
    },
    Pinned {
        file: "crates/claude-agent-rust-sdk-macros/README.md",
        template: "claude-agent-rust-sdk = { version = \"{minor}\"",
    },
];

const CHANGELOG_FILE: &str = "CHANGELOG.md";

/// What the command was asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Bump {
    Patch,
    Minor,
    Exact(Version),
}

struct Arguments {
    bump: Bump,
    dry_run: bool,
    root: std::path::PathBuf,
}

const USAGE: &str = "usage: cargo xtask release prepare <patch|minor|X.Y.Z[-pre]> [--dry-run]";

fn parse(arguments: &[String]) -> Result<Arguments, String> {
    let mut bump = None;
    let mut dry_run = false;
    let mut root = repo::workspace_root();
    let mut rest = arguments.iter();
    while let Some(argument) = rest.next() {
        match argument.as_str() {
            "--dry-run" => dry_run = true,
            "--root" => {
                root = rest
                    .next()
                    .map(Into::into)
                    .ok_or_else(|| "--root needs a path".to_owned())?;
            }
            option if option.starts_with("--") => {
                return Err(format!("unknown option {option}\n{USAGE}"));
            }
            "patch" if bump.is_none() => bump = Some(Bump::Patch),
            "minor" if bump.is_none() => bump = Some(Bump::Minor),
            exact if bump.is_none() => bump = Some(Bump::Exact(Version::parse(exact)?)),
            extra => return Err(format!("unexpected argument `{extra}`\n{USAGE}")),
        }
    }
    Ok(Arguments {
        bump: bump.ok_or_else(|| USAGE.to_owned())?,
        dry_run,
        root,
    })
}

/// The upstream versions the submodule knows, from its tags and its
/// changelog headings.
fn upstream_versions(repo: &Repo) -> Vec<Version> {
    let upstream = repo.path(repo::UPSTREAM);
    let mut versions: Vec<Version> = Repo::git_in(&upstream, &["tag", "--list", "v*"])
        .unwrap_or_default()
        .lines()
        .filter_map(|tag| Version::parse(tag.trim().strip_prefix('v')?).ok())
        .collect();
    if let Ok(text) = std::fs::read_to_string(upstream.join("CHANGELOG.md")) {
        versions.extend(
            text.lines()
                .filter_map(|line| Version::parse(line.strip_prefix("## ")?.trim()).ok()),
        );
    }
    versions
}

/// The `Upstream:` line of the section of `next`: from the upstream
/// version after the newest one the previous section covers, to the pinned
/// one. For a release, its pre-release sections do not count as previous:
/// its section gathers them, so its range starts where theirs does.
fn upstream_line(repo: &Repo, changelog: &Changelog, next: &Version) -> Result<String, String> {
    let current = repo.library_constant(repo::SDK_CONSTANT)?;
    let cli = repo.library_constant(repo::CLI_CONSTANT)?;
    let current_version = Version::parse(&current)?;
    let releases = changelog.releases();
    let (gathered, previous): (Vec<_>, Vec<_>) = releases
        .iter()
        .partition(|release| release.version.is_pre_release_of(next));
    let first = match previous.first() {
        None => gathered
            .last()
            .map_or_else(|| current.clone(), |oldest| oldest.first_upstream.clone()),
        Some(previous) => {
            let last = Version::parse(&previous.last_upstream)?;
            if last >= current_version {
                current.clone()
            } else {
                upstream_versions(repo)
                    .into_iter()
                    .filter(|version| *version > last && *version <= current_version)
                    .min()
                    .map_or_else(|| current.clone(), |version| version.to_string())
            }
        }
    };
    Ok(changelog::upstream_text(&first, &current, &cli))
}

/// The plan of one run: the new version and the new content of each file
/// written before cargo and the regeneration run.
struct Plan {
    current: Version,
    next: Version,
    files: Vec<(String, String)>,
    section: String,
}

fn plan(repo: &Repo, bump: &Bump) -> Result<Plan, String> {
    repo.require_main()?;
    repo.require_clean()?;
    let current = repo.workspace_version()?;
    let next = match bump {
        Bump::Patch => current.next_patch(),
        Bump::Minor => current.next_minor(),
        Bump::Exact(version) => version.clone(),
    };
    let tag = format!("v{next}");
    if repo.tag_exists(&tag) {
        return Err(format!("the tag {tag} already exists"));
    }
    if let Some(released) = repo.release_tags(false)?.first()
        && next <= *released
    {
        return Err(format!(
            "{next} is not greater than the last released version {released}"
        ));
    }
    let previous = repo
        .release_tags(true)?
        .first()
        .map(|version| format!("v{version}"));
    let messages = repo.commit_messages(previous.as_deref())?;
    if *bump == Bump::Patch {
        let breaking = messages
            .iter()
            .find(|message| changelog::conventional(message).is_some_and(|commit| commit.breaking));
        let since = previous.as_deref().unwrap_or("the first commit");
        if let Some(message) = breaking {
            return Err(format!(
                "breaking change since {since}: use minor (`{}`)",
                message.lines().next().unwrap_or_default().trim()
            ));
        }
        let features = crate::semver::features(&repo.root)?;
        let (verdict, _) = crate::semver::compare(&repo.root, &features)?;
        if let crate::semver::Verdict::Breaking(items) = verdict {
            return Err(format!(
                "breaking change since {since}: use minor (cargo-semver-checks reports:{})",
                crate::semver::listing(&items)
            ));
        }
    }

    let mut log = Changelog::parse(&repo.read(CHANGELOG_FILE)?);
    let problems = log.problems();
    if !problems.is_empty() {
        return Err(format!("{CHANGELOG_FILE}:\n  {}", problems.join("\n  ")));
    }
    let upstream = upstream_line(repo, &log, &next)?;
    log.release(
        &next,
        &repo::today(),
        &upstream,
        &changelog::draft(&messages),
    )?;
    let section = log
        .section_of(&next.to_string())
        .unwrap_or_default()
        .to_owned();

    let values = |version: &Version| Values {
        version: version.to_string(),
        minor: version.major_minor(),
        ..Values::default()
    };
    let mut files = repo::rewrite_copies(repo, &VERSION_COPIES, &values(&current), &values(&next))?;
    files.push((CHANGELOG_FILE.to_owned(), log.render()));
    files.retain(|(file, content)| repo.read(file).ok().as_ref() != Some(content));
    Ok(Plan {
        current,
        next,
        files,
        section,
    })
}

/// Writes the plan, then lets cargo and the regeneration rewrite their
/// files; returns the files that changed.
fn apply(repo: &Repo, plan: &Plan) -> Result<Vec<String>, String> {
    let mut transaction = Transaction::new(&repo.root);
    for (file, content) in &plan.files {
        transaction.write(file, content)?;
    }
    transaction.record("Cargo.lock");
    repo.cargo(&["update", "--workspace", "--offline"])?;
    transaction.record("docs/migration.md");
    transaction.record("docs/parity.md");
    crate::parity::regenerate(&crate::parity::Locations::at(repo.root.clone()))?;
    let changed = repo.changed_files()?;
    let outside: Vec<&String> = changed
        .iter()
        .filter(|file| !repo::RELEASE_FILES.contains(&file.as_str()))
        .collect();
    if !outside.is_empty() {
        return Err(format!(
            "the release changed files outside the release set: {outside:?}"
        ));
    }
    transaction.commit();
    Ok(changed)
}

/// What the run did, or with `changed` set to `None`, what it would do.
fn report(plan: &Plan, changed: Option<&[String]>) -> String {
    let dry_run = changed.is_none();
    let mut text = String::new();
    let verb = if dry_run { "would move" } else { "moved" };
    let _ = writeln!(
        text,
        "release prepare: {verb} {} -> {}",
        plan.current, plan.next
    );
    let _ = writeln!(text, "\nFiles:");
    match changed {
        Some(changed) => {
            for file in changed {
                let _ = writeln!(text, "  {file}");
            }
        }
        None => {
            for (file, _) in &plan.files {
                let _ = writeln!(text, "  {file}");
            }
            for file in ["Cargo.lock", "docs/migration.md", "docs/parity.md"] {
                let _ = writeln!(text, "  {file} (regenerated when it changes)");
            }
        }
    }
    let _ = writeln!(
        text,
        "\nCHANGELOG.md section:\n\n## {} - {}\n{}",
        plan.next,
        repo::today(),
        plan.section.trim_end()
    );
    if dry_run {
        let _ = writeln!(text, "\ndry run: nothing was written");
    } else {
        let _ = writeln!(
            text,
            "\nReview the section, then run: cargo xtask release tag"
        );
    }
    text
}

/// Runs the command.
pub fn run(arguments: &[String]) -> Result<(), String> {
    let arguments = parse(arguments)?;
    let repo = Repo {
        root: arguments.root.clone(),
    };
    let plan = plan(&repo, &arguments.bump)?;
    if arguments.dry_run {
        print!("{}", report(&plan, None));
    } else {
        let changed = apply(&repo, &plan)?;
        print!("{}", report(&plan, Some(&changed)));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments_take_one_bump_and_the_options() {
        let parsed = parse(&["patch".to_owned(), "--dry-run".to_owned()]).unwrap();
        assert_eq!(parsed.bump, Bump::Patch);
        assert!(parsed.dry_run);
        assert_eq!(
            parse(&["0.2.0-rc.1".to_owned()]).unwrap().bump,
            Bump::Exact(Version::parse("0.2.0-rc.1").unwrap())
        );
        assert!(parse(&[]).is_err());
        assert!(parse(&["minor".to_owned(), "patch".to_owned()]).is_err());
        assert!(parse(&["--force".to_owned()]).is_err());
        assert!(parse(&["v0.2.0".to_owned()]).is_err());
    }
}
