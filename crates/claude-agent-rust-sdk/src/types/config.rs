//! Configuration values accepted by the options.
//!
//! These types differ from upstream in one deliberate way:
//!
//! - A stdio MCP server is written to `--mcp-config` without a `type` key.
//!   The Python SDK passes the dictionary as the caller wrote it, so it
//!   writes `"type": "stdio"` only when the caller did; the CLI reads a
//!   missing type as stdio, so both forms start the same server.

use std::any::Any;
use std::fmt;
use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;

use indexmap::IndexMap;
use serde::ser::SerializeMap;
use serde::{Deserialize, Serialize, Serializer};
use serde_json::{Map, Value};

use super::BoxFuture;
use super::macros::{setters, wire_enum};
use super::permission::PermissionMode;

wire_enum! {
    /// Beta feature to enable.
    pub enum SdkBeta {
        /// One million token context window.
        Context1m20250807 => "context-1m-2025-08-07",
    }
}

wire_enum! {
    /// Filesystem settings source to load.
    pub enum SettingSource {
        /// Global user settings.
        User => "user",
        /// Project settings.
        Project => "project",
        /// Local settings.
        Local => "local",
    }
}

wire_enum! {
    /// How much effort the model puts into its response.
    pub enum EffortLevel {
        /// Minimal thinking, fastest responses.
        Low => "low",
        /// Moderate thinking.
        Medium => "medium",
        /// Deep reasoning.
        High => "high",
        /// Extended reasoning depth.
        Xhigh => "xhigh",
        /// Maximum effort.
        Max => "max",
    }
}

/// Preset system prompt, with optional additions.
///
/// The preset is the default system prompt of the CLI (`claude_code` on the
/// wire).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[non_exhaustive]
pub struct SystemPromptPreset {
    /// Instructions appended to the preset.
    pub append: Option<String>,
    /// Strips the per-user dynamic sections (working directory, memory, git
    /// status) from the system prompt so it stays cacheable across users.
    /// The stripped content is injected into the first user message.
    pub exclude_dynamic_sections: Option<bool>,
    /// Whether the session keeps the system prompt it recorded on its first
    /// request. When false the prompt is rebuilt on every request.
    pub snapshot: Option<bool>,
}

impl SystemPromptPreset {
    /// Creates the preset without additions.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    setters! {
        opt append: String;
        opt exclude_dynamic_sections: bool;
        opt snapshot: bool;
    }
}

/// Custom system prompt, in the form that can also set `snapshot`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct SystemPromptCustom {
    /// The system prompt.
    pub prompt: String,
    /// Same as [`SystemPromptPreset::snapshot`], applied to `prompt`.
    pub snapshot: Option<bool>,
}

impl SystemPromptCustom {
    /// Creates a custom prompt.
    #[must_use]
    pub fn new(prompt: impl Into<String>) -> Self {
        Self {
            prompt: prompt.into(),
            snapshot: None,
        }
    }

    setters! {
        opt snapshot: bool;
    }
}

/// System prompt read from a file.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct SystemPromptFile {
    /// Path of the file.
    pub path: String,
}

impl SystemPromptFile {
    /// Creates a file prompt.
    #[must_use]
    pub fn new(path: impl Into<String>) -> Self {
        Self { path: path.into() }
    }
}

/// System prompt configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum SystemPrompt {
    /// A custom system prompt.
    Text(String),
    /// The default system prompt, with optional additions.
    Preset(SystemPromptPreset),
    /// A custom system prompt that can also set `snapshot`.
    Custom(SystemPromptCustom),
    /// A system prompt read from a file.
    File(SystemPromptFile),
}

impl From<&str> for SystemPrompt {
    fn from(text: &str) -> Self {
        Self::Text(text.to_owned())
    }
}

impl From<String> for SystemPrompt {
    fn from(text: String) -> Self {
        Self::Text(text)
    }
}

impl From<SystemPromptPreset> for SystemPrompt {
    fn from(preset: SystemPromptPreset) -> Self {
        Self::Preset(preset)
    }
}

