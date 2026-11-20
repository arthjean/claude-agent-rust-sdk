//! Tool definitions: the typed and raw constructors, the annotations, and
//! the result a handler returns.

use std::fmt;
use std::future::Future;
use std::sync::Arc;

use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value, json};

use super::schema;
use crate::error::{BoxError, Error, Result};
use crate::types::BoxFuture;
use crate::types::macros::setters;

const MAX_RESULT_SIZE_META_KEY: &str = "anthropic/maxResultSizeChars";

/// Hints about the behavior of a tool, plus `max_result_size_chars`.
///
/// The hints are the standard MCP tool annotations. `max_result_size_chars`
/// is not an MCP hint: it is the size, in characters, up to which the CLI
/// keeps the result of this tool inline instead of saving it to a file and
/// showing a preview. It reaches the CLI in the `_meta` of the tool.
///
/// ```
/// use claude_agent_rust_sdk::ToolAnnotations;
///
/// let annotations = ToolAnnotations::new()
///     .read_only_hint(true)
///     .max_result_size_chars(500_000);
/// assert_eq!(annotations.read_only_hint, Some(true));
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[non_exhaustive]
pub struct ToolAnnotations {
    /// Human-readable title of the tool.
    pub title: Option<String>,
    /// The tool does not modify its environment.
    pub read_only_hint: Option<bool>,
    /// The tool may perform destructive updates.
    pub destructive_hint: Option<bool>,
    /// Calling the tool again with the same arguments has no further effect.
    pub idempotent_hint: Option<bool>,
    /// The tool interacts with entities outside this process.
    pub open_world_hint: Option<bool>,
    /// Size, in characters, up to which the CLI keeps the result inline.
    pub max_result_size_chars: Option<u64>,
}

impl ToolAnnotations {
    /// Creates annotations with nothing set.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    setters! {
        opt title: String;
        val read_only_hint: bool;
        val destructive_hint: bool;
        val idempotent_hint: bool;
        val open_world_hint: bool;
        val max_result_size_chars: u64;
    }

    fn to_wire(&self) -> Option<Value> {
        let mut wire = Map::new();
        if let Some(title) = &self.title {
            wire.insert("title".to_owned(), json!(title));
        }
        for (name, hint) in [
            ("readOnlyHint", self.read_only_hint),
            ("destructiveHint", self.destructive_hint),
            ("idempotentHint", self.idempotent_hint),
            ("openWorldHint", self.open_world_hint),
        ] {
            if let Some(hint) = hint {
                wire.insert(name.to_owned(), json!(hint));
            }
        }
        (!wire.is_empty()).then_some(Value::Object(wire))
    }
}

/// Contents of a resource embedded in a tool result.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ResourceContents {
    /// Text contents.
    Text(String),
    /// Binary contents, encoded in base64.
    Blob(String),
}

/// One block of the content a tool returns.
///
/// The CLI renders text and images. A resource link and a text resource are
/// sent as text, and a binary resource is left out with a warning.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ToolContent {
    /// Plain text.
    #[non_exhaustive]
    Text {
        /// The text.
        text: String,
    },
    /// An image.
    #[non_exhaustive]
    Image {
        /// Image data, encoded in base64.
        data: String,
        /// MIME type of the image.
        mime_type: String,
    },
    /// A link to a resource, sent as text: its name, URI, and description on
    /// separate lines.
    #[non_exhaustive]
    ResourceLink {
        /// Name of the resource.
        name: Option<String>,
        /// URI of the resource.
        uri: Option<String>,
        /// Description of the resource.
        description: Option<String>,
    },
    /// An embedded resource. Text contents are sent as text; binary contents
    /// are left out.
    #[non_exhaustive]
    Resource {
        /// URI of the resource.
        uri: String,
        /// MIME type of the resource.
        mime_type: Option<String>,
        /// Contents of the resource.
        contents: ResourceContents,
    },
}

