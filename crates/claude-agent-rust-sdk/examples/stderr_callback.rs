//! Capturing the error output of the CLI with a callback.
//!
//! The callback receives each line the CLI writes to its error output:
//! warnings and errors. A clean run usually writes nothing. For the
//! verbose debug log of the CLI, pass `.extra_arg("debug-file", Some(path))`
//! and read that file instead.
//!
//! Run it with `cargo run --example stderr_callback`.

use std::sync::{Arc, Mutex, PoisonError};

use claude_agent_rust_sdk::{ClaudeAgentOptions, Error, Message, StderrCallback, query};
use futures_util::StreamExt;

#[tokio::main]
async fn main() -> Result<(), Error> {
    let stderr_lines: Arc<Mutex<Vec<String>>> = Arc::default();

    let callback = {
        let stderr_lines = Arc::clone(&stderr_lines);
        StderrCallback::from_sync(move |line| {
            if line.contains("[ERROR]") {
                println!("Error detected: {line}");
            }
            stderr_lines
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(line.to_owned());
        })
    };
    let options = ClaudeAgentOptions::builder().stderr(callback).build()?;

    println!("Running query with stderr capture...");
    let mut messages = query("What is 2+2?", options);
    while let Some(message) = messages.next().await {
        if let Message::Assistant(assistant) = message? {
            for block in &assistant.content {
                if let Some(text) = block.as_text() {
                    println!("Response: {text}");
                }
            }
        }
    }

    let stderr_lines = stderr_lines.lock().unwrap_or_else(PoisonError::into_inner);
    println!("\nCaptured {} stderr lines", stderr_lines.len());
    if let Some(first) = stderr_lines.first() {
        let preview: String = first.chars().take(100).collect();
        println!("First stderr line: {preview}");
    }
    Ok(())
}
