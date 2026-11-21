//! Hook events, hook inputs, hook outputs, and hook callbacks.

use std::fmt;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value};

use super::BoxFuture;
use super::macros::{extra_field, setters, wire_enum};
use super::permission::PermissionBehavior;
use crate::error::BoxError;

const HOOK_EVENT_NAME_KEY: &str = "hook_event_name";

wire_enum! {
    /// Event a hook callback can be registered for.
    pub enum HookEvent {
        /// Before a tool runs.
        PreToolUse => "PreToolUse",
        /// After a tool ran successfully.
        PostToolUse => "PostToolUse",
        /// After a tool failed.
        PostToolUseFailure => "PostToolUseFailure",
        /// When the user submits a prompt.
        UserPromptSubmit => "UserPromptSubmit",
        /// When the main agent stops.
        Stop => "Stop",
        /// When a subagent stops.
        SubagentStop => "SubagentStop",
        /// Before the conversation is compacted.
        PreCompact => "PreCompact",
        /// When the CLI raises a notification.
        Notification => "Notification",
        /// When a subagent starts.
        SubagentStart => "SubagentStart",
        /// When a tool call needs a permission decision.
        PermissionRequest => "PermissionRequest",
    }
}

/// Reasoning effort in effect when a hook fires.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[non_exhaustive]
pub struct HookEffort {
    /// Effort level for the current turn, such as `low` or `high`, after
    /// any downgrade for the selected model.
    pub level: String,
    /// Fields this version of the crate does not model, kept verbatim.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// Reads a duration in milliseconds from any JSON number.
fn lenient_milliseconds<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<u64>, D::Error> {
    let value = Option::<Value>::deserialize(deserializer)?;
    Ok(value.and_then(|value| {
        value
            .as_u64()
            .or_else(|| value.as_f64().map(|number| number as u64))
    }))
}

/// Reads a string a callback can do without: absent or `null` reads as
/// the empty string, so a CLI that drops or nulls it does not turn the
/// whole input into [`HookInput::Unknown`]. Another JSON type is still an
/// error.
fn lenient_string<'de, D: Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
    Ok(Option::<String>::deserialize(deserializer)?.unwrap_or_default())
}

/// Reads the effort of a hook input from an object or a bare level.
fn lenient_effort<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<HookEffort>, D::Error> {
    Ok(match Option::<Value>::deserialize(deserializer)? {
        Some(Value::String(level)) => Some(HookEffort {
            level,
            extra: Map::new(),
        }),
        Some(value @ Value::Object(_)) => HookEffort::deserialize(value).ok(),
        _ => None,
    })
}

/// Fields shared by every hook input.
///
/// It includes the subagent context (`agent_id`, `agent_type`): when
/// several subagents run in parallel their hooks interleave on one control
/// channel, and `agent_id` is the only reliable way to attribute a call.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[non_exhaustive]
pub struct HookInputBase {
    /// Session the hook fired in; empty when the CLI sends none.
    #[serde(default, deserialize_with = "lenient_string")]
    pub session_id: String,
    /// Path of the session transcript; empty when the CLI sends none.
    #[serde(default, deserialize_with = "lenient_string")]
    pub transcript_path: String,
    /// Working directory of the session; empty when the CLI sends none.
    #[serde(default, deserialize_with = "lenient_string")]
    pub cwd: String,
    /// Permission mode in effect.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_mode: Option<String>,
    /// Id of the user prompt being processed; absent until the first input.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_id: Option<String>,
    /// Effort in effect for the current turn. An object with `level` is
    /// read, and so is a bare level string; any other value reads as
    /// `None`.
    #[serde(
        default,
        deserialize_with = "lenient_effort",
        skip_serializing_if = "Option::is_none"
    )]
    pub effort: Option<HookEffort>,
    /// Id of the subagent the hook fired in; absent on the main thread.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    /// Agent type name, such as `general-purpose`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_type: Option<String>,
    /// Fields this version of the crate does not model, kept verbatim.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// Input of a `PreToolUse` hook.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct PreToolUseHookInput {
    /// Shared fields.
    #[serde(flatten)]
    pub base: HookInputBase,
    /// Name of the tool about to run.
    pub tool_name: String,
    /// Input of the tool, normally a JSON object.
    pub tool_input: Value,
    /// Identifier of the tool call; empty when the CLI sends none.
    #[serde(default, deserialize_with = "lenient_string")]
    pub tool_use_id: String,
}

/// Input of a `PostToolUse` hook.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct PostToolUseHookInput {
    /// Shared fields.
    #[serde(flatten)]
    pub base: HookInputBase,
    /// Name of the tool that ran.
    pub tool_name: String,
    /// Input of the tool, normally a JSON object.
    pub tool_input: Value,
    /// Output of the tool; `Value::Null` when the CLI sent none.
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub tool_response: Value,
    /// Identifier of the tool call; empty when the CLI sends none.
    #[serde(default, deserialize_with = "lenient_string")]
    pub tool_use_id: String,
    /// Tool execution time in milliseconds, without prompt and hook time.
    /// Any JSON number is read, as the message parser reads counts: a
    /// fraction is truncated and a negative value reads as zero.
    #[serde(
        default,
        deserialize_with = "lenient_milliseconds",
        skip_serializing_if = "Option::is_none"
    )]
    pub duration_ms: Option<u64>,
}

