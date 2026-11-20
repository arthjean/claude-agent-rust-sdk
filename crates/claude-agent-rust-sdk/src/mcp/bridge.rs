//! Routing of the `mcp_message` control requests of one session to the
//! in-process servers of its options.

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::{Value, json};

use super::server::NativeSession;
use crate::error::Result;
use crate::types::{McpSdkServerInstance, McpServerConfig, McpServers, SdkServer};

const SERVER_NOT_FOUND: i64 = -32601;
const INTERNAL_ERROR: i64 = -32603;

enum Bridge {
    Native(NativeSession),
    #[cfg(feature = "rmcp")]
    Rmcp(super::rmcp_bridge::RmcpBridge),
}

#[cfg_attr(not(feature = "rmcp"), allow(unused_variables))]
fn bridge_for(key: &str, instance: &McpSdkServerInstance) -> Bridge {
    match &instance.0 {
        SdkServer::Native(server) => Bridge::Native(NativeSession::new(Arc::clone(server))),
        #[cfg(feature = "rmcp")]
        SdkServer::Rmcp(server) => Bridge::Rmcp(super::rmcp_bridge::RmcpBridge::new(
            key.to_owned(),
            server.clone(),
        )),
    }
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
            bridges.insert(key.clone(), bridge_for(key, &config.instance));
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
