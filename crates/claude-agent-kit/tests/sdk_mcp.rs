#![cfg(feature = "testing")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use claude_agent_kit::{
    BoxError, ClaudeAgentOptions, Error, McpSdkServerConfig, McpSdkServerInstance, McpServerConfig,
    SdkMcpTool, ToolAnnotations, ToolContent, ToolResult, create_sdk_mcp_server,
    query_with_transport,
};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::{Barrier, Notify};

use common::capture::Warnings;
use common::mcp::{PROTOCOL_VERSION, SdkMcpClient, initialize_params, texts};
use common::mock::{
    assistant, cli_answering, drain, result, result_texts, settle, silent_cli, success,
};

#[derive(Debug, Deserialize, JsonSchema)]
struct Add {
    /// First operand.
    a: f64,
    b: f64,
}

fn add() -> SdkMcpTool {
    SdkMcpTool::typed::<Add>("add", "Add two numbers", |args| async move {
        Ok(ToolResult::text((args.a + args.b).to_string()))
    })
}

fn noop(name: &str) -> SdkMcpTool {
    SdkMcpTool::raw(
        name,
        "Does nothing",
        json!({"type": "object", "properties": {}}),
        |_arguments| async { Ok(ToolResult::default()) },
    )
    .unwrap()
}

fn server(tools: impl IntoIterator<Item = SdkMcpTool>) -> McpSdkServerConfig {
    create_sdk_mcp_server("srv", None, tools).unwrap()
}

fn returning(name: &str, outcome: ToolResult) -> SdkMcpTool {
    SdkMcpTool::raw(name, "Returns a fixed result", json!({}), move |_| {
        let outcome = outcome.clone();
        async move { Ok(outcome) }
    })
    .unwrap()
}

struct SetOnDrop(Arc<AtomicUsize>);

