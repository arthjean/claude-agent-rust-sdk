//! Native MCP server: the JSON-RPC dispatcher that serves a set of tools
//! without any MCP library.
//!
//! The `initialize` result declares the capabilities the Python SDK's
//! server declares through the `mcp` 1.x package: `experimental` empty and
//! `tools` with `listChanged` false, so the CLI receives the same handshake
//! from both SDKs.

use std::collections::HashMap;
use std::panic::AssertUnwindSafe;
use std::sync::{Arc, Mutex, PoisonError};

use futures_util::FutureExt;
use serde_json::{Value, json};
use tokio::sync::oneshot;

use super::tool::{SdkMcpTool, ToolFailure, ToolResult};
use crate::error::{Error, Result};
use crate::types::{McpSdkServerConfig, McpSdkServerInstance};

const DEFAULT_SERVER_VERSION: &str = "1.0.0";
const LATEST_PROTOCOL_VERSION: &str = "2025-11-25";
const SUPPORTED_PROTOCOL_VERSIONS: [&str; 4] = [
    "2024-11-05",
    "2025-03-26",
    "2025-06-18",
    LATEST_PROTOCOL_VERSION,
];

const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;
const REQUEST_CANCELLED: i64 = -32800;

/// Creates an MCP server that runs inside this process and serves these
/// tools to the CLI over the control channel.
///
/// The version defaults to `1.0.0`. Register the returned configuration with
/// [`ClaudeAgentOptionsBuilder::mcp_server`](crate::ClaudeAgentOptionsBuilder::mcp_server);
/// the model then sees each tool as `mcp__<server key>__<tool name>`.
///
/// A call to an unknown tool, arguments that fail validation, and a handler
/// that returns an error are all reported to the model as error results,
/// never as protocol errors.
///
/// ```
/// use claude_agent_rust_sdk::{ClaudeAgentOptions, SdkMcpTool, ToolResult, create_sdk_mcp_server};
/// use schemars::JsonSchema;
/// use serde::Deserialize;
///
/// #[derive(Deserialize, JsonSchema)]
/// struct Add {
///     a: f64,
///     b: f64,
/// }
///
/// # fn main() -> Result<(), claude_agent_rust_sdk::Error> {
/// let add = SdkMcpTool::typed::<Add>("add", "Add two numbers", |args| async move {
///     Ok(ToolResult::text((args.a + args.b).to_string()))
/// });
/// let calculator = create_sdk_mcp_server("calculator", None, [add])?;
/// let options = ClaudeAgentOptions::builder()
///     .mcp_server("calc", calculator)
///     .allowed_tools(["mcp__calc__add"])
///     .build()?;
/// # let _ = options;
/// # Ok(())
/// # }
/// ```
///
/// # Errors
///
/// Returns [`ErrorKind::Config`](crate::ErrorKind::Config) when two tools have the same name.
pub fn create_sdk_mcp_server(
    name: impl Into<String>,
    version: Option<&str>,
    tools: impl IntoIterator<Item = SdkMcpTool>,
) -> Result<McpSdkServerConfig> {
    let name = name.into();
    let server = SdkMcpServer::new(
        name.clone(),
        version.unwrap_or(DEFAULT_SERVER_VERSION).to_owned(),
        tools.into_iter().collect(),
    )?;
    Ok(McpSdkServerConfig::new(
        name,
        McpSdkServerInstance::new(server),
    ))
}

#[cfg(feature = "jsonschema")]
type Validator = std::result::Result<jsonschema::Validator, String>;

struct Tool {
    definition: SdkMcpTool,
    #[cfg(feature = "jsonschema")]
    validator: Validator,
}

pub(crate) struct SdkMcpServer {
    name: String,
    version: String,
    tools: Vec<Tool>,
    index: HashMap<String, usize>,
}

