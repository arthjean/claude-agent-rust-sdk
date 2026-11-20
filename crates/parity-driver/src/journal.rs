//! The journal: what the application observed, written for the harness.
//!
//! When `PARITY_JOURNAL` names a file, the driver appends one JSON object
//! per line to it: every message the application received, the return
//! value or the error of every client method it called, and the arguments
//! every callback received. `parity/README.md` documents the format; the
//! Python driver writes the same one, so `cargo xtask parity-diff` can
//! compare the two journals.
//!
//! Values are written in their canonical form: an instance of an SDK type
//! becomes an object with `"$type"`, the name of the Python class, and one
//! key per field of that class, under the Python field name; raw JSON the
//! SDK passes through (a hook input, a `data` frame, a tool input) is
//! written as is; an absent optional value is `null`. This module maps
//! each Rust type onto its Python class by hand, so a field the Rust type
//! loses or adds shows up as a difference.

use std::fs::{File, OpenOptions};
use std::io::Write as _;
use std::sync::{Mutex, OnceLock, PoisonError};

use claude_agent_rust_sdk::{
    ContentBlock, Error, ErrorKind, HookInput, Message, MessageOrigin, PermissionRuleValue,
    PermissionUpdate, RateLimitInfo, SessionKey, SystemMessage, ToolPermissionContext,
    ToolResultContent, UserContent,
};
use serde_json::{Map, Value, json};

/// Environment variable naming the journal file.
const JOURNAL_VARIABLE: &str = "PARITY_JOURNAL";

static JOURNAL: OnceLock<Option<Mutex<File>>> = OnceLock::new();

fn file() -> Option<&'static Mutex<File>> {
    JOURNAL
        .get_or_init(|| {
            let path = std::env::var_os(JOURNAL_VARIABLE)?;
            OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .ok()
                .map(Mutex::new)
        })
        .as_ref()
}

/// Appends one event to the journal; does nothing without a journal.
pub(crate) fn write(event: &str, mut fields: Map<String, Value>) {
    let Some(file) = file() else {
        return;
    };
    fields.insert("event".to_owned(), Value::String(event.to_owned()));
    let mut line = Value::Object(fields).to_string();
    line.push('\n');
    let mut file = file.lock().unwrap_or_else(PoisonError::into_inner);
    let _ = file.write_all(line.as_bytes());
    let _ = file.flush();
}

/// Builds the fields of an event from `(name, value)` pairs.
pub(crate) fn fields<const N: usize>(pairs: [(&str, Value); N]) -> Map<String, Value> {
    pairs
        .into_iter()
        .map(|(name, value)| (name.to_owned(), value))
        .collect()
}

fn typed<const N: usize>(class: &str, pairs: [(&str, Value); N]) -> Value {
    let mut object = fields(pairs);
    object.insert("$type".to_owned(), Value::String(class.to_owned()));
    Value::Object(object)
}

fn text(value: Option<&str>) -> Value {
    value.map_or(Value::Null, |value| Value::String(value.to_owned()))
}

fn strings(value: Option<&Vec<String>>) -> Value {
    value.map_or(Value::Null, |items| json!(items))
}

fn raw(value: Option<&Value>) -> Value {
    value.cloned().unwrap_or(Value::Null)
}

fn serialized<T: serde::Serialize>(value: &T) -> Value {
    serde_json::to_value(value).unwrap_or(Value::Null)
}

fn origin(value: Option<&MessageOrigin>) -> Value {
    value.map_or(Value::Null, |origin| Value::Object(origin.raw.clone()))
}

fn session_key(key: Option<&SessionKey>) -> Value {
    key.map_or(Value::Null, serialized)
}

/// The canonical form of a session key, as `SessionKey` in Python.
pub(crate) fn key(key: &SessionKey) -> Value {
    serialized(key)
}

