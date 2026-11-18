//! The system prompt configurations of [`SystemPrompt`].
//!
//! The same question is asked under each configuration:
//!
//! - none: an empty system prompt, so the model answers as plain Claude;
//! - a string: a custom system prompt;
//! - the preset: the default system prompt of the CLI;
//! - the preset with text appended to it;
//! - the preset without its per-user dynamic sections, which keeps the
//!   prompt identical, and so cacheable, across users and directories;
//! - a custom prompt rebuilt on every request instead of recorded once;
//! - a prompt read from a file.
//!
//! Run it with `cargo run --example system_prompt`.

use claude_agent_rust_sdk::{
    BoxError, ClaudeAgentOptions, Error, Message, SystemPrompt, SystemPromptCustom,
    SystemPromptFile, SystemPromptPreset, query,
};
use futures_util::StreamExt;

/// Asks the question under the given options and prints the answer.
async fn ask(title: &str, options: ClaudeAgentOptions) -> Result<(), Error> {
    println!("=== {title} ===");

    let mut messages = query("What is 2 + 2?", options);
    while let Some(message) = messages.next().await {
        if let Message::Assistant(assistant) = message? {
            for block in &assistant.content {
                if let Some(text) = block.as_text() {
                    println!("Claude: {text}");
                }
            }
        }
    }
    println!();
    Ok(())
}

/// Builds options that differ from the defaults only by the system prompt.
fn with_system_prompt(prompt: impl Into<SystemPrompt>) -> Result<ClaudeAgentOptions, Error> {
    ClaudeAgentOptions::builder().system_prompt(prompt).build()
}

#[tokio::main]
async fn main() -> Result<(), BoxError> {
    ask(
        "No System Prompt (Vanilla Claude)",
        ClaudeAgentOptions::default(),
    )
    .await?;

    ask(
        "String System Prompt",
        with_system_prompt("You are a pirate assistant. Respond in pirate speak.")?,
    )
    .await?;

    ask(
        "Preset System Prompt (Default)",
        with_system_prompt(SystemPromptPreset::new())?,
    )
    .await?;

    ask(
        "Preset System Prompt with Append",
        with_system_prompt(
            SystemPromptPreset::new().append("Always end your response with a fun fact."),
        )?,
    )
    .await?;

    ask(
        "Preset System Prompt without Dynamic Sections",
        with_system_prompt(SystemPromptPreset::new().exclude_dynamic_sections(true))?,
    )
    .await?;

    ask(
        "Custom System Prompt Rebuilt on Every Request",
        with_system_prompt(
            SystemPromptCustom::new("You are a poet. Respond in a rhyming couplet.")
                .snapshot(false),
        )?,
    )
    .await?;

    let directory = tempfile::tempdir()?;
    let prompt_file = directory.path().join("system_prompt.txt");
    std::fs::write(
        &prompt_file,
        "You are a math teacher. Explain each step of your answer.",
    )?;
    ask(
        "System Prompt Read from a File",
        with_system_prompt(SystemPromptFile::new(prompt_file.to_string_lossy()))?,
    )
    .await?;

    Ok(())
}