/// Input of a `PostToolUseFailure` hook.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct PostToolUseFailureHookInput {
    /// Shared fields.
    #[serde(flatten)]
    pub base: HookInputBase,
    /// Name of the tool that failed.
    pub tool_name: String,
    /// Input of the tool, normally a JSON object.
    pub tool_input: Value,
    /// Identifier of the tool call; empty when the CLI sends none.
    #[serde(default, deserialize_with = "lenient_string")]
    pub tool_use_id: String,
    /// Error text.
    pub error: String,
    /// Whether the failure was an interruption.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_interrupt: Option<bool>,
    /// Tool execution time in milliseconds, without prompt and hook time.
    /// Any JSON number is read, as the message parser reads counts: a
    /// fraction is truncated and a negative value reads as zero.
    #[serde(
        default,
        deserialize_with = "lenient_milliseconds",
        skip_serializing_if = "Option::is_none"
    )]
    pub duration_ms: Option<u64>,
}

/// Input of a `UserPromptSubmit` hook.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct UserPromptSubmitHookInput {
    /// Shared fields.
    #[serde(flatten)]
    pub base: HookInputBase,
    /// The submitted prompt.
    pub prompt: String,
}

/// Background task in flight, as listed on `Stop` and `SubagentStop` inputs.
///
/// `description` and `command` are capped at 1000 characters by the CLI.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[non_exhaustive]
pub struct BackgroundTaskSummary {
    /// Id of the task.
    pub id: String,
    /// Task type label, such as `shell` or `subagent` (`type` on the wire).
    #[serde(rename = "type")]
    pub kind: String,
    /// Status of the task.
    pub status: String,
    /// Description of the task.
    pub description: String,
    /// Shell command line, for `shell` tasks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// Subagent type name, for `subagent` tasks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_type: Option<String>,
    /// MCP server name, for `monitor` and MCP tasks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server: Option<String>,
    /// MCP tool name, for `monitor` and MCP tasks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    /// Workflow name, for `workflow` tasks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Fields this version of the crate does not model, kept verbatim.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// Scheduled wakeup of the session, as listed on `Stop` and `SubagentStop`
/// inputs.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[non_exhaustive]
pub struct SessionCronSummary {
    /// Id of the scheduled task.
    pub id: String,
    /// Cron expression, such as `0 9 * * 1-5`.
    pub schedule: String,
    /// False for a one-shot wakeup, true for a task that fires on every
    /// match.
    pub recurring: bool,
    /// Prompt submitted when the cron fires, capped at 1000 characters.
    pub prompt: String,
    /// Fields this version of the crate does not model, kept verbatim.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// Input of a `Stop` hook.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct StopHookInput {
    /// Shared fields.
    #[serde(flatten)]
    pub base: HookInputBase,
    /// Whether a stop hook is already active.
    pub stop_hook_active: bool,
    /// Text of the last assistant message before stopping.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_assistant_message: Option<String>,
    /// Background work in flight in this session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background_tasks: Option<Vec<BackgroundTaskSummary>>,
    /// Scheduled wakeups that will resume this session later.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_crons: Option<Vec<SessionCronSummary>>,
}

/// Input of a `SubagentStop` hook.
///
/// The subagent identity is in [`HookInputBase::agent_id`] and
/// [`HookInputBase::agent_type`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct SubagentStopHookInput {
    /// Shared fields.
    #[serde(flatten)]
    pub base: HookInputBase,
    /// Whether a stop hook is already active.
    pub stop_hook_active: bool,
    /// Path of the subagent transcript.
    pub agent_transcript_path: String,
    /// Text of the subagent's last assistant message before stopping.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_assistant_message: Option<String>,
    /// Background work in flight, scoped to the parent session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background_tasks: Option<Vec<BackgroundTaskSummary>>,
    /// Scheduled wakeups, scoped to the parent session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_crons: Option<Vec<SessionCronSummary>>,
}

wire_enum! {
    /// What triggered a compaction.
    pub enum CompactTrigger {
        /// Requested by the user.
        Manual => "manual",
        /// Triggered automatically.
        Auto => "auto",
    }
}

/// Input of a `PreCompact` hook.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct PreCompactHookInput {
    /// Shared fields.
    #[serde(flatten)]
    pub base: HookInputBase,
    /// What triggered the compaction.
    pub trigger: CompactTrigger,
    /// Instructions given with the compaction request, when any.
    #[serde(default)]
    pub custom_instructions: Option<String>,
}

/// Input of a `Notification` hook.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct NotificationHookInput {
    /// Shared fields.
    #[serde(flatten)]
    pub base: HookInputBase,
    /// Notification text.
    pub message: String,
    /// Notification title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Notification category.
    pub notification_type: String,
}

/// Input of a `SubagentStart` hook.
///
/// The subagent identity is in [`HookInputBase::agent_id`] and
/// [`HookInputBase::agent_type`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct SubagentStartHookInput {
    /// Shared fields.
    #[serde(flatten)]
    pub base: HookInputBase,
}