/// The canonical form of a content block.
pub(crate) fn content_block(block: &ContentBlock) -> Value {
    match block {
        ContentBlock::Text(block) => typed("TextBlock", [("text", json!(block.text))]),
        ContentBlock::Thinking(block) => typed(
            "ThinkingBlock",
            [
                ("thinking", json!(block.thinking)),
                ("signature", json!(block.signature)),
            ],
        ),
        ContentBlock::ToolUse(block) => typed(
            "ToolUseBlock",
            [
                ("id", json!(block.id)),
                ("name", json!(block.name)),
                ("input", block.input.clone()),
            ],
        ),
        ContentBlock::ToolResult(block) => typed(
            "ToolResultBlock",
            [
                ("tool_use_id", json!(block.tool_use_id)),
                (
                    "content",
                    match &block.content {
                        None => Value::Null,
                        Some(ToolResultContent::Text(text)) => json!(text),
                        Some(ToolResultContent::Blocks(blocks)) => json!(blocks),
                        Some(ToolResultContent::Other(other)) => other.clone(),
                        Some(other) => json!(format!("{other:?}")),
                    },
                ),
                ("is_error", json!(block.is_error)),
            ],
        ),
        ContentBlock::ServerToolUse(block) => typed(
            "ServerToolUseBlock",
            [
                ("id", json!(block.id)),
                ("name", json!(block.name.as_str())),
                ("input", block.input.clone()),
            ],
        ),
        ContentBlock::ServerToolResult(block) => typed(
            "ServerToolResultBlock",
            [
                ("tool_use_id", json!(block.tool_use_id)),
                ("content", block.content.clone()),
            ],
        ),
        ContentBlock::Unknown(value) => typed("Unknown", [("data", value.clone())]),
        other => typed("Unmapped", [("debug", json!(format!("{other:?}")))]),
    }
}

fn blocks(blocks: &[ContentBlock]) -> Value {
    blocks.iter().map(content_block).collect()
}

fn system(message: &SystemMessage) -> Value {
    match message {
        SystemMessage::TaskStarted(message) => typed(
            "TaskStartedMessage",
            [
                ("subtype", json!(message.subtype)),
                ("data", message.data.clone()),
                ("task_id", json!(message.task_id)),
                ("description", json!(message.description)),
                ("uuid", json!(message.uuid)),
                ("session_id", json!(message.session_id)),
                ("tool_use_id", text(message.tool_use_id.as_deref())),
                ("task_type", text(message.task_type.as_deref())),
            ],
        ),
        SystemMessage::TaskProgress(message) => typed(
            "TaskProgressMessage",
            [
                ("subtype", json!(message.subtype)),
                ("data", message.data.clone()),
                ("task_id", json!(message.task_id)),
                ("description", json!(message.description)),
                ("usage", serialized(&message.usage)),
                ("uuid", json!(message.uuid)),
                ("session_id", json!(message.session_id)),
                ("tool_use_id", text(message.tool_use_id.as_deref())),
                ("last_tool_name", text(message.last_tool_name.as_deref())),
            ],
        ),
        SystemMessage::TaskNotification(message) => typed(
            "TaskNotificationMessage",
            [
                ("subtype", json!(message.subtype)),
                ("data", message.data.clone()),
                ("task_id", json!(message.task_id)),
                ("status", json!(message.status.as_str())),
                ("output_file", json!(message.output_file)),
                ("summary", json!(message.summary)),
                ("uuid", json!(message.uuid)),
                ("session_id", json!(message.session_id)),
                ("tool_use_id", text(message.tool_use_id.as_deref())),
                (
                    "usage",
                    message.usage.as_ref().map_or(Value::Null, serialized),
                ),
            ],
        ),
        SystemMessage::TaskUpdated(message) => typed(
            "TaskUpdatedMessage",
            [
                ("subtype", json!(message.subtype)),
                ("data", message.data.clone()),
                ("task_id", json!(message.task_id)),
                ("patch", Value::Object(message.patch.clone())),
                (
                    "status",
                    text(message.status.as_ref().map(|status| status.as_str())),
                ),
                ("session_id", text(message.session_id.as_deref())),
                ("uuid", text(message.uuid.as_deref())),
            ],
        ),
        SystemMessage::MirrorError(message) => typed(
            "MirrorErrorMessage",
            [
                ("subtype", json!(message.subtype)),
                ("data", message.data.clone()),
                ("key", session_key(message.key.as_ref())),
                ("error", json!(message.error)),
            ],
        ),
        SystemMessage::HookEvent(message) => typed(
            "HookEventMessage",
            [
                ("subtype", json!(message.subtype)),
                ("data", message.data.clone()),
                ("hook_event_name", json!(message.hook_event_name)),
                ("session_id", text(message.session_id.as_deref())),
                ("uuid", text(message.uuid.as_deref())),
            ],
        ),
        SystemMessage::Other(message) => typed(
            "SystemMessage",
            [
                ("subtype", json!(message.subtype)),
                ("data", message.data.clone()),
            ],
        ),
        other => typed("Unmapped", [("debug", json!(format!("{other:?}")))]),
    }
}

