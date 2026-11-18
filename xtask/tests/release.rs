//! `release prepare`, `release tag`, `changelog-section`, and
//! `upstream-sync`, run as the `xtask` binary against a temporary
//! workspace: a git repository on `main` with the files the commands
//! rewrite, a two-crate cargo workspace, and a real submodule pointing at
//! a local stand-in for the upstream repository.
//!
//! Git runs with its own global configuration and without the identity
//! variables of the environment, so neither the identity nor the signing
//! settings of the machine running the tests leak in.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const SUBMODULE: &str = "upstream/claude-agent-sdk-python";
const PIN_SDK: &str = "0.2.182";
const PIN_CLI: &str = "2.1.325";
/// Variables that would override the git settings of the fixture.
const GIT_OVERRIDES: [&str; 6] = [
    "GIT_AUTHOR_NAME",
    "GIT_AUTHOR_EMAIL",
    "GIT_COMMITTER_NAME",
    "GIT_COMMITTER_EMAIL",
    "GIT_DIR",
    "GIT_WORK_TREE",
];

/// Points git at the fixture's configuration only.
fn isolate(command: &mut Command, config: &Path) {
    command
        .env("GIT_CONFIG_GLOBAL", config)
        .env("GIT_CONFIG_NOSYSTEM", "1");
    for variable in GIT_OVERRIDES {
        command.env_remove(variable);
    }
}

struct Fixture {
    _directory: tempfile::TempDir,
    root: PathBuf,
    origin: PathBuf,
    config: PathBuf,
    /// The directory of the stand-in `cargo-semver-checks`: what it
    /// answers, and the log of how it was run.
    semver: PathBuf,
}

/// The value of the constant `name` of `xtask/src/semver.rs`.
fn semver_pin(name: &str) -> String {
    let source =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/semver.rs"))
            .unwrap();
    source
        .split(&format!("pub const {name}: &str = \""))
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .unwrap()
        .to_owned()
}

/// The report of a real run of the tool on a renamed constant.
const BREAK_REPORT: &str =
    "    Checking claude-agent-rust-sdk v0.1.0 -> v0.1.0 (no change; assume minor)
     Checked [   0.073s] 202 checks: 201 pass, 1 fail, 0 warn, 58 skip

--- failure pub_module_level_const_missing: pub module-level const is missing ---

Description:
A public const is missing or renamed
        ref: https://doc.rust-lang.org/cargo/reference/semver.html#item-remove

Failed in:
  VERSION in file /registry/claude-agent-rust-sdk-0.1.0/src/lib.rs:220

     Summary semver requires new major version: 1 major and 0 minor checks failed
    Finished [   2.822s] claude-agent-rust-sdk
";

fn write(path: &Path, content: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
}

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
    .replace("\r\n", "\n")
}

fn library(sdk: &str, cli: &str) -> String {
    format!(
        "//! It is tested against CLI\n//! {cli} ([`CLI_VERSION`]), and more.\n\n/// The CLI.\npub const CLI_VERSION: &str = \"{cli}\";\n\n/// Upstream.\npub const UPSTREAM_SDK_VERSION: &str = \"{sdk}\";\n"
    )
}

fn pyproject(version: &str) -> String {
    format!(
        "[project]\nname = \"claude-agent-sdk\"\nversion = \"{version}\"\ndescription = \"x\"\n"
    )
}