impl From<SystemPromptCustom> for SystemPrompt {
    fn from(custom: SystemPromptCustom) -> Self {
        Self::Custom(custom)
    }
}

impl From<SystemPromptFile> for SystemPrompt {
    fn from(file: SystemPromptFile) -> Self {
        Self::File(file)
    }
}

/// Named set of built-in tools.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ToolsPreset {
    /// Every default tool of the CLI (`claude_code` on the wire).
    ClaudeCode,
}

/// Base set of available built-in tools.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Tools {
    /// Specific tool names. An empty list disables every built-in tool.
    List(Vec<String>),
    /// A named set of tools.
    Preset(ToolsPreset),
}

impl Tools {
    /// Creates a list of tool names.
    #[must_use]
    pub fn list(names: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self::List(names.into_iter().map(Into::into).collect())
    }
}

impl From<Vec<String>> for Tools {
    fn from(names: Vec<String>) -> Self {
        Self::List(names)
    }
}

impl<const N: usize> From<[&str; N]> for Tools {
    fn from(names: [&str; N]) -> Self {
        Self::list(names)
    }
}

impl From<ToolsPreset> for Tools {
    fn from(preset: ToolsPreset) -> Self {
        Self::Preset(preset)
    }
}

/// Skills to enable for the main session.
///
/// This is a context filter, not a sandbox: skills left out are hidden from
/// the model and rejected by the Skill tool, but their files stay on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Skills {
    /// Only the listed skills. Names must be exact. An empty list hides
    /// every skill.
    List(Vec<String>),
    /// Every discovered skill.
    All,
}

impl Skills {
    /// Creates a list of skill names.
    #[must_use]
    pub fn list(names: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self::List(names.into_iter().map(Into::into).collect())
    }
}

impl From<Vec<String>> for Skills {
    fn from(names: Vec<String>) -> Self {
        Self::List(names)
    }
}

impl<const N: usize> From<[&str; N]> for Skills {
    fn from(names: [&str; N]) -> Self {
        Self::list(names)
    }
}

/// API-side task budget in tokens.
///
/// When set, the model knows its remaining token budget, so it can pace
/// tool use and wrap up before the limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct TaskBudget {
    /// Total budget in tokens.
    pub total: u64,
}

impl TaskBudget {
    /// Creates a budget.
    #[must_use]
    pub fn new(total: u64) -> Self {
        Self { total }
    }
}

wire_enum! {
    /// Memory scope of an agent.
    pub enum AgentMemory {
        /// User scope.
        User => "user",
        /// Project scope.
        Project => "project",
        /// Local scope.
        Local => "local",
    }
}

/// Effort of an agent: a named level or a token count.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
#[non_exhaustive]
pub enum AgentEffort {
    /// A named level.
    Level(EffortLevel),
    /// A thinking token count.
    Tokens(u64),
}

impl From<EffortLevel> for AgentEffort {
    fn from(level: EffortLevel) -> Self {
        Self::Level(level)
    }
}

impl From<u64> for AgentEffort {
    fn from(tokens: u64) -> Self {
        Self::Tokens(tokens)
    }
}

/// MCP server entry of an agent: a server name or an inline definition.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
#[non_exhaustive]
pub enum AgentMcpServer {
    /// Name of a server configured elsewhere.
    Name(String),
    /// Inline `{name: config}` object.
    Inline(Map<String, Value>),
}

impl From<&str> for AgentMcpServer {
    fn from(name: &str) -> Self {
        Self::Name(name.to_owned())
    }
}

impl From<String> for AgentMcpServer {
    fn from(name: String) -> Self {
        Self::Name(name)
    }
}

impl From<Map<String, Value>> for AgentMcpServer {
    fn from(definition: Map<String, Value>) -> Self {
        Self::Inline(definition)
    }
}

