/// Adds one.
#[claude_agent_rust_sdk::tool(crate = "nowhere")]
async fn add_one(args: f64) -> Result<claude_agent_rust_sdk::ToolResult, String> {
    Ok(claude_agent_rust_sdk::ToolResult::text((args + 1.0).to_string()))
}

fn main() {}