fn cli_module(cli: &str) -> String {
    format!("\"\"\"Bundled Claude Code CLI version.\"\"\"\n\n__cli_version__ = \"{cli}\"\n")
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let config = directory.path().join("gitconfig");
        write(
            &config,
            "[user]\n\tname = Owner\n\temail = owner@example.invalid\n[init]\n\tdefaultBranch = main\n[protocol \"file\"]\n\tallow = always\n[core]\n\tautocrlf = false\n",
        );
        let origin = directory.path().join("origin");
        let root = directory.path().join("workspace");
        let semver = directory.path().join("semver");
        let fixture = Self {
            _directory: directory,
            root,
            origin,
            config,
            semver,
        };
        fixture.build_semver_stand_in();
        fixture.build_origin();
        fixture.build_workspace();
        fixture
    }

    /// A `cargo-semver-checks` that reports the pinned version, logs its
    /// arguments and the variables that choose the toolchain, and answers
    /// what `semver_answers` set: by default, a compatible API.
    fn build_semver_stand_in(&self) {
        let directory = &self.semver;
        write(
            &directory.join("version.txt"),
            &format!("cargo-semver-checks {}\n", semver_pin("TOOL_VERSION")),
        );
        self.semver_answers(0, "     Summary no semver update required\n");
        let path = |file: &str| directory.join(file).display().to_string();
        if cfg!(windows) {
            write(
                &self.semver_tool(),
                &format!(
                    "@echo off\r\nif \"%~2\"==\"--version\" goto version\r\necho %*>>\"{log}\"\r\nif defined CARGO goto cargo\r\necho RUSTUP_TOOLCHAIN=%RUSTUP_TOOLCHAIN% CARGO=unset>>\"{log}\"\r\ngoto answer\r\n:cargo\r\necho RUSTUP_TOOLCHAIN=%RUSTUP_TOOLCHAIN% CARGO=set>>\"{log}\"\r\n:answer\r\ntype \"{output}\"\r\nset /p CODE=<\"{code}\"\r\nexit /b %CODE%\r\n:version\r\ntype \"{version}\"\r\nexit /b 0\r\n",
                    log = path("log.txt"),
                    output = path("output.txt"),
                    code = path("code.txt"),
                    version = path("version.txt"),
                ),
            );
        } else {
            write(
                &self.semver_tool(),
                &format!(
                    "#!/bin/sh\nif [ \"$2\" = --version ]; then cat '{version}'; exit 0; fi\nprintf '%s\\n' \"$*\" >> '{log}'\nif [ -n \"${{CARGO+x}}\" ]; then cargo=set; else cargo=unset; fi\nprintf 'RUSTUP_TOOLCHAIN=%s CARGO=%s\\n' \"$RUSTUP_TOOLCHAIN\" \"$cargo\" >> '{log}'\ncat '{output}'\nexit \"$(cat '{code}')\"\n",
                    log = path("log.txt"),
                    output = path("output.txt"),
                    code = path("code.txt"),
                    version = path("version.txt"),
                ),
            );
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(
                    self.semver_tool(),
                    std::fs::Permissions::from_mode(0o755),
                )
                .unwrap();
            }
        }
    }

    fn semver_tool(&self) -> PathBuf {
        self.semver.join(if cfg!(windows) {
            "cargo-semver-checks.cmd"
        } else {
            "cargo-semver-checks"
        })
    }

    /// Makes the stand-in tool exit with `code` after printing `output`.
    fn semver_answers(&self, code: i32, output: &str) {
        write(&self.semver.join("code.txt"), &format!("{code}\n"));
        write(&self.semver.join("output.txt"), output);
    }

    /// How the stand-in tool was run since the last call, then forgets it.
    /// Quotes are dropped: on Windows, Rust quotes the arguments of a batch
    /// file that hold `~` or `,`, and `%*` keeps the quotes, while the
    /// program still receives each argument unquoted.
    fn semver_log(&self) -> String {
        let log = self.semver.join("log.txt");
        let text = std::fs::read_to_string(&log)
            .unwrap_or_default()
            .replace("\r\n", "\n")
            .replace('"', "");
        let _ = std::fs::remove_file(&log);
        text
    }

    fn git_command(&self, directory: &Path) -> Command {
        let mut command = Command::new("git");
        command.current_dir(directory);
        isolate(&mut command, &self.config);
        command
    }

    fn git_in(&self, directory: &Path, arguments: &[&str]) -> String {
        let output = self
            .git_command(directory)
            .args(arguments)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {arguments:?}: {}",
            text(&output)
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    fn git(&self, arguments: &[&str]) -> String {
        self.git_in(&self.root, arguments)
    }

    /// The stand-in upstream: the pin `v0.2.182`, then a CLI-only release
    /// `v0.2.183`, another `v0.2.184`, and a release with a code change
    /// `v0.2.185`.
    fn build_origin(&self) {
        let origin = &self.origin;
        let files = |sdk: &str, cli: &str, flag: &str| {
            write(&origin.join("pyproject.toml"), &pyproject(sdk));
            write(
                &origin.join("src/claude_agent_sdk/_cli_version.py"),
                &cli_module(cli),
            );
            write(
                &origin.join("src/claude_agent_sdk/_version.py"),
                &format!("__version__ = \"{sdk}\"\n"),
            );
            write(
                &origin.join("CHANGELOG.md"),
                &format!(
                    "# Changelog\n\n## {sdk}\n\n- Updated bundled Claude CLI to version {cli}\n"
                ),
            );
            write(
                &origin.join("src/claude_agent_sdk/__init__.py"),
                "__all__ = [\n    \"query\",\n]\n",
            );
            write(
                &origin.join("src/claude_agent_sdk/types.py"),
                "class ClaudeAgentOptions:\n    model: str | None = None\n    cli_path: str | None = None\n",
            );
            write(
                &origin.join("src/claude_agent_sdk/_internal/transport/subprocess_cli.py"),
                &format!("cmd = [\"--verbose\", \"{flag}\"]\n"),
            );
            write(
                &origin.join("tests/test_sample.py"),
                "def test_one():\n    assert True\n",
            );
        };
        std::fs::create_dir_all(origin).unwrap();
        self.git_in(origin, &["init", "--quiet"]);
        for (sdk, cli, flag) in [
            (PIN_SDK, PIN_CLI, "--old"),
            ("0.2.183", "2.1.326", "--old"),
            ("0.2.184", "2.1.327", "--old"),
            ("0.2.185", "2.1.328", "--new"),
        ] {
            files(sdk, cli, flag);
            self.git_in(origin, &["add", "--all"]);
            self.git_in(
                origin,
                &["commit", "--quiet", "--message", &format!("Release {sdk}")],
            );
            self.git_in(origin, &["tag", &format!("v{sdk}")]);
        }
    }

    fn build_workspace(&self) {
        let root = &self.root;
        let file = |relative: &str, content: &str| write(&root.join(relative), content);
        file(".gitignore", "target/\n");
        file(
            "Cargo.toml",
            "[workspace]\nresolver = \"3\"\nmembers = [\"crates/claude-agent-rust-sdk\", \"crates/claude-agent-rust-sdk-macros\"]\n\n[workspace.package]\nversion = \"0.1.0\"\nedition = \"2024\"\nlicense = \"MIT OR Apache-2.0\"\n\n[workspace.dependencies]\nuuid = { version = \"1\" }\n",
        );
        file(
            "crates/claude-agent-rust-sdk/Cargo.toml",
            "[package]\nname = \"claude-agent-rust-sdk\"\nversion.workspace = true\nedition.workspace = true\n\n[features]\ndefault = [\"extra\"]\nextra = []\ntesting = []\n\n[dependencies]\nclaude-agent-rust-sdk-macros = { path = \"../claude-agent-rust-sdk-macros\", version = \"=0.1.0\" }\n",
        );
        file(
            "crates/claude-agent-rust-sdk/src/lib.rs",
            &library(PIN_SDK, PIN_CLI),
        );
        file(
            "crates/claude-agent-rust-sdk-macros/Cargo.toml",
            "[package]\nname = \"claude-agent-rust-sdk-macros\"\nversion.workspace = true\nedition.workspace = true\n",
        );
        file(
            "crates/claude-agent-rust-sdk-macros/src/lib.rs",
            "//! Macros.\n",
        );
        file(
            "crates/claude-agent-rust-sdk-macros/README.md",
            "```toml\nclaude-agent-rust-sdk = { version = \"0.1\", features = [\"macros\"] }\n```\n",
        );
        file(
            "README.md",
            &format!(
                "# SDK\n\nIt is at parity with version {PIN_SDK}.\nTested\n  against {PIN_CLI}.\n\n```toml\n[dependencies]\nclaude-agent-rust-sdk = \"0.1\"\n```\n"
            ),
        );
        file(
            "CHANGELOG.md",
            "# Changelog\n\n## Unreleased\n\n### Added\n\n- The first release.\n",
        );
        file(
            "docs/migration.md",
            &format!(
                "# Guide\n\nIt is at parity with version {PIN_SDK} of the SDK.\n\n<!-- parity:table:begin -->\n<!-- parity:table:end -->\n"
            ),
        );
        file(
            "docs/parity.md",
            "# Parity\n\n<!-- parity:page:begin -->\n<!-- parity:page:end -->\n",
        );
        file(
            "parity/surface.toml",
            "[names]\nquery = { rust = \"claude_agent_rust_sdk::query\" }\n",
        );
        file(
            "parity/tests.toml",
            "[tests.\"tests/test_sample.py\"]\ntest_one = { rust = \"crates/claude-agent-rust-sdk/tests/a.rs::a\" }\n",
        );
        file(
            "parity/differences.toml",
            "[wire.sdk-version]\ncase = \"*\"\nlocation = \"env CLAUDE_AGENT_SDK_VERSION\"\npython = '\"{upstream_version}\"'\nrust = '\"{crate_version}\"'\ncategory = \"deliberate\"\nreason = \"Each SDK sends its own version.\"\n",
        );
        file(
            "parity/cases/default.json",
            r#"{"name": "default", "scenario": {}, "prompt": "hi", "options": {"model": "m"}}"#,
        );
        file(
            "parity/cases/coverage.toml",
            "[excluded]\ncli_path = \"Set by the drivers.\"\n",
        );
        file("parity/scenarios/.keep", "");
        std::fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../parity/case.schema.json"),
            root.join("parity/case.schema.json"),
        )
        .unwrap();
        let commit = self.git_in(&self.origin, &["rev-parse", "v0.2.182^{commit}"]);
        let commit = commit.trim();
        for notices in [
            "THIRD_PARTY_NOTICES",
            "crates/claude-agent-rust-sdk/THIRD_PARTY_NOTICES",
        ] {
            file(
                notices,
                &format!("Pinned at commit\n{commit} (v{PIN_SDK}).\n"),
            );
        }
        file(
            ".github/workflows/e2e.yml",
            &format!("env:\n  CLAUDE_CODE_VERSION: {PIN_CLI}\n"),
        );
        file(
            "crates/claude-agent-rust-sdk/tests/workspace.rs",
            &format!(
                "const UPSTREAM_COMMIT: &str = \"{commit}\";\nassert_eq!(CLI_VERSION, \"{PIN_CLI}\");\nassert_eq!(UPSTREAM_SDK_VERSION, \"{PIN_SDK}\");\n"
            ),
        );
        file(
            "crates/fake-claude/src/scenario.rs",
            &format!(
                "//!   \"claude_code_version\": \"{PIN_CLI}\",\nconst DEFAULT_VERSION: &str = \"{PIN_CLI}\";\nassert_eq!(v, Some(\"{PIN_CLI}\"));\n"
            ),
        );
        file(
            "crates/fake-claude/tests/cli.rs",
            &format!(
                "\"{PIN_CLI} (fake-claude)\"\nassert_eq!(frames[0][\"claude_code_version\"], \"{PIN_CLI}\");\n"
            ),
        );

        self.git(&["init", "--quiet"]);
        let origin = self.origin.to_str().unwrap().replace('\\', "/");
        self.git(&["submodule", "--quiet", "add", &origin, SUBMODULE]);
        self.git_in(
            &root.join(SUBMODULE),
            &["checkout", "--quiet", "--detach", "v0.2.182"],
        );
        let output = Command::new(cargo())
            .current_dir(root)
            .args(["generate-lockfile", "--offline"])
            .output()
            .unwrap();
        assert!(output.status.success(), "{}", text(&output));
        self.git(&["add", "--all"]);
        self.git(&["commit", "--quiet", "--message", "feat: the first commit"]);
        let generated = self.xtask(&["migration-table"], &self.locations());
        assert!(generated.status.success(), "{}", text(&generated));
        let generated = self.xtask(&["parity-page"], &self.locations());
        assert!(generated.status.success(), "{}", text(&generated));
        self.git(&[
            "commit",
            "--quiet",
            "--all",
            "--message",
            "docs: generate the pages",
        ]);
    }

    fn locations(&self) -> Vec<String> {
        let path = |relative: &str| self.root.join(relative).to_str().unwrap().to_owned();
        vec![
            "--root".to_owned(),
            path(""),
            "--upstream".to_owned(),
            path(SUBMODULE),
            "--parity".to_owned(),
            path("parity"),
            "--migration".to_owned(),
            path("docs/migration.md"),
            "--page".to_owned(),
            path("docs/parity.md"),
        ]
    }

    fn xtask_with(&self, arguments: &[&str], extra: &[String], cargo: Option<&Path>) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_xtask"));
        command.current_dir(&self.root).args(arguments).args(extra);
        isolate(&mut command, &self.config);
        command.env("XTASK_SEMVER_CHECKS", self.semver_tool());
        if let Some(cargo) = cargo {
            command.env("CARGO", cargo);
        }
        command.output().unwrap()
    }

    fn xtask(&self, arguments: &[&str], extra: &[String]) -> Output {
        self.xtask_with(arguments, extra, None)
    }

    /// Runs `command` (a word or two) with `--root` pointing at the
    /// fixture.
    fn run(&self, arguments: &[&str]) -> Output {
        let root = vec!["--root".to_owned(), self.root.to_str().unwrap().to_owned()];
        self.xtask(arguments, &root)
    }

    fn read(&self, relative: &str) -> String {
        std::fs::read_to_string(self.root.join(relative))
            .unwrap()
            .replace("\r\n", "\n")
    }

    fn commit(&self, message: &str, files: &[(&str, &str)]) {
        for (file, content) in files {
            write(&self.root.join(file), content);
        }
        self.git(&["add", "--all"]);
        self.git(&["commit", "--quiet", "--message", message]);
    }

    fn status(&self) -> String {
        self.git(&["status", "--porcelain", "--untracked-files=all"])
    }

    /// The content of every tracked file, to show that a refused command
    /// changed nothing.
    fn snapshot(&self) -> Vec<(String, String)> {
        self.git(&["ls-files"])
            .lines()
            .filter(|file| !file.starts_with("upstream/"))
            .map(|file| (file.to_owned(), self.read(file)))
            .collect()
    }

    fn release_tag(&self, version: &str) {
        self.git(&[
            "tag",
            "--annotate",
            &format!("v{version}"),
            "--message",
            "released",
        ]);
    }
}

