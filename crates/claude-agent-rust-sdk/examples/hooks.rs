//! Hook callbacks registered through the options.
//!
//! A hook is an async function the CLI calls at a given event: before a
//! tool runs, after it ran, when a prompt is submitted. Its answer can
//! allow or deny the call, add context for the model, or stop the run.
//!
//! Examples:
//!
//! - `PreToolUse`: deny shell commands that match a pattern.
//! - `UserPromptSubmit`: add context when a prompt is submitted.
//! - `PostToolUse`: review the output of a tool, with `reason` and
//!   `systemMessage`.
//! - `DecisionFields`: answer with a permission decision of `allow` or
//!   `deny`.
//! - `ContinueControl`: stop the run with `continue` set to false and a
//!   stop reason.
//!
//! Run one with `cargo run --example hooks -- PreToolUse`, every one with
//! `-- all`, or pass nothing to list them.

use std::process::ExitCode;

use claude_agent_rust_sdk::{
    BoxError, ClaudeAgentOptions, ClaudeSdkClient, HookContext, HookInput, HookMatcher, HookOutput,
    HookPermissionDecision, Message, PostToolUseHookSpecificOutput, PreToolUseHookSpecificOutput,
    SyncHookOutput, UserPromptSubmitHookSpecificOutput,
};
use futures_util::StreamExt;

/// Name and description of each example, in the order `all` runs them.
const EXAMPLES: [(&str, &str); 5] = [
    ("PreToolUse", "Block commands using a PreToolUse hook"),
    ("UserPromptSubmit", "Add context at prompt submission"),
    (
        "PostToolUse",
        "Review tool output with reason and systemMessage",
    ),
    (
        "DecisionFields",
        "Use a permission decision of allow or deny with a reason",
    ),
    (
        "ContinueControl",
        "Control execution with continue and a stop reason",
    ),
];

/// Names of the shell tool: `Bash`, or `PowerShell` on Windows.
const SHELL_TOOLS: [&str; 2] = ["Bash", "PowerShell"];

/// Matcher for the shell tool on every platform.
const SHELL_MATCHER: &str = "Bash|PowerShell";

/// Command fragments the `PreToolUse` example refuses to run.
const BLOCKED_PATTERNS: [&str; 1] = ["foo.sh"];

