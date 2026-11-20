//! Parser that turns one JSON message of CLI output into a typed
//! [`Message`].
//!
//! The parser dispatches on the `type` field by hand and reproduces the
//! required and optional fields of the upstream Python parser. It differs
//! from upstream in four deliberate ways:
//!
//! - a message whose `type` is unknown becomes [`Message::Unknown`] with
//!   its raw JSON instead of being skipped;
//! - a content block whose `type` is unknown becomes
//!   [`ContentBlock::Unknown`] with its raw JSON instead of being dropped.
//!   In a user message only `text`, `tool_use`, and `tool_result` blocks
//!   are read, as upstream reads them; any other block, `thinking`
//!   included, is kept as [`ContentBlock::Unknown`], where upstream drops
//!   it, so its fields are never required;
//! - a required field of the wrong JSON type is an error, and an optional
//!   field of the wrong JSON type reads as absent. An integer field accepts
//!   any JSON number: a fraction is truncated and a value out of range
//!   saturates, so a negative count reads as zero;
//! - `modelUsage` is typed as [`ModelUsage`] per model, where upstream keeps
//!   the dictionary: keys the struct does not model stay reachable in
//!   [`ModelUsage::extra`], and an entry that is not a JSON object is
//!   skipped.

use indexmap::IndexMap;
use serde_json::{Map, Value};

use crate::error::Error;
use crate::session_store::SessionKey;
use crate::types::{
    AssistantMessage, AssistantMessageError, ContentBlock, ConversationResetMessage,
    DeferredToolUse, GenericSystemMessage, HookEventMessage, Message, MessageOrigin,
    MirrorErrorMessage, ModelUsage, RateLimitEvent, RateLimitInfo, RateLimitStatus, RateLimitType,
    ResultMessage, ServerToolName, ServerToolResultBlock, ServerToolUseBlock, StreamEvent,
    SystemMessage, TaskNotificationMessage, TaskNotificationStatus, TaskProgressMessage,
    TaskStartedMessage, TaskUpdatedMessage, TaskUpdatedStatus, TaskUsage, TextBlock, ThinkingBlock,
    ToolResultBlock, ToolResultContent, ToolUseBlock, UserContent, UserMessage,
};

type Parsed<T> = Result<T, String>;

/// Parses one message of CLI output.
///
/// A message whose `type` is unknown to this version of the crate is
/// returned as [`Message::Unknown`] with its raw JSON.
///
/// # Errors
///
/// Returns [`ErrorKind::MessageParse`](crate::ErrorKind::MessageParse), carrying the original data, when the
/// value is not a JSON object, when it has no `type`, or when a message of
/// a known type lacks a required field or has it with the wrong JSON type.
pub fn parse_message(data: Value) -> Result<Message, Error> {
    match parse(&data) {
        Ok(Some(Message::System(mut system))) => {
            system.set_data(data);
            Ok(Message::System(system))
        }
        Ok(Some(message)) => Ok(message),
        Ok(None) => Ok(Message::Unknown(data)),
        Err(message) => Err(Error::message_parse(message, Some(data))),
    }
}

fn parse(data: &Value) -> Parsed<Option<Message>> {
    let Some(object) = data.as_object() else {
        return Err(format!(
            "Invalid message data type (expected dict, got {})",
            python_type_name(data)
        ));
    };
    let message_type = object.get("type");
    let type_name = message_type.and_then(Value::as_str);

    if type_name == Some("system")
        && let Some(subtype @ ("hook_started" | "hook_response")) =
            object.get("subtype").and_then(Value::as_str)
    {
        return Ok(Some(Message::System(parse_hook_event(object, subtype))));
    }

    if !message_type.is_some_and(is_truthy) {
        return Err("Message missing 'type' field".to_owned());
    }

    let message = match type_name {
        Some("user") => Message::User(parse_user(&Fields::new(object, "user"))?),
        Some("assistant") => {
            Message::Assistant(parse_assistant(&Fields::new(object, "assistant"))?)
        }
        Some("system") => Message::System(parse_system(&Fields::new(object, "system"))?),
        Some("result") => Message::Result(parse_result(&Fields::new(object, "result"))?),
        Some("stream_event") => {
            Message::StreamEvent(parse_stream_event(&Fields::new(object, "stream_event"))?)
        }
        Some("rate_limit_event") => {
            Message::RateLimit(parse_rate_limit(&Fields::new(object, "rate_limit_event"))?)
        }
        Some("conversation_reset") => Message::ConversationReset(parse_conversation_reset(
            &Fields::new(object, "conversation_reset"),
        )?),
        _ => return Ok(None),
    };
    Ok(Some(message))
}

