//! Checks specific to this crate: the CLI accepts the `sdk-rs` entrypoint
//! and the native JSON-RPC dispatcher that serves in-process tools.

use claude_agent_rust_sdk::{
    ContentBlock, HookInput, HookMatcher, SdkMcpTool, SyncHookOutput, ToolResult,
    create_sdk_mcp_server,
};
use schemars::JsonSchema;
use serde::Deserialize;

use crate::suite::{self, SHELL_MATCHER, SHELL_TOOLS};

const ENTRYPOINT_VARIABLE: &str = "CLAUDE_CODE_ENTRYPOINT";
const ENTRYPOINT: &str = "sdk-rs";

#[tokio::test]
#[ignore = "runs the real CLI; run with `cargo xtask e2e`"]
async fn the_handshake_and_one_turn_succeed_with_the_sdk_rs_entrypoint() {
    let outputs = suite::log::<String>();
    let seen = outputs.clone();
    let options = suite::options()
        .allowed_tools(SHELL_TOOLS)
        .hook(
            "PostToolUse",
            HookMatcher::new()
                .matcher(SHELL_MATCHER)
                .hook(suite::hook(move |input| {
                    if let HookInput::PostToolUse(input) = input {
                        seen.lock().unwrap().push(input.tool_response.to_string());
                    }
                    SyncHookOutput::default()
                })),
        )
        .max_turns(4)
        .build()
        .unwrap();
    assert!(!options.env.contains_key(ENTRYPOINT_VARIABLE));

    let client = suite::connect(options).await;
    let server_info = client.server_info().unwrap();
    assert!(server_info.is_some(), "the handshake returned no data");

    let messages = suite::turn(
        &client,
        "Print the value of the environment variable CLAUDE_CODE_ENTRYPOINT with one shell \
         command and nothing else. In Bash run `printenv CLAUDE_CODE_ENTRYPOINT`; in PowerShell \
         run `echo $env:CLAUDE_CODE_ENTRYPOINT`. Then reply with the single word DONE.",
    )
    .await;
    client.disconnect().await.unwrap();

    let result = suite::result(&messages);
    assert!(!result.is_error, "{result:#?}");
    let outputs = suite::entries(&outputs);
    assert!(
        outputs.iter().any(|output| output.contains(ENTRYPOINT)),
        "no shell output showed {ENTRYPOINT_VARIABLE}={ENTRYPOINT}: {outputs:?}"
    );
}

#[derive(Deserialize, JsonSchema)]
struct Operands {
    a: i64,
    b: i64,
}

#[tokio::test]
#[ignore = "runs the real CLI; run with `cargo xtask e2e`"]
async fn the_cli_accepts_the_native_mcp_dispatcher() {
    let calls = suite::log::<(i64, i64)>();
    let seen = calls.clone();
    let add = SdkMcpTool::typed::<Operands>("add", "Add two integers", move |args| {
        let seen = seen.clone();
        async move {
            seen.lock().unwrap().push((args.a, args.b));
            Ok(ToolResult::text(format!("native-sum={}", args.a + args.b)))
        }
    });
    let server = create_sdk_mcp_server("calculator", Some("1.0.0"), [add]).unwrap();
    let options = suite::options()
        .mcp_server("calc", server)
        .allowed_tools(["mcp__calc__add"])
        .max_turns(4)
        .build()
        .unwrap();

    let messages = suite::run(
        "Call the mcp__calc__add tool with a=19 and b=23, then reply with its result.",
        options,
    )
    .await;

    let tools = suite::init_list(&messages, "tools");
    assert!(
        tools.iter().any(|tool| tool == "mcp__calc__add"),
        "the tool is not listed as mcp__<key>__<tool>: {tools:?}"
    );
    assert_eq!(suite::entries(&calls), [(19, 23)]);
    let returned = suite::blocks(&messages).into_iter().any(|(_, block)| {
        matches!(block, ContentBlock::ToolResult(result)
            if format!("{:?}", result.content).contains("native-sum=42"))
    });
    assert!(
        returned,
        "the result of the call did not come back: {messages:#?}"
    );
    assert!(!suite::result(&messages).is_error);
}
