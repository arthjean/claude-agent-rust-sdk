//! Loading agents defined in files through `setting_sources`.
//!
//! Besides inline [`AgentDefinition`](claude_agent_rust_sdk::AgentDefinition)s,
//! the CLI loads agents from Markdown files under `.claude/agents/` of the
//! project when the project settings are a setting source. The example
//! writes `test-agent.md` into a temporary project, starts a session there
//! with `setting_sources = [project]`, and checks that the init message
//! lists the agent and that the session answers in full.
//!
//! Run it with `cargo run --example filesystem_agents`.

use claude_agent_rust_sdk::{
    BoxError, ClaudeAgentOptions, ClaudeSdkClient, Message, SettingSource,
};
use futures_util::StreamExt;

const AGENT: &str = "test-agent";

const AGENT_FILE: &str = "---
name: test-agent
description: A simple test agent for SDK testing
tools: Read
---

# Test Agent

You are a simple test agent. When asked a question, provide a brief, helpful answer.
";

#[tokio::main]
async fn main() -> Result<(), BoxError> {
    println!("=== Filesystem Agents Example ===");
    println!("Testing: setting_sources = [project] with .claude/agents/{AGENT}.md\n");

    let project = tempfile::tempdir()?;
    let agents = project.path().join(".claude").join("agents");
    std::fs::create_dir_all(&agents)?;
    std::fs::write(agents.join(format!("{AGENT}.md")), AGENT_FILE)?;

    let options = ClaudeAgentOptions::builder()
        .setting_sources([SettingSource::Project])
        .cwd(project.path())
        .build()?;
    let client = ClaudeSdkClient::new(options);
    client.connect().await?;
    client.query("Say hello in exactly 3 words").await?;

    let mut kinds = Vec::new();
    let mut agents_found = Vec::new();
    {
        let mut messages = client.receive_response();
        while let Some(message) = messages.next().await {
            let message = message?;
            match &message {
                Message::System(system) => {
                    kinds.push("SystemMessage");
                    if let Some(init) = system.init_data() {
                        agents_found = init.agents.unwrap_or_default();
                        println!("Init message received. Agents loaded: {agents_found:?}");
                    }
                }
                Message::Assistant(assistant) => {
                    kinds.push("AssistantMessage");
                    for text in assistant.content.iter().filter_map(|block| block.as_text()) {
                        println!("Assistant: {text}");
                    }
                }
                Message::Result(result) => {
                    kinds.push("ResultMessage");
                    println!(
                        "Result: subtype={}, cost=${:.4}",
                        result.subtype,
                        result.total_cost_usd.unwrap_or_default()
                    );
                }
                _ => kinds.push("other"),
            }
        }
    }
    client.disconnect().await?;

    println!("\n=== Summary ===");
    println!("Message types received: {kinds:?}");
    println!("Total messages: {}\n", kinds.len());
    let full = ["SystemMessage", "AssistantMessage", "ResultMessage"]
        .iter()
        .all(|kind| kinds.contains(kind));
    if full {
        println!("SUCCESS: Received full response (init, assistant, result)");
    } else {
        println!("FAILURE: Did not receive full response");
    }
    if agents_found.iter().any(|name| name == AGENT) {
        println!("SUCCESS: {AGENT} was loaded from the filesystem");
    } else {
        println!("WARNING: {AGENT} was NOT loaded");
    }
    Ok(())
}