fn cargo() -> PathBuf {
    std::env::var_os("CARGO").map_or_else(|| PathBuf::from("cargo"), PathBuf::from)
}

fn assert_refused(fixture: &Fixture, output: &Output, message: &str) {
    assert!(!output.status.success(), "{}", text(output));
    assert!(
        text(output).contains(message),
        "no `{message}` in:\n{}",
        text(output)
    );
    assert_eq!(fixture.status(), "", "a refused command changed files");
}

// --- changelog-section -----------------------------------------------------

#[test]
fn changelog_section_prints_the_body_of_a_version_and_fails_naming_a_missing_one() {
    let fixture = Fixture::new();
    fixture.commit(
        "docs: a released section",
        &[(
            "CHANGELOG.md",
            "# Changelog\n\n## Unreleased\n\n## 0.1.0 - 2026-11-19\n\nUpstream: Python SDK 0.2.182, CLI 2.1.325\n\n### Added\n\n- First.\n",
        )],
    );
    let output = fixture.run(&["changelog-section", "0.1.0"]);
    assert!(output.status.success(), "{}", text(&output));
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n"),
        "Upstream: Python SDK 0.2.182, CLI 2.1.325\n\n### Added\n\n- First.\n"
    );

    let missing = fixture.run(&["changelog-section", "0.9.9"]);
    assert!(!missing.status.success());
    assert!(
        text(&missing).contains("CHANGELOG.md has no section for 0.9.9"),
        "{}",
        text(&missing)
    );
}

