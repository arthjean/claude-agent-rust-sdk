//! The parity commands, run as the `xtask` binary against a small upstream
//! checkout (a git repository with one commit), ledgers, cases, and pages
//! written by each test.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const EXISTING_PATH: &str = "claude_agent_rust_sdk::query";
const EXISTING_TEST: &str =
    "xtask/tests/parity.rs::this_test_exists_so_a_ledger_can_name_a_real_rust_test";

struct Fixture {
    _directory: tempfile::TempDir,
    root: PathBuf,
}

impl Fixture {
    fn new(exports: &[&str], surface: &str, tests: &str) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().to_path_buf();
        let quoted: Vec<String> = exports
            .iter()
            .map(|name| format!("    \"{name}\","))
            .collect();
        write(
            &root.join("upstream/src/claude_agent_sdk/__init__.py"),
            &format!("__all__ = [\n{}\n]\n", quoted.join("\n")),
        );
        write(
            &root.join("upstream/tests/test_sample.py"),
            "def test_one():\n    assert True\n",
        );
        write(
            &root.join("upstream/pyproject.toml"),
            "[project]\nname = \"claude-agent-sdk\"\nversion = \"9.8.7\"\n",
        );
        write(
            &root.join("upstream/src/claude_agent_sdk/types.py"),
            "class ClaudeAgentOptions:\n    model: str | None = None\n    cli_path: str | None = None\n",
        );
        commit(&root.join("upstream"));
        write(&root.join("parity/surface.toml"), surface);
        write(&root.join("parity/tests.toml"), tests);
        write(&root.join("parity/differences.toml"), "");
        write(
            &root.join("parity/cases/default.json"),
            r#"{"name": "default", "scenario": {}, "prompt": "hi", "options": {"model": "m"}}"#,
        );
        write(
            &root.join("parity/cases/coverage.toml"),
            "[excluded]\ncli_path = \"Set by the drivers.\"\n",
        );
        std::fs::create_dir_all(root.join("parity/scenarios")).unwrap();
        std::fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../parity/case.schema.json"),
            root.join("parity/case.schema.json"),
        )
        .unwrap();
        write(
            &root.join("migration.md"),
            "# Guide\n\n<!-- parity:table:begin -->\n<!-- parity:table:end -->\n",
        );
        write(&root.join("parity.md"), PAGE);
        let fixture = Self {
            _directory: directory,
            root,
        };
        for command in ["migration-table", "parity-page"] {
            let output = fixture.run(command, &[]);
            assert!(output.status.success(), "{command}: {}", stderr(&output));
        }
        fixture
    }

    fn run(&self, command: &str, extra: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_xtask"))
            .arg(command)
            .arg("--upstream")
            .arg(self.root.join("upstream"))
            .arg("--parity")
            .arg(self.root.join("parity"))
            .arg("--migration")
            .arg(self.root.join("migration.md"))
            .arg("--page")
            .arg(self.root.join("parity.md"))
            .args(extra)
            .output()
            .unwrap()
    }
}

const PAGE: &str = "# Parity\n\nWritten by hand.\n\n<!-- parity:page:begin -->\n<!-- parity:page:end -->\n\n## What Rust adds\n";

