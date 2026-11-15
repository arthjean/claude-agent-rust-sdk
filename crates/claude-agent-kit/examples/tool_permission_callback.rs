//! A tool-permission callback that allows, denies, and rewrites tool calls.
//!
//! The callback replaces the interactive permission prompt of the CLI: it
//! runs for every tool call that would otherwise ask the user. This one
//!
//! 1. allows read-only tools,
//! 2. denies writes to system directories and redirects other writes to a
//!    `safe_output` directory by changing the input of the call,
//! 3. denies dangerous shell commands,
//! 4. asks you on the terminal about any other tool,
//!
//! and logs every request. The session runs in a temporary directory,
//! removed when the example ends.
//!
//! Run it with `cargo run --example tool_permission_callback`.

use std::io::Write;
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};

use claude_agent_kit::{
    BoxError, CanUseTool, ClaudeAgentOptions, ClaudeSdkClient, Message, PermissionMode,
    PermissionResult, PermissionUpdate, ToolPermissionContext,
};
use futures_util::StreamExt;
use serde_json::Value;

/// Tools that only read.
const READ_ONLY_TOOLS: [&str; 3] = ["Read", "Glob", "Grep"];

/// Tools that write a file at `file_path`.
const WRITE_TOOLS: [&str; 3] = ["Write", "Edit", "MultiEdit"];

/// Names of the shell tool: `Bash`, or `PowerShell` on Windows.
const SHELL_TOOLS: [&str; 2] = ["Bash", "PowerShell"];

/// Command fragments that are never allowed.
const DANGEROUS_COMMANDS: [&str; 5] = ["rm -rf", "sudo", "chmod 777", "dd if=", "mkfs"];

/// One permission request, kept for the summary.
struct ToolRequest {
    tool: String,
    input: Value,
    suggestions: Vec<PermissionUpdate>,
}

type UsageLog = Arc<Mutex<Vec<ToolRequest>>>;

/// Asks a yes or no question on the terminal, off the async runtime.
async fn confirm(question: &'static str) -> Result<bool, BoxError> {
    let answer = tokio::task::spawn_blocking(move || {
        print!("{question}");
        std::io::stdout().flush()?;
        let mut answer = String::new();
        std::io::stdin().read_line(&mut answer)?;
        Ok::<String, std::io::Error>(answer)
    })
    .await??;
    Ok(matches!(answer.trim().to_lowercase().as_str(), "y" | "yes"))
}

/// Decides one tool call from the name of the tool and its input.
async fn decide(
    log: UsageLog,
    tool_name: String,
    input: Value,
    context: ToolPermissionContext,
) -> Result<PermissionResult, BoxError> {
    log.lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push(ToolRequest {
            tool: tool_name.clone(),
            input: input.clone(),
            suggestions: context.suggestions,
        });

    println!("\nTool permission request: {tool_name}");
    println!("   Input: {}", serde_json::to_string_pretty(&input)?);

    if READ_ONLY_TOOLS.contains(&tool_name.as_str()) {
        println!("   Allowing {tool_name} (read-only operation)");
        return Ok(PermissionResult::allow());
    }

    if WRITE_TOOLS.contains(&tool_name.as_str()) {
        let file_path = input["file_path"].as_str().unwrap_or_default();
        if file_path.starts_with("/etc/") || file_path.starts_with("/usr/") {
            println!("   Denying write to system directory: {file_path}");
            return Ok(PermissionResult::deny(format!(
                "Cannot write to system directory: {file_path}"
            )));
        }

        if !file_path.starts_with("/tmp/") && !file_path.starts_with("./") {
            let file_name = Path::new(file_path)
                .file_name()
                .map(|name| name.to_string_lossy())
                .unwrap_or_default();
            let safe_path = format!("./safe_output/{file_name}");
            println!("   Redirecting write from {file_path} to {safe_path}");
            let mut updated_input = input.clone();
            if let Some(fields) = updated_input.as_object_mut() {
                fields.insert("file_path".to_owned(), Value::from(safe_path));
            }
            return Ok(PermissionResult::allow_with_input(updated_input));
        }
        return Ok(PermissionResult::allow());
    }

    if SHELL_TOOLS.contains(&tool_name.as_str()) {
        let command = input["command"].as_str().unwrap_or_default();
        for dangerous in DANGEROUS_COMMANDS {
            if command.contains(dangerous) {
                println!("   Denying dangerous command: {command}");
                return Ok(PermissionResult::deny(format!(
                    "Dangerous command pattern detected: {dangerous}"
                )));
            }
        }
        println!("   Allowing shell command: {command}");
        return Ok(PermissionResult::allow());
    }

    println!("   Unknown tool: {tool_name}");
    if confirm("   Allow this tool? (y/N): ").await? {
        Ok(PermissionResult::allow())
    } else {
        Ok(PermissionResult::deny("User denied permission"))
    }
}

#[tokio::main]
async fn main() -> Result<(), BoxError> {
    println!("{}", "=".repeat(60));
    println!("Tool Permission Callback Example");
    println!("{}", "=".repeat(60));
    println!("\nThis example demonstrates how to:");
    println!("1. Allow/deny tools based on type");
    println!("2. Modify tool inputs for safety");
    println!("3. Log tool usage");
    println!("4. Prompt for unknown tools");
    println!("{}", "=".repeat(60));

    let log = UsageLog::default();
    let callback = {
        let log = Arc::clone(&log);
        CanUseTool::new(move |tool_name, input, context| {
            decide(Arc::clone(&log), tool_name, input, context)
        })
    };

    let workspace = tempfile::tempdir()?;
    let options = ClaudeAgentOptions::builder()
        .can_use_tool(callback)
        .permission_mode(PermissionMode::Default)
        .cwd(workspace.path())
        .build()?;

    let client = ClaudeSdkClient::new(options);
    client.connect().await?;

    println!("\nSending query to Claude...");
    client
        .query(
            "Please do the following:\n\
             1. List the files in the current directory\n\
             2. Create a simple Python hello world script at hello.py\n\
             3. Run the script to test it",
        )
        .await?;

    println!("\nReceiving response...");
    let mut message_count = 0;
    {
        let mut messages = client.receive_response();
        while let Some(message) = messages.next().await {
            message_count += 1;
            match message? {
                Message::Assistant(assistant) => {
                    for block in &assistant.content {
                        if let Some(text) = block.as_text() {
                            println!("\nClaude: {text}");
                        }
                    }
                }
                Message::Result(result) => {
                    println!("\nTask completed!");
                    println!("   Duration: {}ms", result.duration_ms);
                    if let Some(cost) = result.total_cost_usd {
                        println!("   Cost: ${cost:.4}");
                    }
                    println!("   Messages processed: {message_count}");
                }
                _ => {}
            }
        }
    }
    client.disconnect().await?;

    println!("\n{}", "=".repeat(60));
    println!("Tool Usage Summary");
    println!("{}", "=".repeat(60));
    let requests = log.lock().unwrap_or_else(PoisonError::into_inner);
    for (index, request) in requests.iter().enumerate() {
        println!("\n{}. Tool: {}", index + 1, request.tool);
        println!(
            "   Input: {}",
            serde_json::to_string_pretty(&request.input)?
        );
        if !request.suggestions.is_empty() {
            println!("   Suggestions: {:?}", request.suggestions);
        }
    }
    Ok(())
}