impl ToolContent {
    /// Creates a text block.
    #[must_use]
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text { text: text.into() }
    }

    /// Creates an image block from base64 data and a MIME type.
    #[must_use]
    pub fn image(data: impl Into<String>, mime_type: impl Into<String>) -> Self {
        Self::Image {
            data: data.into(),
            mime_type: mime_type.into(),
        }
    }

    /// Creates a resource link from a name and a URI.
    #[must_use]
    pub fn resource_link(name: impl Into<String>, uri: impl Into<String>) -> Self {
        Self::ResourceLink {
            name: Some(name.into()),
            uri: Some(uri.into()),
            description: None,
        }
    }

    /// Creates a resource link from its parts, any of which may be missing,
    /// as the MCP wire allows.
    #[must_use]
    pub fn resource_link_parts(
        name: Option<String>,
        uri: Option<String>,
        description: Option<String>,
    ) -> Self {
        Self::ResourceLink {
            name,
            uri,
            description,
        }
    }

    /// Creates an embedded text resource.
    #[must_use]
    pub fn text_resource(uri: impl Into<String>, text: impl Into<String>) -> Self {
        Self::Resource {
            uri: uri.into(),
            mime_type: None,
            contents: ResourceContents::Text(text.into()),
        }
    }

    /// Creates an embedded binary resource from base64 data.
    #[must_use]
    pub fn blob_resource(uri: impl Into<String>, blob: impl Into<String>) -> Self {
        Self::Resource {
            uri: uri.into(),
            mime_type: None,
            contents: ResourceContents::Blob(blob.into()),
        }
    }

    fn to_wire(&self) -> Option<Value> {
        match self {
            Self::Text { text } => Some(json!({"type": "text", "text": text})),
            Self::Image { data, mime_type } => {
                Some(json!({"type": "image", "data": data, "mimeType": mime_type}))
            }
            Self::ResourceLink {
                name,
                uri,
                description,
            } => {
                let parts: Vec<&str> = [name, uri, description]
                    .into_iter()
                    .filter_map(|part| part.as_deref())
                    .filter(|part| !part.is_empty())
                    .collect();
                let text = if parts.is_empty() {
                    "Resource link".to_owned()
                } else {
                    parts.join("\n")
                };
                Some(json!({"type": "text", "text": text}))
            }
            Self::Resource {
                contents: ResourceContents::Text(text),
                ..
            } => Some(json!({"type": "text", "text": text})),
            Self::Resource {
                contents: ResourceContents::Blob(_),
                ..
            } => {
                tracing::warn!("Binary embedded resource cannot be converted to text, skipping");
                None
            }
        }
    }
}

impl From<&str> for ToolContent {
    fn from(text: &str) -> Self {
        Self::text(text)
    }
}

impl From<String> for ToolContent {
    fn from(text: String) -> Self {
        Self::text(text)
    }
}

/// What a tool handler returns: content blocks and an error flag.
///
/// ```
/// use claude_agent_rust_sdk::{ToolContent, ToolResult};
///
/// let result = ToolResult::text("4");
/// assert_eq!(result.content, [ToolContent::text("4")]);
/// assert!(!result.is_error);
///
/// let failure = ToolResult::error("Division by zero");
/// assert!(failure.is_error);
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[non_exhaustive]
pub struct ToolResult {
    /// Content blocks shown to the model.
    pub content: Vec<ToolContent>,
    /// Whether the call failed. The model reads the content as an error.
    pub is_error: bool,
}

impl ToolResult {
    /// Creates a successful result from content blocks.
    #[must_use]
    pub fn new(content: impl IntoIterator<Item = impl Into<ToolContent>>) -> Self {
        Self {
            content: content.into_iter().map(Into::into).collect(),
            is_error: false,
        }
    }

    /// Creates a successful result holding one text block.
    #[must_use]
    pub fn text(text: impl Into<String>) -> Self {
        Self::new([ToolContent::text(text)])
    }

    /// Creates a failed result holding one text block.
    #[must_use]
    pub fn error(text: impl Into<String>) -> Self {
        Self::text(text).is_error(true)
    }

    /// Adds one content block.
    #[must_use]
    pub fn with(mut self, content: impl Into<ToolContent>) -> Self {
        self.content.push(content.into());
        self
    }

    /// Sets the error flag.
    #[must_use]
    pub fn is_error(mut self, is_error: bool) -> Self {
        self.is_error = is_error;
        self
    }

    pub(crate) fn to_wire(&self) -> Value {
        let content: Vec<Value> = self
            .content
            .iter()
            .filter_map(ToolContent::to_wire)
            .collect();
        json!({"content": content, "isError": self.is_error})
    }
}

/// An async function usable as the handler of a tool taking `Args`.
///
/// It is implemented for every function and closure that takes the
/// arguments and returns a future of `Result<ToolResult, BoxError>`. There
/// is no reason to implement it by hand.
pub trait ToolHandler<Args>:
    Fn(Args) -> <Self as ToolHandler<Args>>::Future + Send + Sync + 'static
{
    /// Future the handler returns.
    type Future: Future<Output = std::result::Result<ToolResult, BoxError>> + Send + 'static;
}

