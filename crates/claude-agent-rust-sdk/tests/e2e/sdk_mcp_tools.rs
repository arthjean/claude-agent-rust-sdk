//! In-process MCP tools: execution and permission enforcement.

use claude_agent_kit::{
    McpSdkServerConfig, SdkMcpTool, SettingSource, ToolResult, create_sdk_mcp_server,
};
use schemars::JsonSchema;
use serde::Deserialize;

use crate::suite::{self, Log};

#[derive(Deserialize, JsonSchema)]
struct Echo {
    text: String,
}

#[derive(Deserialize, JsonSchema)]
struct Greet {
    name: String,
}

fn echo(executions: &Log<String>) -> SdkMcpTool {
    let executions = executions.clone();
    SdkMcpTool::typed::<Echo>("echo", "Echo back the input text", move |args| {
        let executions = executions.clone();
        async move {
            executions.lock().unwrap().push("echo".to_owned());
            Ok(ToolResult::text(format!("Echo: {}", args.text)))
        }
    })
}

fn greet(executions: &Log<String>) -> SdkMcpTool {
    let executions = executions.clone();
    SdkMcpTool::typed::<Greet>("greet", "Greet a person by name", move |args| {
        let executions = executions.clone();
        async move {
            executions.lock().unwrap().push("greet".to_owned());
            Ok(ToolResult::text(format!("Hello, {}!", args.name)))
        }
    })
}

fn server(name: &str, tools: impl IntoIterator<Item = SdkMcpTool>) -> McpSdkServerConfig {
    create_sdk_mcp_server(name, Some("1.0.0"), tools).unwrap()
}

async fn ask(options: claude_agent_kit::ClaudeAgentOptions, prompt: &str) {
    let client = suite::connect(options).await;
    suite::turn(&client, prompt).await;
    client.disconnect().await.unwrap();
}

#[tokio::test]
#[ignore = "runs the real CLI; run with `cargo xtask e2e`"]
async fn an_allowed_tool_is_executed() {
    let executions = suite::log::<String>();
    let options = suite::options()
        .mcp_server("test", server("test", [echo(&executions)]))
        .allowed_tools(["mcp__test__echo"])
        .build()
        .unwrap();

    ask(options, "Call the mcp__test__echo tool with any text").await;

    let executions = suite::entries(&executions);
    assert!(executions.contains(&"echo".to_owned()), "{executions:?}");
}

#[tokio::test]
#[ignore = "runs the real CLI; run with `cargo xtask e2e`"]
async fn a_disallowed_tool_is_never_executed() {
    let executions = suite::log::<String>();
    let options = suite::options()
        .mcp_server(
            "test",
            server("test", [echo(&executions), greet(&executions)]),
        )
        .disallowed_tools(["mcp__test__echo"])
        .allowed_tools(["mcp__test__greet"])
        .build()
        .unwrap();

    ask(
        options,
        "First use the greet tool to greet 'Alice'. After that completes, use the echo tool to \
         echo 'test'. Do these one at a time, not in parallel.",
    )
    .await;

    let executions = suite::entries(&executions);
    assert!(!executions.contains(&"echo".to_owned()), "{executions:?}");
    assert!(executions.contains(&"greet".to_owned()), "{executions:?}");
}

#[tokio::test]
#[ignore = "runs the real CLI; run with `cargo xtask e2e`"]
async fn several_tools_of_one_server_are_executed() {
    let executions = suite::log::<String>();
    let options = suite::options()
        .mcp_server(
            "multi",
            server("multi", [echo(&executions), greet(&executions)]),
        )
        .allowed_tools(["mcp__multi__echo", "mcp__multi__greet"])
        .build()
        .unwrap();

    ask(
        options,
        "Call mcp__multi__echo with text='test' and mcp__multi__greet with name='Bob'",
    )
    .await;

    let executions = suite::entries(&executions);
    assert!(executions.contains(&"echo".to_owned()), "{executions:?}");
    assert!(executions.contains(&"greet".to_owned()), "{executions:?}");
}

#[tokio::test]
#[ignore = "runs the real CLI; run with `cargo xtask e2e`"]
async fn a_tool_without_permission_is_not_executed() {
    let executions = suite::log::<String>();
    let options = suite::options()
        .mcp_server("noperm", server("noperm", [echo(&executions)]))
        .setting_sources(Vec::<SettingSource>::new())
        .build()
        .unwrap();

    ask(options, "Call the mcp__noperm__echo tool").await;

    assert_eq!(suite::entries(&executions), Vec::<String>::new());
}