impl Drop for SetOnDrop {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

fn blocked(name: &str, started: &Arc<Notify>, dropped: &Arc<AtomicUsize>) -> SdkMcpTool {
    let started = Arc::clone(started);
    let dropped = Arc::clone(dropped);
    SdkMcpTool::raw(name, "Never returns", json!({}), move |_| {
        let started = Arc::clone(&started);
        let guard = SetOnDrop(Arc::clone(&dropped));
        async move {
            let _guard = guard;
            started.notify_one();
            std::future::pending::<Result<ToolResult, BoxError>>().await
        }
    })
    .unwrap()
}

#[test]
fn typed_tool_derives_its_schema_from_the_argument_type() {
    let tool = add();
    let schema = tool.input_schema();
    assert_eq!(schema["type"], "object");
    assert_eq!(schema["properties"]["a"]["type"], "number");
    assert_eq!(schema["properties"]["a"]["description"], "First operand.");
    assert_eq!(schema["properties"]["b"]["type"], "number");
    assert_eq!(schema["required"], json!(["a", "b"]));
    assert!(schema.get("$schema").is_none());
    assert_eq!(tool.name(), "add");
    assert_eq!(tool.description(), "Add two numbers");
}

#[test]
fn typed_tool_inlines_nested_types_and_marks_optional_fields() {
    #[derive(Deserialize, JsonSchema)]
    #[allow(dead_code)]
    struct Point {
        x: i32,
        y: i32,
    }

    #[derive(Deserialize, JsonSchema)]
    #[allow(dead_code)]
    struct Draw {
        points: Vec<Point>,
        label: Option<String>,
    }

    let tool =
        SdkMcpTool::typed::<Draw>("draw", "Draws", |_args| async { Ok(ToolResult::default()) });
    let schema = tool.input_schema();
    assert_eq!(schema["required"], json!(["points"]));
    assert_eq!(schema["properties"]["points"]["type"], "array");
    let point = &schema["properties"]["points"]["items"];
    assert_eq!(point["type"], "object");
    assert_eq!(point["properties"]["x"]["type"], "integer");
    assert_eq!(point["required"], json!(["x", "y"]));
    assert!(schema.get("$defs").is_none());
}

#[test]
fn raw_schema_that_is_not_a_json_object_is_a_config_error() {
    for schema in [json!("object"), json!(["a"]), json!(true), Value::Null] {
        let outcome = SdkMcpTool::raw("broken", "Bad schema", schema.clone(), |_| async {
            Ok(ToolResult::default())
        });
        let Err(Error::Config { message }) = outcome else {
            panic!("expected a configuration error for {schema}");
        };
        assert!(message.contains("broken"), "{message}");
    }
}

#[test]
fn two_tools_with_the_same_name_are_a_config_error() {
    let outcome = create_sdk_mcp_server("calc", None, [add(), noop("other"), add()]);
    let Err(Error::Config { message }) = outcome else {
        panic!("expected a configuration error");
    };
    assert!(message.contains("'add'"), "{message}");
    assert!(message.contains("'calc'"), "{message}");
}

#[test]
fn server_config_is_an_sdk_server_that_keeps_its_instance_local() {
    let config = create_sdk_mcp_server("calculator", None, [add()]).unwrap();
    assert_eq!(config.name, "calculator");
    let config = McpServerConfig::from(config);
    assert_eq!(config.type_name(), "sdk");
    assert_eq!(
        serde_json::to_value(&config).unwrap(),
        json!({"type": "sdk", "name": "calculator"})
    );
}

#[tokio::test(start_paused = true)]
async fn server_version_defaults_to_1_0_0() {
    let client = SdkMcpClient::start([
        ("default", create_sdk_mcp_server("hello", None, []).unwrap()),
        (
            "explicit",
            create_sdk_mcp_server("hello", Some("3.2.1"), []).unwrap(),
        ),
    ])
    .await;
    let default = client.initialize("default").await;
    assert_eq!(
        default["serverInfo"],
        json!({"name": "hello", "version": "1.0.0"})
    );
    let explicit = client.initialize("explicit").await;
    assert_eq!(explicit["serverInfo"]["version"], "3.2.1");
}

#[tokio::test(start_paused = true)]
async fn initialize_reports_server_info_capabilities_and_the_negotiated_version() {
    let client = SdkMcpClient::start([("srv", server([noop("noop")]))]).await;
    let response = client
        .request("srv", "initialize", initialize_params())
        .await;
    let result = &response["result"];
    assert_eq!(result["serverInfo"]["name"], "srv");
    assert_eq!(result["protocolVersion"], PROTOCOL_VERSION);
    assert!(result["capabilities"]["tools"].is_object());

    let mut unknown_version = initialize_params();
    unknown_version["protocolVersion"] = json!("1999-01-01");
    let response = client.request("srv", "initialize", unknown_version).await;
    assert_eq!(response["result"]["protocolVersion"], "2025-11-25");
}

#[tokio::test(start_paused = true)]
async fn ping_is_answered_with_an_empty_result() {
    let client = SdkMcpClient::connected(server([])).await;
    let response = client.request("srv", "ping", json!({})).await;
    assert_eq!(response["result"], json!({}));
}

#[tokio::test(start_paused = true)]
async fn notification_is_acknowledged_in_a_success_control_response() {
    let client = SdkMcpClient::start([("srv", server([]))]).await;
    client
        .request("srv", "initialize", initialize_params())
        .await;
    let request_id = client.post(
        "srv",
        json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
    );
    client.control_response(&request_id).await;
    let frame = client
        .transport
        .written_frames()
        .into_iter()
        .find(|frame| frame["response"]["request_id"] == request_id.as_str())
        .unwrap();
    assert_eq!(
        frame,
        json!({
            "type": "control_response",
            "response": {
                "subtype": "success",
                "request_id": request_id,
                "response": {"mcp_response": {"jsonrpc": "2.0", "result": {}}},
            },
        })
    );
    assert!(client.list_tools("srv").await.is_empty());
}

#[tokio::test(start_paused = true)]
async fn messages_are_routed_to_the_server_registered_under_the_options_key() {
    let client = SdkMcpClient::start([
        (
            "first",
            create_sdk_mcp_server("shared-name", None, [noop("one")]).unwrap(),
        ),
        (
            "second",
            create_sdk_mcp_server("shared-name", None, [noop("two")]).unwrap(),
        ),
    ])
    .await;
    assert_eq!(client.list_tools("first").await[0]["name"], "one");
    assert_eq!(client.list_tools("second").await[0]["name"], "two");

    let response = client.request("shared-name", "tools/list", json!({})).await;
    assert_eq!(response["error"]["code"], -32601);
}

#[tokio::test(start_paused = true)]
async fn unknown_server_is_a_jsonrpc_error_naming_it() {
    let client = SdkMcpClient::start([("srv", server([]))]).await;
    let response = client.request("missing", "tools/list", json!({})).await;
    assert_eq!(response["error"]["code"], -32601);
    assert_eq!(response["error"]["message"], "Server 'missing' not found");
}

#[tokio::test(start_paused = true)]
async fn unsupported_method_is_a_jsonrpc_method_not_found() {
    let client = SdkMcpClient::connected(server([])).await;
    let response = client.request("srv", "resources/list", json!({})).await;
    assert_eq!(response["error"]["code"], -32601);
    assert!(response.get("result").is_none());
}

#[tokio::test(start_paused = true)]
async fn malformed_message_is_a_jsonrpc_error_and_the_session_survives() {
    let client = SdkMcpClient::connected(server([])).await;
    let response = client.send("srv", json!({"jsonrpc": "2.0", "id": 5})).await;
    assert_eq!(response["id"], 5);
    assert_eq!(response["error"]["code"], -32603);
    assert!(client.list_tools("srv").await.is_empty());
}

#[tokio::test(start_paused = true)]
async fn message_whose_id_is_not_a_jsonrpc_id_is_a_notification() {
    let client = SdkMcpClient::connected(server([])).await;
    let reply = client
        .send(
            "srv",
            json!({"jsonrpc": "2.0", "id": 2.5, "method": "tools/list"}),
        )
        .await;
    assert_eq!(reply, json!({"jsonrpc": "2.0", "result": {}}));
    assert!(client.list_tools("srv").await.is_empty());
}

#[tokio::test(start_paused = true)]
async fn control_request_without_server_name_or_message_is_refused() {
    let client = SdkMcpClient::start([("srv", server([]))]).await;
    for request in [
        json!({"subtype": "mcp_message", "message": {"jsonrpc": "2.0", "method": "ping", "id": 1}}),
        json!({"subtype": "mcp_message", "server_name": "srv"}),
        json!({"subtype": "mcp_message", "server_name": "srv", "message": {}}),
    ] {
        let request_id = format!("bad-{}", client.next_id());
        client.transport.push_frame(json!({
            "type": "control_request",
            "request_id": request_id,
            "request": request,
        }));
        let response = client.control_response(&request_id).await;
        assert_eq!(response["subtype"], "error");
        assert_eq!(
            response["error"],
            "Missing server_name or message for MCP request"
        );
    }
}

#[tokio::test(start_paused = true)]
async fn tools_list_wire_format_omits_empty_fields() {
    let read_data = noop("read_data").annotations(
        ToolAnnotations::new()
            .read_only_hint(true)
            .open_world_hint(false),
    );
    let delete_item = noop("delete_item").annotations(
        ToolAnnotations::new()
            .title("Delete")
            .destructive_hint(true)
            .idempotent_hint(true),
    );
    let large = noop("large").annotations(
        ToolAnnotations::new()
            .read_only_hint(true)
            .max_result_size_chars(500_000),
    );
    let only_size =
        noop("only_size").annotations(ToolAnnotations::new().max_result_size_chars(333));
    let undescribed = SdkMcpTool::raw("undescribed", "", json!({"type": "object"}), |_| async {
        Ok(ToolResult::default())
    })
    .unwrap();
    let client = SdkMcpClient::connected(server([
        add(),
        read_data,
        delete_item,
        large,
        only_size,
        undescribed,
    ]))
    .await;
    let tools = client.list_tools("srv").await;
    let names: Vec<&str> = tools
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "add",
            "read_data",
            "delete_item",
            "large",
            "only_size",
            "undescribed"
        ]
    );
    let tool = |name: &str| {
        tools
            .iter()
            .find(|tool| tool["name"] == name)
            .unwrap()
            .clone()
    };

    let plain = tool("add");
    let keys: Vec<&String> = plain.as_object().unwrap().keys().collect();
    assert_eq!(keys, ["description", "inputSchema", "name"]);
    assert_eq!(plain["description"], "Add two numbers");
    assert_eq!(plain["inputSchema"]["required"], json!(["a", "b"]));

    assert_eq!(
        tool("read_data")["annotations"],
        json!({"readOnlyHint": true, "openWorldHint": false})
    );
    assert!(tool("read_data").get("_meta").is_none());
    assert_eq!(
        tool("delete_item")["annotations"],
        json!({"title": "Delete", "destructiveHint": true, "idempotentHint": true})
    );
    assert_eq!(
        tool("large")["_meta"],
        json!({"anthropic/maxResultSizeChars": 500_000})
    );
    assert_eq!(tool("large")["annotations"], json!({"readOnlyHint": true}));
    assert_eq!(
        tool("only_size")["_meta"],
        json!({"anthropic/maxResultSizeChars": 333})
    );
    assert!(tool("only_size").get("annotations").is_none());
    assert_eq!(
        tool("undescribed"),
        json!({"name": "undescribed", "inputSchema": {"type": "object"}})
    );

    assert_eq!(client.list_tools("srv").await, tools);
}

