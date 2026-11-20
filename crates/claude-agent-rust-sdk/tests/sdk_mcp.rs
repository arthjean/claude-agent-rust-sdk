#![cfg(feature = "testing")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use claude_agent_rust_sdk::ErrorKind;
use claude_agent_rust_sdk::{
    BoxError, ClaudeAgentOptions, ClaudeAgentOptionsBuilder, McpSdkServerConfig, McpServerConfig,
    SdkMcpTool, SubprocessTransport, ToolAnnotations, ToolContent, ToolResult,
    create_sdk_mcp_server, query_with_transport,
};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::{Barrier, Notify};

use common::capture::Warnings;
use common::mcp::{PROTOCOL_VERSION, SdkMcpClient, initialize_params, texts};
use common::mock::{
    Recorder, assistant, cli_answering, drain, result, result_texts, settle, silent_cli, success,
};
use common::{Fake, SCENARIO_VARIABLE, fake_claude, next_message};

const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);
const MAX_MCP_OUTPUT_TOKENS: &str = "MAX_MCP_OUTPUT_TOKENS";

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
fn typed_tool_maps_text_integer_float_and_boolean_fields_and_requires_them_all() {
    #[derive(Deserialize, JsonSchema)]
    #[allow(dead_code)]
    struct AllTypes {
        name: String,
        count: i64,
        score: f64,
        active: bool,
    }

    let tool = SdkMcpTool::typed::<AllTypes>("all", "Takes every basic type", |_args| async {
        Ok(ToolResult::default())
    });
    let schema = tool.input_schema();
    assert_eq!(schema["type"], "object");
    assert_eq!(schema["properties"]["name"], json!({"type": "string"}));
    assert_eq!(schema["properties"]["count"]["type"], "integer");
    assert_eq!(schema["properties"]["score"]["type"], "number");
    assert_eq!(schema["properties"]["active"], json!({"type": "boolean"}));
    let mut required: Vec<&str> = schema["required"]
        .as_array()
        .unwrap()
        .iter()
        .map(|name| name.as_str().unwrap())
        .collect();
    required.sort_unstable();
    assert_eq!(required, ["active", "count", "name", "score"]);
}

#[test]
fn typed_tool_keeps_the_description_of_an_optional_field_and_leaves_it_out_of_required() {
    #[derive(Deserialize, JsonSchema)]
    #[allow(dead_code)]
    struct Config {
        /// Config name
        name: String,
        /// Timeout in seconds
        timeout: Option<i64>,
    }

    let tool = SdkMcpTool::typed::<Config>("configure", "Configures", |_args| async {
        Ok(ToolResult::default())
    });
    let schema = tool.input_schema();
    assert_eq!(
        schema["properties"]["name"],
        json!({"type": "string", "description": "Config name"})
    );
    let timeout = &schema["properties"]["timeout"];
    assert_eq!(timeout["description"], "Timeout in seconds");
    assert_eq!(timeout["type"], json!(["integer", "null"]));
    assert_eq!(schema["required"], json!(["name"]));
}

#[test]
fn typed_tool_whose_argument_struct_has_no_fields_lists_an_object_schema_requiring_nothing() {
    #[derive(Deserialize, JsonSchema)]
    struct Empty {}

    let tool = SdkMcpTool::typed::<Empty>("empty", "Takes nothing", |_args| async {
        Ok(ToolResult::default())
    });
    let schema = tool.input_schema();
    assert_eq!(schema["type"], "object");
    assert!(schema.get("required").is_none());
    assert_eq!(schema.get("properties").unwrap_or(&json!({})), &json!({}));
}

#[test]
fn typed_tool_whose_argument_type_is_not_one_object_lists_an_object_schema() {
    #[derive(Deserialize, JsonSchema)]
    #[allow(dead_code)]
    struct Point {
        x: i32,
    }

    #[derive(Deserialize, JsonSchema)]
    #[serde(tag = "kind")]
    #[allow(dead_code)]
    enum Shape {
        Circle { radius: f64 },
        Square { side: f64 },
    }

    #[derive(Deserialize, JsonSchema)]
    enum Mode {
        Fast,
        Slow,
    }

    fn schema<Args>() -> Value
    where
        Args: JsonSchema + serde::de::DeserializeOwned + Send + 'static,
    {
        SdkMcpTool::typed::<Args>("tool", "", |_args| async { Ok(ToolResult::default()) })
            .input_schema()
            .clone()
    }

    let empty = json!({"type": "object", "properties": {}});
    let shape = schema::<Shape>();
    assert_eq!(shape["type"], "object");
    assert!(shape.get("oneOf").is_none(), "{shape}");
    assert_eq!(schema::<Mode>(), empty);
    assert_eq!(schema::<String>(), empty);
    assert_eq!(schema::<Vec<Point>>(), empty);

    let nullable = schema::<Option<Point>>();
    assert_eq!(nullable["type"], "object");
    assert_eq!(nullable["properties"]["x"]["type"], "integer");
    assert_eq!(nullable["required"], json!(["x"]));

    let map = schema::<std::collections::HashMap<String, i32>>();
    assert_eq!(map["type"], "object");
    assert_eq!(map["additionalProperties"]["type"], "integer");
}