/// Input of a `PermissionRequest` hook.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct PermissionRequestHookInput {
    /// Shared fields.
    #[serde(flatten)]
    pub base: HookInputBase,
    /// Name of the tool that needs a decision.
    pub tool_name: String,
    /// Input of the tool, normally a JSON object.
    pub tool_input: Value,
    /// Permission updates suggested by the CLI, as raw JSON.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_suggestions: Option<Vec<Value>>,
}

/// Input handed to a hook callback, selected by `hook_event_name`.
///
/// An event unknown to this version of the crate, or a payload that does not
/// have the documented shape, becomes [`HookInput::Unknown`] with the raw
/// JSON: reading a hook input never fails. A mismatch is logged at warn
/// level. A hook that guards tool calls reads the tool through
/// [`HookInput::tool_name`] and [`HookInput::tool_input`], which read both
/// forms, so that an input of an unexpected shape is still checked.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum HookInput {
    /// `PreToolUse` event.
    PreToolUse(PreToolUseHookInput),
    /// `PostToolUse` event.
    PostToolUse(PostToolUseHookInput),
    /// `PostToolUseFailure` event.
    PostToolUseFailure(PostToolUseFailureHookInput),
    /// `UserPromptSubmit` event.
    UserPromptSubmit(UserPromptSubmitHookInput),
    /// `Stop` event.
    Stop(StopHookInput),
    /// `SubagentStop` event.
    SubagentStop(SubagentStopHookInput),
    /// `PreCompact` event.
    PreCompact(PreCompactHookInput),
    /// `Notification` event.
    Notification(NotificationHookInput),
    /// `SubagentStart` event.
    SubagentStart(SubagentStartHookInput),
    /// `PermissionRequest` event.
    PermissionRequest(PermissionRequestHookInput),
    /// An event or shape unknown to this version of the crate.
    Unknown(Value),
}

macro_rules! hook_input_variants {
    ($( $variant:ident($ty:ty) => $wire:literal ),+ $(,)?) => {
        impl HookInput {
            /// Builds a hook input from its wire form. This never fails.
            #[must_use]
            pub fn from_wire(wire: Value) -> Self {
                match wire.get(HOOK_EVENT_NAME_KEY).and_then(Value::as_str) {
                    $(
                        Some($wire) => match <$ty>::deserialize(&wire) {
                            Ok(mut input) => {
                                input.base.extra.shift_remove(HOOK_EVENT_NAME_KEY);
                                Self::$variant(input)
                            }
                            Err(error) => {
                                tracing::warn!(
                                    event = $wire,
                                    %error,
                                    "a hook input does not match the type of its event; it is \
                                     passed as HookInput::Unknown with its raw JSON"
                                );
                                Self::Unknown(wire)
                            }
                        },
                    )+
                    _ => Self::Unknown(wire),
                }
            }

            /// Converts the hook input back to its wire form.
            #[must_use]
            pub fn to_wire(&self) -> Value {
                match self {
                    $(
                        Self::$variant(input) => {
                            let mut wire = serde_json::to_value(input).unwrap_or(Value::Null);
                            if let Value::Object(object) = &mut wire {
                                object.insert(
                                    HOOK_EVENT_NAME_KEY.to_owned(),
                                    Value::String($wire.to_owned()),
                                );
                            }
                            wire
                        }
                    )+
                    Self::Unknown(wire) => wire.clone(),
                }
            }

            /// Name of the hook event, empty when the raw input has none.
            #[must_use]
            pub fn hook_event_name(&self) -> &str {
                match self {
                    $( Self::$variant(_) => $wire, )+
                    Self::Unknown(wire) => wire
                        .get(HOOK_EVENT_NAME_KEY)
                        .and_then(Value::as_str)
                        .unwrap_or_default(),
                }
            }

            /// Shared fields, absent for [`HookInput::Unknown`].
            #[must_use]
            pub fn base(&self) -> Option<&HookInputBase> {
                match self {
                    $( Self::$variant(input) => Some(&input.base), )+
                    Self::Unknown(_) => None,
                }
            }
        }
    };
}

hook_input_variants! {
    PreToolUse(PreToolUseHookInput) => "PreToolUse",
    PostToolUse(PostToolUseHookInput) => "PostToolUse",
    PostToolUseFailure(PostToolUseFailureHookInput) => "PostToolUseFailure",
    UserPromptSubmit(UserPromptSubmitHookInput) => "UserPromptSubmit",
    Stop(StopHookInput) => "Stop",
    SubagentStop(SubagentStopHookInput) => "SubagentStop",
    PreCompact(PreCompactHookInput) => "PreCompact",
    Notification(NotificationHookInput) => "Notification",
    SubagentStart(SubagentStartHookInput) => "SubagentStart",
    PermissionRequest(PermissionRequestHookInput) => "PermissionRequest",
}

impl HookInput {
    /// Name of the tool the hook is about, for the events that carry one
    /// (`PreToolUse`, `PostToolUse`, `PostToolUseFailure`,
    /// `PermissionRequest`), read from the raw JSON of an
    /// [`Unknown`](Self::Unknown) input too.
    ///
    /// A hook that guards tool calls should read the tool through this
    /// method rather than match only the typed variant: an input whose
    /// shape changed is then still checked, instead of let through.
    #[must_use]
    pub fn tool_name(&self) -> Option<&str> {
        match self {
            Self::PreToolUse(input) => Some(&input.tool_name),
            Self::PostToolUse(input) => Some(&input.tool_name),
            Self::PostToolUseFailure(input) => Some(&input.tool_name),
            Self::PermissionRequest(input) => Some(&input.tool_name),
            Self::Unknown(wire) => wire.get("tool_name").and_then(Value::as_str),
            _ => None,
        }
    }