fn display(message: &Message) {
    match message {
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

/// Sends one prompt and prints the response.
async fn turn(client: &ClaudeSdkClient, prompt: &str) -> Result<(), BoxError> {
    println!("User: {prompt}");
    client.query(prompt).await?;
    let mut messages = client.receive_response();
    while let Some(message) = messages.next().await {
        display(&message?);
    }
    Ok(())
}

/// A `PreToolUse` answer that denies the call.
fn deny(reason: impl Into<String>) -> HookOutput {
    let decision = PreToolUseHookSpecificOutput::default()
        .permission_decision(HookPermissionDecision::Deny)
        .permission_decision_reason(reason);
    SyncHookOutput::default()
        .hook_specific_output(decision)
        .into()
}

/// Denies shell commands that contain a blocked pattern.
///
/// The tool is read with `HookInput::tool_name` and `tool_input`, which
/// also read an input of a shape this crate does not know
/// (`HookInput::Unknown`), and a call whose tool or command cannot be read
/// is denied: a guard that cannot check a command must not let it through.
async fn check_bash_command(
    input: HookInput,
    _tool_use_id: Option<String>,
    _context: HookContext,
) -> Result<HookOutput, BoxError> {
    if input.hook_event_name() != "PreToolUse" {
        return Ok(HookOutput::default());
    }
    let Some(tool_name) = input.tool_name() else {
        return Ok(deny("The hook input names no tool to check"));
    };
    if !SHELL_TOOLS.contains(&tool_name) {
        return Ok(HookOutput::default());
    }

    let Some(command) = input
        .tool_input()
        .and_then(|tool_input| tool_input["command"].as_str())
    else {
        return Ok(deny("The hook input holds no command to check"));
    };
    for pattern in BLOCKED_PATTERNS {
        if command.contains(pattern) {
            eprintln!("Blocked command: {command}");
            return Ok(deny(format!("Command contains invalid pattern: {pattern}")));
        }
    }
    Ok(HookOutput::default())
}

/// Adds context the model reads along with the prompt.
async fn add_custom_instructions(
    _input: HookInput,
    _tool_use_id: Option<String>,
    _context: HookContext,
) -> Result<HookOutput, BoxError> {
    let context = UserPromptSubmitHookSpecificOutput::default()
        .additional_context("My favorite color is hot pink");
    Ok(SyncHookOutput::default()
        .hook_specific_output(context)
        .into())
}

/// Reviews the output of a tool and warns the model about an error.
async fn review_tool_output(
    input: HookInput,
    _tool_use_id: Option<String>,
    _context: HookContext,
) -> Result<HookOutput, BoxError> {
    let HookInput::PostToolUse(input) = input else {
        return Ok(HookOutput::default());
    };
    if !input
        .tool_response
        .to_string()
        .to_lowercase()
        .contains("error")
    {
        return Ok(HookOutput::default());
    }

    let context = PostToolUseHookSpecificOutput::default().additional_context(
        "The command encountered an error. You may want to try a different approach.",
    );
    Ok(SyncHookOutput::default()
        .system_message("The command produced an error")
        .reason("Tool execution failed - consider checking the command syntax")
        .hook_specific_output(context)
        .into())
}

/// Denies writes to files with `important` in their name, and explicitly
/// allows every other call.
async fn strict_approval_hook(
    input: HookInput,
    _tool_use_id: Option<String>,
    _context: HookContext,
) -> Result<HookOutput, BoxError> {
    if input.hook_event_name() != "PreToolUse" {
        return Ok(HookOutput::default());
    }
    let Some(tool_name) = input.tool_name() else {
        return Ok(deny("The hook input names no tool to check"));
    };

    if tool_name == "Write" {
        let Some(file_path) = input
            .tool_input()
            .and_then(|tool_input| tool_input["file_path"].as_str())
        else {
            return Ok(deny("The hook input holds no file path to check"));
        };
        if file_path.to_lowercase().contains("important") {
            eprintln!("Blocked Write to: {file_path}");
            let decision = PreToolUseHookSpecificOutput::default()
                .permission_decision(HookPermissionDecision::Deny)
                .permission_decision_reason("Security policy blocks writes to important files");
            return Ok(SyncHookOutput::default()
                .reason(
                    "Writes to files containing 'important' in the name are not allowed for safety",
                )
                .system_message("Write operation blocked by security policy")
                .hook_specific_output(decision)
                .into());
        }
    }

    let decision = PreToolUseHookSpecificOutput::default()
        .permission_decision(HookPermissionDecision::Allow)
        .permission_decision_reason("Tool passed security checks");
    Ok(SyncHookOutput::default()
        .reason("Tool use approved after security review")
        .hook_specific_output(decision)
        .into())
}

/// Stops the run when the output of a tool mentions a critical error.
async fn stop_on_error_hook(
    input: HookInput,
    _tool_use_id: Option<String>,
    _context: HookContext,
) -> Result<HookOutput, BoxError> {
    let HookInput::PostToolUse(input) = input else {
        return Ok(HookOutput::default());
    };

    if input
        .tool_response
        .to_string()
        .to_lowercase()
        .contains("critical")
    {
        eprintln!("Critical error detected - stopping execution");
        return Ok(SyncHookOutput::default()
            .continue_(false)
            .stop_reason("Critical error detected in tool output - execution halted for safety")
            .system_message("Execution stopped due to critical error")
            .into());
    }
    Ok(SyncHookOutput::default().continue_(true).into())
}

async fn example_pretooluse() -> Result<(), BoxError> {
    println!("=== PreToolUse Example ===");
    println!("A PreToolUse hook blocks some shell commands and lets others run.\n");

    let options = ClaudeAgentOptions::builder()
        .allowed_tools(SHELL_TOOLS)
        .hook(
            "PreToolUse",
            HookMatcher::new()
                .matcher(SHELL_MATCHER)
                .hook(check_bash_command),
        )
        .build()?;
    let client = ClaudeSdkClient::new(options);
    client.connect().await?;

    println!("Test 1: a command the hook should block...");
    turn(&client, "Run the bash command: ./foo.sh --help").await?;

    println!("\n{}\n", "=".repeat(50));

    println!("Test 2: a command the hook should allow...");
    turn(
        &client,
        "Run the bash command: echo 'Hello from hooks example!'",
    )
    .await?;

    client.disconnect().await?;
    Ok(())
}

async fn example_userpromptsubmit() -> Result<(), BoxError> {
    println!("=== UserPromptSubmit Example ===");
    println!("A UserPromptSubmit hook adds context the model can answer from.\n");

    let options = ClaudeAgentOptions::builder()
        .hook(
            "UserPromptSubmit",
            HookMatcher::new().hook(add_custom_instructions),
        )
        .build()?;
    let client = ClaudeSdkClient::new(options);
    client.connect().await?;

    turn(&client, "What's my favorite color?").await?;

    client.disconnect().await?;
    Ok(())
}

async fn example_posttooluse() -> Result<(), BoxError> {
    println!("=== PostToolUse Example ===");
    println!("A PostToolUse hook gives feedback with reason and systemMessage.\n");

    let options = ClaudeAgentOptions::builder()
        .allowed_tools(SHELL_TOOLS)
        .hook(
            "PostToolUse",
            HookMatcher::new()
                .matcher(SHELL_MATCHER)
                .hook(review_tool_output),
        )
        .build()?;
    let client = ClaudeSdkClient::new(options);
    client.connect().await?;

    turn(&client, "Run this command: ls /nonexistent_directory").await?;

    client.disconnect().await?;
    Ok(())
}

/// The files are written in a temporary directory, removed when the
/// example ends.
async fn example_decision_fields() -> Result<(), BoxError> {
    println!("=== Permission Decision Example ===");
    println!("A PreToolUse hook answers allow or deny, with reason and systemMessage.\n");

    let workspace = tempfile::tempdir()?;
    let mut allowed_tools = vec!["Write"];
    allowed_tools.extend(SHELL_TOOLS);
    let options = ClaudeAgentOptions::builder()
        .allowed_tools(allowed_tools)
        .cwd(workspace.path())
        .hook(
            "PreToolUse",
            HookMatcher::new()
                .matcher("Write")
                .hook(strict_approval_hook),
        )
        .build()?;
    let client = ClaudeSdkClient::new(options);
    client.connect().await?;

    println!("Test 1: writing to important_config.txt (should be blocked)...");
    turn(
        &client,
        "Write the text 'test data' to a file called important_config.txt",
    )
    .await?;

    println!("\n{}\n", "=".repeat(50));

    println!("Test 2: writing to regular_file.txt (should be approved)...");
    turn(
        &client,
        "Write the text 'test data' to a file called regular_file.txt",
    )
    .await?;

    client.disconnect().await?;
    Ok(())
}

async fn example_continue_control() -> Result<(), BoxError> {
    println!("=== Continue/Stop Control Example ===");
    println!("A PostToolUse hook halts the run with continue set to false.\n");

    let options = ClaudeAgentOptions::builder()
        .allowed_tools(SHELL_TOOLS)
        .hook(
            "PostToolUse",
            HookMatcher::new()
                .matcher(SHELL_MATCHER)
                .hook(stop_on_error_hook),
        )
        .build()?;
    let client = ClaudeSdkClient::new(options);
    client.connect().await?;

    turn(
        &client,
        "Run this bash command: echo 'CRITICAL ERROR: system failure'",
    )
    .await?;

    client.disconnect().await?;
    Ok(())
}

/// Runs the example of that name. Returns false for an unknown name.
async fn run(name: &str) -> Result<bool, BoxError> {
    match name {
        "PreToolUse" => example_pretooluse().await?,
        "UserPromptSubmit" => example_userpromptsubmit().await?,
        "PostToolUse" => example_posttooluse().await?,
        "DecisionFields" => example_decision_fields().await?,
        "ContinueControl" => example_continue_control().await?,
        _ => return Ok(false),
    }
    println!();
    Ok(true)
}

fn print_examples() {
    println!("Available examples:");
    println!("  all - Run every example");
    for (name, description) in EXAMPLES {
        println!("  {name} - {description}");
    }
}

#[tokio::main]
async fn main() -> Result<ExitCode, BoxError> {
    let Some(name) = std::env::args().nth(1) else {
        println!("Usage: cargo run --example hooks -- <example>\n");
        print_examples();
        return Ok(ExitCode::SUCCESS);
    };

    if name == "all" {
        for (example, _) in EXAMPLES {
            run(example).await?;
            println!("{}\n", "-".repeat(50));
        }
    } else if !run(&name).await? {
        eprintln!("Error: Unknown example '{name}'\n");
        print_examples();
        return Ok(ExitCode::FAILURE);
    }
    Ok(ExitCode::SUCCESS)
}