/// Definition of a custom subagent.
///
/// It serializes with the key names the CLI expects (`disallowedTools`,
/// `mcpServers`, `initialPrompt`, `maxTurns`, `permissionMode`) and leaves
/// out every unset field.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct AgentDefinition {
    /// When to use this agent.
    pub description: String,
    /// System prompt of the agent.
    pub prompt: String,
    /// Tools the agent may use.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<String>>,
    /// Tools the agent may not use.
    #[serde(
        default,
        rename = "disallowedTools",
        skip_serializing_if = "Option::is_none"
    )]
    pub disallowed_tools: Option<Vec<String>>,
    /// Model alias (`sonnet`, `opus`, `haiku`, `inherit`) or a full model
    /// id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Skills available to the agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skills: Option<Vec<String>>,
    /// Memory scope of the agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory: Option<AgentMemory>,
    /// MCP servers available to the agent.
    #[serde(
        default,
        rename = "mcpServers",
        skip_serializing_if = "Option::is_none"
    )]
    pub mcp_servers: Option<Vec<AgentMcpServer>>,
    /// Prompt the agent starts with.
    #[serde(
        default,
        rename = "initialPrompt",
        skip_serializing_if = "Option::is_none"
    )]
    pub initial_prompt: Option<String>,
    /// Maximum number of turns.
    #[serde(default, rename = "maxTurns", skip_serializing_if = "Option::is_none")]
    pub max_turns: Option<u32>,
    /// Whether the agent runs in the background.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background: Option<bool>,
    /// Effort of the agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<AgentEffort>,
    /// Permission mode of the agent.
    #[serde(
        default,
        rename = "permissionMode",
        skip_serializing_if = "Option::is_none"
    )]
    pub permission_mode: Option<PermissionMode>,
}

impl AgentDefinition {
    /// Creates a definition with only its description and prompt set.
    #[must_use]
    pub fn new(description: impl Into<String>, prompt: impl Into<String>) -> Self {
        Self {
            description: description.into(),
            prompt: prompt.into(),
            tools: None,
            disallowed_tools: None,
            model: None,
            skills: None,
            memory: None,
            mcp_servers: None,
            initial_prompt: None,
            max_turns: None,
            background: None,
            effort: None,
            permission_mode: None,
        }
    }

    setters! {
        list tools: String;
        list disallowed_tools: String;
        opt model: String;
        list skills: String;
        opt memory: AgentMemory;
        list mcp_servers: AgentMcpServer;
        opt initial_prompt: String;
        val max_turns: u32;
        opt background: bool;
        opt effort: AgentEffort;
        opt permission_mode: PermissionMode;
    }
}

/// MCP server launched as a child process speaking over stdio.
///
/// It is written to the CLI without a `type` key, which the CLI reads as
/// stdio.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct McpStdioServerConfig {
    /// Command to run.
    pub command: String,
    /// Arguments of the command.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args: Option<Vec<String>>,
    /// Environment of the command.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<IndexMap<String, String>>,
}

impl McpStdioServerConfig {
    /// Creates a configuration from the command to run.
    #[must_use]
    pub fn new(command: impl Into<String>) -> Self {
        Self {
            command: command.into(),
            args: None,
            env: None,
        }
    }

    setters! {
        list args: String;
    }

    /// Sets the environment of the command.
    #[must_use]
    pub fn env(
        mut self,
        env: impl IntoIterator<Item = (impl Into<String>, impl Into<String>)>,
    ) -> Self {
        self.env = Some(
            env.into_iter()
                .map(|(key, value)| (key.into(), value.into()))
                .collect(),
        );
        self
    }
}

/// MCP server reached over a URL, with optional headers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct McpRemoteServerConfig {
    /// URL of the server.
    pub url: String,
    /// HTTP headers sent with each request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub headers: Option<IndexMap<String, String>>,
}

impl McpRemoteServerConfig {
    /// Creates a configuration from the URL of the server.
    #[must_use]
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            headers: None,
        }
    }

    /// Sets the HTTP headers.
    #[must_use]
    pub fn headers(
        mut self,
        headers: impl IntoIterator<Item = (impl Into<String>, impl Into<String>)>,
    ) -> Self {
        self.headers = Some(
            headers
                .into_iter()
                .map(|(key, value)| (key.into(), value.into()))
                .collect(),
        );
        self
    }
}

/// MCP server over server-sent events.
pub type McpSseServerConfig = McpRemoteServerConfig;

