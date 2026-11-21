//! Native MCP server: the JSON-RPC dispatcher that serves a set of tools
//! without any MCP library.
//!
//! The `initialize` result declares the capabilities the Python SDK's
//! server declares through the `mcp` 1.x package: `experimental` empty and
//! `tools` with `listChanged` false, so the CLI receives the same handshake
//! from both SDKs.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::sync::{Notify, oneshot};
use tokio::time::Instant;

use super::tool::{SdkMcpTool, ToolResult};
use crate::error::{Error, Result};
use crate::types::{McpSdkServerConfig, McpSdkServerInstance, SdkServer};

const DEFAULT_SERVER_VERSION: &str = "1.0.0";
const LATEST_PROTOCOL_VERSION: &str = "2025-11-25";
const SUPPORTED_PROTOCOL_VERSIONS: [&str; 4] = [
    "2024-11-05",
    "2025-03-26",
    "2025-06-18",
    LATEST_PROTOCOL_VERSION,
];

/// Time the end of a session waits for the requests still running to stop.
pub(crate) const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);

const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;
const INTERNAL_ERROR: i64 = -32603;
/// The code of the answer to a cancelled call: the Python `mcp` package
/// (1.x, which upstream runs) answers `0`, not the `-32800` of MCP 2.
const REQUEST_CANCELLED: i64 = 0;

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
        McpSdkServerInstance(SdkServer::Native(Arc::new(server))),
    ))
}

pub(crate) struct SdkMcpServer {
    name: String,
    version: String,
    tools: Vec<SdkMcpTool>,
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
            tools.push(definition);
        }
        Ok(Self {
            name,
            version,
            tools,
            index,
        })
    }

    /// Answers the handshake. Its parameters are checked as the `mcp`
    /// package checks them: a `protocolVersion` string or integer, a `capabilities`
    /// object, and a `clientInfo` with a `name` and a `version`.
    fn initialize(&self, params: &Value) -> std::result::Result<Value, RpcError> {
        let client = params.get("clientInfo");
        let well_formed = params
            .get("protocolVersion")
            .is_some_and(|version| version.is_string() || version.is_i64() || version.is_u64())
            && params.get("capabilities").is_some_and(Value::is_object)
            && client
                .and_then(|client| client.get("name"))
                .is_some_and(Value::is_string)
            && client
                .and_then(|client| client.get("version"))
                .is_some_and(Value::is_string);
        if !well_formed {
            return Err(invalid_params(
                "initialize needs a protocolVersion, capabilities, and a clientInfo with a name \
                 and a version",
            ));
        }
        let requested = params.get("protocolVersion").and_then(Value::as_str);
        let negotiated = requested
            .filter(|version| SUPPORTED_PROTOCOL_VERSIONS.contains(version))
            .unwrap_or(LATEST_PROTOCOL_VERSION);
        Ok(json!({
            "protocolVersion": negotiated,
            "capabilities": {"experimental": {}, "tools": {"listChanged": false}},
            "serverInfo": {"name": self.name, "version": self.version},
        }))
    }

    fn list_tools(&self) -> Value {
        let tools: Vec<Value> = self.tools.iter().map(SdkMcpTool::to_wire).collect();
        json!({"tools": tools})
    }

    async fn call_tool(&self, name: &str, arguments: Value) -> Value {
        let Some(tool) = self.index.get(name).map(|index| &self.tools[*index]) else {
            return ToolResult::error(format!("Tool '{name}' not found")).to_wire();
        };
        tool.call(arguments).await.to_wire()
    }

    async fn dispatch(&self, method: &str, params: &Value) -> std::result::Result<Value, RpcError> {
        match method {
            "initialize" => self.initialize(params),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(self.list_tools()),
            "tools/call" => {
                let name = params
                    .get("name")
                    .and_then(Value::as_str)
                    .ok_or_else(|| invalid_params("tools/call needs a tool name"))?;
                let arguments = match params.get("arguments") {
                    None | Some(Value::Null) => json!({}),
                    Some(arguments @ Value::Object(_)) => arguments.clone(),
                    Some(_) => {
                        return Err(invalid_params(
                            "the arguments of tools/call must be an object",
                        ));
                    }
                };
                Ok(self.call_tool(name, arguments).await)
            }
            other => Err(RpcError {
                code: METHOD_NOT_FOUND,
                message: format!("Method not found: {other}"),
                data: None,
            }),
        }
    }
}

struct RpcError {
    code: i64,
    message: String,
    data: Option<Value>,
}

/// The `-32602` error the `mcp` package answers for parameters it cannot
/// read: the same message and an empty `data` whatever the cause, which
/// goes to the debug log.
fn invalid_params(detail: &str) -> RpcError {
    tracing::debug!("SDK MCP request with invalid parameters: {detail}");
    RpcError {
        code: INVALID_PARAMS,
        message: "Invalid request parameters".to_owned(),
        data: Some(Value::from("")),
    }
}