#[tokio::test(start_paused = true)]
async fn typed_and_raw_tools_are_called_with_their_arguments() {
    let echo = SdkMcpTool::raw(
        "echo",
        "Echo the arguments",
        json!({"type": "object", "properties": {"text": {"type": "string"}}}),
        |arguments| async move { Ok(ToolResult::text(arguments.to_string())) },
    )
    .unwrap();
    let client = SdkMcpClient::connected(server([add(), echo])).await;

    let sum = client
        .call_tool("srv", "add", json!({"a": 2, "b": 3.5}))
        .await;
    assert_eq!(
        sum,
        json!({"content": [{"type": "text", "text": "5.5"}], "isError": false})
    );

    let echoed = client.call_tool("srv", "echo", json!({"text": "hi"})).await;
    assert_eq!(texts(&echoed), [r#"{"text":"hi"}"#]);

    let response = client
        .request("srv", "tools/call", json!({"name": "echo"}))
        .await;
    assert_eq!(texts(&response["result"]), ["{}"]);
}

#[tokio::test(start_paused = true)]
async fn tool_results_convert_each_content_type() {
    let (warnings, _guard) = Warnings::install();
    let mixed = ToolResult::new([
        ToolContent::text("plain"),
        ToolContent::image("aW1hZ2U=", "image/png"),
        ToolContent::ResourceLink {
            name: Some("Report".to_owned()),
            uri: Some("file:///report.pdf".to_owned()),
            description: Some("Quarterly report".to_owned()),
        },
        ToolContent::resource_link("Data", "file:///data.csv"),
        ToolContent::ResourceLink {
            name: None,
            uri: None,
            description: None,
        },
        ToolContent::text_resource("file:///notes.txt", "embedded notes"),
        ToolContent::blob_resource("file:///archive.zip", "YmluYXJ5"),
        ToolContent::text("after"),
    ]);
    let client = SdkMcpClient::connected(server([returning("mixed", mixed)])).await;
    let outcome = client.call_tool("srv", "mixed", json!({})).await;
    assert_eq!(
        outcome["content"],
        json!([
            {"type": "text", "text": "plain"},
            {"type": "image", "data": "aW1hZ2U=", "mimeType": "image/png"},
            {"type": "text", "text": "Report\nfile:///report.pdf\nQuarterly report"},
            {"type": "text", "text": "Data\nfile:///data.csv"},
            {"type": "text", "text": "Resource link"},
            {"type": "text", "text": "embedded notes"},
            {"type": "text", "text": "after"},
        ])
    );
    assert_eq!(outcome["isError"], false);
    let dropped: Vec<String> = warnings
        .all()
        .into_iter()
        .filter(|warning| warning.contains("Binary embedded resource"))
        .collect();
    assert_eq!(dropped.len(), 1, "{dropped:?}");
}

#[tokio::test(start_paused = true)]
async fn is_error_is_sent_as_is_error_in_camel_case() {
    let client = SdkMcpClient::connected(server([
        returning("fail", ToolResult::error("Division by zero")),
        returning("pass", ToolResult::text("fine")),
    ]))
    .await;
    let failed = client.call_tool("srv", "fail", json!({})).await;
    assert_eq!(failed["isError"], true);
    assert!(failed.get("is_error").is_none());
    assert_eq!(texts(&failed), ["Division by zero"]);
    let passed = client.call_tool("srv", "pass", json!({})).await;
    assert_eq!(passed["isError"], false);
}

#[tokio::test(start_paused = true)]
async fn unknown_tool_is_an_error_result() {
    let client = SdkMcpClient::connected(server([add()])).await;
    let outcome = client.call_tool("srv", "nope", json!({})).await;
    assert_eq!(outcome["isError"], true);
    assert_eq!(texts(&outcome), ["Tool 'nope' not found"]);
}

#[tokio::test(start_paused = true)]
async fn invalid_arguments_are_rejected_before_the_handler_runs() {
    let calls = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&calls);
    let add = SdkMcpTool::typed::<Add>("add", "Add two numbers", move |args| {
        counted.fetch_add(1, Ordering::SeqCst);
        async move { Ok(ToolResult::text((args.a + args.b).to_string())) }
    });
    let client = SdkMcpClient::connected(server([add])).await;

    let missing = client.call_tool("srv", "add", json!({"a": 1})).await;
    assert_eq!(missing["isError"], true);
    let [message] = &texts(&missing)[..] else {
        panic!("expected one text block");
    };
    assert!(message.starts_with("Input validation error: "), "{message}");
    assert!(message.contains('b'), "{message}");

    let wrong_type = client
        .call_tool("srv", "add", json!({"a": 1, "b": "two"}))
        .await;
    assert_eq!(wrong_type["isError"], true);
    let [message] = &texts(&wrong_type)[..] else {
        panic!("expected one text block");
    };
    assert!(message.starts_with("Input validation error: "), "{message}");
    assert!(message.contains("two"), "{message}");

    let fine = client
        .call_tool("srv", "add", json!({"a": 1, "b": 2}))
        .await;
    assert_eq!(texts(&fine), ["3"]);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test(start_paused = true)]
async fn handler_error_is_an_error_result_carrying_the_error_text() {
    let failing = SdkMcpTool::raw("fail", "Always fails", json!({}), |_| async {
        Err::<ToolResult, BoxError>("the database is unreachable".into())
    })
    .unwrap();
    let client = SdkMcpClient::connected(server([failing])).await;
    let outcome = client.call_tool("srv", "fail", json!({})).await;
    assert_eq!(
        outcome,
        json!({
            "content": [{"type": "text", "text": "the database is unreachable"}],
            "isError": true,
        })
    );
}

#[tokio::test(start_paused = true)]
async fn handler_panic_is_an_error_result_and_the_server_carries_on() {
    let panicking = SdkMcpTool::raw("explode", "Panics", json!({}), |_| async {
        if true {
            panic!("boom");
        }
        Ok(ToolResult::default())
    })
    .unwrap();
    let client = SdkMcpClient::connected(server([panicking, add()])).await;
    let outcome = client.call_tool("srv", "explode", json!({})).await;
    assert_eq!(outcome["isError"], true);
    assert_eq!(texts(&outcome), ["Tool 'explode' panicked"]);
    let sum = client
        .call_tool("srv", "add", json!({"a": 1, "b": 1}))
        .await;
    assert_eq!(texts(&sum), ["2"]);
}

#[tokio::test(start_paused = true)]
async fn handler_that_panics_before_returning_its_future_is_an_error_result() {
    let panicking = SdkMcpTool::raw("explode", "Panics", json!({}), |arguments: Value| {
        assert!(arguments.is_null(), "boom");
        async { Ok(ToolResult::default()) }
    })
    .unwrap();
    let client = SdkMcpClient::connected(server([panicking, add()])).await;
    let outcome = client.call_tool("srv", "explode", json!({})).await;
    assert_eq!(outcome["isError"], true);
    assert_eq!(texts(&outcome), ["Tool 'explode' panicked"]);
    let sum = client
        .call_tool("srv", "add", json!({"a": 1, "b": 1}))
        .await;
    assert_eq!(texts(&sum), ["2"]);
}

#[tokio::test(start_paused = true)]
async fn concurrent_tool_calls_on_one_server_run_in_parallel() {
    let barrier = Arc::new(Barrier::new(2));
    let meet = SdkMcpTool::raw("meet", "Waits for its twin", json!({}), move |arguments| {
        let barrier = Arc::clone(&barrier);
        async move {
            barrier.wait().await;
            Ok(ToolResult::text(arguments["who"].to_string()))
        }
    })
    .unwrap();
    let client = SdkMcpClient::connected(server([meet])).await;
    let call = |id: u64, who: &str| {
        json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "tools/call",
            "params": {"name": "meet", "arguments": {"who": who}},
        })
    };
    let first = client.post("srv", call(100, "first"));
    let second = client.post("srv", call(101, "second"));
    let first = client.answer(&first).await;
    let second = client.answer(&second).await;
    assert_eq!(first["id"], 100);
    assert_eq!(texts(&first["result"]), [r#""first""#]);
    assert_eq!(second["id"], 101);
    assert_eq!(texts(&second["result"]), [r#""second""#]);
}

#[tokio::test(start_paused = true)]
async fn request_id_already_in_flight_is_refused_without_reaching_the_tool() {
    let release = Arc::new(Notify::new());
    let calls = Arc::new(AtomicUsize::new(0));
    let wait = {
        let release = Arc::clone(&release);
        let calls = Arc::clone(&calls);
        SdkMcpTool::raw("wait", "Waits", json!({}), move |_| {
            let release = Arc::clone(&release);
            calls.fetch_add(1, Ordering::SeqCst);
            async move {
                release.notified().await;
                Ok(ToolResult::text("done"))
            }
        })
        .unwrap()
    };
    let client = SdkMcpClient::connected(server([wait])).await;
    let call = json!({
        "jsonrpc": "2.0",
        "id": 7,
        "method": "tools/call",
        "params": {"name": "wait", "arguments": {}},
    });
    let first = client.post("srv", call.clone());
    settle().await;
    let second = client.send("srv", call.clone()).await;
    assert_eq!(second["id"], 7);
    assert_eq!(second["error"]["code"], -32603);
    assert_eq!(
        second["error"]["message"],
        "Request id 7 is already in flight"
    );
    assert!(client.answered(&first).is_none());

    release.notify_one();
    let first = client.answer(&first).await;
    assert_eq!(texts(&first["result"]), ["done"]);
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    let again = client.post("srv", call);
    settle().await;
    release.notify_one();
    assert_eq!(texts(&client.answer(&again).await["result"]), ["done"]);
}

#[tokio::test(start_paused = true)]
async fn cancelled_notification_ends_the_running_tool_call() {
    let started = Arc::new(Notify::new());
    let dropped = Arc::new(AtomicUsize::new(0));
    let client = SdkMcpClient::connected(server([blocked("slow", &started, &dropped)])).await;
    let call = client.post(
        "srv",
        json!({
            "jsonrpc": "2.0",
            "id": 77,
            "method": "tools/call",
            "params": {"name": "slow", "arguments": {}},
        }),
    );
    started.notified().await;
    let acknowledgement = client
        .notify(
            "srv",
            "notifications/cancelled",
            json!({"requestId": 77, "reason": "user interrupted"}),
        )
        .await;
    assert_eq!(acknowledgement, json!({"jsonrpc": "2.0", "result": {}}));

    let response = client.answer(&call).await;
    assert_eq!(response["id"], 77);
    assert_eq!(response["error"]["code"], -32800);
    assert!(
        response["error"]["message"]
            .as_str()
            .unwrap()
            .to_lowercase()
            .contains("cancelled")
    );
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
    assert_eq!(client.list_tools("srv").await[0]["name"], "slow");
}

#[tokio::test(start_paused = true)]
async fn control_cancel_request_stops_the_tool_and_writes_no_response() {
    let started = Arc::new(Notify::new());
    let dropped = Arc::new(AtomicUsize::new(0));
    let client = SdkMcpClient::connected(server([blocked("slow", &started, &dropped)])).await;
    let call = json!({
        "jsonrpc": "2.0",
        "id": 9,
        "method": "tools/call",
        "params": {"name": "slow", "arguments": {}},
    });
    let request_id = client.post("srv", call.clone());
    started.notified().await;
    client.transport.push_frame(json!({
        "type": "control_cancel_request",
        "request_id": request_id,
    }));
    for _ in 0..10 {
        settle().await;
    }
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
    assert!(client.answered(&request_id).is_none());

    let retry = client.post("srv", call);
    started.notified().await;
    assert!(client.answered(&retry).is_none());
    client.close().await;
    assert_eq!(dropped.load(Ordering::SeqCst), 2);
}

#[tokio::test(start_paused = true)]
async fn string_prompt_keeps_stdin_open_for_the_in_process_server_until_the_result() {
    let transport = silent_cli();
    let script = transport.clone();
    transport.respond_with(move |frame| {
        if frame["type"] == "control_request" {
            return vec![success(frame, json!({}))];
        }
        if frame["type"] == "user" {
            return vec![json!({
                "type": "control_request",
                "request_id": "mcp-init",
                "request": {
                    "subtype": "mcp_message",
                    "server_name": "greeter",
                    "message": {
                        "jsonrpc": "2.0",
                        "id": 0,
                        "method": "initialize",
                        "params": initialize_params(),
                    },
                },
            })];
        }
        if frame["type"] == "control_response" && frame["response"]["request_id"] == "mcp-init" {
            assert!(!script.input_ended(), "stdin closed before the result");
            return vec![assistant("Hello!"), result("Hello!")];
        }
        Vec::new()
    });
    let options = ClaudeAgentOptions::builder()
        .mcp_server(
            "greeter",
            create_sdk_mcp_server("greeter", None, [noop("greet")]).unwrap(),
        )
        .build()
        .unwrap();
    let messages = drain(query_with_transport(
        "Greet Alice",
        options,
        transport.clone(),
    ))
    .await;
    assert_eq!(result_texts(&messages), ["Hello!"]);
    assert!(transport.input_ended());
    let handshake = common::mock::response_to(&transport, "mcp-init").unwrap();
    assert_eq!(handshake["subtype"], "success");
    assert_eq!(
        handshake["response"]["mcp_response"]["result"]["serverInfo"]["name"],
        "greeter"
    );
}

#[tokio::test(start_paused = true)]
async fn string_prompt_without_callbacks_closes_stdin_at_once() {
    let transport = cli_answering(|frame| {
        if frame["type"] == "user" {
            vec![assistant("Hello!"), result("Hello!")]
        } else {
            Vec::new()
        }
    });
    let stream = query_with_transport("Hi", ClaudeAgentOptions::default(), transport.clone());
    let messages = drain(stream).await;
    assert_eq!(result_texts(&messages), ["Hello!"]);
    assert!(transport.input_ended());
}

#[tokio::test(start_paused = true)]
async fn in_process_server_of_an_unknown_kind_is_a_config_error() {
    let options = ClaudeAgentOptions::builder()
        .mcp_server(
            "mystery",
            McpSdkServerConfig::new("mystery", McpSdkServerInstance::new(41_u32)),
        )
        .build()
        .unwrap();
    let transport = silent_cli();
    let messages = drain(query_with_transport("Hi", options, transport.clone())).await;
    let [Err(Error::Config { message })] = &messages[..] else {
        panic!("expected one configuration error, got {messages:?}");
    };
    assert!(message.contains("'mystery'"), "{message}");
    assert_eq!(transport.connect_count(), 0);
}

#[cfg(feature = "jsonschema")]
mod schema_validation {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn raw_schema_constraints_are_enforced() {
        let calls = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&calls);
        let validate = SdkMcpTool::raw(
            "validate",
            "Validate input",
            json!({
                "type": "object",
                "properties": {"name": {"type": "string", "minLength": 2}},
                "required": ["name"],
            }),
            move |_| {
                counted.fetch_add(1, Ordering::SeqCst);
                async { Ok(ToolResult::text("OK")) }
            },
        )
        .unwrap();
        let client = SdkMcpClient::connected(server([validate])).await;

        let too_short = client
            .call_tool("srv", "validate", json!({"name": "x"}))
            .await;
        assert_eq!(too_short["isError"], true);
        assert!(texts(&too_short)[0].starts_with("Input validation error: "));

        let missing = client.call_tool("srv", "validate", json!({})).await;
        assert_eq!(missing["isError"], true);
        assert!(texts(&missing)[0].starts_with("Input validation error: "));
        assert!(texts(&missing)[0].contains("name"));
        assert_eq!(calls.load(Ordering::SeqCst), 0);

        let fine = client
            .call_tool("srv", "validate", json!({"name": "xy"}))
            .await;
        assert_eq!(texts(&fine), ["OK"]);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn schema_the_validator_cannot_use_is_an_error_result() {
        let broken = SdkMcpTool::raw(
            "broken",
            "Has an invalid schema",
            json!({"type": "object", "properties": {"x": {"type": "bogus"}}}),
            |_| async { Ok(ToolResult::text("unreachable")) },
        )
        .unwrap();
        let client = SdkMcpClient::connected(server([broken])).await;
        let outcome = client.call_tool("srv", "broken", json!({"x": 1})).await;
        assert_eq!(outcome["isError"], true);
        assert!(texts(&outcome)[0].contains("bogus"), "{outcome}");
    }
}
