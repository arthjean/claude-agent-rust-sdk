//! Status values returned by the CLI: MCP server status and context usage.
//!
//! These types use the wire key names (camelCase where the CLI uses it)
//! because they come straight from the CLI output. A missing field takes
//! its default value.
//!
//! The Python SDK returns these responses as the dictionaries the CLI sent.
//! To keep every key reachable, each struct holds the keys it does not
//! model in a flattened `extra` map, written back on serialization. A
//! server configuration of a known type is the one exception to typing:
//! the configuration types are shared with the options and have no
//! `extra`, so a configuration carrying a key they do not model is kept
//! whole as [`McpServerStatusConfig::Unknown`].

use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value};

use super::config::{McpHttpServerConfig, McpSseServerConfig, McpStdioServerConfig};
use super::macros::wire_enum;

/// In-process server configuration as returned in status responses: only
/// its serializable fields.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[non_exhaustive]
pub struct McpSdkServerConfigStatus {
    /// Name of the server.
    pub name: String,
    /// Keys of the response this version of the crate does not model,
    /// kept as the CLI sent them and written back on serialization.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// Configuration of a server proxied through claude.ai, as returned in
/// status responses.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[non_exhaustive]
pub struct McpClaudeAiProxyServerConfig {
    /// URL of the proxy.
    pub url: String,
    /// Id of the proxied server.
    pub id: String,
    /// Keys of the response this version of the crate does not model,
    /// kept as the CLI sent them and written back on serialization.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// Server configuration as returned in status responses.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum McpServerStatusConfig {
    /// A child process speaking over stdio.
    Stdio(McpStdioServerConfig),
    /// A server over server-sent events.
    Sse(McpSseServerConfig),
    /// A server over streamable HTTP.
    Http(McpHttpServerConfig),
    /// A server inside the SDK host process.
    Sdk(McpSdkServerConfigStatus),
    /// A server proxied through claude.ai.
    ClaudeAiProxy(McpClaudeAiProxyServerConfig),
    /// A configuration of a type unknown to this version of the crate.
    Unknown(Value),
}

impl McpServerStatusConfig {
    /// Wire value of the `type` key, when it is known.
    #[must_use]
    pub fn type_name(&self) -> Option<&str> {
        match self {
            Self::Stdio(_) => Some("stdio"),
            Self::Sse(_) => Some("sse"),
            Self::Http(_) => Some("http"),
            Self::Sdk(_) => Some("sdk"),
            Self::ClaudeAiProxy(_) => Some("claudeai-proxy"),
            Self::Unknown(raw) => raw.get("type").and_then(Value::as_str),
        }
    }

    fn from_wire(raw: Value) -> Self {
        let modeled: &[&str] = match raw.get("type").and_then(Value::as_str) {
            None | Some("stdio") => &["type", "command", "args", "env"],
            Some("sse" | "http") => &["type", "url", "headers"],
            _ => &[],
        };
        if !modeled.is_empty()
            && raw
                .as_object()
                .is_some_and(|object| object.keys().any(|key| !modeled.contains(&key.as_str())))
        {
            return Self::Unknown(raw);
        }
        let parsed = match raw.get("type").and_then(Value::as_str) {
            None | Some("stdio") => McpStdioServerConfig::deserialize(&raw).map(Self::Stdio),
            Some("sse") => McpSseServerConfig::deserialize(&raw).map(Self::Sse),
            Some("http") => McpHttpServerConfig::deserialize(&raw).map(Self::Http),
            Some("sdk") => McpSdkServerConfigStatus::deserialize(&raw).map(Self::Sdk),
            Some("claudeai-proxy") => {
                McpClaudeAiProxyServerConfig::deserialize(&raw).map(Self::ClaudeAiProxy)
            }
            Some(_) => return Self::Unknown(raw),
        };
        parsed.unwrap_or(Self::Unknown(raw))
    }
}

impl Serialize for McpServerStatusConfig {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let (type_name, body) = match self {
            Self::Stdio(config) => (None, serde_json::to_value(config)),
            Self::Sse(config) => (Some("sse"), serde_json::to_value(config)),
            Self::Http(config) => (Some("http"), serde_json::to_value(config)),
            Self::Sdk(config) => (Some("sdk"), serde_json::to_value(config)),
            Self::ClaudeAiProxy(config) => (Some("claudeai-proxy"), serde_json::to_value(config)),
            Self::Unknown(raw) => return raw.serialize(serializer),
        };
        let body = body.map_err(serde::ser::Error::custom)?;
        let fields = body.as_object().cloned().unwrap_or_else(Map::new);
        let mut map = serializer.serialize_map(None)?;
        if let Some(type_name) = type_name {
            map.serialize_entry("type", type_name)?;
        }
        for (key, value) in &fields {
            map.serialize_entry(key, value)?;
        }
        map.end()
    }
}

impl<'de> Deserialize<'de> for McpServerStatusConfig {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Value::deserialize(deserializer).map(Self::from_wire)
    }
}

