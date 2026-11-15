//! End-to-end check of the definition of done of EP-005: a session mirrored
//! to a store and then held only in that store is resumed by the real CLI.
//!
//! It runs the real CLI, so it needs API credentials and is ignored by
//! default. Run it with:
//!
//! ```text
//! CLAUDE_AGENT_KIT_E2E=1 cargo test --test store_resume_e2e -- --ignored --nocapture
//! ```
//!
//! The first query asks the model to remember a code word, with an
//! [`InMemorySessionStore`] receiving the mirrored transcript. The local
//! project directory is then removed, so the store holds the only copy.
//! The second query resumes the session from the store and asks for the
//! code word. The directories the CLI created are removed afterwards.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use claude_agent_kit::{
    ClaudeAgentOptions, ContentBlock, InMemorySessionStore, Message, SessionKey,
    project_key_for_directory, query,
};
use futures_util::StreamExt;

const CODE_WORD: &str = "PELICAN-4417";

fn projects_dir() -> PathBuf {
    match std::env::var("CLAUDE_CONFIG_DIR") {
        Ok(directory) if !directory.is_empty() => PathBuf::from(directory).join("projects"),
        _ => std::env::home_dir()
            .unwrap()
            .join(".claude")
            .join("projects"),
    }
}

fn resume_dirs() -> BTreeSet<PathBuf> {
    std::fs::read_dir(std::env::temp_dir())
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| {
                    path.file_name()
                        .is_some_and(|name| name.to_string_lossy().starts_with("claude-resume-"))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn options(cwd: &Path, store: &InMemorySessionStore, resume: Option<&str>) -> ClaudeAgentOptions {
    let mut builder = ClaudeAgentOptions::builder()
        .cwd(cwd.to_path_buf())
        .model("haiku")
        .max_turns(1)
        .session_store(store.clone());
    if let Some(resume) = resume {
        builder = builder.resume(resume);
    }
    builder.build().unwrap()
}

/// Runs one query and returns the session id and the assistant text.
async fn run(prompt: &str, options: ClaudeAgentOptions) -> (String, String) {
    let mut messages = query(prompt, options);
    let mut text = String::new();
    let mut session_id = None;
    while let Some(message) = messages.next().await {
        match message.unwrap() {
            Message::Assistant(assistant) => {
                for block in &assistant.content {
                    if let ContentBlock::Text(block) = block {
                        text.push_str(&block.text);
                    }
                }
            }
            Message::Result(result) => {
                assert!(!result.is_error, "the turn failed: {result:?}");
                session_id = Some(result.session_id.clone());
            }
            _ => {}
        }
    }
    (session_id.expect("no result message"), text)
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "runs the real CLI; set CLAUDE_AGENT_KIT_E2E=1 and pass --ignored"]
async fn a_session_held_only_in_a_store_is_resumed_by_the_cli() {
    if std::env::var("CLAUDE_AGENT_KIT_E2E").as_deref() != Ok("1") {
        eprintln!("CLAUDE_AGENT_KIT_E2E is not 1: skipping");
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let cwd = base.path().join("store-resume-case");
    std::fs::create_dir_all(&cwd).unwrap();
    let project_key = project_key_for_directory(Some(&cwd));
    let local_project = projects_dir().join(&project_key);
    let resume_before = resume_dirs();
    let store = InMemorySessionStore::new();

    let (session_id, first) = run(
        &format!("Remember the code word {CODE_WORD}. Reply with the single word OK."),
        options(&cwd, &store, None),
    )
    .await;
    println!("first turn: session {session_id}, reply {first:?}");
    let mirrored = store.get_entries(&SessionKey::new(project_key.as_str(), session_id.as_str()));
    println!("mirrored entries: {}", mirrored.len());
    assert!(
        mirrored
            .iter()
            .any(|entry| entry.get("type").and_then(|kind| kind.as_str()) == Some("user")),
        "the store holds the user turn"
    );

    std::fs::remove_dir_all(&local_project).unwrap();
    let (resumed_id, second) = run(
        "What was the code word I asked you to remember? Reply with the code word only.",
        options(&cwd, &store, Some(&session_id)),
    )
    .await;
    println!("second turn: session {resumed_id}, reply {second:?}");
    let _ = std::fs::remove_dir_all(&local_project);

    assert!(
        second.contains(CODE_WORD),
        "the resumed session lost the first turn: {second:?}"
    );
    assert_eq!(resumed_id, session_id, "the CLI resumed the same session");
    let after = store.get_entries(&SessionKey::new(project_key.as_str(), session_id.as_str()));
    assert!(
        after.len() > mirrored.len(),
        "the resumed turn is mirrored to the store"
    );
    assert_eq!(
        resume_dirs(),
        resume_before,
        "the temporary config directory is removed"
    );
}
