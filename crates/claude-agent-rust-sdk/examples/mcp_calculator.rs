//! Calculator tools served to Claude from this process.
//!
//! The tools are plain Rust functions. They run inside this program, with
//! no separate MCP server process: the CLI calls them over the control
//! channel.
//!
//! Run it with `cargo run --example mcp_calculator`, or pass prompts of
//! your own: `cargo run --example mcp_calculator -- "What is 7 * 6?"`.

use claude_agent_rust_sdk::{
    BoxError, ClaudeAgentOptions, ClaudeSdkClient, ContentBlock, Message, SdkMcpTool,
    ToolAnnotations, ToolResult, UserContent, create_sdk_mcp_server,
};
use futures_util::StreamExt;
use schemars::JsonSchema;
use serde::Deserialize;

#[derive(Deserialize, JsonSchema)]
struct Operands {
    /// First operand.
    a: f64,
    /// Second operand.
    b: f64,
}

#[derive(Deserialize, JsonSchema)]
struct Radicand {
    /// Number to take the square root of.
    n: f64,
}

#[derive(Deserialize, JsonSchema)]
struct Power {
    base: f64,
    exponent: f64,
}

fn binary(
    name: &str,
    description: &str,
    symbol: &'static str,
    operation: fn(f64, f64) -> f64,
) -> SdkMcpTool {
    SdkMcpTool::typed::<Operands>(name, description, move |args| async move {
        let result = operation(args.a, args.b);
        Ok(ToolResult::text(format!(
            "{} {symbol} {} = {result}",
            args.a, args.b
        )))
    })
    .annotations(ToolAnnotations::new().read_only_hint(true))
}

fn calculator_tools() -> Vec<SdkMcpTool> {
    let divide = SdkMcpTool::typed::<Operands>(
        "divide",
        "Divide one number by another",
        |args| async move {
            if args.b == 0.0 {
                return Ok(ToolResult::error("Error: Division by zero is not allowed"));
            }
            Ok(ToolResult::text(format!(
                "{} / {} = {}",
                args.a,
                args.b,
                args.a / args.b
            )))
        },
    );
    let sqrt = SdkMcpTool::typed::<Radicand>("sqrt", "Calculate square root", |args| async move {
        if args.n < 0.0 {
            return Err(BoxError::from(format!(
                "Cannot calculate the square root of the negative number {}",
                args.n
            )));
        }
        Ok(ToolResult::text(format!(
            "sqrt({}) = {}",
            args.n,
            args.n.sqrt()
        )))
    });
    let power =
        SdkMcpTool::typed::<Power>("power", "Raise a number to a power", |args| async move {
            Ok(ToolResult::text(format!(
                "{}^{} = {}",
                args.base,
                args.exponent,
                args.base.powf(args.exponent)
            )))
        });
    vec![
        binary("add", "Add two numbers", "+", |a, b| a + b),
        binary(
            "subtract",
            "Subtract one number from another",
            "-",
            |a, b| a - b,
        ),
        binary("multiply", "Multiply two numbers", "*", |a, b| a * b),
        divide,
        sqrt,
        power,
    ]
}

fn display(message: &Message) {
    match message {
        Message::Assistant(assistant) => {
            for block in &assistant.content {
                match block {
                    ContentBlock::Text(text) => println!("Claude: {}", text.text),
                    ContentBlock::ToolUse(call) => {
                        println!("Using tool: {} with input {}", call.name, call.input);
                    }
                    _ => {}
                }
            }
        }
        Message::User(user) => {
            if let UserContent::Blocks(blocks) = &user.content {
                for block in blocks {
                    if let ContentBlock::ToolResult(result) = block {
                        println!("Tool result: {:?}", result.content);
                    }
                }
            }
        }
        Message::Result(result) => {
            println!("Result ended");
            if let Some(cost) = result.total_cost_usd {
                println!("Cost: ${cost:.6}");
            }
        }
        _ => {}
    }
}

#[tokio::main]
async fn main() -> Result<(), claude_agent_rust_sdk::Error> {
    let tools = calculator_tools();
    let allowed: Vec<String> = tools
        .iter()
        .map(|tool| format!("mcp__calc__{}", tool.name()))
        .collect();
    let calculator = create_sdk_mcp_server("calculator", Some("2.0.0"), tools)?;
    let options = ClaudeAgentOptions::builder()
        .mcp_server("calc", calculator)
        .allowed_tools(allowed)
        .build()?;

    let mut prompts: Vec<String> = std::env::args().skip(1).collect();
    if prompts.is_empty() {
        prompts = [
            "Calculate 15 + 27",
            "What is 100 divided by 7?",
            "Calculate (12 + 8) * 3 - 10",
        ]
        .map(str::to_owned)
        .to_vec();
    }

    for prompt in prompts {
        println!("\n{}", "=".repeat(50));
        println!("Prompt: {prompt}");
        println!("{}", "=".repeat(50));

        let client = ClaudeSdkClient::new(options.clone());
        client.connect().await?;
        client.query(prompt.as_str()).await?;
        {
            let mut messages = client.receive_response();
            while let Some(message) = messages.next().await {
                display(&message?);
            }
        }
        client.disconnect().await?;
    }
    Ok(())
}
