//! A session mirrored to a store and then held only in that store is
//! resumed by the real CLI: the definition of done of EP-005.
//!
//! The first query asks the model to remember a code word, with an
//! [`InMemorySessionStore`] receiving the mirrored transcript. The local
//! project directory is then removed, so the store holds the only copy.
//! The second query resumes the session from the store and asks for the
//! code word. The directories the CLI created are removed afterwards.

use std::collections::BTreeSet;
use std::path::PathBuf;

use claude_agent_kit::{ClaudeAgentOptions, InMemorySessionStore, Message, SessionKey};

use crate::suite;

const CODE_WORD: &str = "PELICAN-4417";

/// Temporary resume directories that hold a transcript of `project_key`,
/// so the sessions other tests run in parallel are not counted.
fn resume_dirs(project_key: &str) -> BTreeSet<PathBuf> {
    std::fs::read_dir(std::env::temp_dir())
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| {
                    path.file_name()
                        .is_some_and(|name| name.to_string_lossy().starts_with("claude-resume-"))
                        && path.join("projects").join(project_key).is_dir()
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Runs one query through the suite and returns the session id and the
/// text of the top-level assistant messages.
async fn turn(prompt: &str, options: ClaudeAgentOptions) -> (String, String) {
    let messages = suite::run(prompt, options).await;
    let result = suite::result(&messages);
    assert!(!result.is_error, "the turn failed: {result:#?}");
    let text = messages
        .iter()
        .filter_map(|message| match message {
            Message::Assistant(assistant) if assistant.parent_tool_use_id.is_none() => {
                Some(assistant)
            }
            _ => None,
        })
        .flat_map(|assistant| assistant.content.iter().filter_map(|block| block.as_text()))
        .collect();
    (result.session_id.clone(), text)
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "runs the real CLI; run with `cargo xtask e2e`"]
async fn a_session_held_only_in_a_store_is_resumed_by_the_cli() {
    let first_options = suite::options();
    let (_directory, cwd) = suite::workspace();
    let project_key = claude_agent_kit::project_key_for_directory(Some(&cwd));
    let local_project = suite::project_directory(&cwd);
    let store = InMemorySessionStore::new();

    let (session_id, first) = turn(
        &format!("Remember the code word {CODE_WORD}. Reply with the single word OK."),
        first_options
            .cwd(&cwd)
            .max_turns(1)
            .session_store(store.clone())
            .build()
            .unwrap(),
    )
    .await;
    println!("first turn: session {session_id}, reply {first:?}");
    let key = SessionKey::new(project_key.as_str(), session_id.as_str());
    let mirrored = store.get_entries(&key);
    assert!(
        mirrored
            .iter()
            .any(|entry| entry.get("type").and_then(|kind| kind.as_str()) == Some("user")),
        "the store holds the user turn"
    );

    std::fs::remove_dir_all(&local_project).unwrap();
    let (resumed_id, second) = turn(
        "What was the code word I asked you to remember? Reply with the code word only.",
        suite::options()
            .cwd(&cwd)
            .max_turns(1)
            .session_store(store.clone())
            .resume(session_id.as_str())
            .build()
            .unwrap(),
    )
    .await;
    println!("second turn: session {resumed_id}, reply {second:?}");
    suite::forget(&cwd);

    assert!(
        second.contains(CODE_WORD),
        "the resumed session lost the first turn: {second:?}"
    );
    assert_eq!(resumed_id, session_id, "the CLI resumed the same session");
    assert!(
        store.get_entries(&key).len() > mirrored.len(),
        "the resumed turn is mirrored to the store"
    );
    assert_eq!(
        resume_dirs(&project_key),
        BTreeSet::new(),
        "the temporary config directory is removed"
    );
}
