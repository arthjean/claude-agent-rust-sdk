//! MCP servers that run inside this process.
//!
//! The CLI reaches them through the control channel: each JSON-RPC message
//! arrives in an `mcp_message` control request and its answer leaves in the
//! control response. [`create_sdk_mcp_server`] serves [`SdkMcpTool`]s with a
//! native JSON-RPC dispatcher, with no MCP library involved. With the `rmcp`
//! feature, `McpServerConfig::sdk_rmcp`
//! serves a server built with the `rmcp` crate instead.

mod bridge;
#[cfg(feature = "rmcp")]
pub(crate) mod rmcp_bridge;
mod schema;
pub(crate) mod server;
mod tool;

pub(crate) use bridge::McpBridges;
pub use server::create_sdk_mcp_server;
pub use tool::{
    ResourceContents, SdkMcpTool, ToolAnnotations, ToolContent, ToolHandler, ToolResult,
};
