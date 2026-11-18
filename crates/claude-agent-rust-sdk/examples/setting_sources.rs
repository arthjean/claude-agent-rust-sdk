//! Choosing which filesystem settings a session loads.
//!
//! The CLI reads settings, and with them slash commands, agents, and
//! skills, from three sources:
//!
//! - [`SettingSource::User`]: the global settings of the user, under
//!   `~/.claude/`;
//! - [`SettingSource::Project`]: the settings of the project, under
//!   `.claude/` in the working directory;
//! - [`SettingSource::Local`]: the local settings of the project, kept
//!   out of version control.
//!
//! When `setting_sources` is not set, the CLI loads all three. An empty
//! list loads none.
//!
//! The example creates a temporary project that defines one slash
//! command, then shows under which setting sources the session lists it.
//!
//! Scenarios: `default`, `disable_all`, `user_only`, `project_and_user`.
//! Run one with `cargo run --example setting_sources -- default`, every
//! one with `-- all`, or pass nothing to list them.

use std::path::Path;
use std::process::ExitCode;

use claude_agent_kit::{
    BoxError, ClaudeAgentOptions, ClaudeAgentOptionsBuilder, ClaudeSdkClient, Message,
    SettingSource,
};
use futures_util::StreamExt;

/// Name and description of each scenario, in the order `all` runs them.
const SCENARIOS: [(&str, &str); 4] = [
    ("default", "No setting_sources: every source is loaded"),
    ("disable_all", "An empty list: no source is loaded"),
    ("user_only", "Only the user settings"),
    ("project_and_user", "The user and the project settings"),
];

/// Slash command the temporary project defines.
const PROJECT_COMMAND: &str = "sdk-example-greeting";

/// Creates a project whose settings define [`PROJECT_COMMAND`].
fn create_project() -> std::io::Result<tempfile::TempDir> {
    let project = tempfile::tempdir()?;
    let commands = project.path().join(".claude").join("commands");
    std::fs::create_dir_all(&commands)?;
    std::fs::write(
        commands.join(format!("{PROJECT_COMMAND}.md")),
        "Greet the user in one short sentence.\n",
    )?;
    Ok(project)
}

/// Starts a session and returns the slash commands its init message
/// lists.
async fn slash_commands(options: ClaudeAgentOptions) -> Result<Vec<String>, BoxError> {
    let client = ClaudeSdkClient::new(options);
    client.connect().await?;
    client.query("What is 2 + 2?").await?;

    let mut commands = Vec::new();
    {
        let mut messages = client.receive_response();
        while let Some(message) = messages.next().await {
            if let Message::System(system) = message?
                && let Some(init) = system.init_data()
            {
                commands = init.slash_commands;
                break;
            }
        }
    }
    client.disconnect().await?;
    Ok(commands)
}

/// Runs one scenario and reports whether the project command is listed.
async fn check(
    title: &str,
    sources: &str,
    options: ClaudeAgentOptionsBuilder,
    project: &Path,
    expect_project_command: bool,
) -> Result<(), BoxError> {
    println!("=== {title} ===");
    println!("Setting sources: {sources}");
    let expectation = if expect_project_command {
        "IS"
    } else {
        "is NOT"
    };
    println!("Expected: the project command /{PROJECT_COMMAND} {expectation} available\n");

    let commands = slash_commands(options.cwd(project).build()?).await?;
    println!("Available slash commands: {commands:?}");

    let available = commands.iter().any(|command| command == PROJECT_COMMAND);
    let state = if available { "is" } else { "is NOT" };
    let verdict = if available == expect_project_command {
        "as expected"
    } else {
        "unexpected"
    };
    println!("/{PROJECT_COMMAND} {state} available ({verdict})\n");
    Ok(())
}

/// Runs the scenario of that name. Returns false for an unknown name.
async fn run(name: &str, project: &Path) -> Result<bool, BoxError> {
    let options = ClaudeAgentOptions::builder();
    match name {
        "default" => {
            check(
                "Default Behavior Example",
                "not set (default)",
                options,
                project,
                true,
            )
            .await?;
        }
        "disable_all" => {
            check(
                "Disable All Sources Example",
                "[]",
                options.setting_sources(Vec::<SettingSource>::new()),
                project,
                false,
            )
            .await?;
        }
        "user_only" => {
            check(
                "User Settings Only Example",
                "[user]",
                options.setting_sources([SettingSource::User]),
                project,
                false,
            )
            .await?;
        }
        "project_and_user" => {
            check(
                "Project + User Settings Example",
                "[user, project]",
                options.setting_sources([SettingSource::User, SettingSource::Project]),
                project,
                true,
            )
            .await?;
        }
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
        println!("Usage: cargo run --example setting_sources -- <scenario>\n");
        print_scenarios();
        return Ok(ExitCode::SUCCESS);
    };

    let project = create_project()?;
    if name == "all" {
        for (scenario, _) in SCENARIOS {
            run(scenario, project.path()).await?;
            println!("{}\n", "-".repeat(50));
        }
    } else if !run(&name, project.path()).await? {
        eprintln!("Error: Unknown scenario '{name}'\n");
        print_scenarios();
        return Ok(ExitCode::FAILURE);
    }
    Ok(ExitCode::SUCCESS)
}
