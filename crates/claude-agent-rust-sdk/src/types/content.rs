//! Content blocks carried by user and assistant messages.

use serde_json::{Map, Value};

use super::macros::wire_enum;

/// One block of message content.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum ContentBlock {
    /// Plain text.
    Text(TextBlock),
    /// Model reasoning.
    Thinking(ThinkingBlock),
    /// A tool call requested by the model.
    ToolUse(ToolUseBlock),
    /// The result of a tool call.
    ToolResult(ToolResultBlock),
    /// A tool call the API executes on the model's behalf.
    ServerToolUse(ServerToolUseBlock),
    /// The result of a server-side tool call.
    ServerToolResult(ServerToolResultBlock),
    /// A block whose `type` is unknown to this version of the crate, kept as
    /// raw JSON.
    Unknown(Value),
}

impl ContentBlock {
    /// Returns the text of a [`ContentBlock::Text`] block.
    #[must_use]
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Self::Text(block) => Some(block.text.as_str()),
            _ => None,
        }
    }
}

/// Text content block.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct TextBlock {
    /// The text.
    pub text: String,
    /// Keys this version of the crate does not model, such as the
    /// `citations` of a text block, kept as the CLI sent them.
    pub extra: Map<String, Value>,
}

impl TextBlock {
    /// Creates a text block.
    #[must_use]
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            extra: Map::new(),
        }
    }
}

/// Thinking content block.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ThinkingBlock {
    /// The reasoning text.
    pub thinking: String,
    /// Signature that authenticates the reasoning.
    pub signature: String,
    /// Keys this version of the crate does not model, such as the
    /// `citations` of a text block, kept as the CLI sent them.
    pub extra: Map<String, Value>,
}

impl ThinkingBlock {
    /// Creates a thinking block.
    #[must_use]
    pub fn new(thinking: impl Into<String>, signature: impl Into<String>) -> Self {
        Self {
            thinking: thinking.into(),
            signature: signature.into(),
            extra: Map::new(),
        }
    }
}

/// Tool use content block.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct ToolUseBlock {
    /// Identifier of the tool call.
    pub id: String,
    /// Name of the tool.
    pub name: String,
    /// Tool input, normally a JSON object.
    pub input: Value,
    /// Keys this version of the crate does not model, such as the
    /// `citations` of a text block, kept as the CLI sent them.
    pub extra: Map<String, Value>,
}

impl ToolUseBlock {
    /// Creates a tool use block.
    #[must_use]
    pub fn new(id: impl Into<String>, name: impl Into<String>, input: Value) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            input,
            extra: Map::new(),
        }
    }
}

/// Tool result content block.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct ToolResultBlock {
    /// Identifier of the tool call this result answers.
    pub tool_use_id: String,
    /// Result content, absent when the wire value is missing or null.
    pub content: Option<ToolResultContent>,
    /// Whether the tool call failed, when reported.
    pub is_error: Option<bool>,
    /// Keys this version of the crate does not model, such as the
    /// `citations` of a text block, kept as the CLI sent them.
    pub extra: Map<String, Value>,
}

impl ToolResultBlock {
    /// Creates a tool result block.
    #[must_use]
    pub fn new(
        tool_use_id: impl Into<String>,
        content: Option<ToolResultContent>,
        is_error: Option<bool>,
    ) -> Self {
        Self {
            tool_use_id: tool_use_id.into(),
            content,
            is_error,
            extra: Map::new(),
        }
    }
}

/// Content of a [`ToolResultBlock`].
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum ToolResultContent {
    /// A plain string.
    Text(String),
    /// A list of content parts, each kept as raw JSON.
    Blocks(Vec<Value>),
    /// Any other JSON value, kept verbatim.
    Other(Value),
}

impl ToolResultContent {
    /// Returns the string of a [`ToolResultContent::Text`] value.
    #[must_use]
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Self::Text(text) => Some(text.as_str()),
            _ => None,
        }
    }
}

impl From<&str> for ToolResultContent {
    fn from(text: &str) -> Self {
        Self::Text(text.to_owned())
    }
}

impl From<String> for ToolResultContent {
    fn from(text: String) -> Self {
        Self::Text(text)
    }
}

impl From<Value> for ToolResultContent {
    fn from(value: Value) -> Self {
        match value {
            Value::String(text) => Self::Text(text),
            Value::Array(items) => Self::Blocks(items),
            other => Self::Other(other),
        }
    }
}

wire_enum! {
    /// Name of a tool the API executes on the model's behalf.
    pub enum ServerToolName {
        /// The advisor tool.
        Advisor => "advisor",
        /// Web search.
        WebSearch => "web_search",
        /// Web fetch.
        WebFetch => "web_fetch",
        /// Code execution.
        CodeExecution => "code_execution",
        /// Shell code execution.
        BashCodeExecution => "bash_code_execution",
        /// Text editor code execution.
        TextEditorCodeExecution => "text_editor_code_execution",
        /// Tool search by regular expression.
        ToolSearchToolRegex => "tool_search_tool_regex",
        /// Tool search by BM25 ranking.
        ToolSearchToolBm25 => "tool_search_tool_bm25",
    }
}

/// Server-side tool use block, such as `advisor` or `web_search`.
///
/// These tools run on the API side, so they appear in the stream next to
/// regular tool calls but the caller never returns a result for them.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct ServerToolUseBlock {
    /// Identifier of the tool call.
    pub id: String,
    /// Which server tool was invoked.
    pub name: ServerToolName,
    /// Tool input, normally a JSON object.
    pub input: Value,
    /// Keys this version of the crate does not model, such as the
    /// `citations` of a text block, kept as the CLI sent them.
    pub extra: Map<String, Value>,
}

impl ServerToolUseBlock {
    /// Creates a server tool use block.
    #[must_use]
    pub fn new(id: impl Into<String>, name: impl Into<ServerToolName>, input: Value) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            input,
            extra: Map::new(),
        }
    }
}

/// Result block returned for a server-side tool call.
///
/// `content` is the raw value from the API; its shape depends on the tool.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct ServerToolResultBlock {
    /// Identifier of the tool call this result answers.
    pub tool_use_id: String,
    /// Raw result, normally a JSON object.
    pub content: Value,
    /// Keys this version of the crate does not model, such as the
    /// `citations` of a text block, kept as the CLI sent them.
    pub extra: Map<String, Value>,
}

impl ServerToolResultBlock {
    /// Creates a server tool result block.
    #[must_use]
    pub fn new(tool_use_id: impl Into<String>, content: Value) -> Self {
        Self {
            tool_use_id: tool_use_id.into(),
            content,
            extra: Map::new(),
        }
    }
}
