//! Routing of the `mcp_message` control requests of one session to the
//! in-process servers of its options.

use std::collections::HashMap;

use serde_json::{Value, json};

use super::server::{NativeSession, SdkMcpServer};
use crate::error::{Error, Result};
use crate::types::{McpSdkServerInstance, McpServerConfig, McpServers};

const SERVER_NOT_FOUND: i64 = -32601;
const INTERNAL_ERROR: i64 = -32603;

enum Bridge {
    Native(NativeSession),
    #[cfg(feature = "rmcp")]
    Rmcp(super::rmcp_bridge::RmcpBridge),
}

#[cfg_attr(not(feature = "rmcp"), allow(unused_variables))]
fn bridge_for(key: &str, instance: &McpSdkServerInstance) -> Option<Bridge> {
    if let Some(server) = instance.downcast_arc::<SdkMcpServer>() {
        return Some(Bridge::Native(NativeSession::new(server)));
    }
    #[cfg(feature = "rmcp")]
    if let Some(server) = instance.downcast_ref::<super::rmcp_bridge::RmcpServer>() {
        return Some(Bridge::Rmcp(super::rmcp_bridge::RmcpBridge::new(
            key.to_owned(),
            server.clone(),
        )));
    }
    None
}

/// The in-process servers of one session, keyed by the name they have in
/// the options.
#[derive(Default)]
pub(crate) struct McpBridges {
    servers: HashMap<String, Bridge>,
}

impl McpBridges {
    pub(crate) fn from_servers(servers: &McpServers) -> Result<Self> {
        let McpServers::Map(servers) = servers else {
            return Ok(Self::default());
        };
        let mut bridges = HashMap::new();
        for (key, server) in servers {
            let McpServerConfig::Sdk(config) = server else {
                continue;
            };
            let bridge = bridge_for(key, &config.instance).ok_or_else(|| {
                Error::config(format!(
                    "The in-process MCP server '{key}' is not one this crate can serve: create \
                     it with create_sdk_mcp_server or McpServerConfig::sdk_rmcp"
                ))
            })?;
            bridges.insert(key.clone(), bridge);
        }
        Ok(Self { servers: bridges })
    }

    /// Routes one JSON-RPC message to the server registered under
    /// `server_name` and returns what the CLI is answered: the response to
    /// a request, an empty acknowledgement for a notification, or a
    /// JSON-RPC error when the message could not be delivered.
    pub(crate) async fn handle(&self, server_name: &str, message: &Value) -> Value {
        let failure = |code: i64, text: String| {
            json!({
                "jsonrpc": "2.0",
                "id": message.get("id"),
                "error": {"code": code, "message": text},
            })
        };
        let Some(bridge) = self.servers.get(server_name) else {
            return failure(
                SERVER_NOT_FOUND,
                format!("Server '{server_name}' not found"),
            );
        };
        let outcome = match bridge {
            Bridge::Native(session) => session.handle(message).await,
            #[cfg(feature = "rmcp")]
            Bridge::Rmcp(bridge) => bridge.handle(message).await,
        };
        match outcome {
            Ok(Some(response)) => response,
            Ok(None) => json!({"jsonrpc": "2.0", "result": {}}),
            Err(text) => failure(INTERNAL_ERROR, text),
        }
    }

    /// Stops every server session. Calling it again does nothing.
    pub(crate) async fn close(&self) {
        #[cfg(feature = "rmcp")]
        futures_util::future::join_all(self.servers.values().map(|bridge| async move {
            if let Bridge::Rmcp(bridge) = bridge {
                bridge.close().await;
            }
        }))
        .await;
    }
}
