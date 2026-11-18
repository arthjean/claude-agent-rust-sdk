//! Custom subagents defined in the options.
//!
//! An [`AgentDefinition`] gives a subagent its description, its system
//! prompt, the tools it may use, and optionally its model. Claude hands a
//! task to a subagent when the prompt asks for it or the description
//! fits.
//!
//! The session runs in the directory of this crate, so the prompts can
//! name its files.
//!
//! Run it with `cargo run --example agents`.

use claude_agent_rust_sdk::{
    AgentDefinition, ClaudeAgentOptions, Error, Message, SettingSource, query,
};
use futures_util::StreamExt;

/// Directory of this crate, used as the working directory of the session.
const CRATE_DIRECTORY: &str = env!("CARGO_MANIFEST_DIR");

/// Runs one query and prints the text of Claude and the cost.
async fn run(prompt: &str, options: ClaudeAgentOptions) -> Result<(), Error> {
    let mut messages = query(prompt, options);
    while let Some(message) = messages.next().await {
        match message? {
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
    println!();
    Ok(())
}

/// A read-only agent that reviews code.
async fn code_reviewer_example() -> Result<(), Error> {
    println!("=== Code Reviewer Agent Example ===");

    let options = ClaudeAgentOptions::builder()
        .cwd(CRATE_DIRECTORY)
        .agent(
            "code-reviewer",
            AgentDefinition::new(
                "Reviews code for best practices and potential issues",
                "You are a code reviewer. Analyze code for bugs, performance issues, \
                 security vulnerabilities, and adherence to best practices. \
                 Provide constructive feedback.",
            )
            .tools(["Read", "Grep"])
            .model("sonnet"),
        )
        .build()?;

    run(
        "Use the code-reviewer agent to review the code in src/types/content.rs",
        options,
    )
    .await
}

/// An agent that writes documentation.
async fn documentation_writer_example() -> Result<(), Error> {
    println!("=== Documentation Writer Agent Example ===");

    let options = ClaudeAgentOptions::builder()
        .cwd(CRATE_DIRECTORY)
        .agent(
            "doc-writer",
            AgentDefinition::new(
                "Writes comprehensive documentation",
                "You are a technical documentation expert. Write clear, comprehensive \
                 documentation with examples. Focus on clarity and completeness.",
            )
            .tools(["Read", "Write", "Edit"])
            .model("sonnet"),
        )
        .build()?;

    run(
        "Use the doc-writer agent to explain what AgentDefinition is used for",
        options,
    )
    .await
}

/// Several agents in one session, next to the agents of the user and
/// project settings.
async fn multiple_agents_example() -> Result<(), Error> {
    println!("=== Multiple Agents Example ===");

    let options = ClaudeAgentOptions::builder()
        .cwd(CRATE_DIRECTORY)
        .agents([
            (
                "analyzer",
                AgentDefinition::new(
                    "Analyzes code structure and patterns",
                    "You are a code analyzer. Examine code structure, patterns, and architecture.",
                )
                .tools(["Read", "Grep", "Glob"]),
            ),
            (
                "tester",
                AgentDefinition::new(
                    "Creates and runs tests",
                    "You are a testing expert. Write comprehensive tests and ensure code quality.",
                )
                .tools(["Read", "Write", "Bash"])
                .model("sonnet"),
            ),
        ])
        .setting_sources([SettingSource::User, SettingSource::Project])
        .build()?;

    run(
        "Use the analyzer agent to find all Rust files in the examples/ directory",
        options,
    )
    .await
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    code_reviewer_example().await?;
    documentation_writer_example().await?;
    multiple_agents_example().await?;
    Ok(())
}
