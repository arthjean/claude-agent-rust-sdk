//! Writes a typed [`Message`] back as the JSON frame of the CLI it was
//! read from, the inverse of [`parse_message`](crate::parse_message).
//!
//! Every message and content block type implements `Serialize` and
//! `Deserialize` through this module, in the wire form: serializing a
//! message read from the CLI gives back a value equal to the frame, and
//! deserializing goes through the parser, so a value that is not a frame
//! of the expected kind is an error. Upstream messages are dataclasses,
//! which `dataclasses.asdict` turns into a dictionary of their fields; the
//! Rust form is the frame instead, which is what a program relays or logs
//! and what the parser reads back.
//!
//! The rules that make the round trip exact:
//!
//! - typed fields are written under their wire key, an absent optional
//!   field is left out, and `parent_tool_use_id` is written as `null` when
//!   absent on user, assistant, and stream event frames, as the CLI
//!   writes it;
//! - the keys in `extra` and `message_extra` follow the typed fields. A
//!   value kept there because the typed field could not hold it is written
//!   back as sent while the typed field still reads it the same way, and
//!   the typed field wins once the program changes it;
//! - system messages and rate limit information keep the whole frame they
//!   were read from (`data`, `raw`): that frame is written with each typed
//!   field the program changed written over it, so a frame read from the
//!   CLI comes back byte for byte;
//! - key order is the order of the typed fields, then of the extra keys,
//!   not the order of the frame: the round trip keeps the value, not the
//!   key order of a frame the crate rebuilds.

use crate::json_map::RemoveKey;
use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value, json};

use crate::message_parser::{
    self, ASSISTANT_KEYS, ASSISTANT_MESSAGE_KEYS, CONVERSATION_RESET_KEYS, HOOK_EVENT_NAME_KEYS,
    Keys, RATE_LIMIT_KEYS, RESULT_KEYS, Reads, SERVER_TOOL_RESULT_KEYS, STREAM_EVENT_KEYS,
    TEXT_KEYS, THINKING_KEYS, TOOL_RESULT_KEYS, TOOL_USE_KEYS, USER_KEYS, USER_MESSAGE_KEYS,
};
use crate::types::{
    AssistantMessage, ContentBlock, ConversationResetMessage, DeferredToolUse,
    GenericSystemMessage, HookEventMessage, Message, MessageOrigin, MirrorErrorMessage,
    RateLimitEvent, RateLimitInfo, ResultMessage, ServerToolResultBlock, ServerToolUseBlock,
    StreamEvent, SystemMessage, TaskNotificationMessage, TaskProgressMessage, TaskStartedMessage,
    TaskUpdatedMessage, TaskUsage, TextBlock, ThinkingBlock, ToolResultBlock, ToolResultContent,
    ToolUseBlock, UserContent, UserMessage,
};

/// A frame built from typed fields, then from the keys kept in `extra`.
struct Frame<'a> {
    map: Map<String, Value>,
    extra: &'a Map<String, Value>,
    keys: Keys,
}

impl<'a> Frame<'a> {
    fn new(extra: &'a Map<String, Value>, keys: Keys) -> Self {
        Self {
            map: Map::new(),
            extra,
            keys,
        }
    }

    fn put(&mut self, key: &str, value: impl Into<Value>) {
        self.map.insert(key.to_owned(), value.into());
    }

    /// Writes an optional field. A value kept in `extra` under the same
    /// key, which the typed field could not hold, is written instead while
    /// the typed field still reads it as it is now.
    fn optional(&mut self, key: &str, typed: Option<Value>) {
        if let Some(raw) = self.extra.get(key)
            && read_as(self.kind(key), raw) == typed
        {
            self.put(key, raw.clone());
            return;
        }
        if let Some(value) = typed {
            self.put(key, value);
        }
    }

    fn kind(&self, key: &str) -> Reads {
        self.keys
            .iter()
            .find(|(name, _)| *name == key)
            .map_or(Reads::Any, |(_, kind)| *kind)
    }

    /// Adds the keys of `extra` the typed fields do not model.
    fn finish(mut self) -> Map<String, Value> {
        for (key, value) in self.extra {
            let modeled = self.keys.iter().any(|(name, _)| *name == key.as_str());
            if !modeled && !self.map.contains_key(key) {
                self.map.insert(key.clone(), value.clone());
            }
        }
        self.map
    }
}