// --- release prepare -------------------------------------------------------

#[test]
fn prepare_turns_the_first_release_candidate_into_a_section_without_a_tag() {
    let fixture = Fixture::new();
    let output = fixture.run(&["release", "prepare", "0.1.0-rc.1"]);
    assert!(output.status.success(), "{}", text(&output));

    assert!(
        fixture
            .read("Cargo.toml")
            .contains("\nversion = \"0.1.0-rc.1\"\n")
    );
    assert!(
        fixture
            .read("crates/claude-agent-rust-sdk/Cargo.toml")
            .contains("version = \"=0.1.0-rc.1\"")
    );
    assert!(
        fixture
            .read("Cargo.lock")
            .contains("version = \"0.1.0-rc.1\"")
    );
    assert!(
        fixture
            .read("README.md")
            .contains("claude-agent-rust-sdk = \"0.1\"")
    );
    let changelog = fixture.read("CHANGELOG.md");
    assert!(
        changelog.starts_with("# Changelog\n\n## Unreleased\n\n## 0.1.0-rc.1 - "),
        "{changelog}"
    );
    assert!(
        changelog.contains(
            "\n\nUpstream: Python SDK 0.2.182, CLI 2.1.325\n\n### Added\n\n- The first release.\n"
        ),
        "{changelog}"
    );
    let page = fixture.read("docs/parity.md");
    assert!(page.contains("`\"0.1.0-rc.1\"`"), "{page}");
    assert!(page.contains("`\"0.2.182\"`"), "{page}");

    let report = text(&output);
    assert!(report.contains("moved 0.1.0 -> 0.1.0-rc.1"), "{report}");
    assert!(report.contains("run: cargo xtask release tag"), "{report}");
    assert!(report.contains("\n  Cargo.lock\n"), "{report}");
    assert!(
        !report.contains("docs/migration.md"),
        "an unchanged file is not listed: {report}"
    );
    assert!(
        fixture.git(&["tag", "--list"]).is_empty(),
        "prepare created a tag"
    );
    assert_eq!(
        fixture.git(&["rev-list", "--count", "HEAD"]).trim(),
        "2",
        "prepare made a commit"
    );
    let changed: Vec<String> = fixture
        .status()
        .lines()
        .map(|line| line[3..].to_owned())
        .collect();
    assert_eq!(
        changed,
        [
            "CHANGELOG.md",
            "Cargo.lock",
            "Cargo.toml",
            "crates/claude-agent-rust-sdk/Cargo.toml",
            "docs/parity.md"
        ]
    );
}

#[test]
fn prepare_patch_drafts_the_entries_from_the_commits_since_the_previous_tag() {
    let fixture = Fixture::new();
    fixture.commit(
        "chore: release v0.1.0",
        &[(
            "CHANGELOG.md",
            "# Changelog\n\n## Unreleased\n\n## 0.1.0 - 2026-11-19\n\nUpstream: Python SDK 0.2.180 to 0.2.182, CLI 2.1.325\n\n### Added\n\n- The first release.\n",
        )],
    );
    fixture.release_tag("0.1.0");
    fixture.commit("feat(client): a feature", &[("notes/a.txt", "a")]);
    fixture.commit("fix: a fix\n\nWith a body.", &[("notes/b.txt", "b")]);
    fixture.commit("docs: left out", &[("notes/c.txt", "c")]);
    fixture.commit("refactor: tidier", &[("notes/d.txt", "d")]);

    let output = fixture.run(&["release", "prepare", "patch"]);
    assert!(output.status.success(), "{}", text(&output));
    let changelog = fixture.read("CHANGELOG.md");
    let section = changelog
        .split("## 0.1.1 - ")
        .nth(1)
        .and_then(|rest| rest.split("\n## ").next())
        .unwrap();
    assert!(
        section.ends_with(
            "\n\nUpstream: Python SDK 0.2.182, CLI 2.1.325\n\n### Added\n\n- a feature\n\n### Changed\n\n- tidier\n\n### Fixed\n\n- a fix\n"
        ),
        "{section}"
    );
    assert!(!section.contains("left out"));
    assert!(
        fixture
            .read("Cargo.toml")
            .contains("\nversion = \"0.1.1\"\n")
    );
}

#[test]
fn prepare_minor_moves_the_readme_snippets_and_the_upstream_range() {
    let fixture = Fixture::new();
    fixture.commit(
        "chore: release v0.1.0",
        &[(
            "CHANGELOG.md",
            "# Changelog\n\n## Unreleased\n\n### Changed\n\n- Hand-written.\n\n## 0.1.0 - 2026-11-19\n\nUpstream: Python SDK 0.2.180, CLI 2.1.320\n\n### Added\n\n- The first release.\n",
        )],
    );
    fixture.release_tag("0.1.0");
    fixture.commit("feat!: a break", &[("notes/a.txt", "a")]);

    let patch = fixture.run(&["release", "prepare", "patch"]);
    assert_refused(
        &fixture,
        &patch,
        "breaking change since v0.1.0: use minor (`feat!: a break`)",
    );

    let output = fixture.run(&["release", "prepare", "minor"]);
    assert!(output.status.success(), "{}", text(&output));
    assert!(
        fixture
            .read("README.md")
            .contains("\nclaude-agent-rust-sdk = \"0.2\"\n")
    );
    assert!(
        fixture
            .read("crates/claude-agent-rust-sdk-macros/README.md")
            .contains("claude-agent-rust-sdk = { version = \"0.2\"")
    );
    let changelog = fixture.read("CHANGELOG.md");
    assert!(
        changelog.contains(
            "\n\nUpstream: Python SDK 0.2.182, CLI 2.1.325\n\n### Changed\n\n- Hand-written.\n"
        ),
        "{changelog}"
    );
    assert!(
        !changelog.contains("- a break"),
        "hand-written entries are kept as they are"
    );
}

#[test]
fn prepare_dry_run_prints_the_plan_and_writes_nothing() {
    let fixture = Fixture::new();
    let before = fixture.snapshot();
    let output = fixture.run(&["release", "prepare", "0.1.0-rc.1", "--dry-run"]);
    assert!(output.status.success(), "{}", text(&output));
    let report = text(&output);
    assert!(
        report.contains("would move 0.1.0 -> 0.1.0-rc.1"),
        "{report}"
    );
    assert!(report.contains("  Cargo.toml\n"), "{report}");
    assert!(report.contains("## 0.1.0-rc.1 - "), "{report}");
    assert!(report.contains("dry run: nothing was written"), "{report}");
    assert_eq!(fixture.status(), "");
    assert_eq!(fixture.snapshot(), before);
}

