//! A resume from a session store carries the user settings into the
//! resumed process.
//!
//! On such a resume the SDK runs the CLI under a temporary configuration
//! directory seeded from the one of the caller. The user settings file
//! must be part of that seed. The output style is the observable: the
//! init message echoes it, nothing can mask it, and it needs no tool
//! call.
//!
//! The CLI runs here under an empty configuration directory, so the
//! credentials must come from the environment: the login of the installed
//! CLI lives in the real configuration directory and is not copied.

use std::path::Path;

use claude_agent_kit::{ClaudeAgentOptionsBuilder, InMemorySessionStore, Message};
use serde_json::{Value, json};

use crate::suite;

async fn run(options: ClaudeAgentOptionsBuilder, prompt: &str) -> (Value, String) {
    let messages: Vec<Message> = suite::run(prompt, options.build().unwrap()).await;
    let result = suite::result(&messages);
    assert!(!result.is_error, "{result:#?}");
    (suite::init(&messages).clone(), result.session_id.clone())
}

fn options(
    cwd: &Path,
    config_directory: &Path,
    store: &InMemorySessionStore,
) -> ClaudeAgentOptionsBuilder {
    suite::options()
        .cwd(cwd)
        .session_store(store.clone())
        .env_var("CLAUDE_CONFIG_DIR", config_directory.to_string_lossy())
        .max_turns(1)
}

#[tokio::test]
#[ignore = "runs the real CLI; run with `cargo xtask e2e`"]
async fn a_store_resume_applies_the_user_settings() {
    assert!(
        suite::credentials_travel_with_the_environment(),
        "this test runs the CLI under an empty configuration directory, so it needs credentials \
         from the environment: set ANTHROPIC_API_KEY or CLAUDE_CODE_OAUTH_TOKEN. The login of \
         the installed CLI is not copied."
    );
    let (_directory, root) = suite::workspace();
    let config_directory = root.join("caller-config");
    let cwd = root.join("project");
    std::fs::create_dir(&config_directory).unwrap();
    std::fs::create_dir(&cwd).unwrap();
    let store = InMemorySessionStore::new();

    let (first_init, session_id) = run(
        options(&cwd, &config_directory, &store),
        "Reply with exactly: one",
    )
    .await;
    assert_eq!(first_init["output_style"], "default");

    std::fs::write(
        config_directory.join("settings.json"),
        json!({"outputStyle": "Explanatory"}).to_string(),
    )
    .unwrap();
    let _ = std::fs::remove_dir_all(config_directory.join("projects"));

    let (resumed_init, resumed_session_id) = run(
        options(&cwd, &config_directory, &store).resume(&session_id),
        "Reply with exactly: two",
    )
    .await;

    assert_eq!(
        resumed_session_id, session_id,
        "the session was not resumed"
    );
    assert_eq!(
        resumed_init["output_style"], "Explanatory",
        "the user settings were not applied on a store resume"
    );
}
