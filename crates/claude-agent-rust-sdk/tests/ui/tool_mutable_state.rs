/// Counts calls.
#[claude_agent_rust_sdk::tool]
async fn count(calls: &mut u32, args: f64) -> Result<claude_agent_rust_sdk::ToolResult, String> {
    *calls += 1;
    Ok(claude_agent_rust_sdk::ToolResult::text(args.to_string()))
}

fn main() {}
