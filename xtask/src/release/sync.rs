//! `cargo xtask upstream-sync <tag>`: moves the upstream submodule pin to
//! a release tag and every copy of the pinned versions with it, then says
//! whether the upstream change was only a version bump (`cli-only`) or
//! needs a port (`needs-port`).
//!
//! The new pin is staged (`git add` of the submodule), because the policy
//! tests read the pin from the index. The command commits nothing and runs
//! no CLI; it uses the network only when the tag is not in the submodule
//! yet, to fetch the tags of its `origin`.

use std::fmt::Write as _;
use std::path::Path;

use super::changelog::{self, Changelog};
use super::repo::{self, Pinned, Repo, Transaction, Values};
use super::version::Version;
use crate::parity::diff::{self, Verdict};

/// Every copy of the upstream SDK version (`{sdk}`), the CLI version
/// (`{cli}`), and the upstream commit (`{commit}`) the policy tests pin.
pub const UPSTREAM_COPIES: [Pinned; 17] = [
    Pinned {
        file: repo::LIBRARY_ROOT,
        template: "pub const CLI_VERSION: &str = \"{cli}\";",
    },
    Pinned {
        file: repo::LIBRARY_ROOT,
        template: "pub const UPSTREAM_SDK_VERSION: &str = \"{sdk}\";",
    },
    Pinned {
        file: repo::LIBRARY_ROOT,
        template: "//! {cli} ([`CLI_VERSION`])",
    },
    Pinned {
        file: "README.md",
        template: "Claude Agent SDK, version {sdk}.",
    },
    Pinned {
        file: "README.md",
        template: "against {cli}.",
    },
    Pinned {
        file: "docs/migration.md",
        template: "a port of version {sdk} of",
    },
    Pinned {
        file: "THIRD_PARTY_NOTICES",
        template: "{commit} (v{sdk}).",
    },
    Pinned {
        file: "crates/claude-agent-rust-sdk/THIRD_PARTY_NOTICES",
        template: "{commit} (v{sdk}).",
    },
    Pinned {
        file: ".github/workflows/e2e.yml",
        template: "CLAUDE_CODE_VERSION: {cli}\n",
    },
    Pinned {
        file: "crates/claude-agent-rust-sdk/tests/workspace.rs",
        template: "const UPSTREAM_COMMIT: &str = \"{commit}\";",
    },
    Pinned {
        file: "crates/claude-agent-rust-sdk/tests/workspace.rs",
        template: "CLI_VERSION, \"{cli}\");",
    },
    Pinned {
        file: "crates/claude-agent-rust-sdk/tests/workspace.rs",
        template: "UPSTREAM_SDK_VERSION, \"{sdk}\");",
    },
    Pinned {
        file: "crates/fake-claude/src/scenario.rs",
        template: "DEFAULT_VERSION: &str = \"{cli}\";",
    },
    Pinned {
        file: "crates/fake-claude/src/scenario.rs",
        template: "\"claude_code_version\": \"{cli}\",",
    },
    Pinned {
        file: "crates/fake-claude/src/scenario.rs",
        template: "Some(\"{cli}\")",
    },
    Pinned {
        file: "crates/fake-claude/tests/cli.rs",
        template: "\"{cli} (fake-claude)\"",
    },
    Pinned {
        file: "crates/fake-claude/tests/cli.rs",
        template: "[\"claude_code_version\"], \"{cli}\");",
    },
];

const CLI_FILE: &str = "src/claude_agent_sdk/_cli_version.py";

struct Arguments {
    tag: String,
    root: std::path::PathBuf,
}

const USAGE: &str = "usage: cargo xtask upstream-sync <tag>";

fn parse(arguments: &[String]) -> Result<Arguments, String> {
    let mut tag = None;
    let mut root = repo::workspace_root();
    let mut rest = arguments.iter();
    while let Some(argument) = rest.next() {
        match argument.as_str() {
            "--root" => {
                root = rest
                    .next()
                    .map(Into::into)
                    .ok_or_else(|| "--root needs a path".to_owned())?;
            }
            option if option.starts_with("--") => {
                return Err(format!("unknown option {option}\n{USAGE}"));
            }
            value if tag.is_none() => tag = Some(value.to_owned()),
            extra => return Err(format!("unexpected argument `{extra}`\n{USAGE}")),
        }
    }
    Ok(Arguments {
        tag: tag.ok_or_else(|| USAGE.to_owned())?,
        root,
    })
}

/// The `__cli_version__ = "..."` of `_cli_version.py`.
fn cli_version(text: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let value = line.trim().strip_prefix("__cli_version__")?.trim_start();
        let value = value.strip_prefix('=')?.trim();
        Some(value.trim_matches('"').to_owned())
    })
}

struct Plan {
    old: Values,
    new: Values,
    target: String,
    verdict: Verdict,
    report: String,
    files: Vec<(String, String)>,
}