    /// Input of the tool the hook is about, for the same events as
    /// [`tool_name`](Self::tool_name), read from the raw JSON of an
    /// [`Unknown`](Self::Unknown) input too.
    #[must_use]
    pub fn tool_input(&self) -> Option<&Value> {
        match self {
            Self::PreToolUse(input) => Some(&input.tool_input),
            Self::PostToolUse(input) => Some(&input.tool_input),
            Self::PostToolUseFailure(input) => Some(&input.tool_input),
            Self::PermissionRequest(input) => Some(&input.tool_input),
            Self::Unknown(wire) => wire.get("tool_input"),
            _ => None,
        }
    }

    /// Id of the subagent the hook fired in, when any.
    #[must_use]
    pub fn agent_id(&self) -> Option<&str> {
        match self {
            Self::Unknown(wire) => wire.get("agent_id").and_then(Value::as_str),
            _ => self.base().and_then(|base| base.agent_id.as_deref()),
        }
    }
}

impl Serialize for HookInput {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.to_wire().serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for HookInput {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Value::deserialize(deserializer).map(Self::from_wire)
    }
}

wire_enum! {
    /// Permission decision returned by a `PreToolUse` hook.
    pub enum HookPermissionDecision {
        /// Allow the call.
        Allow => "allow",
        /// Deny the call.
        Deny => "deny",
        /// Ask through the permission prompt.
        Ask => "ask",
        /// Stop the run and hand the call back to the caller.
        Defer => "defer",
    }
}

wire_enum! {
    /// Action returned by an elicitation hook.
    pub enum ElicitationAction {
        /// Accept the request.
        Accept => "accept",
        /// Decline the request.
        Decline => "decline",
        /// Cancel the request.
        Cancel => "cancel",
    }
}

/// Hook-specific output for `PreToolUse` events.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct PreToolUseHookSpecificOutput {
    /// Permission decision for the call.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_decision: Option<HookPermissionDecision>,
    /// Reason for the decision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_decision_reason: Option<String>,
    /// Replacement input for the tool.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_input: Option<Value>,
    /// Extra context delivered to the model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub additional_context: Option<String>,
    /// Keys this version of the crate does not model, sent to the CLI as
    /// they are, so a key a newer CLI reads can be set before the crate
    /// knows it.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl PreToolUseHookSpecificOutput {
    setters! {
        opt permission_decision: HookPermissionDecision;
        opt permission_decision_reason: String;
        opt updated_input: Value;
        opt additional_context: String;
    }
}

/// Hook-specific output for `PostToolUse` events.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct PostToolUseHookSpecificOutput {
    /// Extra context delivered to the model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub additional_context: Option<String>,
    /// Replaces the tool output before it is sent to the model. For a
    /// built-in tool the value must match the output schema of the tool.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_tool_output: Option<Value>,
    /// Replaces the output for MCP tools only. Prefer `updated_tool_output`.
    #[serde(
        default,
        rename = "updatedMCPToolOutput",
        skip_serializing_if = "Option::is_none"
    )]
    pub updated_mcp_tool_output: Option<Value>,
    /// Keys this version of the crate does not model, sent to the CLI as
    /// they are, so a key a newer CLI reads can be set before the crate
    /// knows it.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl PostToolUseHookSpecificOutput {
    setters! {
        opt additional_context: String;
        opt updated_tool_output: Value;
        opt updated_mcp_tool_output: Value;
    }
}

