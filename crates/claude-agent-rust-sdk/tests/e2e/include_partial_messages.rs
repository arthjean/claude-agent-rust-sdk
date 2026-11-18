//! Partial message streaming.

use claude_agent_rust_sdk::{ContentBlock, Message, ThinkingConfig, ThinkingDisplay};
use serde_json::Value;

use crate::suite;

const THINKING_MODEL: &str = "claude-sonnet-4-5";

fn stream_events(messages: &[Message]) -> Vec<&Value> {
    messages
        .iter()
        .filter_map(|message| match message {
            Message::StreamEvent(event) => Some(&event.event),
            _ => None,
        })
        .collect()
}

#[tokio::test]
#[ignore = "runs the real CLI; run with `cargo xtask e2e`"]
async fn partial_messages_arrive_as_stream_events() {
    let options = suite::options()
        .include_partial_messages(true)
        .model(THINKING_MODEL)
        .max_turns(2)
        .env_var("MAX_THINKING_TOKENS", "8000")
        .build()
        .unwrap();

    let client = suite::connect(options).await;
    let messages = suite::turn(&client, "Think of three jokes, then tell one").await;
    client.disconnect().await.unwrap();

    let leading_system: Vec<&str> = messages
        .iter()
        .map_while(|message| match message {
            Message::System(system) => Some(system.subtype()),
            _ => None,
        })
        .collect();
    assert!(leading_system.contains(&"init"), "{leading_system:?}");

    let event_types: Vec<&str> = stream_events(&messages)
        .iter()
        .filter_map(|event| event["type"].as_str())
        .collect();
    for expected in [
        "message_start",
        "content_block_start",
        "content_block_delta",
        "content_block_stop",
        "message_stop",
    ] {
        assert!(
            event_types.contains(&expected),
            "no {expected} stream event"
        );
    }

    let blocks: Vec<&ContentBlock> = messages
        .iter()
        .filter_map(|message| match message {
            Message::Assistant(assistant) => Some(assistant.content.iter()),
            _ => None,
        })
        .flatten()
        .collect();
    assert!(
        blocks
            .iter()
            .any(|block| matches!(block, ContentBlock::Thinking(_))),
        "no thinking block in the assistant messages"
    );
    assert!(
        blocks
            .iter()
            .any(|block| matches!(block, ContentBlock::Text(_))),
        "no text block in the assistant messages"
    );
    match messages.last() {
        Some(Message::Result(result)) => assert_eq!(result.subtype, "success"),
        other => panic!("the response should end with a result, got {other:?}"),
    }
}

#[tokio::test]
#[ignore = "runs the real CLI; run with `cargo xtask e2e`"]
async fn thinking_is_streamed_through_deltas() {
    let options = suite::options()
        .include_partial_messages(true)
        .model(THINKING_MODEL)
        .max_turns(2)
        .thinking(ThinkingConfig::Enabled {
            budget_tokens: 8000,
            display: Some(ThinkingDisplay::Summarized),
        })
        .build()
        .unwrap();

    let client = suite::connect(options).await;
    let messages = suite::turn(&client, "Think step by step about what 2 + 2 equals").await;
    client.disconnect().await.unwrap();

    let thinking: String = stream_events(&messages)
        .iter()
        .filter(|event| event["type"] == "content_block_delta")
        .filter(|event| event["delta"]["type"] == "thinking_delta")
        .filter_map(|event| event["delta"]["thinking"].as_str())
        .collect();
    assert!(
        thinking.len() > 10,
        "thinking content too short: {thinking:?}"
    );
    assert!(thinking.contains('2'), "{thinking:?}");
}

#[tokio::test]
#[ignore = "runs the real CLI; run with `cargo xtask e2e`"]
async fn partial_messages_are_off_by_default() {
    let options = suite::options()
        .model(THINKING_MODEL)
        .max_turns(2)
        .build()
        .unwrap();

    let client = suite::connect(options).await;
    let messages = suite::turn(&client, "Say hello").await;
    client.disconnect().await.unwrap();

    assert_eq!(stream_events(&messages).len(), 0);
    assert!(
        messages
            .iter()
            .any(|message| matches!(message, Message::System(_)))
    );
    assert!(
        messages
            .iter()
            .any(|message| matches!(message, Message::Assistant(_)))
    );
    suite::result(&messages);
}