/// What the typed field reads from a value the parser kept in `extra`.
fn read_as(kind: Reads, raw: &Value) -> Option<Value> {
    match kind {
        Reads::Strings => raw.as_array().map(|items| {
            Value::Array(
                items
                    .iter()
                    .filter(|item| item.is_string())
                    .cloned()
                    .collect(),
            )
        }),
        _ if kind.reads(raw) && !raw.is_null() => Some(raw.clone()),
        _ => None,
    }
}

fn text(value: Option<&String>) -> Option<Value> {
    value.map(|text| Value::String(text.clone()))
}

fn texts(value: Option<&Vec<String>>) -> Option<Value> {
    value.map(|items| items.iter().cloned().map(Value::String).collect())
}

fn number<T: Into<serde_json::Number> + Copy>(value: Option<T>) -> Option<Value> {
    value.map(|number| Value::Number(number.into()))
}

fn float(value: Option<f64>) -> Option<Value> {
    value.map(|number| json!(number))
}

fn origin(value: Option<&MessageOrigin>) -> Option<Value> {
    value.map(origin_to_wire)
}

fn origin_to_wire(origin: &MessageOrigin) -> Value {
    let mut raw = origin.raw.clone();
    let kind = origin.kind.as_str();
    if raw.get("kind").and_then(Value::as_str) != Some(kind) {
        raw.insert("kind".to_owned(), Value::String(kind.to_owned()));
    }
    Value::Object(raw)
}

/// The wire form of one content block.
pub(crate) fn block_to_wire(block: &ContentBlock) -> Value {
    match block {
        ContentBlock::Text(block) => text_block(block),
        ContentBlock::Thinking(block) => thinking_block(block),
        ContentBlock::ToolUse(block) => tool_use_block(block),
        ContentBlock::ToolResult(block) => tool_result_block(block),
        ContentBlock::ServerToolUse(block) => server_tool_use_block(block),
        ContentBlock::ServerToolResult(block) => server_tool_result_block(block),
        ContentBlock::Unknown(raw) => raw.clone(),
    }
}

fn text_block(block: &TextBlock) -> Value {
    let mut frame = Frame::new(&block.extra, TEXT_KEYS);
    frame.put("type", "text");
    frame.put("text", block.text.clone());
    Value::Object(frame.finish())
}

fn thinking_block(block: &ThinkingBlock) -> Value {
    let mut frame = Frame::new(&block.extra, THINKING_KEYS);
    frame.put("type", "thinking");
    frame.put("thinking", block.thinking.clone());
    frame.put("signature", block.signature.clone());
    Value::Object(frame.finish())
}

fn tool_use_block(block: &ToolUseBlock) -> Value {
    let mut frame = Frame::new(&block.extra, TOOL_USE_KEYS);
    frame.put("type", "tool_use");
    frame.put("id", block.id.clone());
    frame.put("name", block.name.clone());
    frame.put("input", block.input.clone());
    Value::Object(frame.finish())
}

fn tool_result_block(block: &ToolResultBlock) -> Value {
    let mut frame = Frame::new(&block.extra, TOOL_RESULT_KEYS);
    frame.put("type", "tool_result");
    frame.put("tool_use_id", block.tool_use_id.clone());
    frame.optional("content", block.content.as_ref().map(tool_result_content));
    frame.optional("is_error", block.is_error.map(Value::Bool));
    Value::Object(frame.finish())
}

fn tool_result_content(content: &ToolResultContent) -> Value {
    match content {
        ToolResultContent::Text(text) => Value::String(text.clone()),
        ToolResultContent::Blocks(blocks) => Value::Array(blocks.clone()),
        ToolResultContent::Other(value) => value.clone(),
    }
}

fn server_tool_use_block(block: &ServerToolUseBlock) -> Value {
    let mut frame = Frame::new(&block.extra, TOOL_USE_KEYS);
    frame.put("type", "server_tool_use");
    frame.put("id", block.id.clone());
    frame.put("name", block.name.as_str());
    frame.put("input", block.input.clone());
    Value::Object(frame.finish())
}

