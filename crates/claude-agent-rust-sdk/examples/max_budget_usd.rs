//! Cost control with `max_budget_usd`.
//!
//! The CLI checks the budget after each API call completes, so the final
//! cost may exceed the budget by up to the cost of one call. When the
//! budget is exceeded, the run ends with a result of subtype
//! `error_max_budget_usd`. If the CLI then exits with a failure, [`query`]
//! ends its stream with [`ErrorKind::ResultError`](crate::ErrorKind::ResultError), which carries that result.
//!
//! Run it with `cargo run --example max_budget_usd`.

use claude_agent_rust_sdk::{ClaudeAgentOptions, Error, Message, query};
use futures_util::StreamExt;

/// Subtype of the result that ends a run over budget.
const BUDGET_EXCEEDED: &str = "error_max_budget_usd";

/// Runs one query and prints the answer, the cost, and the final status.
async fn run(prompt: &str, options: ClaudeAgentOptions) -> Result<(), Error> {
    let mut messages = query(prompt, options);
    while let Some(message) = messages.next().await {
        match message {
            Ok(Message::Assistant(assistant)) => {
                for block in &assistant.content {
                    if let Some(text) = block.as_text() {
                        println!("Claude: {text}");
                    }
                }
            }
            Ok(Message::Result(result)) => {
                if let Some(cost) = result.total_cost_usd {
                    println!("Total cost: ${cost:.4}");
                }
                println!("Status: {}", result.subtype);
                if result.subtype == BUDGET_EXCEEDED {
                    println!("Budget limit exceeded!");
                    println!("Note: The cost may exceed the budget by up to one API call's worth");
                }
            }
            Ok(_) => {}
            Err(error)
                if error.result().and_then(|data| data.subtype()) == Some(BUDGET_EXCEEDED) =>
            {
                println!("The query ended with the error result above.");
            }
            Err(error) => return Err(error),
        }
    }
    println!();
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    println!("This example demonstrates using max_budget_usd to control API costs.\n");

    println!("=== Without Budget Limit ===");
    run("What is 2 + 2?", ClaudeAgentOptions::default()).await?;

    println!("=== With Reasonable Budget ($0.10) ===");
    let options = ClaudeAgentOptions::builder().max_budget_usd(0.10).build()?;
    run("What is 2 + 2?", options).await?;

    println!("=== With Tight Budget ($0.0001) ===");
    let options = ClaudeAgentOptions::builder()
        .max_budget_usd(0.0001)
        .build()?;
    run("Read the README.md file and summarize it", options).await?;

    println!(
        "Note: Budget checking happens after each API call completes,\n\
         so the final cost may slightly exceed the specified budget."
    );
    Ok(())
}