#[test]
fn typed_tool_of_an_enum_lists_every_field_of_every_variant_in_one_object() {
    #[derive(Deserialize, JsonSchema)]
    #[serde(tag = "kind")]
    #[allow(dead_code)]
    enum Tagged {
        /// Variant A.
        A {
            a: i32,
            shared: String,
        },
        B {
            b: String,
            shared: String,
        },
    }

    #[derive(Deserialize, JsonSchema)]
    #[allow(dead_code)]
    enum External {
        Circle { radius: f64 },
        Square { side: f64 },
    }

    #[derive(Deserialize, JsonSchema)]
    #[allow(dead_code)]
    struct Flattened {
        id: String,
        #[serde(flatten)]
        shape: Tagged,
    }

    let tagged =
        SdkMcpTool::typed::<Tagged>("tagged", "", |_args| async { Ok(ToolResult::default()) });
    let schema = tagged.input_schema();
    assert_eq!(schema["type"], "object");
    for combinator in ["oneOf", "anyOf", "allOf"] {
        assert!(schema.get(combinator).is_none(), "{schema}");
    }
    let properties = schema["properties"].as_object().unwrap();
    assert_eq!(
        properties.keys().collect::<Vec<_>>(),
        ["kind", "a", "shared", "b"]
    );
    assert_eq!(
        properties["kind"],
        json!({"type": "string", "enum": ["A", "B"]})
    );
    assert_eq!(properties["a"]["type"], "integer");
    assert_eq!(schema["required"], json!(["kind", "shared"]));

    let external =
        SdkMcpTool::typed::<External>("external", "", |_args| async { Ok(ToolResult::default()) });
    let schema = external.input_schema();
    assert_eq!(schema["type"], "object");
    assert!(schema.get("oneOf").is_none(), "{schema}");
    assert_eq!(
        schema["properties"]["Circle"]["properties"]["radius"]["type"],
        "number"
    );
    assert_eq!(
        schema["properties"]["Square"]["properties"]["side"]["type"],
        "number"
    );
    assert!(schema.get("required").is_none(), "{schema}");

    let flattened = SdkMcpTool::typed::<Flattened>("flattened", "", |_args| async {
        Ok(ToolResult::default())
    });
    let schema = flattened.input_schema();
    for combinator in ["oneOf", "anyOf", "allOf"] {
        assert!(schema.get(combinator).is_none(), "{schema}");
    }
    let properties = schema["properties"].as_object().unwrap();
    assert!(
        ["id", "kind", "a", "b", "shared"]
            .iter()
            .all(|name| properties.contains_key(*name)),
        "{schema}"
    );
    assert_eq!(schema["required"], json!(["id", "kind", "shared"]));
}

#[tokio::test(start_paused = true)]
async fn typed_tool_of_an_enum_reads_each_variant_and_refuses_a_mixed_shape() {
    #[derive(Deserialize, JsonSchema)]
    #[serde(tag = "kind", deny_unknown_fields)]
    enum Tagged {
        A { a: i32 },
        B { b: String },
    }

    let tool = SdkMcpTool::typed::<Tagged>("tagged", "Tagged", |args| async move {
        Ok(ToolResult::text(match args {
            Tagged::A { a } => format!("A {a}"),
            Tagged::B { b } => format!("B {b}"),
        }))
    });
    let client = SdkMcpClient::connected(server([tool])).await;
    let a = client
        .call_tool("srv", "tagged", json!({"kind": "A", "a": 1}))
        .await;
    assert_eq!(texts(&a), ["A 1"]);
    let b = client
        .call_tool("srv", "tagged", json!({"kind": "B", "b": "x"}))
        .await;
    assert_eq!(texts(&b), ["B x"]);
    let mixed = client
        .call_tool("srv", "tagged", json!({"kind": "A", "b": "x"}))
        .await;
    assert_eq!(mixed["isError"], true, "{mixed}");
    assert!(
        texts(&mixed)[0].starts_with("Input validation error: "),
        "{mixed}"
    );
}