impl<Args, F, Fut> ToolHandler<Args> for F
where
    F: Fn(Args) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = std::result::Result<ToolResult, BoxError>> + Send + 'static,
{
    type Future = Fut;
}

pub(crate) enum ToolFailure {
    InvalidInput(String),
    Handler(String),
}

type ErasedHandler =
    dyn Fn(Value) -> BoxFuture<'static, std::result::Result<ToolResult, ToolFailure>> + Send + Sync;

/// A tool served to the CLI from this process.
///
/// [`typed`](Self::typed) derives the input schema from a Rust type and
/// hands the handler a value of that type. [`raw`](Self::raw) takes a JSON
/// Schema and hands the handler the arguments as JSON.
///
/// ```
/// use claude_agent_rust_sdk::{SdkMcpTool, ToolResult};
/// use schemars::JsonSchema;
/// use serde::Deserialize;
///
/// #[derive(Deserialize, JsonSchema)]
/// struct Add {
///     a: f64,
///     b: f64,
/// }
///
/// let add = SdkMcpTool::typed::<Add>("add", "Add two numbers", |args| async move {
///     Ok(ToolResult::text((args.a + args.b).to_string()))
/// });
/// assert_eq!(add.name(), "add");
/// assert_eq!(add.input_schema()["required"], serde_json::json!(["a", "b"]));
/// ```
#[derive(Clone)]
pub struct SdkMcpTool {
    name: String,
    description: String,
    input_schema: Value,
    annotations: Option<ToolAnnotations>,
    handler: Arc<ErasedHandler>,
}