/// MCP server over streamable HTTP.
pub type McpHttpServerConfig = McpRemoteServerConfig;

/// Opaque handle to an MCP server that runs inside this process.
///
/// It keeps the server alive and lets the in-process bridge recover its
/// concrete type. A session serves the handles made by
/// [`create_sdk_mcp_server`](crate::create_sdk_mcp_server) and, with the
/// `rmcp` feature, by `McpServerConfig::sdk_rmcp`; it refuses any other
/// with [`Error::Config`](crate::Error::Config).
#[derive(Clone)]
pub struct McpSdkServerInstance(Arc<dyn Any + Send + Sync>);

impl McpSdkServerInstance {
    /// Wraps a server.
    pub fn new<T: Any + Send + Sync>(server: T) -> Self {
        Self(Arc::new(server))
    }

    /// Returns the server when it has type `T`.
    #[must_use]
    pub fn downcast_ref<T: Any + Send + Sync>(&self) -> Option<&T> {
        self.0.downcast_ref()
    }

    pub(crate) fn downcast_arc<T: Any + Send + Sync>(&self) -> Option<Arc<T>> {
        Arc::clone(&self.0).downcast().ok()
    }
}

impl fmt::Debug for McpSdkServerInstance {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("McpSdkServerInstance(..)")
    }
}

/// MCP server that runs inside this process.
///
/// Only `type` and `name` reach the CLI; the instance stays in this process.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct McpSdkServerConfig {
    /// Name of the server.
    pub name: String,
    /// The in-process server.
    pub instance: McpSdkServerInstance,
}

impl McpSdkServerConfig {
    /// Creates a configuration from a name and an in-process server.
    #[must_use]
    pub fn new(name: impl Into<String>, instance: McpSdkServerInstance) -> Self {
        Self {
            name: name.into(),
            instance,
        }
    }
}

/// MCP server configuration.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum McpServerConfig {
    /// A child process speaking over stdio.
    Stdio(McpStdioServerConfig),
    /// A server over server-sent events.
    Sse(McpSseServerConfig),
    /// A server over streamable HTTP.
    Http(McpHttpServerConfig),
    /// A server inside this process.
    Sdk(McpSdkServerConfig),
}

impl McpServerConfig {
    /// Wire value of the `type` key.
    #[must_use]
    pub fn type_name(&self) -> &'static str {
        match self {
            Self::Stdio(_) => "stdio",
            Self::Sse(_) => "sse",
            Self::Http(_) => "http",
            Self::Sdk(_) => "sdk",
        }
    }

    /// Creates the configuration of a server built with the `rmcp` crate
    /// and run inside this process.
    ///
    /// Every request the handler implements is served: tools, resources,
    /// prompts, and the rest. Each query runs the handler over its own
    /// in-memory connection, started by the first message the CLI sends to
    /// the server and closed with the query. Requests and notifications the
    /// server sends to the client (sampling, elicitation, roots, logging,
    /// progress) are not forwarded: notifications are dropped and requests
    /// are refused.
    #[cfg(feature = "rmcp")]
    pub fn sdk_rmcp(name: impl Into<String>, service: impl rmcp::ServerHandler) -> Self {
        Self::Sdk(McpSdkServerConfig::new(
            name,
            McpSdkServerInstance::new(crate::mcp::rmcp_bridge::RmcpServer::new(service)),
        ))
    }
}

impl From<McpStdioServerConfig> for McpServerConfig {
    fn from(config: McpStdioServerConfig) -> Self {
        Self::Stdio(config)
    }
}

impl From<McpSdkServerConfig> for McpServerConfig {
    fn from(config: McpSdkServerConfig) -> Self {
        Self::Sdk(config)
    }
}

impl Serialize for McpServerConfig {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(None)?;
        match self {
            Self::Stdio(config) => {
                map.serialize_entry("command", &config.command)?;
                if let Some(args) = &config.args {
                    map.serialize_entry("args", args)?;
                }
                if let Some(env) = &config.env {
                    map.serialize_entry("env", env)?;
                }
            }
            Self::Sse(config) | Self::Http(config) => {
                map.serialize_entry("type", self.type_name())?;
                map.serialize_entry("url", &config.url)?;
                if let Some(headers) = &config.headers {
                    map.serialize_entry("headers", headers)?;
                }
            }
            Self::Sdk(config) => {
                map.serialize_entry("type", self.type_name())?;
                map.serialize_entry("name", &config.name)?;
            }
        }
        map.end()
    }
}