macro_rules! context_only_output {
    ($( $(#[$meta:meta])* $name:ident ),+ $(,)?) => {
        $(
            $(#[$meta])*
            #[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
            #[serde(rename_all = "camelCase")]
            #[non_exhaustive]
            pub struct $name {
                /// Extra context delivered to the model.
                #[serde(default, skip_serializing_if = "Option::is_none")]
                pub additional_context: Option<String>,
                /// Keys this version of the crate does not model, sent to the CLI as
                /// they are, so a key a newer CLI reads can be set before the crate
                /// knows it.
                #[serde(flatten)]
                pub extra: Map<String, Value>,
            }

            impl $name {
                setters! {
                    opt additional_context: String;
                }
            }
        )+
    };
}

context_only_output! {
    /// Hook-specific output for `PostToolUseFailure` events.
    PostToolUseFailureHookSpecificOutput,
    /// Hook-specific output for `Notification` events.
    NotificationHookSpecificOutput,
    /// Hook-specific output for `SubagentStart` events.
    SubagentStartHookSpecificOutput,
    /// Hook-specific output for `PostToolBatch` events.
    PostToolBatchHookSpecificOutput,
    /// Hook-specific output for `Stop` events. The context is feedback that
    /// is not an error: the conversation continues so the model can act on
    /// it.
    StopHookSpecificOutput,
    /// Hook-specific output for `SubagentStop` events. The context is
    /// feedback that is not an error: the subagent continues so it can act
    /// on it.
    SubagentStopHookSpecificOutput,
    /// Hook-specific output for `PostModelSwitch` events. The context
    /// reaches the model with the next request the new model serves.
    PostModelSwitchHookSpecificOutput,
}

/// Hook-specific output for `UserPromptSubmit` events.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct UserPromptSubmitHookSpecificOutput {
    /// Extra context delivered to the model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub additional_context: Option<String>,
    /// Sets the session title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_title: Option<String>,
    /// When the decision is `block`, omits the original prompt from the
    /// block message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suppress_original_prompt: Option<bool>,
    /// Keys this version of the crate does not model, sent to the CLI as
    /// they are, so a key a newer CLI reads can be set before the crate
    /// knows it.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl UserPromptSubmitHookSpecificOutput {
    setters! {
        opt additional_context: String;
        opt session_title: String;
        opt suppress_original_prompt: bool;
    }
}

/// Hook-specific output for `UserPromptExpansion` events.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct UserPromptExpansionHookSpecificOutput {
    /// Extra context delivered to the model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub additional_context: Option<String>,
    /// When the decision is `block`, omits the original prompt from the
    /// block message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suppress_original_prompt: Option<bool>,
    /// Keys this version of the crate does not model, sent to the CLI as
    /// they are, so a key a newer CLI reads can be set before the crate
    /// knows it.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl UserPromptExpansionHookSpecificOutput {
    setters! {
        opt additional_context: String;
        opt suppress_original_prompt: bool;
    }
}

/// Hook-specific output for `SessionStart` events.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct SessionStartHookSpecificOutput {
    /// Extra context delivered to the model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub additional_context: Option<String>,
    /// Used as the first user message of the session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initial_user_message: Option<String>,
    /// Sets the session title. Ignored when the session starts from `clear`
    /// or `compact`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_title: Option<String>,
    /// Absolute paths to watch for file changes during this session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub watch_paths: Option<Vec<String>>,
    /// Scans skill and command directories again after the hooks complete.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reload_skills: Option<bool>,
    /// Keys this version of the crate does not model, sent to the CLI as
    /// they are, so a key a newer CLI reads can be set before the crate
    /// knows it.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl SessionStartHookSpecificOutput {
    setters! {
        opt additional_context: String;
        opt initial_user_message: String;
        opt session_title: String;
        list watch_paths: String;
        opt reload_skills: bool;
    }
}

/// Hook-specific output for `PermissionRequest` events.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct PermissionRequestHookSpecificOutput {
    /// Decision object, such as `{"behavior": "allow"}`.
    pub decision: Value,
    /// Keys this version of the crate does not model, sent to the CLI as
    /// they are, so a key a newer CLI reads can be set before the crate
    /// knows it.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl PermissionRequestHookSpecificOutput {
    /// Creates the output from a decision object.
    #[must_use]
    pub fn new(decision: Value) -> Self {
        Self {
            decision,
            extra: Map::new(),
        }
    }
}

/// Hook-specific output for `PermissionDenied` events.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[non_exhaustive]
pub struct PermissionDeniedHookSpecificOutput {
    /// Tells the model it may retry the denied call. Ignored for a denial
    /// without a classifier verdict.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry: Option<bool>,
    /// Keys this version of the crate does not model, sent to the CLI as
    /// they are, so a key a newer CLI reads can be set before the crate
    /// knows it.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl PermissionDeniedHookSpecificOutput {
    setters! {
        opt retry: bool;
    }
}

/// Hook-specific output for `PreModelSwitch` events.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct PreModelSwitchHookSpecificOutput {
    /// `allow` proceeds and `deny` cancels the switch. `ask` is treated as
    /// a refusal outside an interactive session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_decision: Option<PermissionBehavior>,
    /// Reason for the decision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_decision_reason: Option<String>,
    /// Keys this version of the crate does not model, sent to the CLI as
    /// they are, so a key a newer CLI reads can be set before the crate
    /// knows it.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl PreModelSwitchHookSpecificOutput {
    setters! {
        opt permission_decision: PermissionBehavior;
        opt permission_decision_reason: String;
    }
}

