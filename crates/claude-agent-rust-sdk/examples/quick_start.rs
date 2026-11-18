//! Quick start: one-shot queries with [`query`].
//!
//! Three queries run in turn: a bare question, a question with custom
//! options, and a task that uses tools. The task runs in a temporary
//! directory, so the file Claude creates does not land in your project.
//!
//! Run it with `cargo run --example quick_start`.

use claude_agent_rust_sdk::{BoxError, ClaudeAgentOptions, Error, Message, query};
use futures_util::StreamExt;

/// Prints the text of an assistant message, and the cost of a result.
fn display(message: &Message) {
    match message {
        Message::Assistant(assistant) => {
            for block in &assistant.content {
                if let Some(text) = block.as_text() {
                    println!("Claude: {text}");
                }
            }
        }
        Message::Result(result) => {
            if let Some(cost) = result.total_cost_usd
                && cost > 0.0
            {
                println!("\nCost: ${cost:.4}");
            }
        }
        _ => {}
    }
}

/// A simple question with the default options.
async fn basic_example() -> Result<(), Error> {
    println!("=== Basic Example ===");

    let mut messages = query("What is 2 + 2?", ClaudeAgentOptions::default());
    while let Some(message) = messages.next().await {
        display(&message?);
    }
    println!();
    Ok(())
}

/// A question with a system prompt and a turn limit.
async fn with_options_example() -> Result<(), Error> {
    println!("=== With Options Example ===");

    let options = ClaudeAgentOptions::builder()
        .system_prompt("You are a helpful assistant that explains things simply.")
        .max_turns(1)
        .build()?;

    let mut messages = query("Explain what Rust is in one sentence.", options);
    while let Some(message) = messages.next().await {
        display(&message?);
    }
    println!();
    Ok(())
}

/// A task that needs tools: the listed tools run without a permission
/// prompt.
async fn with_tools_example() -> Result<(), BoxError> {
    println!("=== With Tools Example ===");

    let workspace = tempfile::tempdir()?;
    let options = ClaudeAgentOptions::builder()
        .allowed_tools(["Read", "Write"])
        .system_prompt("You are a helpful file assistant.")
        .cwd(workspace.path())
        .build()?;

    let mut messages = query(
        "Create a file called hello.txt with 'Hello, World!' in it",
        options,
    );
    while let Some(message) = messages.next().await {
        display(&message?);
    }

    match std::fs::read_to_string(workspace.path().join("hello.txt")) {
        Ok(content) => println!("hello.txt contains: {}", content.trim_end()),
        Err(error) => println!("hello.txt was not created: {error}"),
    }
    println!();
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), BoxError> {
    basic_example().await?;
    with_options_example().await?;
    with_tools_example().await?;
    Ok(())
}