fn server_tool_result_block(block: &ServerToolResultBlock) -> Value {
    let mut frame = Frame::new(&block.extra, SERVER_TOOL_RESULT_KEYS);
    frame.put("type", "advisor_tool_result");
    frame.put("tool_use_id", block.tool_use_id.clone());
    frame.put("content", block.content.clone());
    Value::Object(frame.finish())
}

fn blocks(blocks: &[ContentBlock]) -> Value {
    Value::Array(blocks.iter().map(block_to_wire).collect())
}

/// The wire form of one message.
pub(crate) fn message_to_wire(message: &Message) -> Value {
    match message {
        Message::User(message) => user(message),
        Message::Assistant(message) => assistant(message),
        Message::System(message) => system(message),
        Message::Result(message) => result(message),
        Message::StreamEvent(message) => stream_event(message),
        Message::RateLimit(message) => rate_limit(message),
        Message::ConversationReset(message) => conversation_reset(message),
        Message::Unknown(raw) => raw.clone(),
    }
}

fn user(message: &UserMessage) -> Value {
    let mut body = Frame::new(&message.message_extra, USER_MESSAGE_KEYS);
    let content = match &message.content {
        UserContent::Text(text) => Value::String(text.clone()),
        UserContent::Blocks(content) => blocks(content),
    };
    body.put("content", content);

    let mut frame = Frame::new(&message.extra, USER_KEYS);
    frame.put("type", "user");
    frame.put("message", Value::Object(body.finish()));
    frame.optional("uuid", text(message.uuid.as_ref()));
    frame.put(
        "parent_tool_use_id",
        text(message.parent_tool_use_id.as_ref()).unwrap_or(Value::Null),
    );
    frame.optional("tool_use_result", message.tool_use_result.clone());
    frame.optional("origin", origin(message.origin.as_ref()));
    Value::Object(frame.finish())
}

fn assistant(message: &AssistantMessage) -> Value {
    let mut body = Frame::new(&message.message_extra, ASSISTANT_MESSAGE_KEYS);
    body.put("content", blocks(&message.content));
    body.put("model", message.model.clone());
    body.optional("usage", message.usage.clone().map(Value::from));
    body.optional("id", text(message.message_id.as_ref()));
    body.optional("stop_reason", text(message.stop_reason.as_ref()));

    let mut frame = Frame::new(&message.extra, ASSISTANT_KEYS);
    frame.put("type", "assistant");
    frame.put("message", Value::Object(body.finish()));
    frame.put(
        "parent_tool_use_id",
        text(message.parent_tool_use_id.as_ref()).unwrap_or(Value::Null),
    );
    frame.optional(
        "error",
        message
            .error
            .as_ref()
            .map(|error| Value::String(error.as_str().to_owned())),
    );
    frame.optional("session_id", text(message.session_id.as_ref()));
    frame.optional("uuid", text(message.uuid.as_ref()));
    frame.optional(
        "user_message_uuid",
        text(message.user_message_uuid.as_ref()),
    );
    frame.optional(
        "user_message_uuids",
        texts(message.user_message_uuids.as_ref()),
    );
    Value::Object(frame.finish())
}

fn result(message: &ResultMessage) -> Value {
    let mut frame = Frame::new(&message.extra, RESULT_KEYS);
    frame.put("type", "result");
    frame.put("subtype", message.subtype.clone());
    frame.put("duration_ms", message.duration_ms);
    frame.put("duration_api_ms", message.duration_api_ms);
    frame.put("is_error", message.is_error);
    frame.put("num_turns", message.num_turns);
    frame.put("session_id", message.session_id.clone());
    frame.optional("stop_reason", text(message.stop_reason.as_ref()));
    frame.optional("total_cost_usd", float(message.total_cost_usd));
    frame.optional("usage", message.usage.clone().map(Value::from));
    frame.optional("result", text(message.result.as_ref()));
    frame.optional("structured_output", message.structured_output.clone());
    frame.optional(
        "modelUsage",
        message.model_usage.as_ref().map(|models| {
            Value::Object(
                models
                    .iter()
                    .map(|(model, usage)| (model.clone(), json!(usage)))
                    .collect(),
            )
        }),
    );
    frame.optional(
        "permission_denials",
        message.permission_denials.clone().map(Value::Array),
    );
    frame.optional(
        "deferred_tool_use",
        message.deferred_tool_use.as_ref().map(deferred_tool_use),
    );
    frame.optional("errors", texts(message.errors.as_ref()));
    frame.optional("api_error_status", number(message.api_error_status));
    frame.optional("uuid", text(message.uuid.as_ref()));
    frame.optional("terminal_reason", text(message.terminal_reason.as_ref()));
    frame.optional("origin", origin(message.origin.as_ref()));
    frame.optional(
        "user_message_uuid",
        text(message.user_message_uuid.as_ref()),
    );
    frame.optional(
        "user_message_uuids",
        texts(message.user_message_uuids.as_ref()),
    );
    Value::Object(frame.finish())
}

