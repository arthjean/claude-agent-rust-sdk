#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::{Path, PathBuf};
use std::process::Command;

use claude_agent_kit::{CLI_VERSION, MINIMUM_CLI_VERSION, UPSTREAM_SDK_VERSION};

const UPSTREAM_COMMIT: &str = "4720d367f7db9d5df8a166c74d7f36a538ac8c34";
const UPSTREAM_PATH: &str = "upstream/claude-agent-sdk-python";

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .unwrap()
        .to_path_buf()
}

fn read(relative: &str) -> String {
    let path = workspace_root().join(relative);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()))
        .replace("\r\n", "\n")
}

fn compact(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[test]
fn workspace_has_the_four_members() {
    let manifest = read("Cargo.toml");
    for member in [
        "crates/claude-agent-kit",
        "crates/claude-agent-kit-macros",
        "crates/fake-claude",
        "xtask",
    ] {
        assert!(manifest.contains(&format!("\"{member}\"")), "{member}");
        assert!(workspace_root().join(member).join("Cargo.toml").is_file());
    }

    let library = read("crates/claude-agent-kit/Cargo.toml");
    assert!(library.contains("name = \"claude-agent-kit\""));
    assert!(!library.contains("publish = false"));

    let macros = read("crates/claude-agent-kit-macros/Cargo.toml");
    assert!(macros.contains("name = \"claude-agent-kit-macros\""));
    assert!(macros.contains("proc-macro = true"));

    for (path, name) in [
        ("crates/fake-claude/Cargo.toml", "fake-claude"),
        ("xtask/Cargo.toml", "xtask"),
    ] {
        let manifest = read(path);
        assert!(manifest.contains(&format!("name = \"{name}\"")));
        assert!(manifest.contains("publish = false"), "{name}");
        let main = Path::new(path).with_file_name("src").join("main.rs");
        assert!(workspace_root().join(main).is_file(), "{name} is a binary");
    }
}

#[test]
fn root_manifest_sets_toolchain_license_and_lint_policy() {
    let manifest = compact(&read("Cargo.toml"));
    for setting in [
        "rust-version = \"1.88\"",
        "edition = \"2024\"",
        "license = \"GPL-3.0-or-later\"",
        "[workspace.lints.clippy]",
        "panic = \"deny\"",
        "unimplemented = \"deny\"",
        "dbg_macro = \"deny\"",
        "todo = \"warn\"",
        "unwrap_used = \"warn\"",
        "expect_used = \"warn\"",
    ] {
        assert!(manifest.contains(setting), "{setting}");
    }

    for member in [
        "crates/claude-agent-kit",
        "crates/claude-agent-kit-macros",
        "crates/fake-claude",
        "xtask",
    ] {
        let manifest = compact(&read(&format!("{member}/Cargo.toml")));
        assert!(manifest.contains("[lints] workspace = true"), "{member}");
        assert!(manifest.contains("license.workspace = true"), "{member}");
        assert!(manifest.contains("edition.workspace = true"), "{member}");
        assert!(
            manifest.contains("rust-version.workspace = true"),
            "{member}"
        );
    }
}

#[test]
fn clippy_config_exempts_tests() {
    let config = compact(&read("clippy.toml"));
    for setting in [
        "allow-unwrap-in-tests = true",
        "allow-expect-in-tests = true",
        "allow-panic-in-tests = true",
    ] {
        assert!(config.contains(setting), "{setting}");
    }
}

#[test]
fn library_denies_missing_docs_and_unsafe_code() {
    let library = read("crates/claude-agent-kit/src/lib.rs");
    assert!(library.contains("#![deny(missing_docs)]"));
    assert!(library.contains("#![deny(unsafe_code)]"));
}

fn rust_sources(directory: &Path, found: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(directory).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            rust_sources(&path, found);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            found.push(path);
        }
    }
}

#[test]
fn unsafe_code_is_confined_to_the_process_sys_module() {
    let source_root = workspace_root().join("crates/claude-agent-kit/src");
    let sys_module = source_root.join("process").join("sys.rs");
    let mut sources = Vec::new();
    rust_sources(&source_root, &mut sources);
    assert!(sources.contains(&sys_module));

    for source in sources {
        let text = std::fs::read_to_string(&source).unwrap();
        let allows_unsafe = text.contains("allow(unsafe_code)");
        let uses_unsafe = text.contains("unsafe {") || text.contains("unsafe fn");
        if source == sys_module {
            assert!(text.contains("#![allow(unsafe_code)]"));
            assert!(uses_unsafe);
        } else {
            assert!(!allows_unsafe, "{}", source.display());
            assert!(!uses_unsafe, "{}", source.display());
        }
    }
}

#[test]
fn library_sources_carry_no_implementation_comments() {
    let mut sources = Vec::new();
    rust_sources(
        &workspace_root().join("crates/claude-agent-kit/src"),
        &mut sources,
    );
    rust_sources(
        &workspace_root().join("crates/fake-claude/src"),
        &mut sources,
    );
    for source in sources {
        let text = std::fs::read_to_string(&source).unwrap();
        for (number, line) in text.lines().enumerate() {
            let code = line.trim_start();
            let is_comment =
                code.starts_with("//") && !code.starts_with("///") && !code.starts_with("//!");
            assert!(!is_comment, "{}:{}: {line}", source.display(), number + 1);
        }
    }
}

