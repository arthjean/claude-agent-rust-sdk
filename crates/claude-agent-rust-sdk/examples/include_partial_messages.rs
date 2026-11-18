//! Partial messages: the response as it is being generated.
//!
//! With `include_partial_messages`, the session yields a
//! [`Message::StreamEvent`] for each incremental update, between the
//! regular messages. Each event carries one raw streaming event of the
//! API: the start of a message or of a content block, a delta of text or
//! thinking, a stop. This is what a user interface needs to show text
//! while it is written, or to follow a tool call before it completes.
//!
//! The example prints the deltas as they arrive, then the complete
//! messages.
//!
//! Run it with `cargo run --example include_partial_messages`.

use std::io::Write;

use claude_agent_rust_sdk::{
    BoxError, ClaudeAgentOptions, ClaudeSdkClient, Message, ThinkingConfig, ThinkingDisplay,
};
use futures_util::StreamExt;
use serde_json::Value;

/// Prints one streaming event: block boundaries on their own line, deltas
/// inline as they arrive.
fn display_event(event: &Value) -> std::io::Result<()> {
    let mut output = std::io::stdout().lock();
    match event["type"].as_str() {
        Some("content_block_start") => {
            let kind = event["content_block"]["type"].as_str().unwrap_or("unknown");
            writeln!(output, "\n[{kind} block]")?;
        }
        Some("content_block_delta") => {
            let delta = &event["delta"];
            let fragment = ["text", "thinking", "partial_json"]
                .iter()
                .find_map(|key| delta[*key].as_str());
            if let Some(fragment) = fragment {
                write!(output, "{fragment}")?;
            }
        }
        Some("content_block_stop") => writeln!(output)?,
        Some(other) => writeln!(output, "[{other}]")?,
        None => {}
    }
    output.flush()
}

#[tokio::main]
async fn main() -> Result<(), BoxError> {
    println!("Partial Message Streaming Example");
    println!("{}", "=".repeat(50));

    let options = ClaudeAgentOptions::builder()
        .include_partial_messages(true)
        .thinking(ThinkingConfig::Enabled {
            budget_tokens: 8000,
            display: Some(ThinkingDisplay::Summarized),
        })
        .max_turns(2)
        .build()?;

    let client = ClaudeSdkClient::new(options);
    client.connect().await?;

    let prompt = "Think of three jokes, then tell one";
    println!("Prompt: {prompt}\n");
    println!("{}", "=".repeat(50));
    client.query(prompt).await?;

    {
        let mut messages = client.receive_response();
        while let Some(message) = messages.next().await {
            match message? {
                Message::StreamEvent(partial) => display_event(&partial.event)?,
                Message::Assistant(assistant) => {
                    println!(
                        "\nComplete assistant message: {} content block(s)",
                        assistant.content.len()
                    );
                }
                Message::Result(result) => println!("\nResult: {}", result.subtype),
                _ => {}
            }
        }
    }
    client.disconnect().await?;
    Ok(())
}
