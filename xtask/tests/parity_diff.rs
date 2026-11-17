//! `cargo xtask parity-diff`, run as the `xtask` binary against fixtures
//! that fail before any driver runs: the harness must exit non-zero, name
//! the cause, and never report a match.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn repository() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
}

struct Fixture {
    _directory: tempfile::TempDir,
    upstream: PathBuf,
    parity: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let upstream = directory.path().join("upstream");
        let parity = directory.path().join("parity");
        std::fs::create_dir_all(upstream.join("src/claude_agent_sdk")).unwrap();
        std::fs::write(upstream.join("src/claude_agent_sdk/__init__.py"), "").unwrap();
        std::fs::create_dir_all(parity.join("cases")).unwrap();
        std::fs::create_dir_all(parity.join("scenarios")).unwrap();
        std::fs::copy(
            repository().join("parity/case.schema.json"),
            parity.join("case.schema.json"),
        )
        .unwrap();
        std::fs::write(parity.join("differences.toml"), "").unwrap();
        std::fs::write(
            parity.join("cases/default.json"),
            r#"{"name": "default", "scenario": {}, "prompt": "hi"}"#,
        )
        .unwrap();
        Self {
            _directory: directory,
            upstream,
            parity,
        }
    }

    fn run(&self, extra: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_xtask"))
            .arg("parity-diff")
            .arg("--upstream")
            .arg(&self.upstream)
            .arg("--parity")
            .arg(&self.parity)
            .args(extra)
            .env_remove("PARITY_PYTHON")
            .output()
            .unwrap()
    }
}

fn assert_fails_without_a_match(output: &Output, message: &str) {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "{stdout}{stderr}");
    assert!(
        stderr.contains(message),
        "expected `{message}` in: {stderr}"
    );
    assert!(!stdout.contains("match"), "{stdout}");
}

#[test]
fn an_empty_submodule_stops_the_run_before_any_case() {
    let fixture = Fixture::new();
    std::fs::remove_file(fixture.upstream.join("src/claude_agent_sdk/__init__.py")).unwrap();
    let output = fixture.run(&[]);
    assert_fails_without_a_match(
        &output,
        "upstream submodule is empty: run git submodule update --init",
    );
}

#[test]
fn a_missing_python_fails_the_run_and_reports_no_match() {
    let fixture = Fixture::new();
    let missing = fixture.parity.join("no-such-python");
    let output = fixture.run(&["--python", missing.to_str().unwrap()]);
    assert_fails_without_a_match(&output, "parity-diff: python 3.10 or later not found");
}

#[cfg(unix)]
#[test]
fn a_missing_python_dependency_is_named() {
    use std::os::unix::fs::PermissionsExt as _;

    let fixture = Fixture::new();
    let python = fixture.parity.join("python");
    std::fs::write(
        &python,
        "#!/bin/sh\ncase \"$2\" in\n  *version_info*) echo 3.13 ;;\n  *) echo \"ModuleNotFoundError: No module named 'mcp'\" >&2; exit 1 ;;\nesac\n",
    )
    .unwrap();
    std::fs::set_permissions(&python, std::fs::Permissions::from_mode(0o755)).unwrap();
    let output = fixture.run(&["--python", python.to_str().unwrap()]);
    assert_fails_without_a_match(&output, "No module named 'mcp'");
}

#[test]
fn a_case_that_breaks_the_schema_is_rejected_with_its_file_and_pointer() {
    let fixture = Fixture::new();
    std::fs::write(
        fixture.parity.join("cases/default.json"),
        r#"{"name": "default", "scenario": {}, "prompt": "hi", "steps": [{"wave": true}]}"#,
    )
    .unwrap();
    let output = fixture.run(&[]);
    assert_fails_without_a_match(&output, "default.json: does not match");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains(" at /"), "{stderr}");
}

#[test]
fn a_python_defect_entry_without_its_upstream_line_fails_to_load() {
    let fixture = Fixture::new();
    std::fs::write(
        fixture.parity.join("differences.toml"),
        "[wire.bad]\ncase = \"*\"\nlocation = \"env A\"\npython = '1'\ncategory = \"python-defect\"\nreason = \"r\"\n",
    )
    .unwrap();
    let output = fixture.run(&[]);
    assert_fails_without_a_match(&output, "must cite the upstream line at fault");
}

#[test]
fn an_unknown_case_name_is_rejected() {
    let fixture = Fixture::new();
    let output = fixture.run(&["nonexistent"]);
    assert_fails_without_a_match(&output, "no case named nonexistent");
}