/// Makes `directory` a git repository holding one commit of its files.
fn commit(directory: &Path) {
    let git = |arguments: &[&str]| {
        let output = Command::new("git")
            .arg("-C")
            .arg(directory)
            .args([
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(arguments)
            .output()
            .unwrap();
        assert!(output.status.success(), "git {arguments:?}: {output:?}");
    };
    git(&["init", "--quiet"]);
    git(&["add", "--all"]);
    git(&["commit", "--quiet", "--message", "Upstream"]);
}

fn write(path: &Path, content: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
}

fn surface(entries: &[(&str, &str)]) -> String {
    let lines: Vec<String> = entries
        .iter()
        .map(|(name, entry)| format!("{name} = {entry}"))
        .collect();
    format!("[names]\n{}\n", lines.join("\n"))
}

fn tests_ledger(entry: &str) -> String {
    format!("[tests.\"tests/test_sample.py\"]\ntest_one = {entry}\n")
}

fn rust(path: &str) -> String {
    format!("{{ rust = \"{path}\" }}")
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn this_test_exists_so_a_ledger_can_name_a_real_rust_test() {}

#[test]
fn consistent_ledgers_pass_and_the_report_gives_the_four_shares() {
    let fixture = Fixture::new(
        &["query"],
        &surface(&[("query", &rust(EXISTING_PATH))]),
        &tests_ledger(&rust(EXISTING_TEST)),
    );

    let check = fixture.run("parity-check", &["--deny-pending"]);
    assert!(check.status.success(), "{}", stderr(&check));
    assert!(
        stderr(&check).contains("1 exports and 1 tests are listed"),
        "{}",
        stderr(&check)
    );

    let report = fixture.run("parity-report", &[]);
    assert!(report.status.success(), "{}", stderr(&report));
    let text = String::from_utf8_lossy(&report.stdout).replace("\r\n", "\n");
    for line in [
        "parity/surface.toml: 1 upstream exports",
        "parity/tests.toml: 1 upstream tests",
        "  implemented      1  100.0%",
        "  adapted          0    0.0%",
        "  excluded         0    0.0%",
        "  pending          0    0.0%",
    ] {
        assert!(text.contains(line), "no `{line}` in:\n{text}");
    }
}

#[test]
fn an_export_added_upstream_without_a_ledger_update_fails_the_check_by_name() {
    let fixture = Fixture::new(
        &["query", "BrandNewExport"],
        &surface(&[("query", &rust(EXISTING_PATH))]),
        &tests_ledger(&rust(EXISTING_TEST)),
    );

    let check = fixture.run("parity-check", &[]);

    assert!(!check.status.success());
    assert!(
        stderr(&check)
            .contains("parity/surface.toml: upstream export `BrandNewExport` is not listed"),
        "{}",
        stderr(&check)
    );
}

#[test]
fn a_test_added_upstream_without_a_ledger_update_fails_the_check_by_name() {
    let fixture = Fixture::new(
        &["query"],
        &surface(&[("query", &rust(EXISTING_PATH))]),
        &tests_ledger(&rust(EXISTING_TEST)),
    );
    write(
        &fixture.root.join("upstream/tests/test_added.py"),
        "class TestAdded:\n    def test_new(self):\n        assert True\n",
    );

    let check = fixture.run("parity-check", &[]);

    assert!(!check.status.success());
    assert!(
        stderr(&check).contains(
            "parity/tests.toml: upstream test `tests/test_added.py::TestAdded::test_new` is not \
             listed"
        ),
        "{}",
        stderr(&check)
    );
}

#[test]
fn a_rust_path_that_does_not_exist_fails_the_check() {
    let fixture = Fixture::new(
        &["query", "Missing"],
        &surface(&[
            ("query", &rust(EXISTING_PATH)),
            (
                "Missing",
                &rust("claude_agent_rust_sdk::NoSuchItemAnywhere"),
            ),
        ]),
        &tests_ledger(&rust(EXISTING_TEST)),
    );

    let check = fixture.run("parity-check", &[]);

    assert!(!check.status.success());
    let stderr = stderr(&check);
    assert!(
        stderr.contains(
            "parity/surface.toml: `Missing` names Rust path \
             `claude_agent_rust_sdk::NoSuchItemAnywhere`, which does not exist"
        ),
        "{stderr}"
    );
    assert!(!stderr.contains("`query` names"), "{stderr}");
}

#[test]
fn a_ledger_entry_still_naming_the_former_crate_fails_the_check_by_name() {
    let former = ["claude_agent", "kit"].join("_");
    let fixture = Fixture::new(
        &["query"],
        &surface(&[("query", &rust(&format!("{former}::query")))]),
        &tests_ledger(&rust(EXISTING_TEST)),
    );

    let check = fixture.run("parity-check", &[]);

    assert!(!check.status.success());
    assert!(
        stderr(&check).contains(&format!(
            "parity/surface.toml: `query` names Rust path `{former}::query`, which does not exist"
        )),
        "{}",
        stderr(&check)
    );
}

#[test]
fn a_rust_test_that_does_not_exist_fails_the_check() {
    let fixture = Fixture::new(
        &["query"],
        &surface(&[("query", &rust(EXISTING_PATH))]),
        &tests_ledger(&rust("xtask/tests/parity.rs::no_such_test_anywhere")),
    );

    let check = fixture.run("parity-check", &[]);

    assert!(!check.status.success());
    assert!(
        stderr(&check).contains(
            "parity/tests.toml: `tests/test_sample.py::test_one` names Rust path \
             `xtask/tests/parity.rs::no_such_test_anywhere`, which does not exist"
        ),
        "{}",
        stderr(&check)
    );
}

#[test]
fn a_test_of_another_platform_is_checked_against_the_sources() {
    let elsewhere = if cfg!(windows) { "unix" } else { "windows" };
    let declared = Fixture::new(
        &["query"],
        &surface(&[("query", &rust(EXISTING_PATH))]),
        &tests_ledger(&rust(&format!("{EXISTING_TEST} @{elsewhere}"))),
    );
    let check = declared.run("parity-check", &[]);
    assert!(check.status.success(), "{}", stderr(&check));

    let undeclared = "xtask/tests/parity.rs::declared_on_no_platform";
    let missing = Fixture::new(
        &["query"],
        &surface(&[("query", &rust(EXISTING_PATH))]),
        &tests_ledger(&rust(&format!("{undeclared} @{elsewhere}"))),
    );
    let check = missing.run("parity-check", &[]);
    assert!(!check.status.success());
    assert!(
        stderr(&check).contains(&format!(
            "parity/tests.toml: `tests/test_sample.py::test_one` names Rust path \
             `{undeclared} @{elsewhere}`, which does not exist"
        )),
        "{}",
        stderr(&check)
    );
}

#[test]
fn a_pending_entry_passes_by_default_and_fails_with_deny_pending() {
    let fixture = Fixture::new(
        &["query"],
        &surface(&[("query", &rust(EXISTING_PATH))]),
        &tests_ledger("{ pending = \"not ported yet\" }"),
    );

    let lenient = fixture.run("parity-check", &[]);
    assert!(lenient.status.success(), "{}", stderr(&lenient));

    let strict = fixture.run("parity-check", &["--deny-pending"]);
    assert!(!strict.status.success());
    assert!(
        stderr(&strict).contains("`tests/test_sample.py::test_one` is pending: not ported yet"),
        "{}",
        stderr(&strict)
    );
}

#[test]
fn a_migration_table_out_of_date_with_the_ledger_fails_the_check() {
    let fixture = Fixture::new(
        &["query"],
        &surface(&[("query", &rust(EXISTING_PATH))]),
        &tests_ledger(&rust(EXISTING_TEST)),
    );
    let guide = fixture.root.join("migration.md");
    let table = std::fs::read_to_string(&guide).unwrap();
    assert!(
        table.contains("| `query` | `claude_agent_rust_sdk::query` |"),
        "{table}"
    );
    write(
        &guide,
        "# Guide\n\n<!-- parity:table:begin -->\n<!-- parity:table:end -->\n",
    );

    let check = fixture.run("parity-check", &[]);

    assert!(!check.status.success());
    assert!(
        stderr(&check).contains("run `cargo xtask migration-table`"),
        "{}",
        stderr(&check)
    );
}

#[test]
fn an_adapted_entry_is_resolved_reported_apart_and_rendered_with_its_note() {
    let adapted =
        format!("{{ adapted = \"{EXISTING_PATH}\", note = \"A stream where Python yields.\" }}");
    let fixture = Fixture::new(
        &["query", "Gone"],
        &surface(&[
            ("query", &adapted),
            (
                "Gone",
                "{ adapted = \"claude_agent_rust_sdk::NoSuchAdaptedItem\", note = \"Moved.\" }",
            ),
        ]),
        &tests_ledger(&rust(EXISTING_TEST)),
    );

    let check = fixture.run("parity-check", &[]);
    assert!(!check.status.success());
    let text = stderr(&check);
    assert!(
        text.contains(
            "parity/surface.toml: `Gone` names Rust path `claude_agent_rust_sdk::NoSuchAdaptedItem`, which does not exist"
        ),
        "{text}"
    );
    assert!(!text.contains("`query` names"), "{text}");

    let report = fixture.run("parity-report", &[]);
    assert!(report.status.success(), "{}", stderr(&report));
    let text = String::from_utf8_lossy(&report.stdout).replace("\r\n", "\n");
    for line in [
        "parity/surface.toml: 2 upstream exports",
        "  implemented      0    0.0%",
        "  adapted          2  100.0%",
    ] {
        assert!(text.contains(line), "no `{line}` in:\n{text}");
    }

    let guide = std::fs::read_to_string(fixture.root.join("migration.md")).unwrap();
    assert!(
        guide.contains(
            "| `query` | `claude_agent_rust_sdk::query` | Adapted: A stream where Python yields. |"
        ),
        "{guide}"
    );
}

#[test]
fn an_adapted_entry_without_a_note_fails_the_check_by_name() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    write(
        &root.join("upstream/src/claude_agent_sdk/__init__.py"),
        "__all__ = [\"query\"]\n",
    );
    write(
        &root.join("upstream/tests/test_sample.py"),
        "def test_one():\n    pass\n",
    );
    write(
        &root.join("parity/surface.toml"),
        &surface(&[("query", &format!("{{ adapted = \"{EXISTING_PATH}\" }}"))]),
    );
    write(
        &root.join("parity/tests.toml"),
        &tests_ledger(&rust(EXISTING_TEST)),
    );

    let check = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .arg("parity-check")
        .arg("--upstream")
        .arg(root.join("upstream"))
        .arg("--parity")
        .arg(root.join("parity"))
        .output()
        .unwrap();

    assert!(!check.status.success());
    assert!(
        stderr(&check).contains(
            "parity/surface.toml: `query` is adapted but has no `note` saying how the Rust item differs"
        ),
        "{}",
        stderr(&check)
    );
}

fn page(fixture: &Fixture) -> String {
    std::fs::read_to_string(fixture.root.join("parity.md"))
        .unwrap()
        .replace("\r\n", "\n")
}

fn consistent() -> Fixture {
    Fixture::new(
        &["query"],
        &surface(&[("query", &rust(EXISTING_PATH))]),
        &tests_ledger(&rust(EXISTING_TEST)),
    )
}

#[test]
fn the_page_states_the_upstream_version_commit_counts_and_coverage() {
    let fixture = consistent();
    let commit = Command::new("git")
        .arg("-C")
        .arg(fixture.root.join("upstream"))
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap();
    let commit = String::from_utf8_lossy(&commit.stdout).trim().to_owned();

    let text = page(&fixture);
    for expected in [
        "# Parity\n\nWritten by hand.\n\n<!-- parity:page:begin -->\n## What was compared",
        &format!("Python Claude Agent SDK 9.8.7, at upstream commit `{commit}`, committed on "),
        "| Implemented | 1 (100.0%) | 1 (100.0%) |",
        "| Adapted | 0 (0.0%) | 0 (0.0%) |",
        "| Total | 1 | 1 |",
        "plays 1 cases",
        "The cases set 1 of the 2 fields of `ClaudeAgentOptions`; 1 are excluded",
        "0 differences are reviewed and accepted",
        "## Not yet classified (0)",
        "<!-- parity:page:end -->\n\n## What Rust adds\n",
    ] {
        assert!(text.contains(expected), "no `{expected}` in:\n{text}");
    }
    let check = fixture.run("parity-check", &["--deny-pending"]);
    assert!(check.status.success(), "{}", stderr(&check));
}

#[test]
fn a_stale_page_fails_the_check_and_page_check_only_compares() {
    let fixture = consistent();
    write(
        &fixture.root.join("parity/differences.toml"),
        "[unclassified.later]\nsource = \"4.4 item 10\"\nsummary = \"Budget spelling.\"\n",
    );
    let before = page(&fixture);

    let check = fixture.run("parity-page", &["--check"]);
    assert!(!check.status.success());
    assert!(
        stderr(&check).contains("is out of date with the ledgers; run `cargo xtask parity-page`"),
        "{}",
        stderr(&check)
    );
    assert_eq!(page(&fixture), before);
    let check = fixture.run("parity-check", &[]);
    assert!(!check.status.success());
    assert!(
        stderr(&check).contains("run `cargo xtask parity-page`"),
        "{}",
        stderr(&check)
    );

    let write_page = fixture.run("parity-page", &[]);
    assert!(write_page.status.success(), "{}", stderr(&write_page));
    let text = page(&fixture);
    assert!(text.contains("## Not yet classified (1)"), "{text}");
    assert!(
        text.contains("| 4.4 item 10 | Budget spelling. |"),
        "{text}"
    );
    let check = fixture.run("parity-check", &[]);
    assert!(check.status.success(), "{}", stderr(&check));
}

#[test]
fn a_behavior_entry_naming_a_missing_rust_test_fails_the_check_by_name() {
    let fixture = consistent();
    let entry = |test: &str| {
        format!(
            "[behavior.kept]\nsummary = \"An unknown message\"\npython = \"Skipped.\"\nrust = \"Kept.\"\n\
             ref = \"tests/test_sample.py:1-2\"\nanchor = \"def test_one\"\ncategory = \"rust-more-robust\"\nreason = \"No loss.\"\n\
             test = \"{test}\"\n"
        )
    };
    write(
        &fixture.root.join("parity/differences.toml"),
        &entry("xtask/tests/parity.rs::no_such_behavior_test"),
    );

    let check = fixture.run("parity-check", &[]);
    assert!(!check.status.success());
    assert!(
        stderr(&check).contains(
            "parity/differences.toml: `behavior.kept` names Rust test \
             `xtask/tests/parity.rs::no_such_behavior_test`, which does not exist"
        ),
        "{}",
        stderr(&check)
    );

    write(
        &fixture.root.join("parity/differences.toml"),
        &entry(EXISTING_TEST),
    );
    let write_page = fixture.run("parity-page", &[]);
    assert!(write_page.status.success(), "{}", stderr(&write_page));
    let text = page(&fixture);
    assert!(
        text.contains("### Rust handles what stops Python (1)"),
        "{text}"
    );
    assert!(
        text.contains(&format!(
            "| An unknown message | Skipped. (upstream `tests/test_sample.py:1-2`) | Kept. | No loss. | `{EXISTING_TEST}` |"
        )),
        "{text}"
    );
    let check = fixture.run("parity-check", &[]);
    assert!(check.status.success(), "{}", stderr(&check));
}

#[test]
fn a_hand_written_claim_of_full_parity_fails_the_check() {
    let fixture = consistent();
    let boastful = page(&fixture).replace("Written by hand.", "Full parity with upstream.");
    write(&fixture.root.join("parity.md"), &boastful);

    let check = fixture.run("parity-check", &[]);

    assert!(!check.status.success());
    assert!(
        stderr(&check).contains("the hand-written text claims `full parity`"),
        "{}",
        stderr(&check)
    );
}

#[test]
fn a_claim_of_parity_in_the_migration_guide_fails_the_check_too() {
    let fixture = consistent();
    let guide = std::fs::read_to_string(fixture.root.join("migration.md")).unwrap();
    write(
        &fixture.root.join("migration.md"),
        &format!("The crate is at parity with the Python SDK.\n\n{guide}"),
    );

    let check = fixture.run("parity-check", &[]);

    assert!(!check.status.success());
    assert!(
        stderr(&check).contains("migration.md: the hand-written text claims `at parity with`"),
        "{}",
        stderr(&check)
    );
}
