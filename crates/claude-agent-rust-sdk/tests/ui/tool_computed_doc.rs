#[derive(serde::Deserialize, schemars::JsonSchema)]
struct Args {
    text: String,
}

#[doc = concat!("Echo", " the text.")]
#[claude_agent_rust_sdk::tool]
async fn echo(args: Args) -> Result<claude_agent_rust_sdk::ToolResult, String> {
    Ok(claude_agent_rust_sdk::ToolResult::text(args.text))
}

fn main() {}