#[test]
fn readme_states_who_authenticates_the_affiliation_and_the_license_consequence() {
    let readme = compact(&read("README.md").replace("\n> ", " "));
    for statement in [
        "End users authenticate with their own credentials.",
        "This is not an Anthropic product.",
        "It is not affiliated with, endorsed by, or supported by Anthropic.",
        "GNU General Public License, version 3 or later (GPL-3.0-or-later)",
        "If you distribute a program that links `claude-agent-kit`, you must release that \
         program under the GPL-3.0-or-later or a license compatible with it",
    ] {
        assert!(readme.contains(statement), "{statement}");
    }
    assert!(readme.contains(&format!("tested against {CLI_VERSION}")));
    assert!(readme.contains(&format!("at parity with version {UPSTREAM_SDK_VERSION}")));

    let library = read("crates/claude-agent-kit/src/lib.rs");
    assert!(library.contains("#[doc = include_str!(\"../../../README.md\")]"));
    let manifest = read("crates/claude-agent-kit/Cargo.toml");
    assert!(manifest.contains("readme = \"../../README.md\""));
}

#[test]
fn crate_root_documents_features_platforms_and_the_tested_cli_version() {
    let library = read("crates/claude-agent-kit/src/lib.rs");
    let manifest = read("crates/claude-agent-kit/Cargo.toml");
    let features = manifest
        .split("[features]")
        .nth(1)
        .and_then(|rest| rest.split("\n[").next())
        .expect("the manifest has no [features] table");
    let mut documented = 0;
    for feature in features.lines().filter_map(|line| line.split(" = ").next()) {
        let feature = feature.trim();
        if feature.is_empty() {
            continue;
        }
        assert!(
            library.contains(&format!("//! - `{feature}`: ")),
            "the crate root does not document the feature `{feature}`"
        );
        documented += 1;
    }
    assert!(documented >= 5, "{features}");
    for statement in [
        "//! # Platform support",
        "//! Linux, macOS, and Windows are supported",
        &format!("//! {CLI_VERSION} ([`CLI_VERSION`])"),
        &format!("//! {MINIMUM_CLI_VERSION} ([`MINIMUM_CLI_VERSION`])"),
    ] {
        assert!(library.contains(statement), "{statement}");
    }
}

#[test]
fn every_example_of_the_readme_exists() {
    let readme = read("README.md");
    let table = readme
        .split("## Examples")
        .nth(1)
        .and_then(|rest| rest.split("\n## ").next())
        .expect("the README has no Examples section");
    let named: Vec<&str> = table
        .lines()
        .filter_map(|line| line.strip_prefix("| `"))
        .filter_map(|line| line.split('`').next())
        .collect();
    assert_eq!(named.len(), 12, "{named:?}");
    for example in named {
        let file = workspace_root()
            .join("crates/claude-agent-kit/examples")
            .join(format!("{example}.rs"));
        assert!(file.is_file(), "{} is missing", file.display());
    }
}

#[test]
fn license_is_gpl_version_3_and_upstream_notice_is_preserved() {
    let license = read("LICENSE");
    assert!(license.starts_with("GNU GENERAL PUBLIC LICENSE\nVersion 3, 29 June 2007"));
    assert!(license.contains("TERMS AND CONDITIONS"));
    assert!(license.contains("END OF TERMS AND CONDITIONS"));

    let notices = read("THIRD_PARTY_NOTICES");
    assert!(notices.contains("claude-agent-sdk-python"));
    assert!(notices.contains(UPSTREAM_COMMIT));
    assert!(notices.contains("MIT License"));
    assert!(notices.contains("Copyright (c) 2025 Anthropic, PBC"));
    assert!(notices.contains(
        "The above copyright notice and this permission notice shall be included in all\n\
         copies or substantial portions of the Software."
    ));
    assert!(notices.contains("THE SOFTWARE IS PROVIDED \"AS IS\""));
}

#[test]
fn license_allow_list_is_enforced_in_ci() {
    let policy = read("deny.toml");
    assert!(policy.contains("[licenses]"));
    assert!(policy.contains("\"GPL-3.0-or-later\""));
    assert!(policy.contains("\"MIT\""));
    assert!(policy.contains("\"Apache-2.0\""));
    for incompatible in ["GPL-2.0-only", "AGPL", "SSPL", "BUSL", "CC-BY-NC"] {
        assert!(!policy.contains(incompatible), "{incompatible}");
    }

    let workflow = compact(&read(".github/workflows/ci.yml"));
    assert!(workflow.contains("EmbarkStudios/cargo-deny-action@"));
    assert!(workflow.contains("command: check licenses"));
}