macro_rules! elicitation_output {
    ($( $(#[$meta:meta])* $name:ident ),+ $(,)?) => {
        $(
            $(#[$meta])*
            #[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
            #[non_exhaustive]
            pub struct $name {
                /// Action to take on the request.
                #[serde(default, skip_serializing_if = "Option::is_none")]
                pub action: Option<ElicitationAction>,
                /// Form field values, used when the action is `accept`.
                #[serde(default, skip_serializing_if = "Option::is_none")]
                pub content: Option<Value>,
                /// Keys this version of the crate does not model, sent to the CLI as
                /// they are, so a key a newer CLI reads can be set before the crate
                /// knows it.
                #[serde(flatten)]
                pub extra: Map<String, Value>,
            }

            impl $name {
                setters! {
                    opt action: ElicitationAction;
                    opt content: Value;
                }
            }
        )+
    };
}

elicitation_output! {
    /// Hook-specific output for `Elicitation` events: accepts or declines an
    /// MCP elicitation request from code.
    ElicitationHookSpecificOutput,
    /// Hook-specific output for `ElicitationResult` events: overrides the
    /// action or content before the response reaches the MCP server.
    ElicitationResultHookSpecificOutput,
}

macro_rules! watch_paths_output {
    ($( $(#[$meta:meta])* $name:ident ),+ $(,)?) => {
        $(
            $(#[$meta])*
            #[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
            #[serde(rename_all = "camelCase")]
            #[non_exhaustive]
            pub struct $name {
                /// Absolute paths to watch for file changes. Replaces the
                /// current dynamic watch list.
                #[serde(default, skip_serializing_if = "Option::is_none")]
                pub watch_paths: Option<Vec<String>>,
                /// Keys this version of the crate does not model, sent to the CLI as
                /// they are, so a key a newer CLI reads can be set before the crate
                /// knows it.
                #[serde(flatten)]
                pub extra: Map<String, Value>,
            }

            impl $name {
                setters! {
                    list watch_paths: String;
                }
            }
        )+
    };
}

watch_paths_output! {
    /// Hook-specific output for `CwdChanged` events.
    CwdChangedHookSpecificOutput,
    /// Hook-specific output for `FileChanged` events.
    FileChangedHookSpecificOutput,
}

/// Hook-specific output for `WorktreeCreate` events.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct WorktreeCreateHookSpecificOutput {
    /// Absolute path of the created worktree directory.
    pub worktree_path: String,
    /// Keys this version of the crate does not model, sent to the CLI as
    /// they are, so a key a newer CLI reads can be set before the crate
    /// knows it.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl WorktreeCreateHookSpecificOutput {
    /// Creates the output from the worktree path.
    #[must_use]
    pub fn new(worktree_path: impl Into<String>) -> Self {
        Self {
            worktree_path: worktree_path.into(),
            extra: Map::new(),
        }
    }
}

/// Hook-specific output for `MessageDisplay` events.
///
/// It only affects display: the stored message and what the model sees do
/// not change.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct MessageDisplayHookSpecificOutput {
    /// Text displayed in place of the delta. Leave unset to display the
    /// original.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_content: Option<String>,
    /// Keys this version of the crate does not model, sent to the CLI as
    /// they are, so a key a newer CLI reads can be set before the crate
    /// knows it.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl MessageDisplayHookSpecificOutput {
    setters! {
        opt display_content: String;
    }
}

macro_rules! hook_specific_outputs {
    ($( $variant:ident($ty:ident) ),+ $(,)?) => {
        /// Event-specific part of a hook output, tagged by `hookEventName`.
        ///
        /// An output for an event this version of the crate does not model
        /// is [`HookSpecificOutput::Unknown`]: its JSON, `hookEventName`
        /// included, is sent to the CLI as written.
        #[derive(Debug, Clone, PartialEq)]
        #[non_exhaustive]
        pub enum HookSpecificOutput {
            $(
                #[doc = concat!("Output for `", stringify!($variant), "` events.")]
                $variant($ty),
            )+
            /// Output for an event this version of the crate does not model,
            /// kept as raw JSON with its `hookEventName`.
            Unknown(Value),
        }

        /// The typed outputs, tagged as on the wire, as read.
        #[derive(Deserialize)]
        #[serde(tag = "hookEventName")]
        enum TaggedHookSpecificOutput {
            $( $variant($ty), )+
        }

        /// The typed outputs, tagged as on the wire, as written.
        #[derive(Serialize)]
        #[serde(tag = "hookEventName")]
        enum TaggedHookSpecificOutputRef<'a> {
            $( $variant(&'a $ty), )+
        }

        impl HookSpecificOutput {
            /// Event names that have a typed output, in upstream order.
            pub const EVENT_NAMES: &'static [&'static str] = &[$(stringify!($variant)),+];

            /// Name of the event this output belongs to; for an
            /// [`Unknown`](Self::Unknown) output, its `hookEventName`, or
            /// the empty string when it has none.
            #[must_use]
            pub fn hook_event_name(&self) -> &str {
                match self {
                    $( Self::$variant(_) => stringify!($variant), )+
                    Self::Unknown(raw) => raw
                        .get("hookEventName")
                        .and_then(Value::as_str)
                        .unwrap_or_default(),
                }
            }
        }

        impl Serialize for HookSpecificOutput {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                match self {
                    $(
                        Self::$variant(output) => {
                            TaggedHookSpecificOutputRef::$variant(output).serialize(serializer)
                        }
                    )+
                    Self::Unknown(raw) => raw.serialize(serializer),
                }
            }
        }

        impl<'de> Deserialize<'de> for HookSpecificOutput {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let raw = Value::deserialize(deserializer)?;
                let known = raw
                    .get("hookEventName")
                    .and_then(Value::as_str)
                    .is_some_and(|name| Self::EVENT_NAMES.contains(&name));
                if !known {
                    return Ok(Self::Unknown(raw));
                }
                let tagged = TaggedHookSpecificOutput::deserialize(raw)
                    .map_err(serde::de::Error::custom)?;
                Ok(match tagged {
                    $( TaggedHookSpecificOutput::$variant(output) => Self::$variant(output), )+
                })
            }
        }

        $(
            impl From<$ty> for HookSpecificOutput {
                fn from(output: $ty) -> Self {
                    Self::$variant(output)
                }
            }
        )+
    };
}

hook_specific_outputs! {
    PreToolUse(PreToolUseHookSpecificOutput),
    PostToolUse(PostToolUseHookSpecificOutput),
    PostToolUseFailure(PostToolUseFailureHookSpecificOutput),
    UserPromptSubmit(UserPromptSubmitHookSpecificOutput),
    SessionStart(SessionStartHookSpecificOutput),
    Notification(NotificationHookSpecificOutput),
    SubagentStart(SubagentStartHookSpecificOutput),
    PermissionRequest(PermissionRequestHookSpecificOutput),
    UserPromptExpansion(UserPromptExpansionHookSpecificOutput),
    PostToolBatch(PostToolBatchHookSpecificOutput),
    Stop(StopHookSpecificOutput),
    SubagentStop(SubagentStopHookSpecificOutput),
    PermissionDenied(PermissionDeniedHookSpecificOutput),
    PreModelSwitch(PreModelSwitchHookSpecificOutput),
    PostModelSwitch(PostModelSwitchHookSpecificOutput),
    Elicitation(ElicitationHookSpecificOutput),
    ElicitationResult(ElicitationResultHookSpecificOutput),
    CwdChanged(CwdChangedHookSpecificOutput),
    FileChanged(FileChangedHookSpecificOutput),
    WorktreeCreate(WorktreeCreateHookSpecificOutput),
    MessageDisplay(MessageDisplayHookSpecificOutput),
}

extra_field! {
    PreToolUseHookSpecificOutput,
    PostToolUseHookSpecificOutput,
    PostToolUseFailureHookSpecificOutput,
    UserPromptSubmitHookSpecificOutput,
    SessionStartHookSpecificOutput,
    NotificationHookSpecificOutput,
    SubagentStartHookSpecificOutput,
    PermissionRequestHookSpecificOutput,
    UserPromptExpansionHookSpecificOutput,
    PostToolBatchHookSpecificOutput,
    StopHookSpecificOutput,
    SubagentStopHookSpecificOutput,
    PermissionDeniedHookSpecificOutput,
    PreModelSwitchHookSpecificOutput,
    PostModelSwitchHookSpecificOutput,
    ElicitationHookSpecificOutput,
    ElicitationResultHookSpecificOutput,
    CwdChangedHookSpecificOutput,
    FileChangedHookSpecificOutput,
    WorktreeCreateHookSpecificOutput,
    MessageDisplayHookSpecificOutput,
    AsyncHookOutput,
    SyncHookOutput,
}

wire_enum! {
    /// Top-level decision of a synchronous hook output.
    pub enum HookDecision {
        /// Block the action.
        Block => "block",
    }
}

/// Synchronous hook output, with control and decision fields.
///
/// The default value serializes to `{}`, which is what a callback with no
/// opinion returns.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[non_exhaustive]
pub struct SyncHookOutput {
    /// Whether the agent proceeds after the hook (`continue` on the wire).
    #[serde(default, rename = "continue", skip_serializing_if = "Option::is_none")]
    pub continue_: Option<bool>,
    /// Hides the hook output from the transcript.
    #[serde(
        default,
        rename = "suppressOutput",
        skip_serializing_if = "Option::is_none"
    )]
    pub suppress_output: Option<bool>,
    /// Message shown when `continue_` is false.
    #[serde(
        default,
        rename = "stopReason",
        skip_serializing_if = "Option::is_none"
    )]
    pub stop_reason: Option<String>,
    /// Set to `block` to block the action.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision: Option<HookDecision>,
    /// Warning displayed to the user.
    #[serde(
        default,
        rename = "systemMessage",
        skip_serializing_if = "Option::is_none"
    )]
    pub system_message: Option<String>,
    /// Feedback for the model about the decision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Event-specific controls.
    #[serde(
        default,
        rename = "hookSpecificOutput",
        skip_serializing_if = "Option::is_none"
    )]
    pub hook_specific_output: Option<HookSpecificOutput>,
    /// Extra keys sent verbatim. The CLI ignores keys it does not know.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl SyncHookOutput {
    setters! {
        opt continue_: bool;
        opt suppress_output: bool;
        opt stop_reason: String;
        opt decision: HookDecision;
        opt system_message: String;
        opt reason: String;
        opt hook_specific_output: HookSpecificOutput;
    }

    /// Creates an output that blocks the action with a reason.
    #[must_use]
    pub fn block(reason: impl Into<String>) -> Self {
        Self::default()
            .decision(HookDecision::Block)
            .reason(reason.into())
    }
}