fn rate_limit_info(info: &RateLimitInfo) -> Value {
    typed(
        "RateLimitInfo",
        [
            ("status", json!(info.status.as_str())),
            ("resets_at", json!(info.resets_at)),
            (
                "rate_limit_type",
                text(info.rate_limit_type.as_ref().map(|kind| kind.as_str())),
            ),
            ("utilization", json!(info.utilization)),
            (
                "overage_status",
                text(info.overage_status.as_ref().map(|status| status.as_str())),
            ),
            ("overage_resets_at", json!(info.overage_resets_at)),
            (
                "overage_disabled_reason",
                text(info.overage_disabled_reason.as_deref()),
            ),
            ("raw", info.raw.clone()),
        ],
    )
}

/// The canonical form of a message.
pub(crate) fn message(message: &Message) -> Value {
    match message {
        Message::User(message) => typed(
            "UserMessage",
            [
                (
                    "content",
                    match &message.content {
                        UserContent::Text(text) => json!(text),
                        UserContent::Blocks(items) => blocks(items),
                        other => json!(format!("{other:?}")),
                    },
                ),
                ("uuid", text(message.uuid.as_deref())),
                (
                    "parent_tool_use_id",
                    text(message.parent_tool_use_id.as_deref()),
                ),
                ("tool_use_result", raw(message.tool_use_result.as_ref())),
                ("origin", origin(message.origin.as_ref())),
            ],
        ),
        Message::Assistant(message) => typed(
            "AssistantMessage",
            [
                ("content", blocks(&message.content)),
                ("model", json!(message.model)),
                (
                    "parent_tool_use_id",
                    text(message.parent_tool_use_id.as_deref()),
                ),
                (
                    "error",
                    text(message.error.as_ref().map(|error| error.as_str())),
                ),
                ("usage", raw(message.usage.as_ref())),
                ("message_id", text(message.message_id.as_deref())),
                ("stop_reason", text(message.stop_reason.as_deref())),
                ("session_id", text(message.session_id.as_deref())),
                ("uuid", text(message.uuid.as_deref())),
                (
                    "user_message_uuid",
                    text(message.user_message_uuid.as_deref()),
                ),
                (
                    "user_message_uuids",
                    strings(message.user_message_uuids.as_ref()),
                ),
            ],
        ),
        Message::System(message) => system(message),
        Message::Result(message) => typed(
            "ResultMessage",
            [
                ("subtype", json!(message.subtype)),
                ("duration_ms", json!(message.duration_ms)),
                ("duration_api_ms", json!(message.duration_api_ms)),
                ("is_error", json!(message.is_error)),
                ("num_turns", json!(message.num_turns)),
                ("session_id", json!(message.session_id)),
                ("stop_reason", text(message.stop_reason.as_deref())),
                ("total_cost_usd", json!(message.total_cost_usd)),
                ("usage", raw(message.usage.as_ref())),
                ("result", text(message.result.as_deref())),
                ("structured_output", raw(message.structured_output.as_ref())),
                (
                    "model_usage",
                    message.model_usage.as_ref().map_or(Value::Null, serialized),
                ),
                (
                    "permission_denials",
                    message
                        .permission_denials
                        .as_ref()
                        .map_or(Value::Null, |denials| json!(denials)),
                ),
                (
                    "deferred_tool_use",
                    message
                        .deferred_tool_use
                        .as_ref()
                        .map_or(Value::Null, |tool| {
                            typed(
                                "DeferredToolUse",
                                [
                                    ("id", json!(tool.id)),
                                    ("name", json!(tool.name)),
                                    ("input", tool.input.clone()),
                                ],
                            )
                        }),
                ),
                ("errors", strings(message.errors.as_ref())),
                ("api_error_status", json!(message.api_error_status)),
                ("uuid", text(message.uuid.as_deref())),
                ("terminal_reason", text(message.terminal_reason.as_deref())),
                ("origin", origin(message.origin.as_ref())),
                (
                    "user_message_uuid",
                    text(message.user_message_uuid.as_deref()),
                ),
                (
                    "user_message_uuids",
                    strings(message.user_message_uuids.as_ref()),
                ),
            ],
        ),
        Message::StreamEvent(event) => typed(
            "StreamEvent",
            [
                ("uuid", json!(event.uuid)),
                ("session_id", json!(event.session_id)),
                ("event", event.event.clone()),
                (
                    "parent_tool_use_id",
                    text(event.parent_tool_use_id.as_deref()),
                ),
                (
                    "user_message_uuid",
                    text(event.user_message_uuid.as_deref()),
                ),
                (
                    "user_message_uuids",
                    strings(event.user_message_uuids.as_ref()),
                ),
            ],
        ),
        Message::RateLimit(event) => typed(
            "RateLimitEvent",
            [
                ("rate_limit_info", rate_limit_info(&event.rate_limit_info)),
                ("uuid", json!(event.uuid)),
                ("session_id", json!(event.session_id)),
            ],
        ),
        Message::ConversationReset(message) => typed(
            "ConversationResetMessage",
            [
                ("new_conversation_id", json!(message.new_conversation_id)),
                ("uuid", json!(message.uuid)),
                ("session_id", json!(message.session_id)),
            ],
        ),
        Message::Unknown(value) => typed("Unknown", [("data", value.clone())]),
        other => typed("Unmapped", [("debug", json!(format!("{other:?}")))]),
    }
}