impl SdkMcpTool {
    /// Creates a tool whose input schema is derived from `Args`.
    ///
    /// With the `jsonschema` feature, the arguments of a call are first
    /// validated against the schema, as for [`SdkMcpTool::raw`]. They are
    /// then deserialized into `Args`, after each float without a fractional
    /// part (`3.0`) is turned into an integer where the schema asks for an
    /// integer, since JSON Schema counts it as one. Arguments that do not
    /// deserialize are reported to the model as an error result, and the
    /// handler does not run.
    ///
    /// The arguments of a tool are always a JSON object, so the schema is
    /// listed with the type `object`. The alternatives of an enum are
    /// merged into that object, because the Claude API refuses a tool
    /// schema with `oneOf` or `anyOf` at the top level: the model sees
    /// every field of every variant, and `serde` checks the exact shape. A
    /// type whose schema describes no object (`serde_json::Value`, a scalar,
    /// a sequence, an enum of unit variants) is listed with the schema of an
    /// object that states no property; use [`SdkMcpTool::raw`] to describe
    /// the arguments of such a tool by hand.
    pub fn typed<Args>(
        name: impl Into<String>,
        description: impl Into<String>,
        handler: impl ToolHandler<Args>,
    ) -> Self
    where
        Args: JsonSchema + DeserializeOwned + Send + 'static,
    {
        let input_schema = schema::input_schema_of::<Args>();
        let read_schema = input_schema.clone();
        let handler = move |mut arguments: Value| -> BoxFuture<'static, _> {
            schema::integral_floats_to_integers(&read_schema, &mut arguments);
            match serde_json::from_value::<Args>(arguments) {
                Ok(arguments) => {
                    let call = handler(arguments);
                    Box::pin(async move {
                        call.await
                            .map_err(|error| ToolFailure::Handler(error.to_string()))
                    })
                }
                Err(error) => Box::pin(std::future::ready(Err(ToolFailure::InvalidInput(
                    error.to_string(),
                )))),
            }
        };
        Self {
            name: name.into(),
            description: description.into(),
            input_schema,
            annotations: None,
            handler: Arc::new(handler),
        }
    }

    /// Creates a tool from a JSON Schema and a handler taking the arguments
    /// as JSON.
    ///
    /// With the `jsonschema` feature, which is on by default, the arguments
    /// of a call are validated against the schema before the handler runs,
    /// as in the Python SDK: arguments that break the schema, a schema the
    /// validator cannot compile, and a `$ref` to a remote document (never
    /// fetched) all give an error result, and the handler is not called.
    /// The schema is compiled once, when the server is created.
    ///
    /// In a build with `default-features = false`, arguments are not
    /// validated: the handler receives them as the model sent them.
    ///
    /// The CLI lists a tool only when its schema has `"type": "object"`, and
    /// one tool without it makes the CLI drop every tool of the server, with
    /// no error. A schema with no `type` key, such as `{}`, so gets
    /// `"type": "object"` added in front of its keys; the arguments of a
    /// call are always an object.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::Config`](crate::ErrorKind::Config) when the
    /// schema is not a JSON object, or when its `type` is not `"object"`.
    pub fn raw<F, Fut>(
        name: impl Into<String>,
        description: impl Into<String>,
        input_schema: Value,
        handler: F,
    ) -> Result<Self>
    where
        F: Fn(Value) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = std::result::Result<ToolResult, BoxError>> + Send + 'static,
    {
        let name = name.into();
        let input_schema = object_schema(&name, input_schema)?;
        let handler = move |arguments: Value| -> BoxFuture<'static, _> {
            let call = handler(arguments);
            Box::pin(async move {
                call.await
                    .map_err(|error| ToolFailure::Handler(error.to_string()))
            })
        };
        Ok(Self {
            name,
            description: description.into(),
            input_schema,
            annotations: None,
            handler: Arc::new(handler),
        })
    }

    /// Sets the annotations of the tool.
    #[must_use]
    pub fn annotations(mut self, annotations: ToolAnnotations) -> Self {
        self.annotations = Some(annotations);
        self
    }

    /// Name the model uses to call the tool.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Description shown to the model.
    #[must_use]
    pub fn description(&self) -> &str {
        &self.description
    }

    /// JSON Schema of the arguments.
    #[must_use]
    pub fn input_schema(&self) -> &Value {
        &self.input_schema
    }

    /// Annotations of the tool, when set.
    #[must_use]
    pub fn tool_annotations(&self) -> Option<&ToolAnnotations> {
        self.annotations.as_ref()
    }

    /// The tool as listed to the CLI: `name`, `description`, `inputSchema`,
    /// `annotations`, and `_meta`, with empty fields left out.
    #[must_use]
    pub fn to_wire(&self) -> Value {
        let mut wire = Map::new();
        wire.insert("name".to_owned(), json!(self.name));
        if !self.description.is_empty() {
            wire.insert("description".to_owned(), json!(self.description));
        }
        wire.insert("inputSchema".to_owned(), self.input_schema.clone());
        if let Some(annotations) = &self.annotations {
            if let Some(hints) = annotations.to_wire() {
                wire.insert("annotations".to_owned(), hints);
            }
            if let Some(size) = annotations.max_result_size_chars {
                wire.insert("_meta".to_owned(), json!({MAX_RESULT_SIZE_META_KEY: size}));
            }
        }
        Value::Object(wire)
    }

    pub(crate) fn call(
        &self,
        arguments: Value,
    ) -> BoxFuture<'static, std::result::Result<ToolResult, ToolFailure>> {
        (self.handler)(arguments)
    }
}

impl fmt::Debug for SdkMcpTool {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SdkMcpTool")
            .field("name", &self.name)
            .field("description", &self.description)
            .field("input_schema", &self.input_schema)
            .field("annotations", &self.annotations)
            .finish_non_exhaustive()
    }
}

/// The schema of a [`SdkMcpTool::raw`] tool as the CLI accepts it: its
/// `type` is `"object"`. A schema with no `type`, or with a list of types
/// that holds `"object"`, gets `"type": "object"` as its first key; any
/// other `type` is refused, since the CLI would drop every tool of the
/// server for it.
fn object_schema(name: &str, schema: Value) -> Result<Value> {
    let Value::Object(fields) = schema else {
        return Err(Error::config(format!(
            "The input schema of tool '{name}' must be a JSON object"
        )));
    };
    match fields.get("type") {
        Some(Value::String(kind)) if kind == "object" => return Ok(Value::Object(fields)),
        None => {}
        Some(Value::Array(kinds)) if kinds.iter().any(|kind| kind == "object") => {}
        Some(other) => {
            return Err(Error::config(format!(
                "The input schema of tool '{name}' must have \"type\": \"object\", not {other}: \
                 the CLI drops every tool of a server listing any other type"
            )));
        }
    }
    let mut typed = Map::with_capacity(fields.len() + 1);
    typed.insert("type".to_owned(), json!("object"));
    typed.extend(fields.into_iter().filter(|(key, _)| key != "type"));
    Ok(Value::Object(typed))
}
