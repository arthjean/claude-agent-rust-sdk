//! The real CLI, without the API.
//!
//! The CLI answers the control protocol and writes its init message before
//! it calls the API, so the handshake, the options that reach the command
//! line, and the init data can be checked against the pinned CLI with no
//! credentials: the CLI runs with a placeholder key, an API base URL on a
//! closed local port, and empty configuration and home directories. No
//! request reaches the API, so a run costs nothing.
//!
//! The tests are ignored because they need the CLI installed. The `Offline
//! CLI` job of `.github/workflows/ci.yml` installs the pinned version and
//! runs them:
//!
//! ```text
//! cargo test -p claude-agent-rust-sdk --all-features --test offline_cli -- --ignored
//! ```
//!
//! `CLAUDE_AGENT_RUST_SDK_TEST_CLI` names the executable when the one found
//! on `PATH` is a launcher that needs the full environment.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::PathBuf;
use std::time::Duration;

use claude_agent_rust_sdk::cli::find_cli;
use claude_agent_rust_sdk::{
    CLI_VERSION, ClaudeAgentOptions, ClaudeAgentOptionsBuilder, ClaudeSdkClient, Message,
    SystemInitData,
};
use futures_util::StreamExt;

/// How long the CLI may take to start and answer the handshake.
const LIMIT: Duration = Duration::from_secs(60);

/// The options of a session that cannot reach the API, with the
/// directories it writes to.
struct Offline {
    config: tempfile::TempDir,
    home: tempfile::TempDir,
    cwd: tempfile::TempDir,
}

impl Offline {
    fn new() -> Self {
        Self {
            config: tempfile::tempdir().unwrap(),
            home: tempfile::tempdir().unwrap(),
            cwd: tempfile::tempdir().unwrap(),
        }
    }

    fn options(&self) -> ClaudeAgentOptionsBuilder {
        let cli = std::env::var_os("CLAUDE_AGENT_RUST_SDK_TEST_CLI")
            .map_or_else(|| find_cli().unwrap(), PathBuf::from);
        let path = |dir: &tempfile::TempDir| dir.path().to_string_lossy().into_owned();
        ClaudeAgentOptions::builder()
            .cli_path(cli)
            .cwd(self.cwd.path())
            .env_var("HOME", path(&self.home))
            .env_var("USERPROFILE", path(&self.home))
            .env_var("CLAUDE_CONFIG_DIR", path(&self.config))
            .env_var("ANTHROPIC_API_KEY", "sk-ant-placeholder")
            .env_var("ANTHROPIC_BASE_URL", "http://127.0.0.1:9")
            .env_var("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1")
            .env_var("DISABLE_AUTOUPDATER", "1")
    }
}

/// Connects, sends one prompt, and returns the server info and the init
/// data; the session is closed before the CLI gives up on the API.
async fn handshake(options: ClaudeAgentOptions) -> (serde_json::Value, SystemInitData) {
    let client = ClaudeSdkClient::new(options);
    tokio::time::timeout(LIMIT, client.connect())
        .await
        .expect("the CLI answers initialize")
        .unwrap();
    let info = client.server_info().unwrap().expect("server info");
    client
        .query("Reply with the single word OK.")
        .await
        .unwrap();
    let init = tokio::time::timeout(LIMIT, async {
        let mut messages = client.receive_messages();
        while let Some(message) = messages.next().await {
            if let Message::System(system) = message.unwrap()
                && let Some(init) = system.init_data()
            {
                return init;
            }
        }
        panic!("the session ended before its init message");
    })
    .await
    .expect("the CLI writes its init message");
    client.disconnect().await.unwrap();
    (info, init)
}

#[tokio::test]
#[ignore = "runs the real CLI, without the API; pass --ignored"]
async fn the_pinned_cli_answers_the_handshake_and_reports_its_version() {
    let offline = Offline::new();
    let (info, init) = handshake(offline.options().build().unwrap()).await;
    assert!(info["commands"].is_array(), "{info}");
    assert_eq!(init.claude_code_version, CLI_VERSION);
    assert!(!init.tools.is_empty());
}

#[tokio::test]
#[ignore = "runs the real CLI, without the API; pass --ignored"]
async fn options_reach_the_pinned_cli_as_its_init_message_reports_them() {
    let offline = Offline::new();
    let options = offline
        .options()
        .model("claude-sonnet-4-5")
        .tools(claude_agent_rust_sdk::Tools::list(["Read", "Grep"]))
        .permission_mode(claude_agent_rust_sdk::PermissionMode::Plan)
        .build()
        .unwrap();
    let (_, init) = handshake(options).await;
    assert_eq!(init.model, "claude-sonnet-4-5");
    assert_eq!(init.permission_mode, "plan");
    let mut tools = init.tools.clone();
    tools.sort();
    assert_eq!(tools, ["Grep", "Read"]);
    assert_eq!(
        std::fs::canonicalize(&init.cwd).unwrap(),
        std::fs::canonicalize(offline.cwd.path()).unwrap()
    );
}

/// CLI 2.1.329 adds its built-in `claude-code-guide` agent for every
/// entrypoint that is not an SDK it knows (`sdk-ts`, `sdk-py`, `sdk-cli`),
/// so a session started with the `sdk-rs` label lists it where a Python
/// session does not, and the description of the Agent tool the model sees
/// differs. The assertion flips once the CLI knows `sdk-rs` (see
/// `[wire.entrypoint-label]` in `parity/differences.toml`).
#[tokio::test]
#[ignore = "runs the real CLI, without the API; pass --ignored"]
async fn a_session_with_the_sdk_rs_entrypoint_lists_the_agent_a_python_session_does_not() {
    let offline = Offline::new();
    let (_, init) = handshake(offline.options().build().unwrap()).await;
    let agents = init.agents.unwrap_or_default();
    assert!(
        agents.iter().any(|agent| agent == "claude-code-guide"),
        "the CLI no longer adds claude-code-guide for sdk-rs: {agents:?}"
    );
}