fn rule(rule: &PermissionRuleValue) -> Value {
    typed(
        "PermissionRuleValue",
        [
            ("tool_name", json!(rule.tool_name)),
            ("rule_content", text(rule.rule_content.as_deref())),
        ],
    )
}

/// The canonical form of a permission update.
pub(crate) fn permission_update(update: &PermissionUpdate) -> Value {
    typed(
        "PermissionUpdate",
        [
            ("type", json!(update.kind.as_str())),
            (
                "rules",
                update
                    .rules
                    .as_ref()
                    .map_or(Value::Null, |rules| rules.iter().map(rule).collect()),
            ),
            (
                "behavior",
                text(update.behavior.as_ref().map(|behavior| behavior.as_str())),
            ),
            ("mode", text(update.mode.as_ref().map(|mode| mode.as_str()))),
            ("directories", strings(update.directories.as_ref())),
            (
                "destination",
                text(
                    update
                        .destination
                        .as_ref()
                        .map(|destination| destination.as_str()),
                ),
            ),
        ],
    )
}

/// The canonical form of the context of a permission callback.
pub(crate) fn permission_context(context: &ToolPermissionContext) -> Value {
    typed(
        "ToolPermissionContext",
        [
            ("signal", Value::Null),
            (
                "suggestions",
                context.suggestions.iter().map(permission_update).collect(),
            ),
            ("tool_use_id", text(context.tool_use_id.as_deref())),
            ("agent_id", text(context.agent_id.as_deref())),
            ("blocked_path", text(context.blocked_path.as_deref())),
            ("decision_reason", text(context.decision_reason.as_deref())),
            ("title", text(context.title.as_deref())),
            ("display_name", text(context.display_name.as_deref())),
            ("description", text(context.description.as_deref())),
        ],
    )
}

/// The canonical form of a hook input: the dictionary Python passes on.
pub(crate) fn hook_input(input: &HookInput) -> Value {
    input.to_wire()
}

/// The canonical form of a value the SDK returns as a typed response,
/// such as `McpStatusResponse`: Python returns the dictionary the CLI
/// sent, which the Rust type serializes back to.
pub(crate) fn response<T: serde::Serialize>(value: &T) -> Value {
    serialized(value)
}

/// The kind of an error, as `parity/README.md` names it.
pub(crate) fn error_kind(error: &Error) -> String {
    match error.kind() {
        ErrorKind::CliNotFound => "cli_not_found".to_owned(),
        ErrorKind::Spawn | ErrorKind::NotConnected | ErrorKind::ConnectionClosed => {
            "cli_connection".to_owned()
        }
        ErrorKind::Process => "process".to_owned(),
        ErrorKind::ResultError => "result_error".to_owned(),
        ErrorKind::JsonDecode => "json_decode".to_owned(),
        ErrorKind::MessageParse => "message_parse".to_owned(),
        ErrorKind::Control | ErrorKind::ControlTimeout => "control".to_owned(),
        other => format!("rust:{other:?}"),
    }
}

/// The `error` of a `call` event: its kind, the exit code of a process
/// error, and the text, which the harness does not compare.
pub(crate) fn error(error: &Error) -> Value {
    json!({
        "kind": error_kind(error),
        "exit_code": error.exit_code(),
        "message": error.to_string(),
    })
}
