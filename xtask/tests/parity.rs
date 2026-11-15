//! The parity commands, run as the `xtask` binary against a small upstream
//! checkout and ledgers written by each test.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const EXISTING_PATH: &str = "claude_agent_kit::query";
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
        write(&root.join("parity/surface.toml"), surface);
        write(&root.join("parity/tests.toml"), tests);
        write(
            &root.join("migration.md"),
            "# Guide\n\n<!-- parity:table:begin -->\n<!-- parity:table:end -->\n",
        );
        let fixture = Self {
            _directory: directory,
            root,
        };
        let table = fixture.run("migration-table", &[]);
        assert!(table.status.success(), "{table:?}");
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
            .args(extra)
            .output()
            .unwrap()
    }
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
fn consistent_ledgers_pass_and_the_report_gives_the_three_shares() {
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
            ("Missing", &rust("claude_agent_kit::NoSuchItemAnywhere")),
        ]),
        &tests_ledger(&rust(EXISTING_TEST)),
    );

    let check = fixture.run("parity-check", &[]);

    assert!(!check.status.success());
    let stderr = stderr(&check);
    assert!(
        stderr.contains(
            "parity/surface.toml: `Missing` names Rust path \
             `claude_agent_kit::NoSuchItemAnywhere`, which does not exist"
        ),
        "{stderr}"
    );
    assert!(!stderr.contains("`query` names"), "{stderr}");
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
        table.contains("| `query` | `claude_agent_kit::query` |"),
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
