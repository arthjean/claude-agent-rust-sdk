//! Loading a local plugin into a session.
//!
//! A plugin extends the CLI with slash commands, agents, skills, and
//! hooks. The example writes a demo plugin to a temporary directory, with
//! a manifest under `.claude-plugin/plugin.json` and one `/greet` command,
//! loads it with [`SdkPluginConfig::local`], and prints what the init
//! message of the session reports about it.
//!
//! Run it with `cargo run --example plugin_example`.

use std::path::Path;

use claude_agent_rust_sdk::{BoxError, ClaudeAgentOptions, Message, SdkPluginConfig, query};
use futures_util::StreamExt;

/// Writes the demo plugin under `root` and returns its directory.
fn write_demo_plugin(root: &Path) -> std::io::Result<std::path::PathBuf> {
    let plugin = root.join("demo-plugin");
    std::fs::create_dir_all(plugin.join(".claude-plugin"))?;
    std::fs::create_dir_all(plugin.join("commands"))?;
    std::fs::write(
        plugin.join(".claude-plugin").join("plugin.json"),
        r#"{
  "name": "demo-plugin",
  "description": "A demo plugin showing how to extend Claude Code with custom commands",
  "version": "1.0.0"
}
"#,
    )?;
    std::fs::write(
        plugin.join("commands").join("greet.md"),
        "# Greet Command\n\n\
         When the user runs this command, greet them warmly and explain that this \
         message came from a custom plugin loaded through the Rust SDK.\n",
    )?;
    Ok(plugin)
}

#[tokio::main]
async fn main() -> Result<(), BoxError> {
    println!("=== Plugin Example ===\n");
    let root = tempfile::tempdir()?;
    let plugin = write_demo_plugin(root.path())?;
    println!("Loading plugin from: {}\n", plugin.display());

    let options = ClaudeAgentOptions::builder()
        .plugins([SdkPluginConfig::local(plugin.to_string_lossy())])
        .max_turns(1)
        .build()?;
    let mut messages = query("Hello!", options);
    while let Some(message) = messages.next().await {
        if let Message::System(system) = message?
            && let Some(init) = system.init_data()
        {
            println!("System initialized!");
            if init.plugins.is_empty() {
                println!("The init message lists no plugin.");
            } else {
                println!("Plugins loaded:");
                for loaded in &init.plugins {
                    println!("  - {} (path: {})", loaded.name, loaded.path);
                }
            }
            for error in init.plugin_errors.iter().flatten() {
                println!("Plugin error: {} ({})", error.message, error.plugin);
            }
            let greet = init
                .slash_commands
                .iter()
                .any(|command| command.ends_with("greet"));
            println!("The /greet command is available: {greet}\n");
        }
    }
    Ok(())
}