#[test]
fn prepare_refuses_a_dirty_tree_a_branch_other_than_main_and_a_version_not_ahead() {
    let fixture = Fixture::new();
    write(&fixture.root.join("stray.txt"), "x");
    let dirty = fixture.run(&["release", "prepare", "patch"]);
    assert!(!dirty.status.success());
    assert!(
        text(&dirty).contains("uncommitted changes"),
        "{}",
        text(&dirty)
    );
    assert!(text(&dirty).contains("  stray.txt"), "{}", text(&dirty));
    std::fs::remove_file(fixture.root.join("stray.txt")).unwrap();

    fixture.git(&["checkout", "--quiet", "-b", "topic"]);
    let branch = fixture.run(&["release", "prepare", "0.1.0-rc.1"]);
    assert_refused(
        &fixture,
        &branch,
        "the branch is `topic`; releases are made from `main`",
    );
    fixture.git(&["checkout", "--quiet", "main"]);

    fixture.release_tag("0.1.0-rc.2");
    let older = fixture.run(&["release", "prepare", "0.1.0-rc.1"]);
    assert_refused(
        &fixture,
        &older,
        "0.1.0-rc.1 is not greater than the last released version 0.1.0-rc.2",
    );
    let existing = fixture.run(&["release", "prepare", "0.1.0-rc.2"]);
    assert_refused(&fixture, &existing, "the tag v0.1.0-rc.2 already exists");
    let malformed = fixture.run(&["release", "prepare", "next"]);
    assert_refused(&fixture, &malformed, "`next` is not a version");
}

#[test]
fn prepare_patch_refuses_after_a_breaking_footer_since_the_previous_tag() {
    let fixture = Fixture::new();
    fixture.release_tag("0.1.0");
    fixture.commit(
        "feat: moved\n\nBREAKING CHANGE: the API moved",
        &[("notes/a.txt", "a")],
    );
    let before = fixture.snapshot();
    let output = fixture.run(&["release", "prepare", "patch"]);
    assert_refused(&fixture, &output, "breaking change since v0.1.0: use minor");
    assert_eq!(fixture.snapshot(), before);
}

#[test]
fn a_failure_after_the_first_writes_leaves_no_file_rewritten() {
    let fixture = Fixture::new();
    fixture.commit(
        "docs: a page without its markers",
        &[("docs/parity.md", "# Parity\n\nNo markers.\n")],
    );
    let before = fixture.snapshot();
    let output = fixture.run(&["release", "prepare", "0.1.0-rc.1"]);
    assert!(!output.status.success(), "{}", text(&output));
    assert!(
        text(&output).contains("must contain `<!-- parity:page:begin -->`"),
        "{}",
        text(&output)
    );
    assert_eq!(fixture.status(), "", "a file was left rewritten");
    assert_eq!(fixture.snapshot(), before);
}

#[test]
fn prepare_patch_refuses_when_the_compatibility_tool_reports_a_break() {
    let fixture = Fixture::new();
    fixture.release_tag("0.1.0");
    fixture.commit("fix: a quiet rename", &[("notes/a.txt", "a")]);
    fixture.semver_answers(100, BREAK_REPORT);
    let before = fixture.snapshot();

    let output = fixture.run(&["release", "prepare", "patch"]);
    assert_refused(
        &fixture,
        &output,
        "breaking change since v0.1.0: use minor (cargo-semver-checks reports:\n  pub_module_level_const_missing: VERSION in file /registry/claude-agent-rust-sdk-0.1.0/src/lib.rs:220",
    );
    assert_eq!(fixture.snapshot(), before);
    let log = fixture.semver_log();
    let manifest = fixture.root.join("Cargo.toml").display().to_string();
    assert!(
        log.contains(&format!(
            "semver-checks check-release --color never --manifest-path {manifest} --package claude-agent-rust-sdk --only-explicit-features --features extra\n"
        )),
        "{log}"
    );
    assert!(
        log.contains(&format!(
            "RUSTUP_TOOLCHAIN={} CARGO=unset\n",
            semver_pin("TOOLCHAIN")
        )),
        "{log}"
    );

    let minor = fixture.run(&["release", "prepare", "minor"]);
    assert!(minor.status.success(), "{}", text(&minor));
    assert_eq!(fixture.semver_log(), "", "minor does not consult the tool");
}

#[test]
fn prepare_patch_refuses_when_the_compatibility_tool_compares_nothing() {
    let fixture = Fixture::new();
    fixture.release_tag("0.1.0");
    fixture.commit("fix: a fix", &[("notes/a.txt", "a")]);

    fixture.semver_answers(
        1,
        "error: unsupported rustdoc format v61 for file: /t/doc/x.json\n(supported formats are v56, v57)\n",
    );
    let format = fixture.run(&["release", "prepare", "patch"]);
    assert_refused(
        &fixture,
        &format,
        "cargo-semver-checks did not compare the API (exit 1).",
    );
    assert!(
        text(&format).contains("update the pins together"),
        "{}",
        text(&format)
    );
    assert!(
        text(&format).contains("the `semver` job of .github/workflows/ci.yml"),
        "{}",
        text(&format)
    );

    fixture.semver_answers(0, "    Finished\n");
    let silent = fixture.run(&["release", "prepare", "patch"]);
    assert_refused(&fixture, &silent, "without its comparison summary");

    write(
        &fixture.semver.join("version.txt"),
        "cargo-semver-checks 0.47.0\n",
    );
    let older = fixture.run(&["release", "prepare", "patch"]);
    assert_refused(
        &fixture,
        &older,
        &format!(
            "cargo-semver-checks {} is pinned, `cargo-semver-checks 0.47.0` is installed",
            semver_pin("TOOL_VERSION")
        ),
    );

    std::fs::remove_file(fixture.semver_tool()).unwrap();
    let missing = fixture.run(&["release", "prepare", "patch"]);
    assert_refused(&fixture, &missing, "cannot run cargo-semver-checks");
    assert!(
        text(&missing).contains(&format!(
            "cargo install cargo-semver-checks@{} --locked",
            semver_pin("TOOL_VERSION")
        )),
        "{}",
        text(&missing)
    );
}

// --- semver-check ----------------------------------------------------------