/// Asynchronous hook output: defers the execution of the hook.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct AsyncHookOutput {
    /// Always true (`async` on the wire).
    #[serde(rename = "async")]
    pub async_: bool,
    /// Timeout of the deferred operation, in milliseconds.
    #[serde(
        default,
        rename = "asyncTimeout",
        skip_serializing_if = "Option::is_none"
    )]
    pub async_timeout: Option<u64>,
    /// Keys this version of the crate does not model, sent to the CLI as
    /// they are, so a key a newer CLI reads can be set before the crate
    /// knows it.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl AsyncHookOutput {
    /// Creates an asynchronous output without a timeout.
    #[must_use]
    pub fn new() -> Self {
        Self {
            async_: true,
            async_timeout: None,
            extra: Map::new(),
        }
    }

    setters! {
        val async_timeout: u64;
    }
}

impl Default for AsyncHookOutput {
    fn default() -> Self {
        Self::new()
    }
}

/// Output of a hook callback.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
#[allow(clippy::large_enum_variant)]
pub enum HookOutput {
    /// The hook answered.
    Sync(SyncHookOutput),
    /// The hook defers its execution.
    Async(AsyncHookOutput),
}

impl HookOutput {
    /// Converts the output to the form the CLI expects, with the `continue`
    /// and `async` key names.
    #[must_use]
    pub fn to_wire(&self) -> Value {
        let wire = match self {
            Self::Sync(output) => serde_json::to_value(output),
            Self::Async(output) => serde_json::to_value(output),
        };
        wire.unwrap_or_else(|_| Value::Object(Map::new()))
    }
}