fn plan(repo: &Repo, tag: &str) -> Result<Plan, String> {
    repo.require_clean()?;
    let upstream = repo.path(repo::UPSTREAM);
    let pin = repo
        .git(&["rev-parse", &format!("HEAD:{}", repo::UPSTREAM)])?
        .trim()
        .to_owned();
    let tag_version = tag
        .strip_prefix('v')
        .ok_or_else(|| format!("`{tag}` is not an upstream release tag of the form vX.Y.Z"))?;
    Version::parse(tag_version)
        .map_err(|_| format!("`{tag}` is not an upstream release tag of the form vX.Y.Z"))?;
    let target = diff::resolve(&upstream, &format!("refs/tags/{tag}"))
        .map_err(|_| format!("the upstream tag {tag} is unknown, even after fetching tags"))?;

    let old = Values {
        sdk: repo.library_constant(repo::SDK_CONSTANT)?,
        cli: repo.library_constant(repo::CLI_CONSTANT)?,
        commit: pin.clone(),
        ..Values::default()
    };
    let sdk = crate::differential::ledger::pyproject_version(&diff::show(
        &upstream,
        &target,
        "pyproject.toml",
    )?)
    .ok_or_else(|| format!("pyproject.toml at {tag} has no version line"))?;
    let cli = cli_version(&diff::show(&upstream, &target, CLI_FILE)?)
        .ok_or_else(|| format!("{CLI_FILE} at {tag} has no `__cli_version__` line"))?;
    Version::parse(&cli)
        .map_err(|_| format!("{CLI_FILE} at {tag} gives `{cli}`, not a version X.Y.Z[-pre]"))?;
    if Version::parse(&sdk)? <= Version::parse(&old.sdk)? {
        return Err(format!(
            "{tag} is not newer than the pin v{} (it is version {sdk})",
            old.sdk
        ));
    }
    let new = Values {
        sdk,
        cli,
        commit: target.clone(),
        ..Values::default()
    };

    let mut files = repo::rewrite_copies(repo, &UPSTREAM_COPIES, &old, &new)?;
    let mut log = Changelog::parse(&repo.read("CHANGELOG.md")?);
    log.add_unreleased_entry(
        "Changed",
        &format!(
            "{}Python SDK {} and CLI {}.",
            changelog::SYNC_ENTRY_PREFIX,
            new.sdk,
            new.cli
        ),
    )?;
    files.push(("CHANGELOG.md".to_owned(), log.render()));

    let verdict = diff::verdict(&upstream, &pin, &target)?;
    let report = match verdict {
        Verdict::CliOnly => String::new(),
        Verdict::NeedsPort => diff::run(
            &upstream,
            tag,
            Some(&repo.root.join("parity").join("tests.toml")),
        )?,
    };
    Ok(Plan {
        old,
        new,
        target,
        verdict,
        report,
        files,
    })
}

fn checkout(upstream: &Path, commit: &str) -> Result<(), String> {
    Repo::git_in(upstream, &["checkout", "--quiet", "--detach", commit]).map(drop)
}

fn apply(repo: &Repo, plan: &Plan) -> Result<(), String> {
    let upstream = repo.path(repo::UPSTREAM);
    let mut transaction = Transaction::new(&repo.root);
    for (file, content) in &plan.files {
        transaction.write(file, content)?;
    }
    checkout(&upstream, &plan.target)?;
    let moved = (|| {
        repo.git(&["add", "--", repo::UPSTREAM])?;
        transaction.record("docs/migration.md");
        transaction.record("docs/parity.md");
        crate::parity::regenerate(&crate::parity::Locations::at(repo.root.clone()))
    })();
    if let Err(error) = moved {
        let _ = repo.git(&["reset", "--quiet", "--", repo::UPSTREAM]);
        let _ = checkout(&upstream, &plan.old.commit);
        return Err(error);
    }
    transaction.commit();
    Ok(())
}

fn summary(plan: &Plan, tag: &str) -> String {
    let mut text = String::new();
    let _ = writeln!(
        text,
        "upstream-sync: pin {} -> {tag} ({})",
        plan.old.commit.get(..7).unwrap_or(&plan.old.commit),
        plan.target.get(..7).unwrap_or(&plan.target)
    );
    let _ = writeln!(
        text,
        "Python SDK {} -> {}, CLI {} -> {}",
        plan.old.sdk, plan.new.sdk, plan.old.cli, plan.new.cli
    );
    let _ = writeln!(text, "verdict: {}", plan.verdict.word());
    match plan.verdict {
        Verdict::CliOnly => {
            let _ = writeln!(
                text,
                "\nOnly versions moved upstream. Commit the changes, then run `cargo xtask release prepare patch`."
            );
        }
        Verdict::NeedsPort => {
            let _ = writeln!(text, "\n{}", plan.report.trim_end());
            let _ = writeln!(
                text,
                "\nThe pin and the copies are moved. Port the changes and complete the ledgers until `cargo xtask parity-check --deny-pending` passes before a release."
            );
        }
    }
    text
}

/// Runs the command.
pub fn run(arguments: &[String]) -> Result<(), String> {
    let arguments = parse(arguments)?;
    let repo = Repo {
        root: arguments.root.clone(),
    };
    let plan = plan(&repo, &arguments.tag)?;
    apply(&repo, &plan)?;
    print!("{}", summary(&plan, &arguments.tag));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cli_version_is_read_from_its_module() {
        assert_eq!(
            cli_version("\"\"\"Bundled.\"\"\"\n\n__cli_version__ = \"2.1.326\"\n").as_deref(),
            Some("2.1.326")
        );
        assert_eq!(cli_version("nothing"), None);
    }

    #[test]
    fn arguments_take_exactly_one_tag() {
        assert_eq!(parse(&["v0.2.183".to_owned()]).unwrap().tag, "v0.2.183");
        assert!(parse(&[]).is_err());
        assert!(parse(&["a".to_owned(), "b".to_owned()]).is_err());
        assert!(parse(&["--dry-run".to_owned()]).is_err());
    }
}
