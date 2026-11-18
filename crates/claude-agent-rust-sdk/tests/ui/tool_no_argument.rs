/// Returns the current time.
#[claude_agent_kit::tool]
async fn now() -> Result<claude_agent_kit::ToolResult, String> {
    Ok(claude_agent_kit::ToolResult::text("noon"))
}

fn main() {}
