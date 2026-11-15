//! What a session is asked: a text prompt or a stream of input messages.

use std::fmt;
use std::pin::Pin;

use futures_core::Stream;
use futures_util::StreamExt;
use serde_json::{Map, Value, json};

use crate::error::BoxError;

const CLIENT_COMPOSED_KEY: &str = "client_composed";
const SESSION_ID_KEY: &str = "session_id";

/// Stream of input messages, each one a JSON object written to the CLI as
/// is. An error ends the input.
pub type InputStream = Pin<Box<dyn Stream<Item = Result<Value, BoxError>> + Send>>;

/// Prompt of a query: a text, or a stream of input messages.
///
/// A string converts into a prompt. A stream is wrapped with
/// [`Prompt::stream`], or with [`Prompt::try_stream`] when it can fail.
///
/// ```
/// use claude_agent_kit::{InputMessage, Prompt};
/// use futures_util::stream;
///
/// let text: Prompt = "What is 2 + 2?".into();
/// let stream = Prompt::stream(stream::iter([
///     InputMessage::user("Hello"),
///     InputMessage::user("How are you?"),
/// ]));
/// # let _ = (text, stream);
/// ```
#[non_exhaustive]
pub enum Prompt {
    /// One user message with this text.
    Text(String),
    /// Input messages, written in order as they arrive.
    Stream(InputStream),
}

impl Prompt {
    /// Wraps a stream of input messages.
    ///
    /// Each item is an [`InputMessage`] or a raw JSON object such as
    /// `{"type": "user", "message": {"role": "user", "content": "..."}}`.
    pub fn stream<S, T>(stream: S) -> Self
    where
        S: Stream<Item = T> + Send + 'static,
        T: Into<Value>,
    {
        Self::Stream(Box::pin(stream.map(|message| Ok(message.into()))))
    }

    /// Wraps a stream of input messages that can fail. The first error ends
    /// the input.
    pub fn try_stream<S, T, E>(stream: S) -> Self
    where
        S: Stream<Item = Result<T, E>> + Send + 'static,
        T: Into<Value>,
        E: Into<BoxError>,
    {
        Self::Stream(Box::pin(
            stream.map(|message| message.map(Into::into).map_err(Into::into)),
        ))
    }
}

impl From<&str> for Prompt {
    fn from(text: &str) -> Self {
        Self::Text(text.to_owned())
    }
}

impl From<String> for Prompt {
    fn from(text: String) -> Self {
        Self::Text(text)
    }
}

impl From<&String> for Prompt {
    fn from(text: &String) -> Self {
        Self::Text(text.clone())
    }
}

impl From<InputStream> for Prompt {
    fn from(stream: InputStream) -> Self {
        Self::Stream(stream)
    }
}

impl fmt::Debug for Prompt {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Text(text) => formatter.debug_tuple("Text").field(text).finish(),
            Self::Stream(_) => formatter.write_str("Stream(..)"),
        }
    }
}

/// One user message of an input stream.
///
/// It converts into the JSON object the CLI reads:
///
/// ```
/// use claude_agent_kit::InputMessage;
/// use serde_json::{Value, json};
///
/// let message: Value = InputMessage::user("Hello").session_id("main").into();
/// assert_eq!(
///     message,
///     json!({
///         "type": "user",
///         "message": {"role": "user", "content": "Hello"},
///         "parent_tool_use_id": null,
///         "session_id": "main",
///     })
/// );
/// ```
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct InputMessage {
    /// Content of the message: a text, or a list of content blocks.
    pub content: Value,
    /// Session the message belongs to. When unset, the key is left out and
    /// [`ClaudeSdkClient::query`](crate::ClaudeSdkClient::query) fills it.
    pub session_id: Option<String>,
    /// Tool call this message answers, when it is a tool result.
    pub parent_tool_use_id: Option<String>,
}

impl InputMessage {
    /// Creates a user message. A string gives a text message; a JSON array
    /// gives content blocks.
    #[must_use]
    pub fn user(content: impl Into<Value>) -> Self {
        Self {
            content: content.into(),
            session_id: None,
            parent_tool_use_id: None,
        }
    }

    /// Sets the session the message belongs to.
    #[must_use]
    pub fn session_id(mut self, session_id: impl Into<String>) -> Self {
        self.session_id = Some(session_id.into());
        self
    }

    /// Sets the tool call this message answers.
    #[must_use]
    pub fn parent_tool_use_id(mut self, tool_use_id: impl Into<String>) -> Self {
        self.parent_tool_use_id = Some(tool_use_id.into());
        self
    }

    /// Converts the message to the JSON object the CLI reads.
    #[must_use]
    pub fn to_wire(&self) -> Value {
        let mut wire = Map::new();
        wire.insert("type".to_owned(), json!("user"));
        wire.insert(
            "message".to_owned(),
            json!({"role": "user", "content": self.content}),
        );
        wire.insert(
            "parent_tool_use_id".to_owned(),
            json!(self.parent_tool_use_id),
        );
        if let Some(session_id) = &self.session_id {
            wire.insert(SESSION_ID_KEY.to_owned(), json!(session_id));
        }
        Value::Object(wire)
    }
}

impl From<InputMessage> for Value {
    fn from(message: InputMessage) -> Self {
        message.to_wire()
    }
}

/// User message written for a text prompt.
pub(crate) fn text_message(text: &str, session_id: &str) -> Value {
    json!({
        "type": "user",
        "session_id": session_id,
        "message": {"role": "user", "content": text},
        "parent_tool_use_id": null,
    })
}

/// Marks an outgoing user message as composed by the client, so the CLI
/// delivers it as written. Any value the caller supplied is overwritten.
pub(crate) fn stamp_user_message(mut message: Value, verbatim_prompts: bool) -> Value {
    if verbatim_prompts && let Value::Object(fields) = &mut message {
        fields.insert(CLIENT_COMPOSED_KEY.to_owned(), Value::Bool(true));
    }
    message
}

/// Sets the session of a message that names none.
pub(crate) fn with_default_session(mut message: Value, session_id: &str) -> Value {
    if let Value::Object(fields) = &mut message
        && !fields.contains_key(SESSION_ID_KEY)
    {
        fields.insert(SESSION_ID_KEY.to_owned(), json!(session_id));
    }
    message
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{stamp_user_message, with_default_session};

    #[test]
    fn stamping_overwrites_the_caller_value_only_when_enabled() {
        let message = json!({"type": "user", "client_composed": false});
        assert_eq!(stamp_user_message(message.clone(), false), message);
        assert_eq!(
            stamp_user_message(message, true),
            json!({"type": "user", "client_composed": true})
        );
        assert_eq!(stamp_user_message(json!("text"), true), json!("text"));
    }

    #[test]
    fn default_session_never_replaces_a_given_one() {
        assert_eq!(
            with_default_session(json!({"type": "user"}), "default"),
            json!({"type": "user", "session_id": "default"})
        );
        assert_eq!(
            with_default_session(json!({"session_id": "mine"}), "default"),
            json!({"session_id": "mine"})
        );
    }
}
