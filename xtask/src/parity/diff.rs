//! `upstream-diff`: what changed in the upstream SDK between the pinned
//! commit and another revision, read from the git objects of the submodule.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::Path;
use std::process::Command;

use super::python;

const INIT: &str = "src/claude_agent_sdk/__init__.py";
const TRANSPORT: &str = "src/claude_agent_sdk/_internal/transport/subprocess_cli.py";
const TYPES: &str = "src/claude_agent_sdk/types.py";
const QUERY: &str = "src/claude_agent_sdk/_internal/query.py";
const CHANGELOG: &str = "CHANGELOG.md";
const OPTIONS_CLASS: &str = "ClaudeAgentOptions";

fn git(upstream: &Path, arguments: &[&str]) -> Result<Option<String>, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(upstream)
        .args(arguments)
        .output()
        .map_err(|error| format!("cannot run git: {error}"))?;
    Ok(output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned()))
}

fn commit(upstream: &Path, revision: &str) -> Result<Option<String>, String> {
    let object = format!("{revision}^{{commit}}");
    Ok(
        git(upstream, &["rev-parse", "--verify", "--quiet", &object])?
            .map(|hash| hash.trim().to_owned()),
    )
}

fn resolve(upstream: &Path, revision: &str) -> Result<String, String> {
    if let Some(hash) = commit(upstream, revision)? {
        return Ok(hash);
    }
    eprintln!("xtask: `{revision}` is not in the submodule yet, fetching tags");
    git(upstream, &["fetch", "--quiet", "--tags", "origin"])?;
    commit(upstream, revision)?.ok_or_else(|| {
        format!(
            "`{revision}` is not a revision of {}, even after fetching tags",
            upstream.display()
        )
    })
}

fn show(upstream: &Path, revision: &str, file: &str) -> Result<String, String> {
    Ok(git(upstream, &["show", &format!("{revision}:{file}")])?.unwrap_or_default())
}

fn is_test_module(file: &str) -> bool {
    let name = file.rsplit('/').next().unwrap_or(file);
    name.ends_with(".py") && (name.starts_with("test_") || name.ends_with("_test.py"))
}

fn tests_under(
    upstream: &Path,
    revision: &str,
    directory: &str,
) -> Result<BTreeSet<String>, String> {
    let listing = git(
        upstream,
        &["ls-tree", "-r", "--name-only", revision, "--", directory],
    )?
    .unwrap_or_default();
    let mut tests = BTreeSet::new();
    for file in listing.lines().filter(|file| is_test_module(file)) {
        for test in python::tests(&show(upstream, revision, file)?) {
            tests.insert(format!("{file}::{test}"));
        }
    }
    Ok(tests)
}

/// The compared facts of the upstream SDK at one revision.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Snapshot {
    /// Long options the transport passes to the CLI.
    pub cli_flags: BTreeSet<String>,
    /// Fields of `ClaudeAgentOptions`.
    pub option_fields: BTreeSet<String>,
    /// Control request subtypes.
    pub control_subtypes: BTreeSet<String>,
    /// Names of `__all__`.
    pub exports: BTreeSet<String>,
    /// Unit tests under `tests/`.
    pub tests: BTreeSet<String>,
    /// End-to-end tests under `e2e-tests/`.
    pub e2e_tests: BTreeSet<String>,
    /// Version headings of the changelog.
    pub changelog: BTreeSet<String>,
}

/// Reads the snapshot of one revision of the submodule.
pub fn snapshot(upstream: &Path, revision: &str) -> Result<Snapshot, String> {
    Ok(Snapshot {
        cli_flags: python::cli_flags(&show(upstream, revision, TRANSPORT)?),
        option_fields: python::class_fields(&show(upstream, revision, TYPES)?, OPTIONS_CLASS),
        control_subtypes: python::control_subtypes(
            &show(upstream, revision, TYPES)?,
            &show(upstream, revision, QUERY)?,
        ),
        exports: python::exports(&show(upstream, revision, INIT)?)
            .into_iter()
            .collect(),
        tests: tests_under(upstream, revision, "tests")?,
        e2e_tests: tests_under(upstream, revision, "e2e-tests")?,
        changelog: python::changelog_versions(&show(upstream, revision, CHANGELOG)?)
            .into_iter()
            .collect(),
    })
}