fn deferred_tool_use(deferred: &DeferredToolUse) -> Value {
    json!({"id": deferred.id, "name": deferred.name, "input": deferred.input})
}

fn stream_event(message: &StreamEvent) -> Value {
    let mut frame = Frame::new(&message.extra, STREAM_EVENT_KEYS);
    frame.put("type", "stream_event");
    frame.put("uuid", message.uuid.clone());
    frame.put("session_id", message.session_id.clone());
    frame.put("event", message.event.clone());
    frame.put(
        "parent_tool_use_id",
        text(message.parent_tool_use_id.as_ref()).unwrap_or(Value::Null),
    );
    frame.optional(
        "user_message_uuid",
        text(message.user_message_uuid.as_ref()),
    );
    frame.optional(
        "user_message_uuids",
        texts(message.user_message_uuids.as_ref()),
    );
    Value::Object(frame.finish())
}

fn rate_limit(message: &RateLimitEvent) -> Value {
    let mut frame = Frame::new(&message.extra, RATE_LIMIT_KEYS);
    frame.put("type", "rate_limit_event");
    frame.put("rate_limit_info", rate_limit_info(&message.rate_limit_info));
    frame.put("uuid", message.uuid.clone());
    frame.put("session_id", message.session_id.clone());
    Value::Object(frame.finish())
}

fn rate_limit_info(info: &RateLimitInfo) -> Value {
    let mut raw = Overlay::new(&info.raw);
    raw.text("status", Some(info.status.as_str()));
    raw.signed("resetsAt", info.resets_at);
    raw.text(
        "rateLimitType",
        info.rate_limit_type.as_ref().map(|kind| kind.as_str()),
    );
    raw.float("utilization", info.utilization);
    raw.text(
        "overageStatus",
        info.overage_status.as_ref().map(|status| status.as_str()),
    );
    raw.signed("overageResetsAt", info.overage_resets_at);
    raw.text(
        "overageDisabledReason",
        info.overage_disabled_reason.as_deref(),
    );
    Value::Object(raw.map)
}

fn conversation_reset(message: &ConversationResetMessage) -> Value {
    let mut frame = Frame::new(&message.extra, CONVERSATION_RESET_KEYS);
    frame.put("type", "conversation_reset");
    frame.put("new_conversation_id", message.new_conversation_id.clone());
    frame.put("uuid", message.uuid.clone());
    frame.put("session_id", message.session_id.clone());
    Value::Object(frame.finish())
}

/// A frame kept whole, with each typed field written over it only where
/// the field no longer reads what the frame holds.
struct Overlay {
    map: Map<String, Value>,
}

impl Overlay {
    fn new(raw: &Value) -> Self {
        Self {
            map: raw.as_object().cloned().unwrap_or_default(),
        }
    }

    fn text(&mut self, key: &str, typed: Option<&str>) {
        let held = self.map.get(key).and_then(Value::as_str);
        if held == typed {
            return;
        }
        match typed {
            Some(text) => {
                self.map
                    .insert(key.to_owned(), Value::String(text.to_owned()));
            }
            None => {
                self.map.remove_key(key);
            }
        }
    }

    fn signed(&mut self, key: &str, typed: Option<i64>) {
        let held = self.map.get(key).and_then(|value| {
            value
                .as_i64()
                .or_else(|| value.as_f64().map(|number| number as i64))
        });
        if held != typed {
            self.set(key, typed.map(Value::from));
        }
    }

    fn float(&mut self, key: &str, typed: Option<f64>) {
        let held = self.map.get(key).and_then(Value::as_f64);
        if held != typed {
            self.set(key, typed.map(|number| json!(number)));
        }
    }

    fn value(&mut self, key: &str, typed: Value, held: impl FnOnce(Option<&Value>) -> bool) {
        if !held(self.map.get(key)) {
            self.map.insert(key.to_owned(), typed);
        }
    }