#[test]
fn semver_check_passes_a_compatible_api_and_fails_naming_each_broken_item() {
    let fixture = Fixture::new();
    let compatible = fixture.run(&["semver-check", "--features", "extra,more"]);
    assert!(compatible.status.success(), "{}", text(&compatible));
    assert!(
        text(&compatible).contains("Summary no semver update required"),
        "the tool's report is shown"
    );
    assert!(
        fixture
            .semver_log()
            .contains("--only-explicit-features --features extra,more\n")
    );

    fixture.semver_answers(100, BREAK_REPORT);
    let breaking = fixture.run(&["semver-check"]);
    assert!(!breaking.status.success());
    let message = text(&breaking);
    assert!(
        message.contains("--- failure pub_module_level_const_missing"),
        "{message}"
    );
    assert!(
        message.contains(
            "release it with `cargo xtask release prepare minor`:\n  pub_module_level_const_missing: VERSION in file"
        ),
        "{message}"
    );
    assert!(
        fixture.semver_log().contains("--features extra\n"),
        "without --features, the manifest's features but default and testing"
    );

    fixture.semver_answers(101, "Caused by:\n  unable to update registry `crates-io`\n");
    let offline = fixture.run(&["semver-check"]);
    assert!(!offline.status.success());
    assert!(
        text(&offline).contains("crates.io could not be read"),
        "{}",
        text(&offline)
    );
    assert_eq!(fixture.status(), "");
}

// --- release tag -----------------------------------------------------------

#[test]
fn tag_refuses_without_a_section_with_a_foreign_change_and_with_an_existing_tag() {
    let fixture = Fixture::new();
    let unprepared = fixture.run(&["release", "tag"]);
    assert_refused(
        &fixture,
        &unprepared,
        "CHANGELOG.md has no section for 0.1.0",
    );

    let prepared = fixture.run(&["release", "prepare", "0.1.0-rc.1"]);
    assert!(prepared.status.success(), "{}", text(&prepared));
    write(&fixture.root.join("src.rs"), "fn stray() {}\n");
    let foreign = fixture.run(&["release", "tag"]);
    assert!(!foreign.status.success());
    let message = text(&foreign);
    assert!(
        message.contains("these changes are not part of a release"),
        "{message}"
    );
    assert!(message.contains("\n  src.rs\n"), "{message}");
    assert!(fixture.git(&["tag", "--list"]).is_empty());
    assert_eq!(fixture.git(&["rev-list", "--count", "HEAD"]).trim(), "2");
    std::fs::remove_file(fixture.root.join("src.rs")).unwrap();

    fixture.release_tag("0.1.0-rc.1");
    let existing = fixture.run(&["release", "tag"]);
    assert!(!existing.status.success());
    assert!(text(&existing).contains("the tag v0.1.0-rc.1 already exists"));
    assert_eq!(fixture.git(&["rev-list", "--count", "HEAD"]).trim(), "2");
}

#[cfg(unix)]
mod with_a_stand_in_cargo {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    /// A `cargo` that logs its arguments and fails the commands named in
    /// `FAIL` (`gates`, `package-check`).
    fn stand_in(fixture: &Fixture, fail: &[&str]) -> (PathBuf, PathBuf) {
        let directory = fixture.root.parent().unwrap().join("bin");
        std::fs::create_dir_all(&directory).unwrap();
        let log = directory.join("cargo.log");
        let script = directory.join("cargo");
        let failing: Vec<String> = fail
            .iter()
            .map(|name| format!("    *\" {name}\"*) exit 1 ;;"))
            .collect();
        write(
            &script,
            &format!(
                "#!/bin/sh\necho \"cargo $*\" >> '{}'\ncase \" $*\" in\n{}\nesac\nexit 0\n",
                log.display(),
                failing.join("\n")
            ),
        );
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        (script, log)
    }

    /// A `gpg` that signs anything, so the test sees whether git applied
    /// the signing settings of its configuration.
    fn signing(fixture: &Fixture) {
        let script = fixture.root.parent().unwrap().join("bin/gpg");
        write(
            &script,
            "#!/bin/sh\ncat > /dev/null\nprintf '\\n[GNUPG:] SIG_CREATED D 1 8 00 1 X\\n' >&2\nprintf -- '-----BEGIN PGP SIGNATURE-----\\n\\nstand-in\\n-----END PGP SIGNATURE-----\\n'\n",
        );
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut config = std::fs::read_to_string(&fixture.config).unwrap();
        config.push_str(&format!(
            "[commit]\n\tgpgSign = true\n[tag]\n\tgpgSign = true\n[gpg]\n\tprogram = {}\n[user]\n\tsigningKey = stand-in\n",
            script.display()
        ));
        std::fs::write(&fixture.config, config).unwrap();
    }

    fn tag(fixture: &Fixture, cargo: &Path, extra: &[&str]) -> Output {
        let mut arguments = vec!["release", "tag"];
        arguments.extend(extra);
        let root = vec![
            "--root".to_owned(),
            fixture.root.to_str().unwrap().to_owned(),
        ];
        fixture.xtask_with(&arguments, &root, Some(cargo))
    }

    #[test]
    fn tag_commits_the_prepared_files_and_tags_them_with_the_machine_settings() {
        let fixture = Fixture::new();
        let prepared = fixture.run(&["release", "prepare", "0.1.0-rc.1"]);
        assert!(prepared.status.success(), "{}", text(&prepared));
        let (cargo, log) = stand_in(&fixture, &[]);
        signing(&fixture);

        let output = tag(&fixture, &cargo, &[]);
        assert!(output.status.success(), "{}", text(&output));
        assert_eq!(
            std::fs::read_to_string(&log).unwrap(),
            "cargo xtask gates\ncargo xtask package-check\n"
        );
        assert_eq!(fixture.status(), "");
        assert_eq!(
            fixture.git(&["log", "-1", "--format=%B"]),
            "chore: release v0.1.0-rc.1\n\n",
            "the message carries no trailer"
        );
        assert_eq!(
            fixture.git(&["log", "-1", "--format=%an <%ae>"]).trim(),
            "Owner <owner@example.invalid>"
        );
        let files = fixture.git(&["show", "--name-only", "--format=", "HEAD"]);
        assert_eq!(
            files.lines().collect::<Vec<_>>(),
            [
                "CHANGELOG.md",
                "Cargo.lock",
                "Cargo.toml",
                "crates/claude-agent-rust-sdk/Cargo.toml",
                "docs/parity.md"
            ]
        );
        assert!(
            fixture
                .git(&["cat-file", "commit", "HEAD"])
                .contains("gpgsig -----BEGIN PGP SIGNATURE-----")
        );
        assert_eq!(
            fixture.git(&["cat-file", "-t", "v0.1.0-rc.1"]).trim(),
            "tag"
        );
        let tag_object = fixture.git(&["cat-file", "tag", "v0.1.0-rc.1"]);
        assert!(
            tag_object.contains("-----BEGIN PGP SIGNATURE-----"),
            "{tag_object}"
        );
        assert_eq!(
            fixture.git(&["rev-parse", "v0.1.0-rc.1^{commit}"]),
            fixture.git(&["rev-parse", "HEAD"])
        );
        assert!(
            text(&output).contains("git push --atomic origin main v0.1.0-rc.1"),
            "{}",
            text(&output)
        );
    }

