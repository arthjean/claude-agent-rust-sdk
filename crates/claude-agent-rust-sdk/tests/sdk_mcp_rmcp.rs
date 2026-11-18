#![cfg(all(feature = "testing", feature = "rmcp"))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use claude_agent_rust_sdk::McpServerConfig;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, InitializeRequestParams,
    InitializeResult, ListResourcesResult, ListToolsResult, PaginatedRequestParams, ServerConfig,
    ServerRequest,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData as McpError, RoleServer, ServerHandler};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use tokio::sync::Notify;

use common::mcp::{PROTOCOL_VERSION, SdkMcpClient, initialize_params, texts};
use common::mock::settle;

fn model<T: DeserializeOwned>(value: Value) -> T {
    serde_json::from_value(value).unwrap()
}

fn text_result(text: impl Into<String>) -> Result<CallToolResponse, McpError> {
    let result: CallToolResult = model(json!({
        "content": [{"type": "text", "text": text.into()}],
        "isError": false,
    }));
    Ok(result.into())
}

#[derive(Clone, Default)]
struct Notes {
    initializations: Arc<AtomicUsize>,
    fail_initialize: Arc<AtomicBool>,
    started: Arc<Notify>,
    cancelled: Arc<AtomicUsize>,
    gates: Arc<Mutex<HashMap<String, Arc<Notify>>>>,
}

impl Notes {
    fn gate(&self, name: &str) -> Arc<Notify> {
        Arc::clone(
            self.gates
                .lock()
                .unwrap()
                .entry(name.to_owned())
                .or_default(),
        )
    }

    fn config(&self) -> McpServerConfig {
        McpServerConfig::sdk_rmcp("notes", self.clone())
    }
}

impl ServerHandler for Notes {
    fn get_info(&self) -> ServerConfig {
        model(json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {"tools": {}, "resources": {}},
            "serverInfo": {"name": "notes", "version": "0.3.0"},
        }))
    }

    async fn initialize(
        &self,
        request: InitializeRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<InitializeResult, McpError> {
        self.initializations.fetch_add(1, Ordering::SeqCst);
        if self.fail_initialize.load(Ordering::SeqCst) {
            return Err(McpError::internal_error("the index is still loading", None));
        }
        context.peer.set_peer_info(request.clone());
        self.negotiate_initialize(&request)
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        Ok(model(json!({"tools": [
            {"name": "echo", "description": "Echo the arguments", "inputSchema": {"type": "object"}},
        ]})))
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, McpError> {
        Ok(model(json!({"resources": [
            {"uri": "file:///notes.txt", "name": "notes"},
        ]})))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        let arguments = Value::Object(request.arguments.unwrap_or_default());
        match request.name.as_ref() {
            "echo" => text_result(arguments.to_string()),
            "gate" => {
                let name = arguments["name"].as_str().unwrap_or_default().to_owned();
                self.gate(&name).notified().await;
                text_result(name)
            }
            "announce" => {
                context.peer.notify_tool_list_changed().await.unwrap();
                text_result("announced")
            }
            "ping" => {
                let ping: ServerRequest = model(json!({"method": "ping"}));
                match context.peer.send_request(ping).await {
                    Ok(_) => text_result("pong"),
                    Err(error) => text_result(format!("no pong: {error}")),
                }
            }
            "roots" => {
                #[allow(deprecated)]
                let roots = context.peer.list_roots().await;
                match roots {
                    Ok(_) => text_result("served"),
                    Err(error) => text_result(format!("refused: {error}")),
                }
            }
            "slow" => {
                self.started.notify_one();
                context.ct.cancelled().await;
                self.cancelled.fetch_add(1, Ordering::SeqCst);
                Err(McpError::internal_error("cancelled by the client", None))
            }
            other => Err(McpError::invalid_params(
                format!("unknown tool {other}"),
                None,
            )),
        }
    }
}

fn call(id: u64, name: &str, arguments: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "tools/call",
        "params": {"name": name, "arguments": arguments},
    })
}