/// MCP servers of a session.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum McpServers {
    /// Server configurations keyed by server name.
    Map(IndexMap<String, McpServerConfig>),
    /// An inline JSON configuration, passed to the CLI as is.
    Json(String),
    /// Path of a configuration file, passed to the CLI as is.
    Path(PathBuf),
}

impl McpServers {
    /// Whether no server is configured.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        match self {
            Self::Map(servers) => servers.is_empty(),
            Self::Json(json) => json.is_empty(),
            Self::Path(path) => path.as_os_str().is_empty(),
        }
    }
}

impl Default for McpServers {
    fn default() -> Self {
        Self::Map(IndexMap::new())
    }
}

impl From<IndexMap<String, McpServerConfig>> for McpServers {
    fn from(servers: IndexMap<String, McpServerConfig>) -> Self {
        Self::Map(servers)
    }
}

impl From<PathBuf> for McpServers {
    fn from(path: PathBuf) -> Self {
        Self::Path(path)
    }
}

wire_enum! {
    /// Kind of plugin.
    pub enum SdkPluginType {
        /// A plugin loaded from a local directory.
        Local => "local",
    }
}

/// Plugin to load for a session. Only local plugins are supported.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct SdkPluginConfig {
    /// Kind of plugin (`type` on the wire).
    #[serde(rename = "type")]
    pub kind: SdkPluginType,
    /// Path of the plugin directory.
    pub path: String,
}

impl SdkPluginConfig {
    /// Creates a local plugin from its directory.
    #[must_use]
    pub fn local(path: impl Into<String>) -> Self {
        Self {
            kind: SdkPluginType::Local,
            path: path.into(),
        }
    }

    /// Creates a plugin of an arbitrary kind.
    #[must_use]
    pub fn new(kind: impl Into<SdkPluginType>, path: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            path: path.into(),
        }
    }
}

/// Network configuration of the sandbox.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct SandboxNetworkConfig {
    /// Domain names sandboxed processes can reach.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_domains: Option<Vec<String>>,
    /// Domains that are always blocked, even when allowed above.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub denied_domains: Option<Vec<String>>,
    /// In managed settings, honors only the managed allowed domains.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_managed_domains_only: Option<bool>,
    /// Unix socket paths reachable from the sandbox.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_unix_sockets: Option<Vec<String>>,
    /// Allows every Unix socket (less secure).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_all_unix_sockets: Option<bool>,
    /// Allows binding to localhost ports (macOS only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_local_binding: Option<bool>,
    /// Mach service names to allow (macOS only); a trailing wildcard is
    /// supported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_mach_lookup: Option<Vec<String>>,
    /// Port of your own HTTP proxy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http_proxy_port: Option<u16>,
    /// Port of your own SOCKS5 proxy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub socks_proxy_port: Option<u16>,
}

impl SandboxNetworkConfig {
    /// Creates an empty network configuration.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    setters! {
        list allowed_domains: String;
        list denied_domains: String;
        opt allow_managed_domains_only: bool;
        list allow_unix_sockets: String;
        opt allow_all_unix_sockets: bool;
        opt allow_local_binding: bool;
        list allow_mach_lookup: String;
        val http_proxy_port: u16;
        val socks_proxy_port: u16;
    }
}

/// Sandbox violations to leave unreported.
///
/// Maps a command pattern to a list of strings. A violation is left
/// unreported when the command text contains the pattern and the violation
/// text contains one of the strings. The pattern `*` matches every command.
/// Both checks are plain substring matches. This affects only what is
/// reported, not what the sandbox blocks.
pub type SandboxIgnoreViolations = IndexMap<String, Vec<String>>;