fn python_type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "NoneType",
        Value::Bool(_) => "bool",
        Value::Number(number) if number.is_f64() => "float",
        Value::Number(_) => "int",
        Value::String(_) => "str",
        Value::Array(_) => "list",
        Value::Object(_) => "dict",
    }
}

fn is_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::Number(number) => number.as_f64().is_some_and(|number| number != 0.0),
        Value::String(text) => !text.is_empty(),
        Value::Array(items) => !items.is_empty(),
        Value::Object(object) => !object.is_empty(),
    }
}

fn non_null(value: Option<&Value>) -> Option<&Value> {
    value.filter(|value| !value.is_null())
}

fn as_unsigned(value: &Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| value.as_f64().map(|number| number as u64))
}

fn as_signed(value: &Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_f64().map(|number| number as i64))
}

struct Fields<'a> {
    object: &'a Map<String, Value>,
    family: &'static str,
}

impl<'a> Fields<'a> {
    fn new(object: &'a Map<String, Value>, family: &'static str) -> Self {
        Self { object, family }
    }

    fn missing(&self, key: &str) -> String {
        format!("Missing required field in {} message: '{key}'", self.family)
    }

    fn invalid(&self, key: &str, expected: &str, found: &Value) -> String {
        format!(
            "Invalid field in {} message: '{key}' (expected {expected}, got {})",
            self.family,
            python_type_name(found)
        )
    }

    fn get(&self, key: &str) -> Option<&'a Value> {
        self.object.get(key)
    }

    fn required(&self, key: &str) -> Parsed<&'a Value> {
        self.get(key).ok_or_else(|| self.missing(key))
    }

    fn required_str(&self, key: &str) -> Parsed<String> {
        let value = self.required(key)?;
        value
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| self.invalid(key, "str", value))
    }

    fn required_unsigned(&self, key: &str) -> Parsed<u64> {
        let value = self.required(key)?;
        as_unsigned(value).ok_or_else(|| self.invalid(key, "int", value))
    }

    fn required_bool(&self, key: &str) -> Parsed<bool> {
        let value = self.required(key)?;
        value
            .as_bool()
            .ok_or_else(|| self.invalid(key, "bool", value))
    }

    fn required_object(&self, key: &str) -> Parsed<Fields<'a>> {
        let value = self.required(key)?;
        value
            .as_object()
            .map(|object| Fields::new(object, self.family))
            .ok_or_else(|| self.invalid(key, "dict", value))
    }

    fn optional_str(&self, key: &str) -> Option<String> {
        self.get(key).and_then(Value::as_str).map(str::to_owned)
    }

    fn optional_value(&self, key: &str) -> Option<Value> {
        non_null(self.get(key)).cloned()
    }

    fn optional_string_list(&self, key: &str) -> Option<Vec<String>> {
        self.get(key).and_then(Value::as_array).map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
    }
}

fn parse_hook_event(object: &Map<String, Value>, subtype: &str) -> SystemMessage {
    let hook_event_name = ["hook_event", "hook_name", "hook_event_name"]
        .into_iter()
        .filter_map(|key| object.get(key).and_then(Value::as_str))
        .find(|name| !name.is_empty())
        .unwrap_or_default();
    let fields = Fields::new(object, "system");
    SystemMessage::HookEvent(HookEventMessage {
        subtype: subtype.to_owned(),
        data: Value::Null,
        hook_event_name: hook_event_name.to_owned(),
        session_id: fields.optional_str("session_id"),
        uuid: fields.optional_str("uuid"),
    })
}

/// The content blocks a message of one kind reads into typed variants.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Turn {
    /// A user message: `text`, `tool_use`, and `tool_result`, as upstream
    /// reads them.
    User,
    /// An assistant message: every block type the crate models.
    Assistant,
}

