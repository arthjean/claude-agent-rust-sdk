use std::sync::atomic::{AtomicU64, Ordering};

use claude_agent_kit::testing::MockTransport;
use claude_agent_kit::{ClaudeAgentOptions, ClaudeSdkClient, McpServerConfig};
use serde_json::{Value, json};

use super::mock::{cli, connected, response_to, settle};

pub const PROTOCOL_VERSION: &str = "2025-06-18";

pub fn initialize_params() -> Value {
    json!({
        "protocolVersion": PROTOCOL_VERSION,
        "capabilities": {},
        "clientInfo": {"name": "claude-code", "version": "2.1.325"},
    })
}

pub fn texts(result: &Value) -> Vec<String> {
    result["content"]
        .as_array()
        .expect("the tool result has no content list")
        .iter()
        .filter(|block| block["type"] == "text")
        .map(|block| block["text"].as_str().unwrap().to_owned())
        .collect()
}

pub struct SdkMcpClient {
    pub transport: MockTransport,
    pub client: ClaudeSdkClient,
    ids: AtomicU64,
    control_ids: AtomicU64,
}

impl SdkMcpClient {
    pub async fn start<C: Into<McpServerConfig>>(
        servers: impl IntoIterator<Item = (&'static str, C)>,
    ) -> Self {
        let mut options = ClaudeAgentOptions::builder();
        for (key, server) in servers {
            options = options.mcp_server(key, server);
        }
        let transport = cli();
        let client = connected(&transport, options.build().unwrap()).await;
        Self {
            transport,
            client,
            ids: AtomicU64::new(0),
            control_ids: AtomicU64::new(0),
        }
    }

    pub async fn connected<C: Into<McpServerConfig>>(server: C) -> Self {
        let client = Self::start([("srv", server)]).await;
        client.initialize("srv").await;
        client
    }

    pub fn post(&self, server: &str, message: Value) -> String {
        let request_id = format!("mcp-{}", self.control_ids.fetch_add(1, Ordering::Relaxed));
        self.transport.push_frame(json!({
            "type": "control_request",
            "request_id": request_id,
            "request": {"subtype": "mcp_message", "server_name": server, "message": message},
        }));
        request_id
    }

    pub fn answered(&self, request_id: &str) -> Option<Value> {
        response_to(&self.transport, request_id)
    }

    pub async fn control_response(&self, request_id: &str) -> Value {
        for _ in 0..100_000 {
            if let Some(response) = self.answered(request_id) {
                return response;
            }
            settle().await;
        }
        panic!("control request {request_id} was never answered");
    }

    pub async fn answer(&self, request_id: &str) -> Value {
        let response = self.control_response(request_id).await;
        assert_eq!(response["subtype"], "success", "{response}");
        response["response"]["mcp_response"].clone()
    }

    pub async fn send(&self, server: &str, message: Value) -> Value {
        let request_id = self.post(server, message);
        self.answer(&request_id).await
    }

    pub fn next_id(&self) -> u64 {
        self.ids.fetch_add(1, Ordering::Relaxed)
    }

    pub async fn request(&self, server: &str, method: &str, params: Value) -> Value {
        let id = self.next_id();
        let response = self
            .send(
                server,
                json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}),
            )
            .await;
        assert_eq!(response["jsonrpc"], "2.0", "{response}");
        assert_eq!(response["id"], id, "{response}");
        response
    }

    pub async fn notify(&self, server: &str, method: &str, params: Value) -> Value {
        self.send(
            server,
            json!({"jsonrpc": "2.0", "method": method, "params": params}),
        )
        .await
    }

    pub async fn initialize(&self, server: &str) -> Value {
        let response = self
            .request(server, "initialize", initialize_params())
            .await;
        self.notify(server, "notifications/initialized", json!({}))
            .await;
        response["result"].clone()
    }

    pub async fn list_tools(&self, server: &str) -> Vec<Value> {
        let response = self.request(server, "tools/list", json!({})).await;
        response["result"]["tools"]
            .as_array()
            .unwrap_or_else(|| panic!("no tool list in {response}"))
            .clone()
    }

    pub async fn call_tool(&self, server: &str, name: &str, arguments: Value) -> Value {
        let response = self
            .request(
                server,
                "tools/call",
                json!({"name": name, "arguments": arguments}),
            )
            .await;
        assert!(response.get("error").is_none(), "{response}");
        response["result"].clone()
    }

    pub async fn close(&self) {
        self.client.disconnect().await.unwrap();
    }
}