fn section(report: &mut String, title: &str, base: &BTreeSet<String>, target: &BTreeSet<String>) {
    let added: Vec<&String> = target.difference(base).collect();
    let removed: Vec<&String> = base.difference(target).collect();
    let _ = writeln!(report, "\n{title}: +{} -{}", added.len(), removed.len());
    for item in added {
        let _ = writeln!(report, "  + {item}");
    }
    for item in removed {
        let _ = writeln!(report, "  - {item}");
    }
}

/// Renders the differences between two snapshots.
pub fn render(heading: &str, base: &Snapshot, target: &Snapshot) -> String {
    let mut report = format!("{heading}\n");
    section(&mut report, "CLI flags", &base.cli_flags, &target.cli_flags);
    section(
        &mut report,
        "Option fields (ClaudeAgentOptions)",
        &base.option_fields,
        &target.option_fields,
    );
    section(
        &mut report,
        "Control subtypes",
        &base.control_subtypes,
        &target.control_subtypes,
    );
    section(
        &mut report,
        "Exported names",
        &base.exports,
        &target.exports,
    );
    section(&mut report, "Unit tests", &base.tests, &target.tests);
    section(
        &mut report,
        "End-to-end tests",
        &base.e2e_tests,
        &target.e2e_tests,
    );
    section(
        &mut report,
        "Changelog versions",
        &base.changelog,
        &target.changelog,
    );
    report
}

fn short(hash: &str) -> &str {
    hash.get(..7).unwrap_or(hash)
}

/// Compares the pinned commit of the submodule with `revision`.
pub fn run(upstream: &Path, revision: &str) -> Result<String, String> {
    let pinned = commit(upstream, "HEAD")?.ok_or_else(|| {
        format!(
            "{} is not a git checkout; run `git submodule update --init`",
            upstream.display()
        )
    })?;
    let target = resolve(upstream, revision)?;
    let heading = format!(
        "upstream-diff: {} (pinned) -> {revision} ({})",
        short(&pinned),
        short(&target)
    );
    Ok(render(
        &heading,
        &snapshot(upstream, &pinned)?,
        &snapshot(upstream, &target)?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(items: &[&str]) -> BTreeSet<String> {
        items.iter().map(|item| (*item).to_owned()).collect()
    }

    #[test]
    fn render_lists_additions_and_removals_per_category() {
        let base = Snapshot {
            cli_flags: set(&["--verbose", "--old-flag"]),
            exports: set(&["query"]),
            ..Snapshot::default()
        };
        let target = Snapshot {
            cli_flags: set(&["--verbose", "--new-flag"]),
            exports: set(&["query", "NewExport"]),
            tests: set(&["tests/test_new.py::test_added"]),
            ..Snapshot::default()
        };
        let report = render("heading", &base, &target);
        assert!(report.starts_with("heading\n"));
        assert!(report.contains("CLI flags: +1 -1\n  + --new-flag\n  - --old-flag\n"));
        assert!(report.contains("Exported names: +1 -0\n  + NewExport\n"));
        assert!(report.contains("Unit tests: +1 -0\n  + tests/test_new.py::test_added\n"));
        assert!(report.contains("Option fields (ClaudeAgentOptions): +0 -0\n"));
        assert!(report.contains("Control subtypes: +0 -0\n"));
        assert!(report.contains("End-to-end tests: +0 -0\n"));
    }

    #[test]
    fn test_modules_are_recognized_by_name() {
        assert!(is_test_module("tests/test_query.py"));
        assert!(is_test_module("tests/nested/query_test.py"));
        assert!(!is_test_module("tests/conftest.py"));
        assert!(!is_test_module("tests/test_data.json"));
    }
}
