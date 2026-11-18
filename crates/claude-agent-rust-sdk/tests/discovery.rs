#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::ffi::OsStr;
use std::path::Path;
use std::sync::Mutex;

use claude_agent_kit::cli::{find_cli, managed_install_dir};
use claude_agent_kit::{ClaudeAgentOptions, Error, SubprocessTransport};
use serde_json::json;

use common::capture::Warnings;
use common::{Fake, SCENARIO_VARIABLE, collect, fake_claude, next_message};

const CLI_NAME: &str = if cfg!(windows) {
    "claude.exe"
} else {
    "claude"
};

static PROCESS_STATE: Mutex<()> = Mutex::new(());

fn set(name: &str, value: impl AsRef<OsStr>) {
    unsafe { std::env::set_var(name, value) }
}

fn remove(name: &str) {
    unsafe { std::env::remove_var(name) }
}

fn install(directory: &Path) -> std::path::PathBuf {
    std::fs::create_dir_all(directory).unwrap();
    let installed = directory.join(CLI_NAME);
    std::fs::copy(fake_claude(), &installed).unwrap();
    installed
}

fn not_found_message(error: Error) -> String {
    match error {
        Error::CliNotFound { message } => message,
        other => panic!("expected CliNotFound, got {other:?}"),
    }
}

#[test]
fn the_process_environment_drives_discovery_and_the_version_check_switch() {
    let _serial = PROCESS_STATE
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    let fake = Fake::new(json!({"claude_code_version": null, "steps": [{"exit": 0}]}));
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let built_before_the_install = SubprocessTransport::new(ClaudeAgentOptions::default());
    let on_path = install(&root.path().join("bin"));

    set("PATH", root.path().join("bin"));
    set("HOME", &home);
    set("USERPROFILE", &home);
    set("LOCALAPPDATA", root.path().join("local-app-data"));
    set("XDG_DATA_HOME", root.path().join("xdg-data"));
    set(SCENARIO_VARIABLE, &fake.scenario);
    let system_wide_cli = Path::new("/usr/local/bin/claude").is_file();

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    assert_eq!(find_cli().unwrap(), on_path);
    runtime.block_on(async {
        built_before_the_install.connect().await.unwrap();
        let mut stream = built_before_the_install.read_messages();
        let init = next_message(&mut stream).await;
        assert_eq!(init["subtype"], "init");
        assert!(init["fake_pid"].is_u64(), "{init}");
        built_before_the_install.close().await.unwrap();
    });
    runtime.block_on(async {
        let transport = SubprocessTransport::new(ClaudeAgentOptions::default());
        transport.connect().await.unwrap();
        let mut stream = transport.read_messages();
        assert_eq!(next_message(&mut stream).await["subtype"], "init");
        transport.close().await.unwrap();
    });

    let managed_dir = managed_install_dir().unwrap();
    assert!(managed_dir.starts_with(root.path()), "{managed_dir:?}");
    assert!(managed_dir.ends_with(Path::new("claude-agent-kit").join("cli")));
    let managed = install(&managed_dir);
    assert_eq!(find_cli().unwrap(), managed);
    std::fs::remove_file(&managed).unwrap();

    let explicit = install(&root.path().join("explicit"));
    std::fs::remove_file(&on_path).unwrap();
    runtime.block_on(async {
        let options = ClaudeAgentOptions::builder()
            .cli_path(&explicit)
            .build()
            .unwrap();
        let transport = SubprocessTransport::new(options);
        transport.connect().await.unwrap();
        transport.close().await.unwrap();
    });

    if !system_wide_cli {
        let fallback = install(&home.join(".local").join("bin"));
        assert_eq!(find_cli().unwrap(), fallback);
        std::fs::remove_file(&fallback).unwrap();

        let message = not_found_message(find_cli().unwrap_err());
        assert!(
            message.starts_with("Claude CLI not found. Install"),
            "{message}"
        );
        let connect_message = runtime.block_on(async {
            let transport = SubprocessTransport::new(ClaudeAgentOptions::default());
            let error = transport.connect().await.unwrap_err();
            assert_eq!(transport.process_id(), None);
            not_found_message(error)
        });
        assert_eq!(connect_message, message);
        if cfg!(windows) {
            assert!(message.contains("install.ps1"));
            assert!(message.contains("claude.cmd shim"));
        } else {
            assert!(message.contains("npm install -g @anthropic-ai/claude-code"));
        }
    }

    let read_with_warnings = |skip: Option<&str>| {
        match skip {
            Some(value) => set("CLAUDE_AGENT_SDK_SKIP_VERSION_CHECK", value),
            None => remove("CLAUDE_AGENT_SDK_SKIP_VERSION_CHECK"),
        }
        runtime.block_on(async {
            let (warnings, _guard) = Warnings::install();
            let options = ClaudeAgentOptions::builder()
                .cli_path(&explicit)
                .build()
                .unwrap();
            let transport = SubprocessTransport::new(options);
            transport.connect().await.unwrap();
            collect(transport.read_messages()).await;
            transport.close().await.unwrap();
            warnings.about_the_cli_version()
        })
    };
    assert_eq!(read_with_warnings(None).len(), 1);
    assert_eq!(read_with_warnings(Some("1")).len(), 0);
}

#[cfg(windows)]
struct WorkingDirectory(std::path::PathBuf);

#[cfg(windows)]
impl WorkingDirectory {
    fn enter(directory: &Path) -> Self {
        let previous = std::env::current_dir().unwrap();
        std::env::set_current_dir(directory).unwrap();
        Self(previous)
    }
}

#[cfg(windows)]
impl Drop for WorkingDirectory {
    fn drop(&mut self) {
        std::env::set_current_dir(&self.0).unwrap();
    }
}

#[cfg(windows)]
#[test]
fn the_working_directory_and_relative_path_entries_are_never_searched_on_windows() {
    let _serial = PROCESS_STATE
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let work = root.path().join("work");
    install(&work);
    install(&work.join("tools"));
    let on_path = install(&root.path().join("bin"));

    set("HOME", &home);
    set("USERPROFILE", &home);
    set("LOCALAPPDATA", root.path().join("local-app-data"));
    set("XDG_DATA_HOME", root.path().join("xdg-data"));
    let _directory = WorkingDirectory::enter(&work);
    assert!(Path::new(CLI_NAME).is_file());
    assert!(Path::new("tools").join(CLI_NAME).is_file());

    set(
        "PATH",
        std::env::join_paths([
            Path::new(""),
            Path::new("."),
            Path::new("tools"),
            root.path().join("bin").as_path(),
        ])
        .unwrap(),
    );
    assert_eq!(find_cli().unwrap(), on_path);

    set(
        "PATH",
        std::env::join_paths([Path::new(""), Path::new(".")]).unwrap(),
    );
    let message = not_found_message(find_cli().unwrap_err());
    assert!(message.starts_with("Claude CLI not found."), "{message}");
}
