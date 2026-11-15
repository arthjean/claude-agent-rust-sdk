//! Multi-turn conversations with [`ClaudeSdkClient`].
//!
//! The client keeps one session open, so follow-ups share the context of
//! what came before, and a turn can be interrupted while it runs.
//!
//! Scenarios:
//!
//! - `basic`: one prompt, one response.
//! - `multi_turn`: a follow-up that relies on the first answer.
//! - `interrupt`: stop a long turn, then send a new prompt.
//! - `prompt_stream`: send a stream of input messages as the prompt.
//!
//! Run one with `cargo run --example interactive_client -- basic`, every
//! one with `-- all`, or pass nothing to list them.

use std::process::ExitCode;
use std::time::Duration;

use claude_agent_kit::{
    ClaudeAgentOptions, ClaudeSdkClient, ContentBlock, Error, InputMessage, Message, Prompt,
};
use futures_util::{StreamExt, stream};

/// Name and description of each scenario, in the order `all` runs them.
const SCENARIOS: [(&str, &str); 4] = [
    ("basic", "One prompt, one response"),
    ("multi_turn", "A follow-up that relies on the first answer"),
    ("interrupt", "Stop a long turn, then send a new prompt"),
    ("prompt_stream", "Send a stream of input messages"),
];

/// Prints a message the same way in every scenario.
fn display(message: &Message) {
    match message {
        Message::User(user) => {
            if let Some(text) = user.content.as_text() {
                println!("User: {text}");
            }
            for block in user.content.as_blocks().unwrap_or_default() {
                if let ContentBlock::Text(text) = block {
                    println!("User: {}", text.text);
                }
            }
        }
        Message::Assistant(assistant) => {
            for block in &assistant.content {
                if let Some(text) = block.as_text() {
                    println!("Claude: {text}");
                }
            }
        }
        Message::Result(_) => println!("Result ended"),
        _ => {}
    }
}

/// Prints the messages up to and including the next result.
async fn print_response(client: &ClaudeSdkClient) -> Result<(), Error> {
    let mut messages = client.receive_response();
    while let Some(message) = messages.next().await {
        display(&message?);
    }
    Ok(())
}

/// Connects a client with the default options.
async fn connect() -> Result<ClaudeSdkClient, Error> {
    let client = ClaudeSdkClient::new(ClaudeAgentOptions::default());
    client.connect().await?;
    Ok(client)
}

async fn basic() -> Result<(), Error> {
    println!("=== Basic Example ===");

    let client = connect().await?;
    println!("User: What is 2+2?");
    client.query("What is 2+2?").await?;
    print_response(&client).await?;
    client.disconnect().await
}

async fn multi_turn() -> Result<(), Error> {
    println!("=== Multi-Turn Conversation Example ===");

    let client = connect().await?;
    println!("User: What's the capital of France?");
    client.query("What's the capital of France?").await?;
    print_response(&client).await?;

    println!("\nUser: What's the population of that city?");
    client.query("What's the population of that city?").await?;
    print_response(&client).await?;
    client.disconnect().await
}

/// Every method of the client takes `&self`, so the response is read
/// while the interrupt is sent, with no extra task.
async fn interrupt() -> Result<(), Error> {
    println!("=== Interrupt Example ===");

    let client = connect().await?;
    println!("User: Count from 1 to 100 slowly");
    client
        .query("Count from 1 to 100 slowly, with a brief pause between each number")
        .await?;

    let stop = async {
        tokio::time::sleep(Duration::from_secs(2)).await;
        println!("\n[After 2 seconds, sending interrupt...]");
        client.interrupt().await
    };
    let (response, stopped) = tokio::join!(print_response(&client), stop);
    response?;
    stopped?;

    println!("\nUser: Never mind, just tell me a quick joke");
    client
        .query("Never mind, just tell me a quick joke")
        .await?;
    print_response(&client).await?;
    client.disconnect().await
}

/// With a stream as the first prompt, the client writes the messages in
/// the background and closes the input once the run of the last one has
/// ended. The session then ends by itself, and so does the stream of its
/// messages. The CLI may answer several messages in one turn, so the
/// number of results is not the number of questions.
async fn prompt_stream() -> Result<(), Error> {
    println!("=== Prompt Stream Example ===");

    let questions = stream::iter([
        "Hello! I have multiple questions.",
        "First, what's the capital of Japan?",
        "Second, what's 15% of 200?",
    ])
    .map(|question| {
        println!("User: {question}");
        InputMessage::user(question).session_id("qa-session")
    });

    let client = ClaudeSdkClient::new(ClaudeAgentOptions::default());
    client
        .connect_with_prompt(Prompt::stream(questions))
        .await?;
    {
        let mut messages = client.receive_messages();
        while let Some(message) = messages.next().await {
            display(&message?);
        }
    }
    client.disconnect().await
}

/// Runs the scenario of that name. Returns false for an unknown name.
async fn run(name: &str) -> Result<bool, Error> {
    match name {
        "basic" => basic().await?,
        "multi_turn" => multi_turn().await?,
        "interrupt" => interrupt().await?,
        "prompt_stream" => prompt_stream().await?,
        _ => return Ok(false),
    }
    println!();
    Ok(true)
}

fn print_scenarios() {
    println!("Available scenarios:");
    println!("  all - Run every scenario");
    for (name, description) in SCENARIOS {
        println!("  {name} - {description}");
    }
}

#[tokio::main]
async fn main() -> Result<ExitCode, Error> {
    let Some(name) = std::env::args().nth(1) else {
        println!("Usage: cargo run --example interactive_client -- <scenario>\n");
        print_scenarios();
        return Ok(ExitCode::SUCCESS);
    };

    if name == "all" {
        for (scenario, _) in SCENARIOS {
            run(scenario).await?;
            println!("{}\n", "-".repeat(50));
        }
    } else if !run(&name).await? {
        eprintln!("Error: Unknown scenario '{name}'\n");
        print_scenarios();
        return Ok(ExitCode::FAILURE);
    }
    Ok(ExitCode::SUCCESS)
}
