#[claude_agent_rust_sdk::tool(name = "add", description = "Add three numbers")]
async fn add_numbers(a: f64, b: f64, c: f64) -> Result<claude_agent_rust_sdk::ToolResult, String> {
    Ok(claude_agent_rust_sdk::ToolResult::text((a + b + c).to_string()))
}

fn main() {}