fn parse_block(block: &Value, family: &'static str, turn: Turn) -> Parsed<ContentBlock> {
    let Some(object) = block.as_object() else {
        return Err(format!(
            "Invalid content block (expected dict, got {})",
            python_type_name(block)
        ));
    };
    let fields = Fields::new(object, family);
    let kind = fields.required("type")?.as_str();
    if turn == Turn::User && !matches!(kind, Some("text" | "tool_use" | "tool_result")) {
        return Ok(ContentBlock::Unknown(block.clone()));
    }
    let parsed = match kind {
        Some("text") => ContentBlock::Text(TextBlock {
            text: fields.required_str("text")?,
        }),
        Some("thinking") => ContentBlock::Thinking(ThinkingBlock {
            thinking: fields.required_str("thinking")?,
            signature: fields.required_str("signature")?,
        }),
        Some("tool_use") => ContentBlock::ToolUse(ToolUseBlock {
            id: fields.required_str("id")?,
            name: fields.required_str("name")?,
            input: fields.required("input")?.clone(),
        }),
        Some("tool_result") => ContentBlock::ToolResult(ToolResultBlock {
            tool_use_id: fields.required_str("tool_use_id")?,
            content: fields
                .optional_value("content")
                .map(ToolResultContent::from),
            is_error: fields.get("is_error").and_then(Value::as_bool),
        }),
        Some("server_tool_use") => ContentBlock::ServerToolUse(ServerToolUseBlock {
            id: fields.required_str("id")?,
            name: ServerToolName::from(fields.required_str("name")?),
            input: fields.required("input")?.clone(),
        }),
        Some("advisor_tool_result") => ContentBlock::ServerToolResult(ServerToolResultBlock {
            tool_use_id: fields.required_str("tool_use_id")?,
            content: fields.required("content")?.clone(),
        }),
        _ => ContentBlock::Unknown(block.clone()),
    };
    Ok(parsed)
}

fn parse_blocks(blocks: &[Value], family: &'static str, turn: Turn) -> Parsed<Vec<ContentBlock>> {
    blocks
        .iter()
        .map(|block| parse_block(block, family, turn))
        .collect()
}

fn parse_user(fields: &Fields<'_>) -> Parsed<UserMessage> {
    let content = match fields.required_object("message")?.required("content")? {
        Value::Array(blocks) => {
            UserContent::Blocks(parse_blocks(blocks, fields.family, Turn::User)?)
        }
        Value::String(text) => UserContent::Text(text.clone()),
        other => {
            return Err(format!(
                "Invalid user content (expected str or list, got {})",
                python_type_name(other)
            ));
        }
    };
    Ok(UserMessage {
        content,
        uuid: fields.optional_str("uuid"),
        parent_tool_use_id: fields.optional_str("parent_tool_use_id"),
        tool_use_result: fields.optional_value("tool_use_result"),
        origin: MessageOrigin::from_wire(fields.get("origin")),
    })
}

fn parse_assistant(fields: &Fields<'_>) -> Parsed<AssistantMessage> {
    let message = fields.required_object("message")?;
    let content = match message.required("content")? {
        Value::Array(blocks) => parse_blocks(blocks, fields.family, Turn::Assistant)?,
        other => {
            return Err(format!(
                "Invalid assistant content (expected list, got {})",
                python_type_name(other)
            ));
        }
    };
    Ok(AssistantMessage {
        content,
        model: message.required_str("model")?,
        parent_tool_use_id: fields.optional_str("parent_tool_use_id"),
        error: fields
            .optional_str("error")
            .map(AssistantMessageError::from),
        usage: message.optional_value("usage"),
        message_id: message.optional_str("id"),
        stop_reason: message.optional_str("stop_reason"),
        session_id: fields.optional_str("session_id"),
        uuid: fields.optional_str("uuid"),
        user_message_uuid: fields.optional_str("user_message_uuid"),
        user_message_uuids: fields.optional_string_list("user_message_uuids"),
    })
}

fn parse_task_usage(value: &Value) -> TaskUsage {
    let count = |key: &str| value.get(key).and_then(as_unsigned).unwrap_or_default();
    TaskUsage {
        total_tokens: count("total_tokens"),
        tool_uses: count("tool_uses"),
        duration_ms: count("duration_ms"),
    }
}

