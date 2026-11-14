#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::{Path, PathBuf};
use std::process::Command;

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
