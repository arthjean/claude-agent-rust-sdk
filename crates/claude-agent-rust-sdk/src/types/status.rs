//! Status values returned by the CLI: MCP server status and context usage.
//!
//! These types use the wire key names (camelCase where the CLI uses it)
//! because they come straight from the CLI output. A missing field takes
//! its default value.
//!
//! The Python SDK returns these responses as the dictionaries the CLI sent.
//! To keep every key reachable, each struct holds the keys it does not
//! model in a flattened `extra` map, written back on serialization; the
//! server configuration types, shared with the options, do too. A
//! configuration of a type this crate does not know, or that does not read
//! as its type, is kept whole as [`McpServerStatusConfig::Unknown`].
//!
//! Differs from upstream in one deliberate way: where upstream returns
//! the values as sent, a count that is a fraction is truncated, a negative
//! count is 0, and `null` is 0 or the empty string, as for messages; a
//! value of another JSON type fails the call with the raw response in
//! [`Error::data`](crate::Error::data).

use crate::json_map::RemoveKey;
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

    /// Reads a configuration. The `type` key names the variant, so it is
    /// left out of the fields, and out of their `extra`; a configuration
    /// that does not read as its type is kept whole as `Unknown`.
    fn from_wire(raw: Value) -> Self {
        let mut fields = raw.clone();
        if let Some(object) = fields.as_object_mut() {
            object.remove_key("type");
        }
        let parsed = match raw.get("type").and_then(Value::as_str) {
            None | Some("stdio") => McpStdioServerConfig::deserialize(&fields).map(Self::Stdio),
            Some("sse") => McpSseServerConfig::deserialize(&fields).map(Self::Sse),
            Some("http") => McpHttpServerConfig::deserialize(&fields).map(Self::Http),
            Some("sdk") => McpSdkServerConfigStatus::deserialize(&fields).map(Self::Sdk),
            Some("claudeai-proxy") => {
                McpClaudeAiProxyServerConfig::deserialize(&fields).map(Self::ClaudeAiProxy)
            }
            Some(_) => return Self::Unknown(raw),
        };
        parsed.unwrap_or(Self::Unknown(raw))
    }
}

/// Writes the `type` key of every known kind, `"stdio"` included: CLI
/// 2.1.329 sends it in each status, so a status written back keeps it.
impl Serialize for McpServerStatusConfig {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let (type_name, body) = match self {
            Self::Stdio(config) => (Some("stdio"), serde_json::to_value(config)),
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
    #[serde(deserialize_with = "text")]
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
    #[serde(deserialize_with = "text")]
    pub name: String,
    /// Version the server reports.
    #[serde(deserialize_with = "text")]
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
    #[serde(deserialize_with = "text")]
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
    #[serde(deserialize_with = "text")]
    pub name: String,
    /// Tokens used by the category.
    #[serde(deserialize_with = "count")]
    pub tokens: u64,
    /// Display color of the category.
    #[serde(deserialize_with = "text")]
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
    #[serde(deserialize_with = "count")]
    pub total_tokens: u64,
    /// Effective maximum, which a compaction buffer may reduce.
    #[serde(deserialize_with = "count")]
    pub max_tokens: u64,
    /// Raw context window size of the model.
    #[serde(deserialize_with = "count")]
    pub raw_max_tokens: u64,
    /// Percentage of the context window in use, from 0 to 100.
    #[serde(deserialize_with = "number")]
    pub percentage: f64,
    /// Model the usage is computed for.
    #[serde(deserialize_with = "text")]
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
    #[serde(deserialize_with = "optional_count")]
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
    /// Cumulative API usage of the session. CLI 2.1.329 sends `null`
    /// before the first turn, which reads as `None` and is not written back.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_usage: Option<Value>,
    /// Keys of the response this version of the crate does not model,
    /// kept as the CLI sent them and written back on serialization.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// Reads a count as the CLI's JavaScript may write it: a fraction is
/// truncated toward zero, a negative number is 0, and `null` is 0, so a
/// status response is not refused over one number, as upstream hands the
/// dictionary through.
fn count<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u64, D::Error> {
    Ok(optional_count(deserializer)?.unwrap_or(0))
}

/// [`count`] for an optional field: `null` stays absent.
fn optional_count<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<u64>, D::Error> {
    match Value::deserialize(deserializer)? {
        Value::Null => Ok(None),
        Value::Number(number) => Ok(Some(number.as_u64().unwrap_or_else(|| {
            number
                .as_f64()
                .filter(|value| *value > 0.0)
                .map_or(0, |value| value as u64)
        }))),
        other => Err(serde::de::Error::custom(format!(
            "invalid type: {other}, expected a number"
        ))),
    }
}

/// Reads a number, with `null` as 0.
fn number<'de, D: Deserializer<'de>>(deserializer: D) -> Result<f64, D::Error> {
    Ok(Option::<f64>::deserialize(deserializer)?.unwrap_or(0.0))
}

/// Reads a string, with `null` as the empty string.
fn text<'de, D: Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
    Ok(Option::<String>::deserialize(deserializer)?.unwrap_or_default())
}