#[test]
fn ci_runs_the_four_quality_gates_on_three_operating_systems() {
    let workflow = compact(&read(".github/workflows/ci.yml"));
    assert!(workflow.contains("os: [ubuntu-latest, macos-latest, windows-latest]"));
    assert!(workflow.contains("runs-on: ${{ matrix.os }}"));
    for gate in [
        "run: cargo fmt --all --check",
        "run: cargo clippy --workspace --all-targets --all-features --no-deps",
        "run: cargo test --workspace --all-features",
        "run: cargo doc --workspace --all-features --no-deps",
        "RUSTDOCFLAGS: -D warnings",
    ] {
        assert!(workflow.contains(gate), "{gate}");
    }
    assert!(workflow.contains("permissions: contents: read"));
}

#[test]
fn the_examples_are_built_with_default_features_in_ci_and_in_the_gates() {
    let workflow = compact(&read(".github/workflows/ci.yml"));
    assert!(workflow.contains("run: cargo check --package claude-agent-kit --examples"));
    let gates = compact(&read("xtask/src/gates.rs"));
    assert!(gates.contains(r#"&["check", "--package", "claude-agent-kit", "--examples"]"#));
}

fn ci_job(workflow: &str, name: &str) -> String {
    let mut lines = workflow
        .lines()
        .skip_while(|line| *line != format!("  {name}:"))
        .skip(1);
    let body: Vec<&str> = lines
        .by_ref()
        .take_while(|line| line.is_empty() || line.starts_with("    "))
        .collect();
    assert!(!body.is_empty(), "the workflow has no job `{name}`");
    compact(&body.join("\n"))
}

#[test]
fn ci_checks_the_parity_ledgers_on_every_pull_request() {
    let workflow = read(".github/workflows/ci.yml");
    assert!(compact(&workflow).starts_with("name: CI on: pull_request: push:"));
    let job = ci_job(&workflow, "parity");
    assert!(job.contains("run: cargo xtask parity-check --deny-pending"));
    assert!(job.contains("submodules: true"));
    assert!(!job.contains("if:"), "{job}");
}

#[test]
fn the_end_to_end_suite_runs_only_on_request_on_linux_after_the_tests_and_never_on_forks() {
    let ci = read(".github/workflows/ci.yml");
    assert!(!ci.contains("secrets.ANTHROPIC_API_KEY"));
    assert!(!ci.contains("cargo xtask e2e"));

    let workflow = read(".github/workflows/e2e.yml");
    assert!(compact(&workflow).starts_with("name: End-to-end on: workflow_dispatch: permissions:"));
    for trigger in ["push:", "pull_request", "schedule:", "workflow_run:"] {
        assert!(!workflow.contains(trigger), "{trigger}");
    }
    assert!(compact(&workflow).contains("permissions: contents: read"));

    let job = ci_job(&workflow, "e2e");
    assert!(job.contains("if: ${{ !github.event.repository.fork }}"));
    assert!(job.contains("runs-on: ubuntu-latest"));
    let tests = job
        .find("run: cargo test --workspace --all-features")
        .expect("the job does not run the tests");
    let suite = job
        .find("run: cargo xtask e2e")
        .expect("the job does not run the suite");
    assert!(tests < suite, "the tests must pass before the suite starts");
    assert!(job.contains("ANTHROPIC_API_KEY: ${{ secrets.ANTHROPIC_API_KEY }}"));
    assert!(!job.contains("CLAUDE_AGENT_KIT_E2E_AUTH"), "{job}");

    assert!(workflow.contains(&format!("CLAUDE_CODE_VERSION: {CLI_VERSION}")));
    assert!(job.contains("| bash -s \"$CLAUDE_CODE_VERSION\""));
}

#[test]
fn upstream_sdk_is_a_submodule_pinned_to_the_reference_commit() {
    let modules = compact(&read(".gitmodules"));
    assert!(modules.contains(&format!("path = {UPSTREAM_PATH}")));
    assert!(modules.contains("url = https://github.com/anthropics/claude-agent-sdk-python.git"));

    let root = workspace_root();
    if !root.join(".git").exists() {
        return;
    }
    let Ok(output) = Command::new("git")
        .arg("-C")
        .arg(&root)
        .args(["ls-files", "--stage", "--", UPSTREAM_PATH])
        .output()
    else {
        return;
    };
    assert!(output.status.success());
    let entry = String::from_utf8(output.stdout).unwrap();
    assert!(
        entry.starts_with(&format!("160000 {UPSTREAM_COMMIT} ")),
        "unexpected index entry: {entry:?}"
    );
}

#[test]
fn version_constants_match_the_manifest_and_the_upstream_pin() {
    assert_eq!(claude_agent_kit::VERSION, env!("CARGO_PKG_VERSION"));
    assert_eq!(claude_agent_kit::CLI_VERSION, "2.1.325");
    assert_eq!(claude_agent_kit::UPSTREAM_SDK_VERSION, "0.2.182");
}

#[test]
fn xtask_alias_is_configured() {
    let config = compact(&read(".cargo/config.toml"));
    assert!(config.contains("xtask = \"run --quiet --package xtask --\""));
}