    #[test]
    fn tag_without_gates_still_runs_the_package_check() {
        let fixture = Fixture::new();
        let prepared = fixture.run(&["release", "prepare", "0.1.0-rc.1"]);
        assert!(prepared.status.success(), "{}", text(&prepared));
        let (cargo, log) = stand_in(&fixture, &[]);
        let output = tag(&fixture, &cargo, &["--no-gates"]);
        assert!(output.status.success(), "{}", text(&output));
        assert_eq!(
            std::fs::read_to_string(&log).unwrap(),
            "cargo xtask package-check\n"
        );
        assert!(fixture.git(&["tag", "--list"]).contains("v0.1.0-rc.1"));
    }

    #[test]
    fn a_failing_gate_or_package_check_leaves_neither_the_commit_nor_the_tag() {
        for failing in ["gates", "package-check"] {
            let fixture = Fixture::new();
            let prepared = fixture.run(&["release", "prepare", "0.1.0-rc.1"]);
            assert!(prepared.status.success(), "{}", text(&prepared));
            let prepared_status = fixture.status();
            let (cargo, _) = stand_in(&fixture, &[failing]);
            let output = tag(&fixture, &cargo, &[]);
            assert!(!output.status.success(), "{failing}: {}", text(&output));
            assert!(
                text(&output).contains(&format!("cargo xtask {failing} failed")),
                "{}",
                text(&output)
            );
            assert!(fixture.git(&["tag", "--list"]).is_empty(), "{failing}");
            assert_eq!(
                fixture.git(&["rev-list", "--count", "HEAD"]).trim(),
                "2",
                "{failing}"
            );
            assert_eq!(
                fixture
                    .git(&["status", "--porcelain", "--untracked-files=all"])
                    .lines()
                    .map(|line| &line[3..])
                    .collect::<Vec<_>>(),
                prepared_status
                    .lines()
                    .map(|line| &line[3..])
                    .collect::<Vec<_>>(),
                "{failing}: the prepared files are kept"
            );
        }
    }
}

// --- upstream-sync ---------------------------------------------------------

#[test]
fn sync_to_a_cli_only_release_moves_every_copy_and_the_pin() {
    let fixture = Fixture::new();
    let output = fixture.run(&["upstream-sync", "v0.2.183"]);
    assert!(output.status.success(), "{}", text(&output));
    let report = text(&output);
    assert!(report.contains("verdict: cli-only"), "{report}");
    assert!(
        report.contains("Python SDK 0.2.182 -> 0.2.183, CLI 2.1.325 -> 2.1.326"),
        "{report}"
    );

    let tag = fixture.git_in(&fixture.origin, &["rev-parse", "v0.2.183^{commit}"]);
    let tag = tag.trim();
    let staged = fixture.git(&["ls-files", "--stage", "--", SUBMODULE]);
    assert!(staged.starts_with(&format!("160000 {tag} ")), "{staged}");
    assert_eq!(
        fixture.read("crates/claude-agent-rust-sdk/src/lib.rs"),
        library("0.2.183", "2.1.326")
    );
    assert!(
        fixture
            .read("README.md")
            .contains("It is at parity with version 0.2.183.\nTested\n  against 2.1.326.")
    );
    assert!(
        fixture
            .read("docs/migration.md")
            .contains("version 0.2.183 of")
    );
    for notices in [
        "THIRD_PARTY_NOTICES",
        "crates/claude-agent-rust-sdk/THIRD_PARTY_NOTICES",
    ] {
        assert_eq!(
            fixture.read(notices),
            format!("Pinned at commit\n{tag} (v0.2.183).\n")
        );
    }
    assert!(
        fixture
            .read(".github/workflows/e2e.yml")
            .contains("CLAUDE_CODE_VERSION: 2.1.326\n")
    );
    assert_eq!(
        fixture.read("crates/claude-agent-rust-sdk/tests/workspace.rs"),
        format!(
            "const UPSTREAM_COMMIT: &str = \"{tag}\";\nassert_eq!(CLI_VERSION, \"2.1.326\");\nassert_eq!(UPSTREAM_SDK_VERSION, \"0.2.183\");\n"
        )
    );
    assert!(
        !fixture
            .read("crates/fake-claude/src/scenario.rs")
            .contains(PIN_CLI)
    );
    assert!(
        !fixture
            .read("crates/fake-claude/tests/cli.rs")
            .contains(PIN_CLI)
    );
    let page = fixture.read("docs/parity.md");
    assert!(page.contains("Python Claude Agent SDK 0.2.183"), "{page}");
    assert!(page.contains("`\"0.2.183\"`"), "{page}");
    assert_eq!(
        fixture.read("CHANGELOG.md"),
        "# Changelog\n\n## Unreleased\n\n### Added\n\n- The first release.\n\n### Changed\n\n- Upstream moved to Python SDK 0.2.183 and CLI 2.1.326.\n"
    );
    assert_eq!(
        fixture.git(&["rev-list", "--count", "HEAD"]).trim(),
        "2",
        "sync made a commit"
    );
    assert!(
        fixture.git(&["diff", "--cached", "--name-only"]).trim() == SUBMODULE,
        "only the pin is staged"
    );
}

#[test]
fn sync_several_releases_ahead_to_a_code_change_reports_needs_port() {
    let fixture = Fixture::new();
    let output = fixture.run(&["upstream-sync", "v0.2.185"]);
    assert!(output.status.success(), "{}", text(&output));
    let report = text(&output);
    assert!(report.contains("verdict: needs-port"), "{report}");
    assert!(
        report.contains("CLI flags: +1 -1\n  + --new\n  - --old\n"),
        "{report}"
    );
    assert!(report.contains("complete the ledgers"), "{report}");
    assert!(report.contains("before a release"), "{report}");
    assert!(
        fixture
            .read("crates/claude-agent-rust-sdk/src/lib.rs")
            .contains("\"0.2.185\"")
    );
    assert!(
        fixture
            .read("CHANGELOG.md")
            .contains("- Upstream moved to Python SDK 0.2.185 and CLI 2.1.328.")
    );
}