impl SdkMcpServer {
    fn new(name: String, version: String, definitions: Vec<SdkMcpTool>) -> Result<Self> {
        let mut index = HashMap::with_capacity(definitions.len());
        let mut tools = Vec::with_capacity(definitions.len());
        for definition in definitions {
            if index
                .insert(definition.name().to_owned(), tools.len())
                .is_some()
            {
                return Err(Error::config(format!(
                    "SDK MCP server '{name}' has two tools named '{}'",
                    definition.name()
                )));
            }
            tools.push(Tool {
                #[cfg(feature = "jsonschema")]
                validator: jsonschema::validator_for(definition.input_schema())
                    .map_err(|error| error.to_string()),
                definition,
            });
        }
        Ok(Self {
            name,
            version,
            tools,
            index,
        })
    }

    fn initialize(&self, params: &Value) -> Value {
        let requested = params.get("protocolVersion").and_then(Value::as_str);
        let negotiated = requested
            .filter(|version| SUPPORTED_PROTOCOL_VERSIONS.contains(version))
            .unwrap_or(LATEST_PROTOCOL_VERSION);
        json!({
            "protocolVersion": negotiated,
            "capabilities": {"experimental": {}, "tools": {"listChanged": false}},
            "serverInfo": {"name": self.name, "version": self.version},
        })
    }

    fn list_tools(&self) -> Value {
        let tools: Vec<Value> = self
            .tools
            .iter()
            .map(|tool| tool.definition.to_wire())
            .collect();
        json!({"tools": tools})
    }

    async fn call_tool(&self, name: &str, arguments: Value) -> Value {
        let Some(tool) = self.index.get(name).map(|index| &self.tools[*index]) else {
            return ToolResult::error(format!("Tool '{name}' not found")).to_wire();
        };
        #[cfg(feature = "jsonschema")]
        match &tool.validator {
            Err(invalid_schema) => return ToolResult::error(invalid_schema.clone()).to_wire(),
            Ok(validator) => {
                if let Err(error) = validator.validate(&arguments) {
                    return ToolResult::error(format!("Input validation error: {error}")).to_wire();
                }
            }
        }
        let call =
            AssertUnwindSafe(async move { tool.definition.call(arguments).await }).catch_unwind();
        match call.await {
            Ok(Ok(result)) => result.to_wire(),
            Ok(Err(ToolFailure::InvalidInput(message))) => {
                ToolResult::error(format!("Input validation error: {message}")).to_wire()
            }
            Ok(Err(ToolFailure::Handler(message))) => ToolResult::error(message).to_wire(),
            Err(_) => ToolResult::error(format!("Tool '{name}' panicked")).to_wire(),
        }
    }

    async fn dispatch(&self, method: &str, params: &Value) -> std::result::Result<Value, RpcError> {
        match method {
            "initialize" => Ok(self.initialize(params)),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(self.list_tools()),
            "tools/call" => {
                let name = params.get("name").and_then(Value::as_str).ok_or(RpcError {
                    code: INVALID_PARAMS,
                    message: "Invalid params: tools/call needs a tool name".to_owned(),
                })?;
                let arguments = match params.get("arguments") {
                    None | Some(Value::Null) => json!({}),
                    Some(arguments) => arguments.clone(),
                };
                Ok(self.call_tool(name, arguments).await)
            }
            other => Err(RpcError {
                code: METHOD_NOT_FOUND,
                message: format!("Method not found: {other}"),
            }),
        }
    }
}

struct RpcError {
    code: i64,
    message: String,
}

fn response(id: &Value, outcome: std::result::Result<Value, RpcError>) -> Value {
    match outcome {
        Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
        Err(error) => json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": {"code": error.code, "message": error.message},
        }),
    }
}

/// Renders a JSON-RPC id the way the upstream SDK names it in messages:
/// numbers bare, strings in single quotes.
pub(crate) fn display_id(id: &Value) -> String {
    match id {
        Value::String(text) => format!("'{text}'"),
        other => other.to_string(),
    }
}