    fn set(&mut self, key: &str, value: Option<Value>) {
        match value {
            Some(value) => {
                self.map.insert(key.to_owned(), value);
            }
            None => {
                self.map.remove_key(key);
            }
        }
    }
}

fn system(message: &SystemMessage) -> Value {
    let (subtype, data) = (message.subtype(), message.data());
    let mut frame = Overlay::new(data);
    frame.text("type", Some("system"));
    frame.text("subtype", Some(subtype));
    match message {
        SystemMessage::TaskStarted(message) => task_started(&mut frame, message),
        SystemMessage::TaskProgress(message) => task_progress(&mut frame, message),
        SystemMessage::TaskNotification(message) => task_notification(&mut frame, message),
        SystemMessage::TaskUpdated(message) => task_updated(&mut frame, message),
        SystemMessage::MirrorError(message) => mirror_error(&mut frame, message),
        SystemMessage::HookEvent(message) => hook_event(&mut frame, message),
        SystemMessage::Other(GenericSystemMessage { .. }) => {}
    }
    Value::Object(frame.map)
}

fn usage(frame: &mut Overlay, key: &str, typed: Option<&TaskUsage>) {
    match typed {
        Some(typed) => frame.value(key, json!(typed), |held| {
            held.is_some_and(|held| {
                held.is_object() && message_parser::read_task_usage(held) == *typed
            })
        }),
        None => {
            if frame.map.get(key).is_some_and(Value::is_object) {
                frame.map.remove_key(key);
            }
        }
    }
}

fn task_started(frame: &mut Overlay, message: &TaskStartedMessage) {
    frame.text("task_id", Some(&message.task_id));
    frame.text("description", Some(&message.description));
    frame.text("uuid", Some(&message.uuid));
    frame.text("session_id", Some(&message.session_id));
    frame.text("tool_use_id", message.tool_use_id.as_deref());
    frame.text("task_type", message.task_type.as_deref());
}

fn task_progress(frame: &mut Overlay, message: &TaskProgressMessage) {
    frame.text("task_id", Some(&message.task_id));
    frame.text("description", Some(&message.description));
    let typed = message.usage;
    frame.value("usage", json!(typed), |held| {
        held.is_some_and(|held| message_parser::read_task_usage(held) == typed)
    });
    frame.text("uuid", Some(&message.uuid));
    frame.text("session_id", Some(&message.session_id));
    frame.text("tool_use_id", message.tool_use_id.as_deref());
    frame.text("last_tool_name", message.last_tool_name.as_deref());
}

fn task_notification(frame: &mut Overlay, message: &TaskNotificationMessage) {
    frame.text("task_id", Some(&message.task_id));
    frame.text("status", Some(message.status.as_str()));
    frame.text("output_file", Some(&message.output_file));
    frame.text("summary", Some(&message.summary));
    frame.text("uuid", Some(&message.uuid));
    frame.text("session_id", Some(&message.session_id));
    frame.text("tool_use_id", message.tool_use_id.as_deref());
    usage(frame, "usage", message.usage.as_ref());
}

fn task_updated(frame: &mut Overlay, message: &TaskUpdatedMessage) {
    let mut patch = message.patch.clone();
    let status = message.status.as_ref().map(|status| status.as_str());
    if patch.get("status").and_then(Value::as_str) != status {
        match status {
            Some(status) => {
                patch.insert("status".to_owned(), Value::String(status.to_owned()));
            }
            None => {
                patch.remove_key("status");
            }
        }
    }
    let held_patch = frame.map.get("patch").and_then(Value::as_object);
    if held_patch != Some(&patch) && !(held_patch.is_none() && patch.is_empty()) {
        frame.map.insert("patch".to_owned(), Value::Object(patch));
    }
    let held_id = frame.map.get("task_id").and_then(Value::as_str);
    if held_id.unwrap_or_default() != message.task_id {
        frame
            .map
            .insert("task_id".to_owned(), Value::String(message.task_id.clone()));
    }
    frame.text("session_id", message.session_id.as_deref());
    frame.text("uuid", message.uuid.as_deref());
}

