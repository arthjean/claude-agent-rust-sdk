//! Experiment of US-023: does `project_key_for_directory` name a project
//! directory the way the real CLI does?
//!
//! It runs the real CLI, so it needs API credentials and is ignored by
//! default. Run it with:
//!
//! ```text
//! CLAUDE_AGENT_RUST_SDK_E2E=1 cargo test --test project_dir_naming -- --ignored --nocapture
//! ```
//!
//! For each case the CLI answers one prompt in a fresh working directory;
//! the directory it creates under `projects/` is compared with the key, and
//! the session is looked up through `list_sessions`. The directories the
//! experiment created are removed afterwards. The results are recorded in
//! `docs/validation/project-dir-naming.md`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use claude_agent_rust_sdk::cli::find_cli;
use claude_agent_rust_sdk::{ListSessionsOptions, list_sessions, project_key_for_directory};

fn projects_dir() -> PathBuf {
    match std::env::var("CLAUDE_CONFIG_DIR") {
        Ok(directory) if !directory.is_empty() => PathBuf::from(directory).join("projects"),
        _ => std::env::home_dir()
            .unwrap()
            .join(".claude")
            .join("projects"),
    }
}

fn names(directory: &Path) -> BTreeSet<String> {
    std::fs::read_dir(directory)
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default()
}

fn units(text: &str) -> usize {
    text.encode_utf16().count()
}

fn run_case(cli: &Path, label: &str, cwd: &Path) -> bool {
    std::fs::create_dir_all(cwd).unwrap();
    let projects = projects_dir();
    let before = names(&projects);
    let output = Command::new(cli)
        .args([
            "-p",
            "Reply with the single word OK.",
            "--max-turns",
            "1",
            "--model",
            "haiku",
        ])
        .current_dir(cwd)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "the CLI failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let created: Vec<String> = names(&projects).difference(&before).cloned().collect();
    let key = project_key_for_directory(Some(cwd));
    let found = !list_sessions(
        ListSessionsOptions::new()
            .directory(cwd)
            .include_worktrees(false),
    )
    .is_empty();
    let exact = created.contains(&key);
    println!("case: {label}");
    println!("  cwd units: {}", units(&cwd.to_string_lossy()));
    println!("  created by the CLI: {created:?}");
    println!("  project_key_for_directory: {key}");
    println!("  exact match: {exact}");
    println!("  found by list_sessions: {found}");
    for name in &created {
        std::fs::remove_dir_all(projects.join(name)).unwrap();
    }
    found
}

#[test]
#[ignore = "runs the real CLI; set CLAUDE_AGENT_RUST_SDK_E2E=1 and pass --ignored"]
fn project_directory_naming_matches_the_cli() {
    if std::env::var("CLAUDE_AGENT_RUST_SDK_E2E").as_deref() != Ok("1") {
        eprintln!("CLAUDE_AGENT_RUST_SDK_E2E is not 1: skipping");
        return;
    }
    let cli = find_cli().unwrap();
    let version = Command::new(&cli).arg("--version").output().unwrap();
    println!("cli: {}", String::from_utf8_lossy(&version.stdout).trim());
    let base = tempfile::tempdir().unwrap();
    let mut long = base.path().join("long");
    while units(&long.to_string_lossy()) <= 230 {
        long.push("nested-directory-segment");
    }
    let cases = [
        ("ascii under 200 units", base.path().join("ascii-case")),
        (
            "outside the basic plane",
            base.path().join("emoji-\u{1F600}-case"),
        ),
        ("over 200 units", long),
    ];
    let mut all_found = true;
    for (label, cwd) in &cases {
        all_found &= run_case(&cli, label, cwd);
    }
    assert!(all_found, "a session was not found; see the output above");
}

/// Runs the real CLI without reaching the API: an empty configuration
/// directory, a placeholder key, and an API base URL on a closed local
/// port. The CLI creates the project directory and the transcript of the
/// prompt before it calls the API, so the name is observed and nothing is
/// spent. The CLI is stopped once the transcript exists.
fn created_without_the_api(cli: &Path, cwd: &Path) -> Vec<String> {
    std::fs::create_dir_all(cwd).unwrap();
    let config = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let mut command = Command::new(cli);
    command
        .args(["-p", "Reply with the single word OK.", "--max-turns", "1"])
        .current_dir(cwd)
        .env_clear();
    for (name, value) in [
        ("PATH", std::ffi::OsStr::new("/usr/bin:/bin")),
        ("HOME", home.path().as_os_str()),
        ("CLAUDE_CONFIG_DIR", config.path().as_os_str()),
        (
            "ANTHROPIC_API_KEY",
            std::ffi::OsStr::new("sk-ant-placeholder"),
        ),
        (
            "ANTHROPIC_BASE_URL",
            std::ffi::OsStr::new("http://127.0.0.1:9"),
        ),
        (
            "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC",
            std::ffi::OsStr::new("1"),
        ),
        ("DISABLE_AUTOUPDATER", std::ffi::OsStr::new("1")),
    ] {
        command.env(name, value);
    }
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(std::fs::File::create(home.path().join("stderr")).unwrap())
        .spawn()
        .unwrap();
    let projects = config.path().join("projects");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    let has_transcript = |name: &String| {
        std::fs::read_dir(projects.join(name)).is_ok_and(|entries| {
            entries
                .flatten()
                .any(|entry| entry.path().extension().is_some_and(|ext| ext == "jsonl"))
        })
    };
    let created = loop {
        let created: Vec<String> = names(&projects).into_iter().collect();
        if !created.is_empty() && created.iter().all(has_transcript) {
            break created;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the CLI wrote no transcript: {:?} {}",
            names(&projects),
            std::fs::read_to_string(home.path().join("stderr")).unwrap_or_default()
        );
        std::thread::sleep(std::time::Duration::from_millis(100));
    };
    let _ = child.kill();
    let _ = child.wait();
    created
}

/// The CLI runs with an emptied environment, so a launcher that needs
/// `PATH` (a terminal shim, for example) cannot stand in for it:
/// `CLAUDE_AGENT_RUST_SDK_TEST_CLI` names the executable itself.
#[test]
#[ignore = "runs the real CLI, without the API; pass --ignored"]
fn project_directory_naming_matches_the_cli_without_calling_the_api() {
    let cli = std::env::var_os("CLAUDE_AGENT_RUST_SDK_TEST_CLI")
        .map_or_else(|| find_cli().unwrap(), PathBuf::from);
    let version = Command::new(&cli).arg("--version").output().unwrap();
    println!("cli: {}", String::from_utf8_lossy(&version.stdout).trim());
    let base = tempfile::tempdir().unwrap();
    let mut long = base.path().join("long");
    while units(&long.to_string_lossy()) <= 230 {
        long.push("nested-directory-segment");
    }
    let cases = [
        ("ascii under 200 units", base.path().join("ascii-case")),
        (
            "outside the basic plane",
            base.path().join("emoji-\u{1F600}-case"),
        ),
        ("accented, precomposed", base.path().join("caf\u{e9}-case")),
        ("over 200 units", long),
    ];
    let mut all_exact = true;
    for (label, cwd) in &cases {
        let created = created_without_the_api(&cli, cwd);
        let key = project_key_for_directory(Some(cwd));
        let exact = created == [key.clone()];
        println!("case: {label}");
        println!("  cwd units: {}", units(&cwd.to_string_lossy()));
        println!("  created by the CLI: {created:?}");
        println!("  project_key_for_directory: {key}");
        println!("  exact match: {exact}");
        all_exact &= exact;
    }
    assert!(all_exact, "a name differs; see the output above");
}