#[tokio::test(start_paused = true)]
async fn typed_tool_reads_an_integral_float_as_an_integer() {
    #[derive(Deserialize, JsonSchema)]
    struct Count {
        count: u32,
        offset: Option<i64>,
        ratio: f64,
        page: Vec<u8>,
    }

    let tool = SdkMcpTool::typed::<Count>("count", "Counts", |args| async move {
        Ok(ToolResult::text(format!(
            "{} {:?} {} {:?}",
            args.count, args.offset, args.ratio, args.page
        )))
    });
    let client = SdkMcpClient::connected(server([tool])).await;
    let outcome = client
        .call_tool(
            "srv",
            "count",
            json!({"count": 3.0, "offset": -2.0, "ratio": 4.0, "page": [1.0, 2]}),
        )
        .await;
    assert_eq!(outcome["isError"], false, "{outcome}");
    assert_eq!(texts(&outcome), ["3 Some(-2) 4 [1, 2]"]);

    let fractional = client
        .call_tool(
            "srv",
            "count",
            json!({"count": 3.5, "ratio": 1, "page": []}),
        )
        .await;
    assert_eq!(fractional["isError"], true, "{fractional}");
}

#[tokio::test]
async fn a_tool_is_called_directly_with_the_checks_the_server_applies() {
    assert_eq!(
        add().call(json!({"a": 1, "b": 2.5})).await,
        ToolResult::text("3.5")
    );
    let missing = add().call(json!({"a": 1})).await;
    assert!(missing.is_error);
    let [ToolContent::Text { text, .. }] = &missing.content[..] else {
        panic!("expected one text block, got {missing:?}");
    };
    assert!(text.starts_with("Input validation error: "), "{text}");

    let failing = SdkMcpTool::raw("fail", "Fails", json!({}), |_| async {
        Err::<ToolResult, BoxError>("no luck".into())
    })
    .unwrap();
    assert_eq!(failing.call(json!({})).await, ToolResult::error("no luck"));

    let panicking = SdkMcpTool::raw("boom", "Panics", json!({}), |_| async {
        panic!("boom");
    })
    .unwrap();
    assert_eq!(
        panicking.call(json!({})).await,
        ToolResult::error("Tool 'boom' panicked")
    );
}

#[test]
fn raw_schema_that_is_not_a_json_object_is_a_config_error() {
    for schema in [json!("object"), json!(["a"]), json!(true), Value::Null] {
        let outcome = SdkMcpTool::raw("broken", "Bad schema", schema.clone(), |_| async {
            Ok(ToolResult::default())
        });
        let Err(error) = outcome else {
            panic!("expected a configuration error for {schema}");
        };
        assert_eq!(error.kind(), ErrorKind::Config);
        let message = error.to_string();
        assert!(message.contains("broken"), "{message}");
    }
}

#[test]
fn raw_schema_without_a_type_is_listed_as_an_object_schema() {
    let cases = [
        (json!({}), json!({"type": "object"})),
        (
            json!({"properties": {"q": {"type": "string"}}, "required": ["q"]}),
            json!({"type": "object", "properties": {"q": {"type": "string"}}, "required": ["q"]}),
        ),
        (
            json!({"type": ["object", "null"], "properties": {}}),
            json!({"type": "object", "properties": {}}),
        ),
    ];
    for (given, listed) in cases {
        let tool = SdkMcpTool::raw("t", "d", given.clone(), |_| async {
            Ok(ToolResult::default())
        })
        .unwrap();
        assert_eq!(tool.input_schema(), &listed, "{given}");
        let keys: Vec<_> = tool.to_wire()["inputSchema"]
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect();
        assert_eq!(keys[0], "type", "the type comes first: {given}");
    }
    let written = json!({"type": "object", "additionalProperties": false});
    let tool = SdkMcpTool::raw("t", "d", written.clone(), |_| async {
        Ok(ToolResult::default())
    })
    .unwrap();
    assert_eq!(
        tool.input_schema(),
        &written,
        "an object schema is kept as written"
    );
}

#[test]
fn raw_schema_whose_type_is_not_object_is_a_config_error() {
    for schema in [
        json!({"type": "string"}),
        json!({"type": ["string", "null"]}),
        json!({"type": 3}),
    ] {
        let Err(error) = SdkMcpTool::raw("scalar", "d", schema.clone(), |_| async {
            Ok(ToolResult::default())
        }) else {
            panic!("expected a configuration error for {schema}");
        };
        assert_eq!(error.kind(), ErrorKind::Config);
        assert!(
            error.to_string().contains("\"type\": \"object\""),
            "{error}"
        );
        assert!(error.to_string().contains("scalar"), "{error}");
    }
}