fn mirror_error(frame: &mut Overlay, message: &MirrorErrorMessage) {
    if message_parser::read_session_key(frame.map.get("key")) != message.key {
        frame.set("key", message.key.as_ref().map(|key| json!(key)));
    }
    let held = frame.map.get("error").and_then(Value::as_str);
    if held.unwrap_or_default() != message.error {
        frame
            .map
            .insert("error".to_owned(), Value::String(message.error.clone()));
    }
}

fn hook_event(frame: &mut Overlay, message: &HookEventMessage) {
    if message_parser::read_hook_event_name(&frame.map) != message.hook_event_name {
        for key in HOOK_EVENT_NAME_KEYS {
            frame.map.remove_key(key);
        }
        if !message.hook_event_name.is_empty() {
            frame.map.insert(
                "hook_event_name".to_owned(),
                Value::String(message.hook_event_name.clone()),
            );
        }
    }
    frame.text("session_id", message.session_id.as_deref());
    frame.text("uuid", message.uuid.as_deref());
}

/// `Serialize` in the wire form and `Deserialize` through the parser, for
/// a message type and for the [`Message`] variant that holds it.
macro_rules! wire_message {
    ($($ty:ty => $variant:path, $what:literal;)*) => {$(
        impl Serialize for $ty {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                message_to_wire(&$variant(self.clone())).serialize(serializer)
            }
        }

        impl<'de> Deserialize<'de> for $ty {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                match Message::deserialize(deserializer)? {
                    $variant(message) => Ok(message),
                    _ => Err(D::Error::custom(concat!("expected ", $what))),
                }
            }
        }
    )*};
}

wire_message! {
    UserMessage => Message::User, "a user message";
    AssistantMessage => Message::Assistant, "an assistant message";
    SystemMessage => Message::System, "a system message";
    ResultMessage => Message::Result, "a result message";
    StreamEvent => Message::StreamEvent, "a stream event";
    RateLimitEvent => Message::RateLimit, "a rate limit event";
    ConversationResetMessage => Message::ConversationReset, "a conversation reset message";
}

impl Serialize for Message {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        message_to_wire(self).serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Message {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let frame = Value::deserialize(deserializer)?;
        crate::parse_message(frame).map_err(D::Error::custom)
    }
}

/// `Serialize` in the wire form and `Deserialize` through the parser, for
/// a block type and for the [`ContentBlock`] variant that holds it.
macro_rules! wire_block {
    ($($ty:ty => $variant:path, $what:literal;)*) => {$(
        impl Serialize for $ty {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                block_to_wire(&$variant(self.clone())).serialize(serializer)
            }
        }

        impl<'de> Deserialize<'de> for $ty {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                match ContentBlock::deserialize(deserializer)? {
                    $variant(block) => Ok(block),
                    _ => Err(D::Error::custom(concat!("expected ", $what))),
                }
            }
        }
    )*};
}

wire_block! {
    TextBlock => ContentBlock::Text, "a text block";
    ThinkingBlock => ContentBlock::Thinking, "a thinking block";
    ToolUseBlock => ContentBlock::ToolUse, "a tool use block";
    ToolResultBlock => ContentBlock::ToolResult, "a tool result block";
    ServerToolUseBlock => ContentBlock::ServerToolUse, "a server tool use block";
    ServerToolResultBlock => ContentBlock::ServerToolResult, "a server tool result block";
}

impl Serialize for ContentBlock {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        block_to_wire(self).serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for ContentBlock {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let block = Value::deserialize(deserializer)?;
        message_parser::parse_content_block(&block).map_err(D::Error::custom)
    }
}

impl Serialize for ToolResultContent {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        tool_result_content(self).serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for ToolResultContent {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Value::deserialize(deserializer).map(Self::from)
    }
}

impl Serialize for MessageOrigin {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        origin_to_wire(self).serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for MessageOrigin {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = Value::deserialize(deserializer)?;
        Self::from_wire(Some(&raw))
            .ok_or_else(|| D::Error::custom("expected an object with a string `kind`"))
    }
}

impl Serialize for RateLimitInfo {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        rate_limit_info(self).serialize(serializer)
    }
}

impl Serialize for DeferredToolUse {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        deferred_tool_use(self).serialize(serializer)
    }
}

impl Serialize for UserContent {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Text(text) => text.serialize(serializer),
            Self::Blocks(content) => blocks(content).serialize(serializer),
        }
    }
}