impl Default for HookOutput {
    fn default() -> Self {
        Self::Sync(SyncHookOutput::default())
    }
}

impl From<SyncHookOutput> for HookOutput {
    fn from(output: SyncHookOutput) -> Self {
        Self::Sync(output)
    }
}

impl From<AsyncHookOutput> for HookOutput {
    fn from(output: AsyncHookOutput) -> Self {
        Self::Async(output)
    }
}

impl Serialize for HookOutput {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Sync(output) => output.serialize(serializer),
            Self::Async(output) => output.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for HookOutput {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let wire = Value::deserialize(deserializer)?;
        let is_async = wire.get("async").and_then(Value::as_bool) == Some(true);
        if is_async {
            AsyncHookOutput::deserialize(&wire).map(Self::Async)
        } else {
            SyncHookOutput::deserialize(&wire).map(Self::Sync)
        }
        .map_err(serde::de::Error::custom)
    }
}

/// Context handed to a hook callback.
///
/// It is reserved for a future abort signal and holds nothing today.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[non_exhaustive]
pub struct HookContext {}

type HookCallbackFn = dyn Fn(HookInput, Option<String>, HookContext) -> BoxFuture<'static, Result<HookOutput, BoxError>>
    + Send
    + Sync;

/// Hook callback.
///
/// It receives the typed [`HookInput`], the tool use id when the event has
/// one, and a [`HookContext`], and answers with a [`HookOutput`].
///
/// Any async closure converts into this type:
///
/// ```
/// use claude_agent_rust_sdk::{HookCallback, HookOutput};
///
/// let callback = HookCallback::new(async |_input, _tool_use_id, _context| {
///     Ok(HookOutput::default())
/// });
/// # let _ = callback;
/// ```
#[derive(Clone)]
pub struct HookCallback(Arc<HookCallbackFn>);

impl HookCallback {
    /// Wraps an async closure.
    pub fn new<F, Fut>(callback: F) -> Self
    where
        F: Fn(HookInput, Option<String>, HookContext) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<HookOutput, BoxError>> + Send + 'static,
    {
        Self(Arc::new(move |input, tool_use_id, context| {
            Box::pin(callback(input, tool_use_id, context))
        }))
    }

    /// Invokes the callback.
    pub fn call(
        &self,
        input: HookInput,
        tool_use_id: Option<String>,
        context: HookContext,
    ) -> BoxFuture<'static, Result<HookOutput, BoxError>> {
        (self.0)(input, tool_use_id, context)
    }
}

impl<F, Fut> From<F> for HookCallback
where
    F: Fn(HookInput, Option<String>, HookContext) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<HookOutput, BoxError>> + Send + 'static,
{
    fn from(callback: F) -> Self {
        Self::new(callback)
    }
}

impl fmt::Debug for HookCallback {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("HookCallback(..)")
    }
}

wire_enum! {
    /// What happens when a callback of a matcher fails.
    pub enum HookFailureMode {
        /// A failure counts as no decision.
        Continue => "continue",
        /// A failure blocks the action.
        Block => "block",
    }
}

/// Hook matcher: which calls a group of callbacks applies to.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct HookMatcher {
    /// Pattern of the matcher. For `PreToolUse` it is a tool name such as
    /// `Bash`, or several names joined with `|`.
    pub matcher: Option<String>,
    /// Callbacks to run.
    pub hooks: Vec<HookCallback>,
    /// Timeout for all the callbacks of this matcher. The CLI default is 60
    /// seconds.
    pub timeout: Option<Duration>,
    /// What happens when a callback fails. With
    /// [`HookFailureMode::Block`], a callback that returns an error or a
    /// value the CLI rejects blocks the action, so a callback with no
    /// opinion must return the default [`HookOutput`]. Connecting to a CLI
    /// too old to honor it fails.
    pub on_failure: Option<HookFailureMode>,
}

impl HookMatcher {
    /// Creates a matcher with no pattern and no callback.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the pattern.
    #[must_use]
    pub fn matcher(mut self, matcher: impl Into<String>) -> Self {
        self.matcher = Some(matcher.into());
        self
    }

    /// Adds a callback.
    #[must_use]
    pub fn hook(mut self, callback: impl Into<HookCallback>) -> Self {
        self.hooks.push(callback.into());
        self
    }

    /// Sets the timeout.
    #[must_use]
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    /// Sets the failure mode.
    #[must_use]
    pub fn on_failure(mut self, mode: impl Into<HookFailureMode>) -> Self {
        self.on_failure = Some(mode.into());
        self
    }
}