fn response(id: &Value, outcome: std::result::Result<Value, RpcError>) -> Value {
    match outcome {
        Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
        Err(error) => {
            let mut body = json!({"code": error.code, "message": error.message});
            if let Some(data) = error.data {
                body["data"] = data;
            }
            json!({"jsonrpc": "2.0", "id": id, "error": body})
        }
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

/// The requests of one session still running, keyed by JSON-RPC id, each
/// with the signal that cancels it while `notifications/cancelled` can.
#[derive(Default)]
struct InFlight {
    calls: Mutex<HashMap<String, Option<oneshot::Sender<()>>>>,
    idle: Notify,
}

impl InFlight {
    fn calls(&self) -> MutexGuard<'_, HashMap<String, Option<oneshot::Sender<()>>>> {
        self.calls.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The id of a running request, released when the request is done.
struct Reservation {
    in_flight: Arc<InFlight>,
    key: String,
}

impl Drop for Reservation {
    fn drop(&mut self) {
        self.in_flight.calls().remove(&self.key);
        self.in_flight.idle.notify_waiters();
    }
}

/// The connection of one session to a native server: the server and the
/// requests of this session that are still running.
///
/// Each request runs in a task of its own, which holds its id until the
/// server has answered. A `control_cancel_request` from the CLI drops only
/// the wait for the answer, as in the Python SDK: the tool runs to its end,
/// so a handler is never stopped between two side effects, and a request
/// that reuses the id meanwhile is refused as already in flight. Only
/// `notifications/cancelled`, and the end of the session, stop a running
/// tool.
pub(crate) struct NativeSession {
    server: Arc<SdkMcpServer>,
    in_flight: Arc<InFlight>,
}

impl NativeSession {
    pub(crate) fn new(server: Arc<SdkMcpServer>) -> Self {
        Self {
            server,
            in_flight: Arc::default(),
        }
    }

    /// Handles one JSON-RPC message. Returns the response to a request, and
    /// nothing for a notification or a response.
    pub(crate) async fn handle(
        &self,
        message: &Value,
    ) -> std::result::Result<Option<Value>, String> {
        if message.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
            return Err("Invalid JSON-RPC message: jsonrpc must be \"2.0\"".to_owned());
        }
        let params = message.get("params").unwrap_or(&Value::Null);
        if !(params.is_null() || params.is_object()) {
            return Err("Invalid JSON-RPC message: params must be an object".to_owned());
        }
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
            let mut calls = self.in_flight.calls();
            if calls.contains_key(&key) {
                return Err(format!(
                    "Request id {} is already in flight",
                    display_id(id)
                ));
            }
            calls.insert(key.clone(), Some(cancel));
        }
        let reservation = Reservation {
            in_flight: Arc::clone(&self.in_flight),
            key,
        };
        let server = Arc::clone(&self.server);
        let method = method.to_owned();
        let params = params.clone();
        let call = crate::caller_context::spawn(async move {
            let outcome = tokio::select! {
                biased;
                Ok(()) = cancelled => Err(RpcError {
                    code: REQUEST_CANCELLED,
                    message: "Request cancelled".to_owned(),
                    data: None,
                }),
                outcome = server.dispatch(&method, &params) => outcome,
            };
            drop(reservation);
            outcome
        });
        let outcome = call.await.unwrap_or_else(|_| {
            Err(RpcError {
                code: INTERNAL_ERROR,
                message: "The request was stopped before it was answered".to_owned(),
                data: None,
            })
        });
        Ok(Some(response(id, outcome)))
    }

    fn cancel(&self, params: &Value) {
        let Some(id) = params.get("requestId") else {
            return;
        };
        let cancel = self
            .in_flight
            .calls()
            .get_mut(&id.to_string())
            .and_then(Option::take);
        if let Some(cancel) = cancel {
            let _ = cancel.send(());
        }
    }

    /// Ends the session: cancels every request still running and waits for
    /// them to stop, for at most [`SHUTDOWN_GRACE`].
    pub(crate) async fn close(&self) {
        let deadline = Instant::now() + SHUTDOWN_GRACE;
        let cancels: Vec<_> = self
            .in_flight
            .calls()
            .values_mut()
            .filter_map(Option::take)
            .collect();
        for cancel in cancels {
            let _ = cancel.send(());
        }
        loop {
            let idle = self.in_flight.idle.notified();
            tokio::pin!(idle);
            idle.as_mut().enable();
            if self.in_flight.calls().is_empty() {
                return;
            }
            if tokio::time::timeout_at(deadline, idle).await.is_err() {
                tracing::warn!(
                    "SDK MCP server '{}' still had a tool running {} s after being closed; no \
                     longer waiting for it",
                    self.server.name,
                    SHUTDOWN_GRACE.as_secs()
                );
                return;
            }
        }
    }
}
