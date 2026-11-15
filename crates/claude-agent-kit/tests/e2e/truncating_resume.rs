//! Truncating resume: a fork that drops the turns after a given message.
//!
//! Each test builds a two-turn session, then forks it at the end of turn
//! one. With `resume_drops_turn` set to the prompt of turn two the CLI
//! accepts the discarded range; with any other id it refuses.

use std::path::Path;

use claude_agent_kit::{Error, Message, SessionMessageType, get_session_messages};

use crate::suite;

const TURN_1: &str = "Reply with exactly: one";
const TURN_2: &str = "Reply with exactly: two";
const TURN_3: &str = "Reply with exactly: three";

struct Session {
    id: String,
    keep_at: String,
    turn_2_prompt: String,
}

async fn two_turn_session(cwd: &Path) -> Session {
    let client = suite::connect(suite::options().cwd(cwd).max_turns(1).build().unwrap()).await;
    let first = suite::turn(&client, TURN_1).await;
    let second = suite::turn(&client, TURN_2).await;
    client.disconnect().await.unwrap();

    let keep_at = first
        .iter()
        .rev()
        .find_map(|message| match message {
            Message::Assistant(assistant) => assistant.uuid.clone(),
            _ => None,
        })
        .expect("turn one has no assistant message with an id");
    assert!(!suite::result(&first).is_error);
    let last = suite::result(&second);
    assert!(!last.is_error);
    let id = last.session_id.clone();

    let chain = get_session_messages(&id, Some(cwd), None, 0);
    let shape: Vec<_> = chain
        .iter()
        .map(|message| (message.r#type, message.uuid.as_str()))
        .collect();
    let kept = chain
        .iter()
        .position(|message| message.uuid == keep_at)
        .unwrap_or_else(|| panic!("{keep_at} is not in the transcript: {shape:?}"));
    let turn_2 = chain
        .iter()
        .position(|message| {
            message.r#type == SessionMessageType::User && message.message["content"] == TURN_2
        })
        .unwrap_or_else(|| panic!("the prompt of turn two is not in the transcript: {shape:?}"));
    assert_eq!(
        turn_2,
        kept + 1,
        "unexpected transcript entries between turn one and turn two: {shape:?}"
    );
    Session {
        id,
        keep_at,
        turn_2_prompt: chain[turn_2].uuid.clone(),
    }
}

fn prompts(session_id: &str, cwd: &Path) -> Vec<String> {
    get_session_messages(session_id, Some(cwd), None, 0)
        .into_iter()
        .filter(|message| message.r#type == SessionMessageType::User)
        .filter_map(|message| message.message["content"].as_str().map(str::to_owned))
        .collect()
}

#[tokio::test]
#[ignore = "runs the real CLI; run with `cargo xtask e2e`"]
async fn a_fork_that_names_the_dropped_turn_succeeds_without_it() {
    let (_directory, cwd) = suite::workspace();
    let session = two_turn_session(&cwd).await;
    let options = suite::options()
        .cwd(&cwd)
        .max_turns(1)
        .resume(&session.id)
        .fork_session(true)
        .resume_session_at(&session.keep_at)
        .resume_drops_turn(&session.turn_2_prompt)
        .build()
        .unwrap();

    let messages = suite::run(TURN_3, options).await;

    let forked = suite::result(&messages);
    assert!(!forked.is_error, "{forked:#?}");
    assert_ne!(forked.session_id, session.id);
    let forked_prompts = prompts(&forked.session_id, &cwd);
    let source_prompts = prompts(&session.id, &cwd);
    suite::forget(&cwd);
    assert_eq!(forked_prompts, [TURN_1, TURN_3]);
    assert_eq!(source_prompts, [TURN_1, TURN_2]);
}

#[tokio::test]
#[ignore = "runs the real CLI; run with `cargo xtask e2e`"]
async fn a_fork_that_names_another_turn_is_refused() {
    let (_directory, cwd) = suite::workspace();
    let session = two_turn_session(&cwd).await;
    let options = suite::options()
        .cwd(&cwd)
        .max_turns(1)
        .resume(&session.id)
        .fork_session(true)
        .resume_session_at(&session.keep_at)
        .resume_drops_turn(uuid::Uuid::new_v4().to_string())
        .build()
        .unwrap();

    let (messages, error) = suite::run_to_end(TURN_3, options).await;

    let source_prompts = prompts(&session.id, &cwd);
    suite::forget(&cwd);
    let error = error.unwrap_or_else(|| panic!("the fork was not refused: {messages:#?}"));
    assert!(
        matches!(error, Error::ResultError { .. } | Error::Process { .. }),
        "{error:?}"
    );
    assert!(
        error
            .to_string()
            .contains("Resume rejected by --resume-drops-turn:"),
        "{error}"
    );
    assert_eq!(source_prompts, [TURN_1, TURN_2]);
}