/// Sandbox settings: how the CLI isolates shell commands.
///
/// Filesystem and network restrictions of tools are configured through
/// permission rules, not here. The `network` entry configures the network
/// isolation of the sandbox itself.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct SandboxSettings {
    /// Enables shell sandboxing (macOS and Linux only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    /// Approves shell commands automatically when they are sandboxed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_allow_bash_if_sandboxed: Option<bool>,
    /// Commands that run outside the sandbox, such as `git` or `docker`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub excluded_commands: Option<Vec<String>>,
    /// Lets a command opt out of the sandbox. When false, every command
    /// runs sandboxed unless it is excluded above.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_unsandboxed_commands: Option<bool>,
    /// Network configuration of the sandbox.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network: Option<SandboxNetworkConfig>,
    /// Violations to leave unreported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ignore_violations: Option<SandboxIgnoreViolations>,
    /// Enables a weaker sandbox for unprivileged container environments
    /// (Linux only). Reduces security.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enable_weaker_nested_sandbox: Option<bool>,
}

impl SandboxSettings {
    /// Creates empty sandbox settings.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    setters! {
        opt enabled: bool;
        opt auto_allow_bash_if_sandboxed: bool;
        list excluded_commands: String;
        opt allow_unsandboxed_commands: bool;
        opt network: SandboxNetworkConfig;
        opt ignore_violations: SandboxIgnoreViolations;
        opt enable_weaker_nested_sandbox: bool;
    }
}

wire_enum! {
    /// How thinking is shown.
    pub enum ThinkingDisplay {
        /// Thinking is summarized.
        Summarized => "summarized",
        /// Thinking is omitted.
        Omitted => "omitted",
    }
}

/// Thinking behavior of the model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
#[non_exhaustive]
pub enum ThinkingConfig {
    /// The model decides when and how much to think.
    Adaptive {
        /// How thinking is shown.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        display: Option<ThinkingDisplay>,
    },
    /// A fixed thinking token budget.
    Enabled {
        /// Thinking budget in tokens.
        budget_tokens: u64,
        /// How thinking is shown.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        display: Option<ThinkingDisplay>,
    },
    /// No extended thinking.
    Disabled,
}

impl ThinkingConfig {
    /// Adaptive thinking with the default display.
    #[must_use]
    pub fn adaptive() -> Self {
        Self::Adaptive { display: None }
    }

    /// A fixed thinking budget with the default display.
    #[must_use]
    pub fn enabled(budget_tokens: u64) -> Self {
        Self::Enabled {
            budget_tokens,
            display: None,
        }
    }
}

type StderrCallbackFn = dyn Fn(String) -> BoxFuture<'static, ()> + Send + Sync;

/// Callback that receives each line the CLI writes to its error output.
///
/// Any async closure converts into this type; use
/// [`StderrCallback::from_sync`] for a plain closure.
///
/// ```
/// use claude_agent_kit::StderrCallback;
///
/// let from_async = StderrCallback::new(async |line: String| eprintln!("{line}"));
/// let from_sync = StderrCallback::from_sync(|line| eprintln!("{line}"));
/// # let _ = (from_async, from_sync);
/// ```
#[derive(Clone)]
pub struct StderrCallback(Arc<StderrCallbackFn>);

impl StderrCallback {
    /// Wraps an async closure.
    pub fn new<F, Fut>(callback: F) -> Self
    where
        F: Fn(String) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        Self(Arc::new(move |line| Box::pin(callback(line))))
    }

    /// Wraps a plain closure.
    pub fn from_sync<F>(callback: F) -> Self
    where
        F: Fn(&str) + Send + Sync + 'static,
    {
        Self(Arc::new(move |line| {
            callback(&line);
            Box::pin(std::future::ready(()))
        }))
    }

    /// Invokes the callback.
    pub fn call(&self, line: String) -> BoxFuture<'static, ()> {
        (self.0)(line)
    }
}

impl<F, Fut> From<F> for StderrCallback
where
    F: Fn(String) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    fn from(callback: F) -> Self {
        Self::new(callback)
    }
}

impl fmt::Debug for StderrCallback {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("StderrCallback(..)")
    }
}
