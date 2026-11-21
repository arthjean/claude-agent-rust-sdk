//! Choosing the base set of built-in tools with the `tools` option.
//!
//! [`Tools::list`] names the tools the session has, an empty list
//! disables every built-in tool, and [`ToolsPreset::ClaudeCode`] keeps the
//! default set of the CLI. The init message of the session lists the tools
//! the CLI made available, which the example prints next to the answer.
//!
//! Scenarios: `array`, `empty`, `preset`. Run one with
//! `cargo run --example tools_option -- array`, every one with `-- all`,
//! or pass nothing to list them.

use std::process::ExitCode;

use claude_agent_rust_sdk::{BoxError, ClaudeAgentOptions, Message, Tools, ToolsPreset, query};
use futures_util::StreamExt;

/// Name and description of each scenario, in the order `all` runs them.
const SCENARIOS: [(&str, &str); 3] = [
    ("array", "tools = [Read, Glob, Grep]"),
    ("empty", "tools = [] (disables every built-in tool)"),
    ("preset", "tools = the claude_code preset"),
];

const PROMPT: &str = "What tools do you have available? Just list them briefly.";

/// Runs one query and prints the tools of the init message, the answer,
/// and the cost.
async fn run(title: &str, tools: Tools) -> Result<(), BoxError> {
    println!("=== {title} ===\n");
    let options = ClaudeAgentOptions::builder()
        .tools(tools)
        .max_turns(1)
        .build()?;
    let mut messages = query(PROMPT, options);
    while let Some(message) = messages.next().await {
        match message? {
            Message::System(system) => {
                if let Some(init) = system.init_data() {
                    let shown: Vec<&String> = init.tools.iter().take(5).collect();
                    let more = if init.tools.len() > shown.len() {
                        ", ..."
                    } else {
                        ""
                    };
                    println!(
                        "Tools from the init message ({} tools): {shown:?}{more}\n",
                        init.tools.len()
                    );
                }
            }
            Message::Assistant(assistant) => {
                for text in assistant.content.iter().filter_map(|block| block.as_text()) {
                    println!("Claude: {text}");
                }
            }
            Message::Result(result) => {
                if let Some(cost) = result.total_cost_usd {
                    println!("\nCost: ${cost:.4}");
                }
            }
            _ => {}
        }
    }
    println!();
    Ok(())
}

/// Runs the scenario of that name. Returns false for an unknown name.
async fn scenario(name: &str) -> Result<bool, BoxError> {
    match name {
        "array" => run("Tools Array Example", Tools::list(["Read", "Glob", "Grep"])).await?,
        "empty" => {
            run(
                "Tools Empty Array Example",
                Tools::list(Vec::<String>::new()),
            )
            .await?
        }
        "preset" => run("Tools Preset Example", ToolsPreset::ClaudeCode.into()).await?,
        _ => return Ok(false),
    }
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
async fn main() -> Result<ExitCode, BoxError> {
    let Some(name) = std::env::args().nth(1) else {
        println!("Usage: cargo run --example tools_option -- <scenario>\n");
        print_scenarios();
        return Ok(ExitCode::SUCCESS);
    };
    if name == "all" {
        for (name, _) in SCENARIOS {
            scenario(name).await?;
        }
    } else if !scenario(&name).await? {
        eprintln!("Error: Unknown scenario '{name}'\n");
        print_scenarios();
        return Ok(ExitCode::FAILURE);
    }
    Ok(ExitCode::SUCCESS)
}