/// Tool annotations as returned in MCP server status.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct McpToolAnnotations {
    /// The tool only reads.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_only: Option<bool>,
    /// The tool can destroy data.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub destructive: Option<bool>,
    /// The tool reaches outside its own system.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub open_world: Option<bool>,
    /// Keys of the response this version of the crate does not model,
    /// kept as the CLI sent them and written back on serialization.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// A tool provided by an MCP server.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
#[non_exhaustive]
pub struct McpToolInfo {
    /// Name of the tool.
    pub name: String,
    /// Description of the tool.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Annotations of the tool.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub annotations: Option<McpToolAnnotations>,
    /// Keys of the response this version of the crate does not model,
    /// kept as the CLI sent them and written back on serialization.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// Server information from the MCP handshake.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
#[non_exhaustive]
pub struct McpServerInfo {
    /// Name the server reports.
    pub name: String,
    /// Version the server reports.
    pub version: String,
    /// Keys of the response this version of the crate does not model,
    /// kept as the CLI sent them and written back on serialization.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

wire_enum! {
    /// Connection status of an MCP server.
    #[derive(Default)]
    pub enum McpServerConnectionStatus {
        /// Connected.
        Connected => "connected",
        /// The connection failed.
        Failed => "failed",
        /// Authentication is required.
        NeedsAuth => "needs-auth",
        /// The connection is in progress.
        #[default]
        Pending => "pending",
        /// The server is disabled.
        Disabled => "disabled",
    }
}

/// Status of one MCP server connection.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
#[non_exhaustive]
pub struct McpServerStatus {
    /// Server name as configured.
    pub name: String,
    /// Current connection status.
    pub status: McpServerConnectionStatus,
    /// Server information from the handshake, when connected.
    #[serde(rename = "serverInfo", skip_serializing_if = "Option::is_none")]
    pub server_info: Option<McpServerInfo>,
    /// Error message, when the status is `failed`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Server configuration, including the URL of remote servers.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config: Option<McpServerStatusConfig>,
    /// Configuration scope, such as `project`, `user`, or `managed`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    /// Tools provided by the server, when connected.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<McpToolInfo>>,
    /// Keys of the response this version of the crate does not model,
    /// kept as the CLI sent them and written back on serialization.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// Response to an MCP status request.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
#[non_exhaustive]
pub struct McpStatusResponse {
    /// Status of every configured server.
    #[serde(rename = "mcpServers")]
    pub mcp_servers: Vec<McpServerStatus>,
    /// Keys of the response this version of the crate does not model,
    /// kept as the CLI sent them and written back on serialization.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// One category of context usage, such as the system prompt or the tools.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
#[non_exhaustive]
pub struct ContextUsageCategory {
    /// Name of the category.
    pub name: String,
    /// Tokens used by the category.
    pub tokens: u64,
    /// Display color of the category.
    pub color: String,
    /// Whether the category is deferred.
    #[serde(rename = "isDeferred", skip_serializing_if = "Option::is_none")]
    pub is_deferred: Option<bool>,
    /// Keys of the response this version of the crate does not model,
    /// kept as the CLI sent them and written back on serialization.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// Response to a context usage request: a breakdown of the context window
/// by category.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
#[non_exhaustive]
pub struct ContextUsageResponse {
    /// Token usage by category.
    pub categories: Vec<ContextUsageCategory>,
    /// Tokens currently in the context window.
    pub total_tokens: u64,
    /// Effective maximum, which a compaction buffer may reduce.
    pub max_tokens: u64,
    /// Raw context window size of the model.
    pub raw_max_tokens: u64,
    /// Percentage of the context window in use, from 0 to 100.
    pub percentage: f64,
    /// Model the usage is computed for.
    pub model: String,
    /// Whether automatic compaction is enabled.
    pub is_auto_compact_enabled: bool,
    /// Memory files loaded, with path, type, and token counts.
    pub memory_files: Vec<Value>,
    /// MCP tools with name, server name, tokens, and load status.
    pub mcp_tools: Vec<Value>,
    /// Agent definitions with type, source, and token counts.
    pub agents: Vec<Value>,
    /// Grid used by the CLI to display the context.
    pub grid_rows: Vec<Vec<Value>>,
    /// Token threshold at which automatic compaction triggers.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auto_compact_threshold: Option<u64>,
    /// Built-in tools deferred from the initial tool list.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deferred_builtin_tools: Option<Vec<Value>>,
    /// Built-in tools with name and token counts.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_tools: Option<Vec<Value>>,
    /// System prompt sections with name and token counts.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_prompt_sections: Option<Vec<Value>>,
    /// Slash command usage summary.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub slash_commands: Option<Value>,
    /// Skill usage summary.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skills: Option<Value>,
    /// Breakdown of message tokens by type.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_breakdown: Option<Value>,
    /// Cumulative API usage of the session.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_usage: Option<Value>,
    /// Keys of the response this version of the crate does not model,
    /// kept as the CLI sent them and written back on serialization.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}