#[test]
fn two_tools_with_the_same_name_are_a_config_error() {
    let outcome = create_sdk_mcp_server("calc", None, [add(), noop("other"), add()]);
    let Err(error) = outcome else {
        panic!("expected a configuration error");
    };
    assert_eq!(error.kind(), ErrorKind::Config);
    let message = error.to_string();
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
    assert_eq!(
        result["capabilities"],
        json!({"experimental": {}, "tools": {"listChanged": false}})
    );

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
async fn arguments_that_are_not_an_object_are_jsonrpc_invalid_params_as_upstream() {
    let calls = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&calls);
    let echo = SdkMcpTool::raw("echo", "Echo", json!({}), move |_| {
        counted.fetch_add(1, Ordering::SeqCst);
        async { Ok(ToolResult::text("ran")) }
    })
    .unwrap();
    let client = SdkMcpClient::connected(server([echo])).await;
    for arguments in [json!("x"), json!([1]), json!(3), json!(true)] {
        let response = client
            .request(
                "srv",
                "tools/call",
                json!({"name": "echo", "arguments": arguments}),
            )
            .await;
        assert_eq!(response["error"]["code"], -32602, "{arguments}: {response}");
        assert!(response.get("result").is_none(), "{response}");
    }
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    for call in [
        json!({"name": "echo", "arguments": null}),
        json!({"name": "echo"}),
    ] {
        let response = client.request("srv", "tools/call", call).await;
        assert_eq!(texts(&response["result"]), ["ran"], "{response}");
    }
}

