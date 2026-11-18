/// Returns the current time.
#[claude_agent_rust_sdk::tool]
async fn now() -> Result<claude_agent_rust_sdk::ToolResult, String> {
    Ok(claude_agent_rust_sdk::ToolResult::text("noon"))
}

fn main() {}
