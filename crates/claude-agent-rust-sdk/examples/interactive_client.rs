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
//! - `concurrent`: read the responses while new prompts are sent.
//! - `manual`: process the message stream with custom logic.
//! - `with_options`: configure the client with options.
//! - `bash_command`: show the tool calls and tool results of a turn.
//! - `control_protocol`: read the server info, then interrupt a turn.
//! - `error_handling`: give up on a slow response with a timeout.
//!
//! Run one with `cargo run --example interactive_client -- basic`, every
//! one with `-- all`, or pass nothing to list them.

use std::process::ExitCode;
use std::time::Duration;

use claude_agent_rust_sdk::{
    ClaudeAgentOptions, ClaudeSdkClient, ContentBlock, Error, InputMessage, Message, Prompt,
};
use futures_util::{StreamExt, stream};

/// Name and description of each scenario, in the order `all` runs them.
const SCENARIOS: [(&str, &str); 10] = [
    ("basic", "One prompt, one response"),
    ("multi_turn", "A follow-up that relies on the first answer"),
    ("interrupt", "Stop a long turn, then send a new prompt"),
    ("prompt_stream", "Send a stream of input messages"),
    ("concurrent", "Read responses while sending new prompts"),
    ("manual", "Process the message stream with custom logic"),
    ("with_options", "Configure the client with options"),
    ("bash_command", "Show the tool calls and results of a turn"),
    (
        "control_protocol",
        "Read the server info, then interrupt a turn",
    ),
    (
        "error_handling",
        "Give up on a slow response with a timeout",
    ),
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
        Message::Result(result) => {
            println!("Result ended");
            if let Some(cost) = result.total_cost_usd {
                println!("Cost: ${cost:.4}");
            }
        }
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

/// Reads every message in the background while three prompts are sent a
/// few seconds apart; the reading stops two seconds after the last one.
async fn concurrent() -> Result<(), Error> {
    println!("=== Concurrent Send/Receive Example ===");

    let client = connect().await?;
    let receive = async {
        let mut messages = client.receive_messages();
        while let Some(message) = messages.next().await {
            display(&message?);
        }
        Ok::<(), Error>(())
    };
    let send = async {
        for question in [
            "What is 2 + 2?",
            "What is the square root of 144?",
            "What is 10% of 80?",
        ] {
            println!("\nUser: {question}");
            client.query(question).await?;
            tokio::time::sleep(Duration::from_secs(3)).await;
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
        Ok::<(), Error>(())
    };
    tokio::select! {
        received = receive => received?,
        sent = send => sent?,
    }
    client.disconnect().await
}

async fn manual() -> Result<(), Error> {
    println!("=== Manual Message Handling Example ===");

    let client = connect().await?;
    client
        .query("List 5 programming languages and their main use cases")
        .await?;
    let mut found: Vec<&str> = Vec::new();
    {
        let mut messages = client.receive_messages();
        while let Some(message) = messages.next().await {
            match message? {
                Message::Assistant(assistant) => {
                    for text in assistant.content.iter().filter_map(ContentBlock::as_text) {
                        println!("Claude: {text}");
                        for language in
                            ["Python", "JavaScript", "Java", "C++", "Go", "Rust", "Ruby"]
                        {
                            if text.contains(language) && !found.contains(&language) {
                                found.push(language);
                                println!("Found language: {language}");
                            }
                        }
                    }
                }
                result @ Message::Result(_) => {
                    display(&result);
                    println!("Total languages mentioned: {}", found.len());
                    break;
                }
                _ => {}
            }
        }
    }
    client.disconnect().await
}

async fn with_options() -> Result<(), Error> {
    println!("=== Custom Options Example ===");

    let options = ClaudeAgentOptions::builder()
        .allowed_tools(["Read", "Write"])
        .system_prompt("You are a helpful coding assistant.")
        .build()?;
    let client = ClaudeSdkClient::new(options);
    client.connect().await?;
    println!("User: Create a simple hello.txt file with a greeting message");
    client
        .query("Create a simple hello.txt file with a greeting message")
        .await?;
    let mut tools = Vec::new();
    {
        let mut messages = client.receive_response();
        while let Some(message) = messages.next().await {
            let message = message?;
            if let Message::Assistant(assistant) = &message {
                for block in &assistant.content {
                    if let ContentBlock::ToolUse(tool) = block {
                        tools.push(tool.name.clone());
                    }
                }
            }
            display(&message);
        }
    }
    if !tools.is_empty() {
        println!("Tools used: {}", tools.join(", "));
    }
    client.disconnect().await
}

async fn bash_command() -> Result<(), Error> {
    println!("=== Bash Command Example ===");

    let client = connect().await?;
    println!("User: Run a bash echo command");
    client
        .query("Run a bash echo command that says 'Hello from bash!'")
        .await?;
    let mut kinds = std::collections::BTreeSet::new();
    {
        let mut messages = client.receive_messages();
        while let Some(message) = messages.next().await {
            match message? {
                Message::User(user) => {
                    kinds.insert("UserMessage");
                    for block in user.content.as_blocks().unwrap_or_default() {
                        if let ContentBlock::ToolResult(result) = block {
                            let text = result
                                .content
                                .as_ref()
                                .and_then(|content| content.as_text())
                                .unwrap_or("None");
                            let shown: String = text.chars().take(100).collect();
                            println!("Tool Result (id: {}): {shown}...", result.tool_use_id);
                        }
                    }
                }
                Message::Assistant(assistant) => {
                    kinds.insert("AssistantMessage");
                    for block in &assistant.content {
                        match block {
                            ContentBlock::Text(text) => println!("Claude: {}", text.text),
                            ContentBlock::ToolUse(tool) => {
                                println!("Tool Use: {} (id: {})", tool.name, tool.id);
                                if tool.name == "Bash" {
                                    println!("  Command: {}", tool.input["command"]);
                                }
                            }
                            _ => {}
                        }
                    }
                }
                result @ Message::Result(_) => {
                    kinds.insert("ResultMessage");
                    display(&result);
                    break;
                }
                _ => {
                    kinds.insert("other");
                }
            }
        }
    }
    let kinds: Vec<&str> = kinds.into_iter().collect();
    println!("\nMessage types received: {}", kinds.join(", "));
    client.disconnect().await
}

async fn control_protocol() -> Result<(), Error> {
    println!("=== Control Protocol Example ===");
    println!("Shows server info retrieval and interrupt capability\n");

    let client = connect().await?;
    println!("1. Getting server info...");
    match client.server_info()? {
        Some(info) => {
            let commands = info["commands"].as_array().cloned().unwrap_or_default();
            println!("Server info retrieved successfully!");
            println!("  - Available commands: {}", commands.len());
            println!(
                "  - Output style: {}",
                info["output_style"].as_str().unwrap_or("unknown")
            );
            for command in commands.iter().take(5) {
                println!("    * {}", command["name"].as_str().unwrap_or("unknown"));
            }
        }
        None => println!("No server info available"),
    }

    println!("\n2. Testing interrupt capability...");
    println!("User: Count from 1 to 20 slowly");
    client
        .query("Count from 1 to 20 slowly, pausing between each number")
        .await?;
    let consume = async {
        let mut messages = client.receive_response();
        while let Some(message) = messages.next().await {
            if let Message::Assistant(assistant) = message?
                && let Some(text) = assistant.content.iter().find_map(ContentBlock::as_text)
            {
                let shown: String = text.chars().take(50).collect();
                println!("Claude: {shown}...");
            }
        }
        Ok::<(), Error>(())
    };
    let stop = async {
        tokio::time::sleep(Duration::from_secs(2)).await;
        println!("\n[Sending interrupt after 2 seconds...]");
        match client.interrupt().await {
            Ok(()) => println!("Interrupt sent successfully"),
            Err(error) => println!("Interrupt failed: {error}"),
        }
    };
    let (consumed, ()) = tokio::join!(consume, stop);
    consumed?;

    println!("\nUser: Just say 'Hello!'");
    client.query("Just say 'Hello!'").await?;
    print_response(&client).await?;
    client.disconnect().await
}

/// A response that takes longer than the caller will wait is abandoned
/// with a timeout; the client is disconnected whatever happened.
async fn error_handling() -> Result<(), Error> {
    println!("=== Error Handling Example ===");

    let client = ClaudeSdkClient::new(ClaudeAgentOptions::default());
    let outcome = async {
        client.connect().await?;
        println!("User: Run a bash sleep command for 5 seconds not in the background");
        client
            .query("Run a bash sleep command for 5 seconds not in the background")
            .await?;
        let mut received = 0;
        let read = async {
            let mut messages = client.receive_response();
            while let Some(message) = messages.next().await {
                let message = message?;
                received += 1;
                display(&message);
            }
            Ok::<(), Error>(())
        };
        if tokio::time::timeout(Duration::from_secs(3), read)
            .await
            .is_err()
        {
            println!("\nResponse timeout after 3 seconds - demonstrating graceful handling");
            println!("Received {received} messages before timeout");
        }
        Ok::<(), Error>(())
    }
    .await;
    if let Err(error) = &outcome {
        if error.is_cli_connection() {
            println!("Connection error: {error}");
        } else {
            println!("Unexpected error: {error}");
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
        "concurrent" => concurrent().await?,
        "manual" => manual().await?,
        "with_options" => with_options().await?,
        "bash_command" => bash_command().await?,
        "control_protocol" => control_protocol().await?,
        "error_handling" => error_handling().await?,
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
