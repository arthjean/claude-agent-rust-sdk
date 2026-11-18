#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use claude_agent_kit::{Error, Message, SdkMcpTool, ToolResult, create_sdk_mcp_server, query};
use futures_util::StreamExt;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use common::{Fake, STEP_LIMIT};

#[derive(Deserialize, JsonSchema)]
struct Add {
    a: f64,
    b: f64,
}

fn mcp_request(request_id: &str, id: u64, method: &str, params: Value) -> Value {
    json!({"emit": {
        "type": "control_request",
        "request_id": request_id,
        "request": {
            "subtype": "mcp_message",
            "server_name": "calc",
            "message": {"jsonrpc": "2.0", "id": id, "method": method, "params": params},
        },
    }})
}

fn mcp_answer(request_id: &str, mcp_response: Value) -> Value {
    json!({"expect": {
        "type": "control_response",
        "response": {
            "subtype": "success",
            "request_id": request_id,
            "response": {"mcp_response": mcp_response},
        },
    }})
}

#[tokio::test]
async fn typed_tool_is_served_to_a_cli_process_over_the_control_channel() {
    let calls = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&calls);
    let add = SdkMcpTool::typed::<Add>("add", "Add two numbers", move |args| {
        counted.fetch_add(1, Ordering::SeqCst);
        async move { Ok(ToolResult::text((args.a + args.b).to_string())) }
    });
    let calculator = create_sdk_mcp_server("calculator", Some("2.0.0"), [add]).unwrap();

    let fake = Fake::new(json!({
        "auto_reply": true,
        "steps": [
            {"expect": {"type": "user"}},
            mcp_request("mcp-1", 0, "initialize", json!({
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": {"name": "claude-code", "version": "2.1.325"},
            })),
            mcp_answer("mcp-1", json!({
                "id": 0,
                "result": {
                    "protocolVersion": "2025-06-18",
                    "serverInfo": {"name": "calculator", "version": "2.0.0"},
                },
            })),
            {"emit": {
                "type": "control_request",
                "request_id": "mcp-2",
                "request": {
                    "subtype": "mcp_message",
                    "server_name": "calc",
                    "message": {"jsonrpc": "2.0", "method": "notifications/initialized"},
                },
            }},
            mcp_answer("mcp-2", json!({"jsonrpc": "2.0", "result": {}})),
            mcp_request("mcp-3", 1, "tools/list", json!({})),
            mcp_answer("mcp-3", json!({"id": 1, "result": {}})),
            mcp_request("mcp-4", 2, "tools/call", json!({"name": "add", "arguments": {"a": 2, "b": 3}})),
            mcp_answer("mcp-4", json!({
                "id": 2,
                "result": {"content": [{"type": "text", "text": "5"}], "isError": false},
            })),
            mcp_request("mcp-5", 3, "tools/call", json!({"name": "add", "arguments": {"a": 2}})),
            mcp_answer("mcp-5", json!({"id": 3, "result": {"isError": true}})),
            {"emit": {
                "type": "assistant",
                "parent_tool_use_id": null,
                "session_id": "test",
                "message": {
                    "role": "assistant",
                    "model": "claude-sonnet-5",
                    "content": [{"type": "text", "text": "2 + 3 = 5"}],
                },
            }},
            {"emit": {
                "type": "result",
                "subtype": "success",
                "duration_ms": 1,
                "duration_api_ms": 1,
                "is_error": false,
                "num_turns": 1,
                "session_id": "test",
                "result": "2 + 3 = 5",
            }},
        ],
    }));
    let options = fake
        .options()
        .mcp_server("calc", calculator)
        .allowed_tools(["mcp__calc__add"])
        .build()
        .unwrap();

    let items: Vec<Result<Message, Error>> =
        tokio::time::timeout(STEP_LIMIT, query("What is 2 + 3?", options).collect())
            .await
            .expect("the query stalled");
    let messages: Vec<Message> = items
        .into_iter()
        .map(|item| item.expect("the query yielded an error"))
        .collect();

    let init = messages
        .iter()
        .find_map(|message| match message {
            Message::System(system) if system.subtype() == "init" => Some(system.data().clone()),
            _ => None,
        })
        .expect("no init message");
    let arguments: Vec<String> = serde_json::from_value(init["fake_args"].clone()).unwrap();
    let position = arguments
        .iter()
        .position(|argument| argument == "--mcp-config")
        .expect("no --mcp-config argument");
    let config: Value = serde_json::from_str(&arguments[position + 1]).unwrap();
    assert_eq!(
        config,
        json!({"mcpServers": {"calc": {"type": "sdk", "name": "calculator"}}})
    );

    let results: Vec<&str> = messages
        .iter()
        .filter_map(|message| match message {
            Message::Result(result) => result.result.as_deref(),
            _ => None,
        })
        .collect();
    assert_eq!(results, ["2 + 3 = 5"]);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}
