//! `upstream-diff`: what changed in the upstream SDK between the pinned
//! commit and another revision, read from the git objects of the submodule.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::Path;
use std::process::Command;

use super::ledger::{self, Ledger};
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

/// The commit `revision` names in `upstream`, `None` when it names none.
pub fn commit(upstream: &Path, revision: &str) -> Result<Option<String>, String> {
    let object = format!("{revision}^{{commit}}");
    Ok(
        git(upstream, &["rev-parse", "--verify", "--quiet", &object])?
            .map(|hash| hash.trim().to_owned()),
    )
}

/// The commit `revision` names in `upstream`, fetching the tags of
/// `origin` when it is not there yet.
pub fn resolve(upstream: &Path, revision: &str) -> Result<String, String> {
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

/// The content of `file` at `revision`, empty when the file is missing.
pub fn show(upstream: &Path, revision: &str, file: &str) -> Result<String, String> {
    Ok(git(upstream, &["show", &format!("{revision}:{file}")])?.unwrap_or_default())
}

fn is_test_module(file: &str) -> bool {
    let name = file.rsplit('/').next().unwrap_or(file);
    name.ends_with(".py") && (name.starts_with("test_") || name.ends_with("_test.py"))
}

/// The tests under `directory`, each with its normalized source.
fn tests_under(
    upstream: &Path,
    revision: &str,
    directory: &str,
) -> Result<BTreeMap<String, String>, String> {
    let listing = git(
        upstream,
        &["ls-tree", "-r", "--name-only", revision, "--", directory],
    )?
    .unwrap_or_default();
    let mut tests = BTreeMap::new();
    for file in listing.lines().filter(|file| is_test_module(file)) {
        for (test, source) in python::test_sources(&show(upstream, revision, file)?) {
            tests.insert(format!("{file}::{test}"), source);
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
    /// The normalized source of each unit test, to find a test rewritten
    /// in place, which keeps its name.
    pub test_sources: BTreeMap<String, String>,
    /// End-to-end tests under `e2e-tests/`.
    pub e2e_tests: BTreeSet<String>,
    /// Version headings of the changelog.
    pub changelog: BTreeSet<String>,
}

/// Reads the snapshot of one revision of the submodule.
pub fn snapshot(upstream: &Path, revision: &str) -> Result<Snapshot, String> {
    let test_sources = tests_under(upstream, revision, "tests")?;
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
        tests: test_sources.keys().cloned().collect(),
        test_sources,
        e2e_tests: tests_under(upstream, revision, "e2e-tests")?
            .into_keys()
            .collect(),
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

/// The unit tests both snapshots hold whose source differs, each with what
/// `tests` (the tests ledger, when given) says covers it, so the Rust side
/// gets reviewed: a test rewritten in place keeps its name, so the lists of
/// added and removed tests miss it.
fn changed_tests(report: &mut String, base: &Snapshot, target: &Snapshot, tests: Option<&Ledger>) {
    let changed: Vec<&String> = base
        .test_sources
        .iter()
        .filter(|(test, source)| {
            target
                .test_sources
                .get(*test)
                .is_some_and(|other| other != *source)
        })
        .map(|(test, _)| test)
        .collect();
    let _ = writeln!(report, "\nUnit tests changed in place: {}", changed.len());
    for test in changed {
        let _ = writeln!(report, "  ~ {test}");
        let Some(tests) = tests else {
            continue;
        };
        let covered_by = match tests.entries.get(test) {
            None => "not in the ledger".to_owned(),
            Some(entry) => match (entry.state(), &entry.excluded, &entry.pending) {
                (Some(ledger::State::Excluded), Some(reason), _) => format!("excluded: {reason}"),
                (Some(ledger::State::Pending), _, Some(reason)) => format!("pending: {reason}"),
                _ => entry.paths().cloned().collect::<Vec<_>>().join(", "),
            },
        };
        let _ = writeln!(report, "      review: {covered_by}");
    }
}

/// Renders the differences between two snapshots.
pub fn render(heading: &str, base: &Snapshot, target: &Snapshot, tests: Option<&Ledger>) -> String {
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
    changed_tests(&mut report, base, target, tests);
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

/// The upstream files a release that only moves versions may change.
pub const VERSION_FILES: [&str; 3] = [
    "src/claude_agent_sdk/_cli_version.py",
    "src/claude_agent_sdk/_version.py",
    CHANGELOG,
];
const MANIFEST: &str = "pyproject.toml";

/// Whether an upstream change needs more than moving the pinned versions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Only [`VERSION_FILES`] and the version line of `pyproject.toml`
    /// changed: moving the copies of the versions is the whole port.
    CliOnly,
    /// Anything else changed: the port needs code and ledger work.
    NeedsPort,
}

impl Verdict {
    /// The word `upstream-diff` and `upstream-sync` print.
    pub fn word(self) -> &'static str {
        match self {
            Self::CliOnly => "cli-only",
            Self::NeedsPort => "needs-port",
        }
    }
}

/// The verdict of the change between the commits `base` and `target`,
/// from the files it touches.
pub fn verdict(upstream: &Path, base: &str, target: &str) -> Result<Verdict, String> {
    let files = git(upstream, &["diff", "--name-only", base, target])?
        .ok_or_else(|| format!("cannot list the files changed between {base} and {target}"))?;
    for file in files.lines().filter(|file| !file.is_empty()) {
        if VERSION_FILES.contains(&file) {
            continue;
        }
        if file != MANIFEST {
            return Ok(Verdict::NeedsPort);
        }
        let patch = git(
            upstream,
            &["diff", "--unified=0", base, target, "--", MANIFEST],
        )?
        .ok_or_else(|| format!("cannot read the change of {MANIFEST}"))?;
        let only_version = patch
            .lines()
            .filter(|line| {
                (line.starts_with('+') || line.starts_with('-'))
                    && !line.starts_with("+++")
                    && !line.starts_with("---")
            })
            .all(|line| line[1..].trim_start().starts_with("version"));
        if !only_version {
            return Ok(Verdict::NeedsPort);
        }
    }
    Ok(Verdict::CliOnly)
}

/// Compares the pinned commit of the submodule with `revision`. With
/// `tests_ledger`, each test changed in place names the Rust tests to
/// review.
pub fn run(upstream: &Path, revision: &str, tests_ledger: Option<&Path>) -> Result<String, String> {
    let tests = tests_ledger.map(ledger::load_tests).transpose()?;
    let pinned = commit(upstream, "HEAD")?.ok_or_else(|| {
        format!(
            "{} is not a git checkout; run `git submodule update --init`",
            upstream.display()
        )
    })?;
    let target = resolve(upstream, revision)?;
    let heading = format!(
        "upstream-diff: {} (pinned) -> {revision} ({})\nverdict: {}",
        short(&pinned),
        short(&target),
        verdict(upstream, &pinned, &target)?.word()
    );
    Ok(render(
        &heading,
        &snapshot(upstream, &pinned)?,
        &snapshot(upstream, &target)?,
        tests.as_ref(),
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
        let report = render("heading", &base, &target, None);
        assert!(report.starts_with("heading\n"));
        assert!(report.contains("CLI flags: +1 -1\n  + --new-flag\n  - --old-flag\n"));
        assert!(report.contains("Exported names: +1 -0\n  + NewExport\n"));
        assert!(report.contains("Unit tests: +1 -0\n  + tests/test_new.py::test_added\n"));
        assert!(report.contains("Option fields (ClaudeAgentOptions): +0 -0\n"));
        assert!(report.contains("Control subtypes: +0 -0\n"));
        assert!(report.contains("End-to-end tests: +0 -0\n"));
    }

    #[test]
    fn a_test_rewritten_in_place_is_listed_with_the_rust_tests_to_review() {
        let sources = |body: &str| {
            [
                ("tests/test_a.py::test_kept".to_owned(), "same\n".to_owned()),
                (
                    "tests/test_a.py::test_rewritten".to_owned(),
                    body.to_owned(),
                ),
            ]
            .into_iter()
            .collect::<BTreeMap<_, _>>()
        };
        let base = Snapshot {
            test_sources: sources("old\n"),
            ..Snapshot::default()
        };
        let target = Snapshot {
            test_sources: sources("new\n"),
            ..Snapshot::default()
        };
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("tests.toml");
        std::fs::write(
            &path,
            "[tests.\"tests/test_a.py\"]\n\
             test_kept = { rust = \"tests/a.rs::kept\" }\n\
             test_rewritten = { rust = \"tests/a.rs::rewritten\" }\n",
        )
        .unwrap();
        let ledger = ledger::load_tests(&path).unwrap();
        let report = render("heading", &base, &target, Some(&ledger));
        assert!(
            report.contains(
                "Unit tests changed in place: 1\n  ~ tests/test_a.py::test_rewritten\n      \
                 review: tests/a.rs::rewritten\n"
            ),
            "{report}"
        );
        assert!(!report.contains("test_kept"), "{report}");
        let unlisted = render("heading", &base, &target, None);
        assert!(
            unlisted.contains("  ~ tests/test_a.py::test_rewritten\n\n"),
            "{unlisted}"
        );
    }

    #[test]
    fn test_modules_are_recognized_by_name() {
        assert!(is_test_module("tests/test_query.py"));
        assert!(is_test_module("tests/nested/query_test.py"));
        assert!(!is_test_module("tests/conftest.py"));
        assert!(!is_test_module("tests/test_data.json"));
    }
}