#[test]
fn sync_then_prepare_gives_the_range_of_upstream_releases() {
    let fixture = Fixture::new();
    fixture.commit(
        "chore: release v0.1.0",
        &[(
            "CHANGELOG.md",
            "# Changelog\n\n## Unreleased\n\n## 0.1.0 - 2026-11-19\n\nUpstream: Python SDK 0.2.182, CLI 2.1.325\n\n### Added\n\n- The first release.\n",
        )],
    );
    fixture.release_tag("0.1.0");
    let sync = fixture.run(&["upstream-sync", "v0.2.184"]);
    assert!(sync.status.success(), "{}", text(&sync));
    fixture.git(&[
        "commit",
        "--quiet",
        "--all",
        "--message",
        "chore: port upstream v0.2.184",
    ]);
    let prepare = fixture.run(&["release", "prepare", "patch"]);
    assert!(prepare.status.success(), "{}", text(&prepare));
    let changelog = fixture.read("CHANGELOG.md");
    assert!(
        changelog.contains("## 0.1.1 - ")
            && changelog.contains(
                "\n\nUpstream: Python SDK 0.2.183 to 0.2.184, CLI 2.1.327\n\n### Changed\n\n- Upstream moved to Python SDK 0.2.184 and CLI 2.1.327.\n"
            ),
        "{changelog}"
    );
}

#[test]
fn prepare_of_a_release_gathers_its_release_candidates_and_their_upstream_range() {
    let fixture = Fixture::new();
    let release = |version: &str| {
        let prepare = fixture.run(&["release", "prepare", version]);
        assert!(prepare.status.success(), "{}", text(&prepare));
        fixture.git(&[
            "commit",
            "--quiet",
            "--all",
            "--message",
            &format!("chore: release v{version}"),
        ]);
        fixture.release_tag(version);
    };
    release("0.1.0-rc.1");
    let sync = fixture.run(&["upstream-sync", "v0.2.184"]);
    assert!(sync.status.success(), "{}", text(&sync));
    fixture.git(&[
        "commit",
        "--quiet",
        "--all",
        "--message",
        "chore: port upstream v0.2.184",
    ]);
    release("0.1.0-rc.2");
    fixture.commit("fix: a late fix", &[("late.txt", "fixed\n")]);

    let prepare = fixture.run(&["release", "prepare", "patch"]);
    assert!(prepare.status.success(), "{}", text(&prepare));
    let changelog = fixture.read("CHANGELOG.md");
    let stable = changelog
        .split("\n## ")
        .find(|section| section.starts_with("0.1.0 - "))
        .unwrap_or_else(|| panic!("no 0.1.0 section:\n{changelog}"));
    let body = stable.split_once('\n').unwrap().1;
    assert_eq!(
        body,
        "\nUpstream: Python SDK 0.2.182 to 0.2.184, CLI 2.1.327\n\n### Added\n\n- The first release.\n\n### Changed\n\n- Upstream moved to Python SDK 0.2.184 and CLI 2.1.327.\n\n### Fixed\n\n- a late fix\n",
        "{changelog}"
    );
    assert!(
        changelog.contains("\n## 0.1.0-rc.2 - ") && changelog.contains("\n## 0.1.0-rc.1 - "),
        "the candidate sections stay: {changelog}"
    );
    let report = text(&prepare);
    assert!(report.contains("moved 0.1.0-rc.2 -> 0.1.0"), "{report}");
    assert!(report.contains("- The first release."), "{report}");
}

#[test]
fn sync_refuses_an_unknown_tag_an_older_tag_and_a_dirty_tree() {
    let fixture = Fixture::new();
    let before = fixture.snapshot();
    let pin = fixture.git(&["ls-files", "--stage", "--", SUBMODULE]);

    let unknown = fixture.run(&["upstream-sync", "v0.2.999"]);
    assert_refused(&fixture, &unknown, "the upstream tag v0.2.999 is unknown");

    fixture.git_in(&fixture.origin, &["tag", "v0.2.100", "v0.2.182"]);
    let older = fixture.run(&["upstream-sync", "v0.2.100"]);
    assert_refused(
        &fixture,
        &older,
        "v0.2.100 is not newer than the pin v0.2.182",
    );
    let same = fixture.run(&["upstream-sync", "v0.2.182"]);
    assert_refused(
        &fixture,
        &same,
        "v0.2.182 is not newer than the pin v0.2.182",
    );

    write(&fixture.root.join("README.md"), "edited\n");
    let dirty = fixture.run(&["upstream-sync", "v0.2.183"]);
    assert!(!dirty.status.success());
    assert!(
        text(&dirty).contains("uncommitted changes"),
        "{}",
        text(&dirty)
    );
    fixture.git(&["checkout", "--", "README.md"]);

    assert_eq!(fixture.snapshot(), before);
    assert_eq!(fixture.git(&["ls-files", "--stage", "--", SUBMODULE]), pin);
    let head = fixture.git_in(&fixture.root.join(SUBMODULE), &["rev-parse", "HEAD"]);
    let pinned = fixture.git_in(&fixture.origin, &["rev-parse", "v0.2.182^{commit}"]);
    assert_eq!(head, pinned);
}

#[test]
fn sync_refuses_an_upstream_cli_version_that_is_not_a_version() {
    let fixture = Fixture::new();
    let before = fixture.snapshot();
    let origin = &fixture.origin;
    write(&origin.join("pyproject.toml"), &pyproject("0.2.186"));
    write(
        &origin.join("src/claude_agent_sdk/_cli_version.py"),
        "__cli_version__ = \"2.1.329\"; injected = \"x\"\n",
    );
    fixture.git_in(origin, &["commit", "--quiet", "--all", "--message", "Odd"]);
    fixture.git_in(origin, &["tag", "v0.2.186"]);

    let output = fixture.run(&["upstream-sync", "v0.2.186"]);
    assert_refused(&fixture, &output, "not a version X.Y.Z[-pre]");
    assert_eq!(fixture.snapshot(), before);
}

#[test]
fn upstream_diff_prints_the_verdict() {
    let fixture = Fixture::new();
    let upstream = fixture.root.join(SUBMODULE);
    for (tag, verdict) in [("v0.2.184", "cli-only"), ("v0.2.185", "needs-port")] {
        let output = Command::new(env!("CARGO_BIN_EXE_xtask"))
            .args(["upstream-diff", tag, "--upstream"])
            .arg(&upstream)
            .output()
            .unwrap();
        assert!(output.status.success(), "{}", text(&output));
        assert!(
            text(&output).contains(&format!("\nverdict: {verdict}\n")),
            "{}",
            text(&output)
        );
    }
}
