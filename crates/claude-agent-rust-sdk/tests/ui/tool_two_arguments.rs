#[claude_agent_rust_sdk::tool(name = "add", description = "Add two numbers")]
async fn add_numbers(a: f64, b: f64) -> Result<claude_agent_rust_sdk::ToolResult, String> {
    Ok(claude_agent_rust_sdk::ToolResult::text((a + b).to_string()))
}

fn main() {}