fn parse_session_key(value: Option<&Value>) -> Option<SessionKey> {
    let object = value?.as_object()?;
    let text = |key: &str| object.get(key).and_then(Value::as_str);
    let key = SessionKey::new(text("project_key")?, text("session_id")?);
    Some(match text("subpath") {
        Some(subpath) => key.with_subpath(subpath),
        None => key,
    })
}

fn parse_system(fields: &Fields<'_>) -> Parsed<SystemMessage> {
    let subtype = fields.required_str("subtype")?;
    let data = Value::Null;
    let message = match subtype.as_str() {
        "task_started" => SystemMessage::TaskStarted(TaskStartedMessage {
            task_id: fields.required_str("task_id")?,
            description: fields.required_str("description")?,
            uuid: fields.required_str("uuid")?,
            session_id: fields.required_str("session_id")?,
            tool_use_id: fields.optional_str("tool_use_id"),
            task_type: fields.optional_str("task_type"),
            subtype,
            data,
        }),
        "task_progress" => SystemMessage::TaskProgress(TaskProgressMessage {
            task_id: fields.required_str("task_id")?,
            description: fields.required_str("description")?,
            usage: parse_task_usage(fields.required("usage")?),
            uuid: fields.required_str("uuid")?,
            session_id: fields.required_str("session_id")?,
            tool_use_id: fields.optional_str("tool_use_id"),
            last_tool_name: fields.optional_str("last_tool_name"),
            subtype,
            data,
        }),
        "task_notification" => SystemMessage::TaskNotification(TaskNotificationMessage {
            task_id: fields.required_str("task_id")?,
            status: TaskNotificationStatus::from(fields.required_str("status")?),
            output_file: fields.required_str("output_file")?,
            summary: fields.required_str("summary")?,
            uuid: fields.required_str("uuid")?,
            session_id: fields.required_str("session_id")?,
            tool_use_id: fields.optional_str("tool_use_id"),
            usage: non_null(fields.get("usage")).map(parse_task_usage),
            subtype,
            data,
        }),
        "task_updated" => {
            let patch = fields
                .get("patch")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default();
            let status = patch
                .get("status")
                .and_then(Value::as_str)
                .map(TaskUpdatedStatus::from);
            SystemMessage::TaskUpdated(TaskUpdatedMessage {
                task_id: fields.optional_str("task_id").unwrap_or_default(),
                patch,
                status,
                session_id: fields.optional_str("session_id"),
                uuid: fields.optional_str("uuid"),
                subtype,
                data,
            })
        }
        "mirror_error" => SystemMessage::MirrorError(MirrorErrorMessage {
            key: parse_session_key(fields.get("key")),
            error: fields.optional_str("error").unwrap_or_default(),
            subtype,
            data,
        }),
        _ => SystemMessage::Other(GenericSystemMessage { subtype, data }),
    };
    Ok(message)
}

const MODEL_USAGE_KEYS: [&str; 10] = [
    "inputTokens",
    "outputTokens",
    "cacheReadInputTokens",
    "cacheCreationInputTokens",
    "webSearchRequests",
    "costUSD",
    "contextWindow",
    "maxOutputTokens",
    "canonicalModel",
    "provider",
];

/// Usage of one model. `None` when the entry is not a JSON object.
fn parse_model_usage(value: &Value) -> Option<ModelUsage> {
    let object = value.as_object()?;
    let count = |key: &str| value.get(key).and_then(as_unsigned).unwrap_or_default();
    let text = |key: &str| value.get(key).and_then(Value::as_str).map(str::to_owned);
    Some(ModelUsage {
        input_tokens: count("inputTokens"),
        output_tokens: count("outputTokens"),
        cache_read_input_tokens: count("cacheReadInputTokens"),
        cache_creation_input_tokens: count("cacheCreationInputTokens"),
        web_search_requests: count("webSearchRequests"),
        cost_usd: value
            .get("costUSD")
            .and_then(Value::as_f64)
            .unwrap_or_default(),
        context_window: count("contextWindow"),
        max_output_tokens: count("maxOutputTokens"),
        canonical_model: text("canonicalModel"),
        provider: text("provider"),
        extra: object
            .iter()
            .filter(|(key, _)| !MODEL_USAGE_KEYS.contains(&key.as_str()))
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
    })
}

