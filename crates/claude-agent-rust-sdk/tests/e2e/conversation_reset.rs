//! `/clear` in a live session yields a conversation-reset message.

use claude_agent_rust_sdk::Message;

use crate::suite;

#[tokio::test]
#[ignore = "runs the real CLI; run with `cargo xtask e2e`"]
async fn clear_emits_a_conversation_reset_for_the_old_session() {
    let options = suite::options().max_turns(1).build().unwrap();
    let client = suite::connect(options).await;

    let first = suite::turn(&client, "Reply with exactly: one").await;
    let first_result = suite::result(&first).clone();
    let cleared = suite::turn(&client, "/clear").await;
    client.disconnect().await.unwrap();

    let reset = cleared
        .iter()
        .find_map(|message| match message {
            Message::ConversationReset(reset) => Some(reset),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no conversation reset after /clear: {cleared:#?}"));
    assert_eq!(reset.session_id, first_result.session_id);
    assert!(!reset.new_conversation_id.is_empty());
    assert!(!reset.uuid.is_empty());

    let clear_result = suite::result(&cleared);
    assert_ne!(clear_result.session_id, first_result.session_id);
}
