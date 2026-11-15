//! `upstream-diff`, run as the `xtask` binary against a small git
//! repository standing in for the upstream checkout.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::Path;
use std::process::Command;

const INIT: &str = "src/claude_agent_sdk/__init__.py";
const TRANSPORT: &str = "src/claude_agent_sdk/_internal/transport/subprocess_cli.py";
const TYPES: &str = "src/claude_agent_sdk/types.py";
const QUERY: &str = "src/claude_agent_sdk/_internal/query.py";

fn git(repository: &Path, arguments: &[&str]) {
    let output = Command::new("git")
        .current_dir(repository)
        .args([
            "-c",
            "user.name=fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "tag.gpgsign=false",
            "-c",
            "core.autocrlf=false",
        ])
        .args(arguments)
        .output()
        .unwrap();
    assert!(output.status.success(), "git {arguments:?}: {output:?}");
}

fn write(repository: &Path, file: &str, content: &str) {
    let path = repository.join(file);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
}

fn commit(repository: &Path, files: &[(&str, &str)], message: &str) {
    for (file, content) in files {
        write(repository, file, content);
    }
    git(repository, &["add", "--all"]);
    git(repository, &["commit", "--quiet", "--message", message]);
}

fn types(fields: &[&str], subtype: &str) -> String {
    let fields: Vec<String> = fields
        .iter()
        .map(|field| format!("    {field}: str | None = None"))
        .collect();
    format!(
        "@dataclass\nclass ClaudeAgentOptions:\n{}\n\n\nclass SDKControlInterruptRequest(TypedDict):\n    subtype: Literal[\"{subtype}\"]\n",
        fields.join("\n")
    )
}

#[test]
fn the_diff_lists_what_changed_between_the_pinned_commit_and_a_tag() {
    let directory = tempfile::tempdir().unwrap();
    let repository = directory.path();
    git(repository, &["init", "--quiet"]);
    commit(
        repository,
        &[
            (INIT, "__all__ = [\n    \"query\",\n    \"Removed\",\n]\n"),
            (TRANSPORT, "cmd = [\"--verbose\", \"--old-flag\"]\n"),
            (TYPES, &types(&["model", "old_field"], "interrupt")),
            (QUERY, "request = {\n    \"subtype\": \"set_model\",\n}\n"),
            ("CHANGELOG.md", "# Changelog\n\n## 0.1.0\n"),
            (
                "tests/test_sample.py",
                "def test_kept():\n    pass\n\n\ndef test_dropped():\n    pass\n",
            ),
            ("e2e-tests/test_live.py", "def test_live():\n    pass\n"),
        ],
        "base",
    );
    git(repository, &["tag", "base"]);
    commit(
        repository,
        &[
            (INIT, "__all__ = [\n    \"query\",\n    \"NewExport\",\n]\n"),
            (TRANSPORT, "cmd = [\"--verbose\", \"--new-flag\"]\n"),
            (TYPES, &types(&["model", "new_field"], "rewind")),
            (
                QUERY,
                "request = {\n    \"subtype\": \"set_model\",\n}\nother = {\n    \"subtype\": \"reload\",\n}\n",
            ),
            ("CHANGELOG.md", "# Changelog\n\n## 0.2.0\n\n## 0.1.0\n"),
            (
                "tests/test_sample.py",
                "def test_kept():\n    pass\n\n\nclass TestNew:\n    def test_added(self):\n        pass\n",
            ),
            (
                "e2e-tests/test_live.py",
                "def test_live():\n    pass\n\n\ndef test_more():\n    pass\n",
            ),
        ],
        "next",
    );
    git(repository, &["tag", "v0.2.0"]);
    git(repository, &["checkout", "--quiet", "base"]);

    let output = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .arg("upstream-diff")
        .arg("v0.2.0")
        .arg("--upstream")
        .arg(repository)
        .output()
        .unwrap();

    assert!(output.status.success(), "{output:?}");
    let report = String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n");
    for section in [
        "CLI flags: +1 -1\n  + --new-flag\n  - --old-flag\n",
        "Option fields (ClaudeAgentOptions): +1 -1\n  + new_field\n  - old_field\n",
        "Control subtypes: +2 -1\n  + reload\n  + rewind\n  - interrupt\n",
        "Exported names: +1 -1\n  + NewExport\n  - Removed\n",
        "Unit tests: +1 -1\n  + tests/test_sample.py::TestNew::test_added\n  - \
         tests/test_sample.py::test_dropped\n",
        "End-to-end tests: +1 -0\n  + e2e-tests/test_live.py::test_more\n",
        "Changelog versions: +1 -0\n  + 0.2.0\n",
    ] {
        assert!(
            report.contains(section),
            "no section\n{section}\nin:\n{report}"
        );
    }
    assert!(report.starts_with("upstream-diff: "), "{report}");
    assert!(report.contains("(pinned) -> v0.2.0 ("), "{report}");
}

#[test]
fn an_unknown_tag_fails_naming_it() {
    let directory = tempfile::tempdir().unwrap();
    let repository = directory.path();
    git(repository, &["init", "--quiet"]);
    commit(repository, &[(INIT, "__all__ = [\"query\"]\n")], "base");

    let output = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .arg("upstream-diff")
        .arg("v9.9.9-missing")
        .arg("--upstream")
        .arg(repository)
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("v9.9.9-missing"), "{stderr}");
}