/// A JSON-RPC id is a string or an integer. Anything else makes the message
/// a notification, as MCP reads it.
pub(crate) fn request_id(message: &Value) -> Option<&Value> {
    message
        .get("id")
        .filter(|id| id.is_string() || id.is_i64() || id.is_u64())
}

type InFlight = Mutex<HashMap<String, Option<oneshot::Sender<()>>>>;

struct Reservation<'a> {
    in_flight: &'a InFlight,
    key: String,
}

impl Drop for Reservation<'_> {
    fn drop(&mut self) {
        self.in_flight
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&self.key);
    }
}

/// The connection of one session to a native server: the server and the
/// requests of this session that are still running.
pub(crate) struct NativeSession {
    server: Arc<SdkMcpServer>,
    in_flight: InFlight,
}

impl NativeSession {
    pub(crate) fn new(server: Arc<SdkMcpServer>) -> Self {
        Self {
            server,
            in_flight: Mutex::new(HashMap::new()),
        }
    }

    /// Handles one JSON-RPC message. Returns the response to a request, and
    /// nothing for a notification or a response.
    pub(crate) async fn handle(
        &self,
        message: &Value,
    ) -> std::result::Result<Option<Value>, String> {
        let params = message.get("params").unwrap_or(&Value::Null);
        let Some(method) = message.get("method").and_then(Value::as_str) else {
            if message.get("result").is_some() || message.get("error").is_some() {
                return Ok(None);
            }
            return Err("Invalid JSON-RPC message: it has no method".to_owned());
        };
        let Some(id) = request_id(message) else {
            if method == "notifications/cancelled" {
                self.cancel(params);
            }
            return Ok(None);
        };

        let key = id.to_string();
        let (cancel, cancelled) = oneshot::channel();
        {
            let mut in_flight = self
                .in_flight
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if in_flight.contains_key(&key) {
                return Err(format!(
                    "Request id {} is already in flight",
                    display_id(id)
                ));
            }
            in_flight.insert(key.clone(), Some(cancel));
        }
        let _reservation = Reservation {
            in_flight: &self.in_flight,
            key,
        };
        let outcome = tokio::select! {
            biased;
            Ok(()) = cancelled => Err(RpcError {
                code: REQUEST_CANCELLED,
                message: "Request cancelled".to_owned(),
            }),
            outcome = self.server.dispatch(method, params) => outcome,
        };
        Ok(Some(response(id, outcome)))
    }

    fn cancel(&self, params: &Value) {
        let Some(id) = params.get("requestId") else {
            return;
        };
        let cancel = self
            .in_flight
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get_mut(&id.to_string())
            .and_then(Option::take);
        if let Some(cancel) = cancel {
            let _ = cancel.send(());
        }
    }
}

#[cfg(all(test, feature = "jsonschema"))]
mod tests {
    use serde_json::json;

    use super::SdkMcpServer;
    use crate::mcp::tool::{SdkMcpTool, ToolResult};

    #[test]
    fn each_validator_is_compiled_when_the_server_is_created() {
        let raw = |name: &str, schema| {
            SdkMcpTool::raw(name, "", schema, |_| async { Ok(ToolResult::text("")) }).unwrap()
        };
        let server = SdkMcpServer::new(
            "srv".to_owned(),
            "1.0.0".to_owned(),
            vec![
                raw("valid", json!({"type": "object", "required": ["b"]})),
                raw(
                    "broken",
                    json!({"type": "object", "properties": {"x": {"type": "bogus"}}}),
                ),
            ],
        )
        .unwrap();
        let compiled: Vec<bool> = server
            .tools
            .iter()
            .map(|tool| tool.validator.is_ok())
            .collect();
        assert_eq!(compiled, [true, false]);
        let Ok(validator) = &server.tools[0].validator else {
            unreachable!()
        };
        assert!(!validator.is_valid(&json!({})));
        assert!(validator.is_valid(&json!({"b": 1})));
    }
}