fn parse_deferred_tool_use(fields: &Fields<'_>) -> Parsed<Option<DeferredToolUse>> {
    let Some(deferred) = fields
        .get("deferred_tool_use")
        .filter(|value| is_truthy(value))
    else {
        return Ok(None);
    };
    let deferred = deferred
        .as_object()
        .map(|object| Fields::new(object, fields.family))
        .ok_or_else(|| fields.invalid("deferred_tool_use", "dict", deferred))?;
    Ok(Some(DeferredToolUse {
        id: deferred.required_str("id")?,
        name: deferred.required_str("name")?,
        input: deferred.required("input")?.clone(),
    }))
}

fn parse_result(fields: &Fields<'_>) -> Parsed<ResultMessage> {
    let model_usage = fields
        .get("modelUsage")
        .and_then(Value::as_object)
        .map(|models| {
            models
                .iter()
                .filter_map(|(model, usage)| Some((model.clone(), parse_model_usage(usage)?)))
                .collect::<IndexMap<_, _>>()
        });
    Ok(ResultMessage {
        subtype: fields.required_str("subtype")?,
        duration_ms: fields.required_unsigned("duration_ms")?,
        duration_api_ms: fields.required_unsigned("duration_api_ms")?,
        is_error: fields.required_bool("is_error")?,
        num_turns: fields.required_unsigned("num_turns")?,
        session_id: fields.required_str("session_id")?,
        stop_reason: fields.optional_str("stop_reason"),
        total_cost_usd: fields.get("total_cost_usd").and_then(Value::as_f64),
        usage: fields.optional_value("usage"),
        result: fields.optional_str("result"),
        structured_output: fields.optional_value("structured_output"),
        model_usage,
        permission_denials: fields
            .get("permission_denials")
            .and_then(Value::as_array)
            .cloned(),
        deferred_tool_use: parse_deferred_tool_use(fields)?,
        errors: fields.optional_string_list("errors"),
        api_error_status: fields.get("api_error_status").and_then(as_signed),
        uuid: fields.optional_str("uuid"),
        terminal_reason: fields.optional_str("terminal_reason"),
        origin: MessageOrigin::from_wire(fields.get("origin")),
        user_message_uuid: fields.optional_str("user_message_uuid"),
        user_message_uuids: fields.optional_string_list("user_message_uuids"),
    })
}

fn parse_stream_event(fields: &Fields<'_>) -> Parsed<StreamEvent> {
    Ok(StreamEvent {
        uuid: fields.required_str("uuid")?,
        session_id: fields.required_str("session_id")?,
        event: fields.required("event")?.clone(),
        parent_tool_use_id: fields.optional_str("parent_tool_use_id"),
        user_message_uuid: fields.optional_str("user_message_uuid"),
        user_message_uuids: fields.optional_string_list("user_message_uuids"),
    })
}

fn parse_rate_limit(fields: &Fields<'_>) -> Parsed<RateLimitEvent> {
    let raw = fields.required("rate_limit_info")?;
    let info = fields.required_object("rate_limit_info")?;
    let rate_limit_info = RateLimitInfo {
        status: RateLimitStatus::from(info.required_str("status")?),
        resets_at: info.get("resetsAt").and_then(as_signed),
        rate_limit_type: info.optional_str("rateLimitType").map(RateLimitType::from),
        utilization: info.get("utilization").and_then(Value::as_f64),
        overage_status: info
            .optional_str("overageStatus")
            .map(RateLimitStatus::from),
        overage_resets_at: info.get("overageResetsAt").and_then(as_signed),
        overage_disabled_reason: info.optional_str("overageDisabledReason"),
        raw: raw.clone(),
    };
    Ok(RateLimitEvent {
        rate_limit_info,
        uuid: fields.required_str("uuid")?,
        session_id: fields.required_str("session_id")?,
    })
}

fn parse_conversation_reset(fields: &Fields<'_>) -> Parsed<ConversationResetMessage> {
    Ok(ConversationResetMessage {
        new_conversation_id: fields.required_str("new_conversation_id")?,
        uuid: fields.required_str("uuid")?,
        session_id: fields.required_str("session_id")?,
    })
}