#[tokio::test(start_paused = true)]
async fn rmcp_handler_is_served_with_everything_it_implements() {
    let notes = Notes::default();
    let client = SdkMcpClient::start([("srv", notes.config())]).await;
    let info = client.initialize("srv").await;
    assert_eq!(info["serverInfo"]["name"], "notes");
    assert_eq!(info["serverInfo"]["version"], "0.3.0");
    assert_eq!(info["protocolVersion"], PROTOCOL_VERSION);

    let tools = client.list_tools("srv").await;
    assert_eq!(tools[0]["name"], "echo");

    let echoed = client.call_tool("srv", "echo", json!({"text": "hi"})).await;
    assert_eq!(texts(&echoed), [r#"{"text":"hi"}"#]);

    let resources = client.request("srv", "resources/list", json!({})).await;
    assert_eq!(
        resources["result"]["resources"][0]["uri"],
        "file:///notes.txt"
    );
    client.close().await;
}

#[tokio::test(start_paused = true)]
async fn each_query_runs_one_session_started_by_its_first_message() {
    let notes = Notes::default();
    let first = SdkMcpClient::start([("srv", notes.config())]).await;
    let second = SdkMcpClient::start([("srv", notes.config())]).await;
    for _ in 0..10 {
        settle().await;
    }
    assert_eq!(notes.initializations.load(Ordering::SeqCst), 0);

    first.initialize("srv").await;
    assert_eq!(notes.initializations.load(Ordering::SeqCst), 1);
    first.list_tools("srv").await;
    first.call_tool("srv", "echo", json!({})).await;
    assert_eq!(notes.initializations.load(Ordering::SeqCst), 1);

    second.initialize("srv").await;
    assert_eq!(notes.initializations.load(Ordering::SeqCst), 2);
    first.close().await;
    assert_eq!(second.list_tools("srv").await[0]["name"], "echo");
    second.close().await;
}

#[tokio::test(start_paused = true)]
async fn repeated_handshake_is_served_by_the_same_session() {
    let notes = Notes::default();
    let client = SdkMcpClient::connected(notes.config()).await;
    let held = client.post("srv", call(40, "gate", json!({"name": "held"})));
    for _ in 0..10 {
        settle().await;
    }

    let again = client.initialize("srv").await;
    assert_eq!(again["serverInfo"]["name"], "notes");
    let echoed = client.call_tool("srv", "echo", json!({"text": "hi"})).await;
    assert_eq!(texts(&echoed), [r#"{"text":"hi"}"#]);
    assert!(client.answered(&held).is_none());

    notes.gate("held").notify_one();
    let held = client.answer(&held).await;
    assert_eq!(texts(&held["result"]), ["held"]);
    assert_eq!(client.list_tools("srv").await[0]["name"], "echo");
    client.close().await;
}

#[tokio::test(start_paused = true)]
async fn responses_are_matched_to_requests_by_id() {
    let notes = Notes::default();
    let client = SdkMcpClient::connected(notes.config()).await;
    let first = client.post("srv", call(10, "gate", json!({"name": "first"})));
    let second = client.post("srv", call(11, "gate", json!({"name": "second"})));
    for _ in 0..10 {
        settle().await;
    }
    assert!(client.answered(&first).is_none());

    notes.gate("second").notify_one();
    let second = client.answer(&second).await;
    assert_eq!(second["id"], 11);
    assert_eq!(texts(&second["result"]), ["second"]);
    assert!(client.answered(&first).is_none());

    notes.gate("first").notify_one();
    let first = client.answer(&first).await;
    assert_eq!(first["id"], 10);
    assert_eq!(texts(&first["result"]), ["first"]);
    client.close().await;
}

#[tokio::test(start_paused = true)]
async fn request_id_in_flight_is_refused_and_a_late_response_is_dropped() {
    let notes = Notes::default();
    let client = SdkMcpClient::connected(notes.config()).await;
    let gated = call(20, "gate", json!({"name": "late"}));
    let abandoned = client.post("srv", gated.clone());
    for _ in 0..10 {
        settle().await;
    }

    let duplicate = client.send("srv", gated.clone()).await;
    assert_eq!(duplicate["id"], 20);
    assert_eq!(duplicate["error"]["code"], -32603);
    assert_eq!(
        duplicate["error"]["message"],
        "Request id 20 is already in flight"
    );

    client.transport.push_frame(json!({
        "type": "control_cancel_request",
        "request_id": abandoned,
    }));
    for _ in 0..10 {
        settle().await;
    }
    let still_reserved = client.send("srv", gated.clone()).await;
    assert_eq!(still_reserved["error"]["code"], -32603);

    notes.gate("late").notify_one();
    for _ in 0..10 {
        settle().await;
    }
    assert!(client.answered(&abandoned).is_none());

    let reused = client.post("srv", gated);
    for _ in 0..10 {
        settle().await;
    }
    notes.gate("late").notify_one();
    let reused = client.answer(&reused).await;
    assert_eq!(texts(&reused["result"]), ["late"]);
    client.close().await;
}

#[tokio::test(start_paused = true)]
async fn server_traffic_to_the_client_is_dropped_answered_or_refused() {
    let notes = Notes::default();
    let client = SdkMcpClient::connected(notes.config()).await;

    let announced = client.call_tool("srv", "announce", json!({})).await;
    assert_eq!(texts(&announced), ["announced"]);
    let frames = client.transport.written_frames();
    assert!(
        frames
            .iter()
            .all(|frame| !frame.to_string().contains("list_changed")),
        "a server notification reached the CLI"
    );

    let pong = client.call_tool("srv", "ping", json!({})).await;
    assert_eq!(texts(&pong), ["pong"]);

    let roots = client.call_tool("srv", "roots", json!({})).await;
    let [refusal] = &texts(&roots)[..] else {
        panic!("expected one text block");
    };
    assert!(refusal.starts_with("refused: "), "{refusal}");
    assert!(
        refusal.contains("-32601") || refusal.contains("not supported for SDK servers"),
        "{refusal}"
    );
    client.close().await;
}

#[tokio::test(start_paused = true)]
async fn closing_the_query_cancels_the_requests_still_running() {
    let notes = Notes::default();
    let client = SdkMcpClient::connected(notes.config()).await;
    let pending = client.post("srv", call(30, "slow", json!({})));
    notes.started.notified().await;

    let before = tokio::time::Instant::now();
    client.close().await;
    assert!(before.elapsed() <= std::time::Duration::from_secs(5));
    for _ in 0..10 {
        settle().await;
    }
    assert_eq!(notes.cancelled.load(Ordering::SeqCst), 1);
    assert!(client.answered(&pending).is_none());
}

#[tokio::test(start_paused = true)]
async fn server_that_fails_its_handshake_stays_stopped_until_a_new_initialize() {
    let notes = Notes::default();
    notes.fail_initialize.store(true, Ordering::SeqCst);
    let client = SdkMcpClient::start([("srv", notes.config())]).await;

    let refused = client
        .request("srv", "initialize", initialize_params())
        .await;
    assert!(
        refused["error"]["message"]
            .as_str()
            .unwrap()
            .contains("the index is still loading"),
        "{refused}"
    );

    for _ in 0..2 {
        let stopped = client.request("srv", "tools/list", json!({})).await;
        assert_eq!(stopped["error"]["code"], -32603);
        let message = stopped["error"]["message"].as_str().unwrap();
        assert!(
            message.starts_with("SDK MCP server 'srv' stopped: "),
            "{message}"
        );
        assert!(message.contains("the index is still loading"), "{message}");
    }

    notes.fail_initialize.store(false, Ordering::SeqCst);
    let info = client.initialize("srv").await;
    assert_eq!(info["serverInfo"]["name"], "notes");
    assert_eq!(client.list_tools("srv").await[0]["name"], "echo");
    assert_eq!(notes.initializations.load(Ordering::SeqCst), 2);
    client.close().await;
}

#[tokio::test(start_paused = true)]
async fn malformed_message_is_a_jsonrpc_error_and_the_session_survives() {
    let notes = Notes::default();
    let client = SdkMcpClient::connected(notes.config()).await;
    let response = client.send("srv", json!({"jsonrpc": "2.0", "id": 5})).await;
    assert_eq!(response["id"], 5);
    assert_eq!(response["error"]["code"], -32603);
    assert_eq!(client.list_tools("srv").await[0]["name"], "echo");

    client.close().await;
    let request_id = client.post("srv", json!({"jsonrpc": "2.0", "id": 6, "method": "ping"}));
    for _ in 0..10 {
        settle().await;
    }
    assert!(client.answered(&request_id).is_none());
}