#[tokio::test(start_paused = true)]
async fn initialize_without_its_required_params_is_jsonrpc_invalid_params_as_upstream() {
    let client = SdkMcpClient::connected(server([])).await;
    let complete = initialize_params();
    let mut without_client = complete.clone();
    without_client
        .as_object_mut()
        .unwrap()
        .shift_remove("clientInfo");
    let mut without_version = complete.clone();
    without_version["clientInfo"]
        .as_object_mut()
        .unwrap()
        .shift_remove("version");
    for params in [json!({}), Value::Null, without_client, without_version] {
        let response = client.request("srv", "initialize", params.clone()).await;
        assert_eq!(response["error"]["code"], -32602, "{params}: {response}");
    }
    let response = client.request("srv", "initialize", complete).await;
    assert_eq!(
        response["result"]["serverInfo"]["name"], "srv",
        "{response}"
    );
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

#[tokio::test]
async fn an_annotation_key_the_crate_does_not_model_is_listed_and_a_modeled_one_wins() {
    let annotated = noop("annotated").annotations(
        ToolAnnotations::new()
            .read_only_hint(true)
            .extra_field("futureHint", true)
            .extra_field("readOnlyHint", false),
    );
    let client = SdkMcpClient::connected(server([annotated])).await;
    let tools = client.list_tools("srv").await;
    assert_eq!(
        tools[0]["annotations"],
        json!({"readOnlyHint": true, "futureHint": true})
    );
}

#[tokio::test(start_paused = true)]
async fn tools_list_wire_format_lists_what_upstream_lists() {
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
    assert_eq!(keys, ["name", "description", "inputSchema"]);
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
    assert_eq!(tool("only_size")["annotations"], json!({}));
    assert_eq!(
        tool("undescribed"),
        json!({"name": "undescribed", "description": "", "inputSchema": {"type": "object"}})
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
async fn typed_tool_whose_argument_type_has_no_object_schema_lists_an_empty_object_schema() {
    let received = Recorder::new();
    let opaque = {
        let received = received.clone();
        SdkMcpTool::typed::<Value>("opaque", "Takes anything", move |arguments| {
            received.push(arguments);
            async { Ok(ToolResult::text("ok")) }
        })
    };
    let client = SdkMcpClient::connected(server([opaque])).await;
    let listed = client.list_tools("srv").await;
    let outcome = client
        .call_tool("srv", "opaque", json!({"whatever": 1}))
        .await;

    assert_eq!(
        listed[0]["inputSchema"],
        json!({"type": "object", "properties": {}})
    );
    assert_eq!(texts(&outcome), ["ok"]);
    assert_eq!(outcome["isError"], false);
    assert_eq!(received.all(), [json!({"whatever": 1})]);
}

#[tokio::test(start_paused = true)]
async fn tool_results_convert_each_content_type() {
    let (warnings, _guard) = Warnings::install();
    let mixed = ToolResult::new([
        ToolContent::text("plain"),
        ToolContent::image("aW1hZ2U=", "image/png"),
        ToolContent::resource_link_parts(
            Some("Report".to_owned()),
            Some("file:///report.pdf".to_owned()),
            Some("Quarterly report".to_owned()),
        ),
        ToolContent::resource_link("Data", "file:///data.csv"),
        ToolContent::resource_link_parts(None, None, None),
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
async fn control_cancel_request_lets_the_tool_finish_and_keeps_its_id_reserved_until_then() {
    let started = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let writes = Arc::new(std::sync::Mutex::new(Vec::new()));
    let two_writes = {
        let started = Arc::clone(&started);
        let release = Arc::clone(&release);
        let writes = Arc::clone(&writes);
        SdkMcpTool::raw("two_writes", "Writes twice", json!({}), move |_| {
            let started = Arc::clone(&started);
            let release = Arc::clone(&release);
            let writes = Arc::clone(&writes);
            async move {
                writes.lock().unwrap().push("first");
                started.notify_one();
                release.notified().await;
                writes.lock().unwrap().push("second");
                Ok(ToolResult::text("late"))
            }
        })
        .unwrap()
    };
    let client = SdkMcpClient::connected(server([two_writes, noop("fast")])).await;
    let call = json!({
        "jsonrpc": "2.0",
        "id": 7,
        "method": "tools/call",
        "params": {"name": "two_writes", "arguments": {}},
    });
    let abandoned = client.post("srv", call.clone());
    started.notified().await;
    client.transport.push_frame(json!({
        "type": "control_cancel_request",
        "request_id": abandoned,
    }));
    for _ in 0..10 {
        settle().await;
    }

    let reused = client.send("srv", call.clone()).await;
    assert_eq!(reused["id"], 7);
    assert_eq!(
        reused["error"]["message"],
        "Request id 7 is already in flight"
    );
    assert_eq!(*writes.lock().unwrap(), ["first"]);

    release.notify_one();
    for _ in 0..10 {
        settle().await;
    }
    assert_eq!(*writes.lock().unwrap(), ["first", "second"]);
    assert!(client.answered(&abandoned).is_none());
    let fast = client
        .request(
            "srv",
            "tools/call",
            json!({"name": "fast", "arguments": {}}),
        )
        .await;
    assert_eq!(fast["result"]["isError"], false);

    let again = client.post("srv", call);
    started.notified().await;
    release.notify_one();
    assert_eq!(texts(&client.answer(&again).await["result"]), ["late"]);
    assert_eq!(
        *writes.lock().unwrap(),
        ["first", "second", "first", "second"]
    );
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
async fn closing_ends_every_server_and_the_tool_call_in_flight_and_closing_again_does_nothing() {
    let started = Arc::new(Notify::new());
    let dropped = Arc::new(AtomicUsize::new(0));
    let client = SdkMcpClient::start([
        (
            "one",
            create_sdk_mcp_server("one", None, [blocked("slow", &started, &dropped)]).unwrap(),
        ),
        ("two", create_sdk_mcp_server("two", None, []).unwrap()),
    ])
    .await;
    assert_eq!(client.initialize("one").await["serverInfo"]["name"], "one");
    assert_eq!(client.initialize("two").await["serverInfo"]["name"], "two");

    let in_flight = client.post(
        "one",
        json!({
            "jsonrpc": "2.0",
            "id": 50,
            "method": "tools/call",
            "params": {"name": "slow", "arguments": {}},
        }),
    );
    started.notified().await;
    let runtime = tokio::runtime::Handle::current().metrics();
    assert!(
        runtime.num_alive_tasks() >= 2,
        "the reader and the call run"
    );

    let before = tokio::time::Instant::now();
    client.close().await;
    assert!(before.elapsed() < SHUTDOWN_GRACE, "{:?}", before.elapsed());
    for _ in 0..10 {
        settle().await;
    }
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
    assert!(client.answered(&in_flight).is_none());
    assert_eq!(runtime.num_alive_tasks(), 0);
    assert_eq!(client.transport.close_count(), 1);

    client.close().await;
    assert_eq!(client.transport.close_count(), 1);
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
    assert_eq!(runtime.num_alive_tasks(), 0);
}

async fn child_environment(options: ClaudeAgentOptionsBuilder, fake: &Fake) -> Value {
    let transport = SubprocessTransport::new(
        options
            .cli_path(fake_claude())
            .env_var(SCENARIO_VARIABLE, fake.scenario.to_string_lossy())
            .build()
            .unwrap(),
    );
    transport.connect().await.unwrap();
    let mut stream = transport.read_messages();
    let reported = next_message(&mut stream).await["fake_env"].clone();
    transport.close().await.unwrap();
    reported
}

fn inherited(name: &str) -> Value {
    json!(std::env::var(name).ok())
}

#[tokio::test]
async fn max_mcp_output_tokens_is_not_given_a_default_when_the_options_do_not_set_it() {
    let fake = Fake::new(json!({
        "report_env": [MAX_MCP_OUTPUT_TOKENS, "CLAUDE_CODE_ENTRYPOINT"],
        "steps": [{"exit": 0}],
    }));
    let reported = child_environment(ClaudeAgentOptions::builder(), &fake).await;
    assert_eq!(reported["CLAUDE_CODE_ENTRYPOINT"], "sdk-rs");
    assert_eq!(
        reported[MAX_MCP_OUTPUT_TOKENS],
        inherited(MAX_MCP_OUTPUT_TOKENS)
    );
}

#[tokio::test]
async fn raising_max_mcp_output_tokens_sets_no_tool_result_persistence_variable() {
    let persistence_variables = ["MAX_TOOL_RESULT_CHARS", "DISABLE_TOOL_RESULT_PERSISTENCE"];
    let fake = Fake::new(json!({
        "report_env": [
            MAX_MCP_OUTPUT_TOKENS,
            "MAX_TOOL_RESULT_CHARS",
            "DISABLE_TOOL_RESULT_PERSISTENCE",
        ],
        "steps": [{"exit": 0}],
    }));
    let options = ClaudeAgentOptions::builder().env_var(MAX_MCP_OUTPUT_TOKENS, "500000");
    let reported = child_environment(options, &fake).await;
    assert_eq!(reported[MAX_MCP_OUTPUT_TOKENS], "500000");
    for name in persistence_variables {
        assert_eq!(reported[name], inherited(name), "{name}");
    }
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
    async fn format_is_an_annotation_only_whatever_the_draft_of_the_schema() {
        let drafts = [
            "http://json-schema.org/draft-04/schema#",
            "http://json-schema.org/draft-06/schema#",
            "http://json-schema.org/draft-07/schema#",
            "https://json-schema.org/draft/2019-09/schema",
            "https://json-schema.org/draft/2020-12/schema",
        ];
        let tools = drafts.iter().enumerate().map(|(index, draft)| {
            SdkMcpTool::raw(
                format!("when{index}"),
                "Takes a date",
                json!({
                    "$schema": draft,
                    "type": "object",
                    "properties": {
                        "when": {"type": "string", "format": "date-time"},
                        "mail": {"type": "string", "format": "email"},
                    },
                }),
                |arguments| async move { Ok(ToolResult::text(arguments["when"].to_string())) },
            )
            .unwrap()
        });
        let client = SdkMcpClient::connected(server(tools)).await;
        for (index, draft) in drafts.iter().enumerate() {
            let outcome = client
                .call_tool(
                    "srv",
                    &format!("when{index}"),
                    json!({"when": "nope", "mail": "not an address"}),
                )
                .await;
            assert_eq!(outcome["isError"], false, "{draft}: {outcome}");
            assert_eq!(texts(&outcome), [r#""nope""#], "{draft}");
            let wrong_type = client
                .call_tool("srv", &format!("when{index}"), json!({"when": 3}))
                .await;
            assert_eq!(wrong_type["isError"], true, "{draft}: {wrong_type}");
        }
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
        let healthy = SdkMcpTool::raw(
            "healthy",
            "Has a valid schema",
            json!({"type": "object"}),
            |_| async { Ok(ToolResult::text("still serving")) },
        )
        .unwrap();
        let client = SdkMcpClient::connected(server([broken, healthy])).await;
        let outcome = client.call_tool("srv", "broken", json!({"x": 1})).await;
        assert_eq!(outcome["isError"], true);
        assert!(texts(&outcome)[0].contains("bogus"), "{outcome}");

        let again = client.call_tool("srv", "broken", json!({"x": 1})).await;
        assert_eq!(again["isError"], true);
        let next = client.call_tool("srv", "healthy", json!({})).await;
        assert_eq!(texts(&next), ["still serving"]);
    }

    #[tokio::test(start_paused = true)]
    async fn remote_schema_reference_is_an_error_result_naming_it_without_a_request() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let reference = format!("http://{address}/schema.json");
        let calls = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&calls);
        let remote = SdkMcpTool::raw(
            "remote",
            "Points at a remote schema",
            json!({"type": "object", "properties": {"x": {"$ref": reference}}}),
            move |_| {
                counted.fetch_add(1, Ordering::SeqCst);
                async { Ok(ToolResult::text("unreachable")) }
            },
        )
        .unwrap();
        let client = SdkMcpClient::connected(server([remote])).await;
        let outcome = client.call_tool("srv", "remote", json!({"x": 1})).await;
        assert_eq!(outcome["isError"], true);
        assert!(texts(&outcome)[0].contains(&reference), "{outcome}");
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert!(
            listener.accept().is_err(),
            "the validator connected to {reference}"
        );
    }
}

#[cfg(not(feature = "jsonschema"))]
#[tokio::test(start_paused = true)]
async fn raw_tool_arguments_reach_the_handler_unvalidated_without_the_jsonschema_feature() {
    let calls = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&calls);
    let echo = SdkMcpTool::raw(
        "echo",
        "Requires b",
        json!({"type": "object", "properties": {"b": {"type": "string"}}, "required": ["b"]}),
        move |arguments| {
            counted.fetch_add(1, Ordering::SeqCst);
            async move { Ok(ToolResult::text(arguments.to_string())) }
        },
    )
    .unwrap();
    let client = SdkMcpClient::connected(server([echo])).await;
    let outcome = client.call_tool("srv", "echo", json!({"a": 1})).await;
    assert_ne!(outcome["isError"], true, "{outcome}");
    assert_eq!(texts(&outcome), [r#"{"a":1}"#]);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[cfg(feature = "rmcp")]
mod rmcp_servers {
    use rmcp::model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, InitializeRequestParams,
        InitializeResult, ListToolsResult, PaginatedRequestParams, ServerConfig,
    };
    use rmcp::service::RequestContext;
    use rmcp::{ErrorData as McpError, RoleServer, ServerHandler};
    use serde::de::DeserializeOwned;

    use super::*;

    fn model<T: DeserializeOwned>(value: Value) -> T {
        serde_json::from_value(value).unwrap()
    }

    #[derive(Clone, Default)]
    struct Threaded {
        initializations: Arc<AtomicUsize>,
        started: Arc<Notify>,
        release: Arc<Notify>,
        finished: Arc<AtomicUsize>,
        cancelled: Arc<AtomicUsize>,
    }

    impl Threaded {
        fn config(&self) -> McpServerConfig {
            McpServerConfig::sdk_rmcp("threaded", self.clone())
        }
    }

    impl ServerHandler for Threaded {
        fn get_info(&self) -> ServerConfig {
            model(json!({
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "threaded", "version": "0.1.0"},
            }))
        }

        async fn initialize(
            &self,
            request: InitializeRequestParams,
            context: RequestContext<RoleServer>,
        ) -> Result<InitializeResult, McpError> {
            self.initializations.fetch_add(1, Ordering::SeqCst);
            context.peer.set_peer_info(request.clone());
            self.negotiate_initialize(&request)
        }

        async fn list_tools(
            &self,
            _request: Option<PaginatedRequestParams>,
            _context: RequestContext<RoleServer>,
        ) -> Result<ListToolsResult, McpError> {
            Ok(model(json!({"tools": [
                {"name": "blocking", "inputSchema": {"type": "object", "properties": {}}},
                {"name": "slow", "inputSchema": {"type": "object", "properties": {}}},
                {"name": "echo", "inputSchema": {"type": "object", "properties": {}}},
            ]})))
        }

        async fn call_tool(
            &self,
            request: CallToolRequestParams,
            context: RequestContext<RoleServer>,
        ) -> Result<CallToolResponse, McpError> {
            match request.name.as_ref() {
                "blocking" => {
                    self.started.notify_one();
                    self.release.notified().await;
                    self.finished.fetch_add(1, Ordering::SeqCst);
                }
                "slow" => {
                    self.started.notify_one();
                    context.ct.cancelled().await;
                    self.cancelled.fetch_add(1, Ordering::SeqCst);
                    return Err(McpError::internal_error("cancelled by the client", None));
                }
                _ => {}
            }
            let result: CallToolResult = model(json!({
                "content": [{"type": "text", "text": request.name}],
                "isError": false,
            }));
            Ok(result.into())
        }
    }

    fn call(id: u64, name: &str) -> Value {
        json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "tools/call",
            "params": {"name": name, "arguments": {}},
        })
    }

    #[tokio::test(start_paused = true)]
    async fn closing_ends_every_rmcp_session_and_the_call_in_flight_within_the_grace_period() {
        let one = Threaded::default();
        let two = Threaded::default();
        let client = SdkMcpClient::start([("one", one.config()), ("two", two.config())]).await;
        client.initialize("one").await;
        client.initialize("two").await;
        assert_eq!(one.initializations.load(Ordering::SeqCst), 1);
        assert_eq!(two.initializations.load(Ordering::SeqCst), 1);

        let in_flight = client.post("one", call(50, "slow"));
        one.started.notified().await;
        let runtime = tokio::runtime::Handle::current().metrics();
        assert!(
            runtime.num_alive_tasks() >= 4,
            "the reader, the call and both sessions run"
        );

        let before = tokio::time::Instant::now();
        client.close().await;
        assert!(before.elapsed() < SHUTDOWN_GRACE, "{:?}", before.elapsed());
        for _ in 0..10 {
            settle().await;
        }
        assert_eq!(one.cancelled.load(Ordering::SeqCst), 1);
        assert!(client.answered(&in_flight).is_none());
        assert_eq!(runtime.num_alive_tasks(), 0);
        assert_eq!(client.transport.close_count(), 1);

        client.close().await;
        assert_eq!(client.transport.close_count(), 1);
        assert_eq!(runtime.num_alive_tasks(), 0);
        assert_eq!(one.initializations.load(Ordering::SeqCst), 1);
        assert_eq!(two.initializations.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn message_arriving_after_close_is_not_answered_and_starts_no_new_session() {
        let threaded = Threaded::default();
        let client = SdkMcpClient::connected(threaded.config()).await;
        assert_eq!(threaded.initializations.load(Ordering::SeqCst), 1);
        client.close().await;

        let listing = client.post(
            "srv",
            json!({"jsonrpc": "2.0", "id": 9, "method": "tools/list"}),
        );
        let handshake = client.post(
            "srv",
            json!({
                "jsonrpc": "2.0",
                "id": 10,
                "method": "initialize",
                "params": initialize_params(),
            }),
        );
        for _ in 0..10 {
            settle().await;
        }
        assert!(client.answered(&listing).is_none());
        assert!(client.answered(&handshake).is_none());
        assert_eq!(threaded.initializations.load(Ordering::SeqCst), 1);
        let runtime = tokio::runtime::Handle::current().metrics();
        assert_eq!(runtime.num_alive_tasks(), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn cancelled_call_of_an_rmcp_server_is_answered_with_request_cancelled() {
        let threaded = Threaded::default();
        let client = SdkMcpClient::connected(threaded.config()).await;
        let slow = client.post("srv", call(31, "slow"));
        threaded.started.notified().await;

        let acknowledgement = client
            .notify(
                "srv",
                "notifications/cancelled",
                json!({"requestId": 31, "reason": "user interrupted"}),
            )
            .await;
        assert_eq!(acknowledgement, json!({"jsonrpc": "2.0", "result": {}}));

        let reply = client.answer(&slow).await;
        assert_eq!(
            reply,
            json!({
                "jsonrpc": "2.0",
                "id": 31,
                "error": {"code": -32800, "message": "Request cancelled"},
            })
        );
        while threaded.cancelled.load(Ordering::SeqCst) == 0 {
            settle().await;
        }
        let echoed = client.call_tool("srv", "echo", json!({})).await;
        assert_eq!(texts(&echoed), ["echo"]);
        client.close().await;
    }

    #[tokio::test(start_paused = true)]
    async fn rmcp_server_keeps_serving_after_a_cancelled_call_that_ignores_its_cancellation() {
        let (warnings, _guard) = Warnings::install();
        let threaded = Threaded::default();
        let client = SdkMcpClient::connected(threaded.config()).await;
        let blocking = client.post("srv", call(21, "blocking"));
        threaded.started.notified().await;

        let acknowledgement = client
            .notify(
                "srv",
                "notifications/cancelled",
                json!({"requestId": 21, "reason": "t"}),
            )
            .await;
        assert_eq!(acknowledgement, json!({"jsonrpc": "2.0", "result": {}}));
        for _ in 0..50 {
            settle().await;
        }
        threaded.release.notify_one();
        while threaded.finished.load(Ordering::SeqCst) == 0 {
            settle().await;
        }
        for _ in 0..50 {
            settle().await;
        }

        let echoed = client.call_tool("srv", "echo", json!({})).await;
        assert_eq!(texts(&echoed), ["echo"]);
        assert_eq!(warnings.all(), Vec::<String>::new());

        let reply = client.answer(&blocking).await;
        assert_eq!(reply["id"], 21);
        assert_eq!(reply["error"]["code"], -32800);
        assert_eq!(reply["error"]["message"], "Request cancelled");
        assert!(reply.get("result").is_none());

        let reused = client.send("srv", call(21, "echo")).await;
        assert_eq!(reused["id"], 21);
        assert_eq!(texts(&reused["result"]), ["echo"]);
        assert_eq!(warnings.all(), Vec::<String>::new());
        client.close().await;
    }
}
