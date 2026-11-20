//! Messages emitted by the CLI on its output stream.

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::content::ContentBlock;
use super::macros::wire_enum;
use super::permission::PermissionMode;
use crate::session_store::SessionKey;

/// One message of the output stream.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
#[allow(clippy::large_enum_variant)]
pub enum Message {
    /// A user-role message, including tool results and injected turns.
    User(UserMessage),
    /// An assistant message with content blocks.
    Assistant(AssistantMessage),
    /// A system message; see [`SystemMessage`] for the typed subtypes.
    System(SystemMessage),
    /// The result that ends a turn.
    Result(ResultMessage),
    /// A partial message update.
    StreamEvent(StreamEvent),
    /// A change of rate limit state.
    RateLimit(RateLimitEvent),
    /// The conversation was replaced without ending the connection.
    ConversationReset(ConversationResetMessage),
    /// A message whose `type` is unknown to this version of the crate, kept
    /// as raw JSON.
    Unknown(Value),
}

wire_enum! {
    /// Error category attached to an assistant message.
    pub enum AssistantMessageError {
        /// Authentication failed.
        AuthenticationFailed => "authentication_failed",
        /// Billing problem.
        BillingError => "billing_error",
        /// Rate limit reached.
        RateLimit => "rate_limit",
        /// The request was invalid.
        InvalidRequest => "invalid_request",
        /// The server failed.
        ServerError => "server_error",
        /// Any other failure; details are in the message text.
        UnknownError => "unknown",
    }
}

wire_enum! {
    /// What produced a user-role message.
    ///
    /// Treat any value that is not [`MessageOriginKind::Human`] as "not typed
    /// by a human".
    pub enum MessageOriginKind {
        /// Typed by a human.
        Human => "human",
        /// Arrived on an MCP channel.
        Channel => "channel",
        /// Sent by a peer session.
        Peer => "peer",
        /// A background-task notification.
        TaskNotification => "task-notification",
        /// Sent by a coordinator.
        Coordinator => "coordinator",
        /// Not classified by the CLI.
        Unclassified => "unclassified",
        /// Sent by an observer.
        Observer => "observer",
        /// An automatic continuation.
        AutoContinuation => "auto-continuation",
        /// Activity reported by an observer.
        ObserverActivity => "observer-activity",
    }
}

wire_enum! {
    /// Refinement of [`MessageOriginKind::TaskNotification`].
    pub enum TaskNotificationOriginSubkind {
        /// The fired prompt of a scheduled task.
        ScheduledTrigger => "scheduled-trigger",
        /// A message sent from another session of the same user.
        PeerSendMessage => "peer-send-message",
    }
}

/// Provenance of a user-role message and, on a [`ResultMessage`], of the
/// message that started the turn.
///
/// Only `kind` is always present. The object is passed through from the CLI
/// as is, so [`MessageOrigin::raw`] may hold keys this crate does not model.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct MessageOrigin {
    /// Discriminator.
    pub kind: MessageOriginKind,
    /// The origin object as sent by the CLI.
    pub raw: Map<String, Value>,
}

impl MessageOrigin {
    /// Creates an origin holding only its kind.
    #[must_use]
    pub fn new(kind: impl Into<MessageOriginKind>) -> Self {
        let kind = kind.into();
        let mut raw = Map::new();
        raw.insert("kind".to_owned(), Value::String(kind.as_str().to_owned()));
        Self { kind, raw }
    }

    pub(crate) fn from_wire(value: Option<&Value>) -> Option<Self> {
        let raw = value?.as_object()?;
        let kind = raw.get("kind")?.as_str()?;
        Some(Self {
            kind: MessageOriginKind::from(kind),
            raw: raw.clone(),
        })
    }

    /// For kind `channel`: name of the MCP server the message arrived on.
    #[must_use]
    pub fn server(&self) -> Option<&str> {
        self.string("server")
    }

    /// For kinds `peer` and `observer`: sender address, asserted by the
    /// sender and therefore not a proof of identity.
    #[must_use]
    pub fn from(&self) -> Option<&str> {
        self.string("from")
    }

    /// For kind `peer`: display name of the sender.
    #[must_use]
    pub fn name(&self) -> Option<&str> {
        self.string("name")
    }

    /// For kind `peer`: session id of the sender, when its host gave one.
    #[must_use]
    pub fn from_session(&self) -> Option<&str> {
        self.string("fromSession")
    }

    /// For kinds `peer` and `observer`: task id of the in-process background
    /// subagent that sent the message.
    #[must_use]
    pub fn sender_task_id(&self) -> Option<&str> {
        self.string("senderTaskId")
    }

    /// For kind `peer`: decoded message body, without the peer envelope.
    #[must_use]
    pub fn body(&self) -> Option<&str> {
        self.string("body")
    }

    /// For kind `peer`: verified process id of the connecting process.
    #[must_use]
    pub fn verified_peer_pid(&self) -> Option<i64> {
        self.raw.get("verifiedPeerPid").and_then(Value::as_i64)
    }

    /// For kind `task-notification`: what kind of delivery this is.
    #[must_use]
    pub fn subkind(&self) -> Option<TaskNotificationOriginSubkind> {
        self.string("subkind")
            .map(TaskNotificationOriginSubkind::from)
    }

    fn string(&self, key: &str) -> Option<&str> {
        self.raw.get(key).and_then(Value::as_str)
    }
}

/// Content of a [`UserMessage`].
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum UserContent {
    /// A plain string.
    Text(String),
    /// A list of content blocks.
    Blocks(Vec<ContentBlock>),
}

impl UserContent {
    /// Returns the string of a [`UserContent::Text`] value.
    #[must_use]
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Self::Text(text) => Some(text.as_str()),
            Self::Blocks(_) => None,
        }
    }

    /// Returns the blocks of a [`UserContent::Blocks`] value.
    #[must_use]
    pub fn as_blocks(&self) -> Option<&[ContentBlock]> {
        match self {
            Self::Text(_) => None,
            Self::Blocks(blocks) => Some(blocks),
        }
    }
}

impl From<&str> for UserContent {
    fn from(text: &str) -> Self {
        Self::Text(text.to_owned())
    }
}

impl From<String> for UserContent {
    fn from(text: String) -> Self {
        Self::Text(text)
    }
}

impl From<Vec<ContentBlock>> for UserContent {
    fn from(blocks: Vec<ContentBlock>) -> Self {
        Self::Blocks(blocks)
    }
}

/// User message.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct UserMessage {
    /// A string or a list of content blocks.
    pub content: UserContent,
    /// Unique id of the message, used by file checkpointing.
    pub uuid: Option<String>,
    /// Id of the tool call that spawned the subagent this message belongs to.
    pub parent_tool_use_id: Option<String>,
    /// Metadata about a tool execution, as raw JSON.
    pub tool_use_result: Option<Value>,
    /// Provenance of the message, absent when the CLI did not attribute it.
    pub origin: Option<MessageOrigin>,
    /// Keys of the nested `message` object other than `content`.
    pub message_extra: Map<String, Value>,
    /// Keys of the frame the crate does not model, as the CLI sent them.
    pub extra: Map<String, Value>,
}

impl UserMessage {
    /// Creates a user message with only its content set.
    #[must_use]
    pub fn new(content: impl Into<UserContent>) -> Self {
        Self {
            content: content.into(),
            uuid: None,
            parent_tool_use_id: None,
            tool_use_result: None,
            origin: None,
            message_extra: Map::new(),
            extra: Map::new(),
        }
    }
}

/// Assistant message with content blocks.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct AssistantMessage {
    /// Content blocks, in order.
    pub content: Vec<ContentBlock>,
    /// Model that produced the message.
    pub model: String,
    /// Id of the tool call that spawned the subagent this message belongs to.
    pub parent_tool_use_id: Option<String>,
    /// Error category, when the turn failed.
    pub error: Option<AssistantMessageError>,
    /// Token usage of this turn.
    pub usage: Option<Usage>,
    /// API id of the message (`message.id` on the wire).
    pub message_id: Option<String>,
    /// Why the model stopped.
    pub stop_reason: Option<String>,
    /// Session the message belongs to.
    pub session_id: Option<String>,
    /// Unique id of the message.
    pub uuid: Option<String>,
    /// `uuid` of the user message this turn is answering; see
    /// [`ResultMessage::user_message_uuid`].
    pub user_message_uuid: Option<String>,
    /// `uuid` of every user message this turn has taken in so far.
    pub user_message_uuids: Option<Vec<String>>,
    /// Keys of the nested `message` object the crate does not model, such
    /// as `role` or a field the API added.
    pub message_extra: Map<String, Value>,
    /// Keys of the frame the crate does not model, as the CLI sent them.
    pub extra: Map<String, Value>,
}

impl AssistantMessage {
    /// Creates an assistant message with only its content and model set.
    #[must_use]
    pub fn new(content: Vec<ContentBlock>, model: impl Into<String>) -> Self {
        Self {
            content,
            model: model.into(),
            parent_tool_use_id: None,
            error: None,
            usage: None,
            message_id: None,
            stop_reason: None,
            session_id: None,
            uuid: None,
            user_message_uuid: None,
            user_message_uuids: None,
            message_extra: Map::new(),
            extra: Map::new(),
        }
    }
}

/// System message.
///
/// The subtypes this crate knows get a dedicated typed variant; every other
/// subtype lands in [`SystemMessage::Other`]. Whatever the variant,
/// [`SystemMessage::subtype`] and [`SystemMessage::data`] expose the raw
/// frame.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum SystemMessage {
    /// A task started.
    TaskStarted(TaskStartedMessage),
    /// A task is making progress.
    TaskProgress(TaskProgressMessage),
    /// A task completed, failed, or was stopped.
    TaskNotification(TaskNotificationMessage),
    /// The state of a background task changed.
    TaskUpdated(TaskUpdatedMessage),
    /// A session store append failed.
    MirrorError(MirrorErrorMessage),
    /// A hook started or answered.
    HookEvent(HookEventMessage),
    /// Any other subtype, including `init`.
    Other(GenericSystemMessage),
}

impl SystemMessage {
    /// Subtype of the frame.
    #[must_use]
    pub fn subtype(&self) -> &str {
        match self {
            Self::TaskStarted(message) => &message.subtype,
            Self::TaskProgress(message) => &message.subtype,
            Self::TaskNotification(message) => &message.subtype,
            Self::TaskUpdated(message) => &message.subtype,
            Self::MirrorError(message) => &message.subtype,
            Self::HookEvent(message) => &message.subtype,
            Self::Other(message) => &message.subtype,
        }
    }

    /// Raw frame as sent by the CLI.
    #[must_use]
    pub fn data(&self) -> &Value {
        match self {
            Self::TaskStarted(message) => &message.data,
            Self::TaskProgress(message) => &message.data,
            Self::TaskNotification(message) => &message.data,
            Self::TaskUpdated(message) => &message.data,
            Self::MirrorError(message) => &message.data,
            Self::HookEvent(message) => &message.data,
            Self::Other(message) => &message.data,
        }
    }

    /// Typed view of an `init` frame.
    ///
    /// Returns `None` for any other subtype, or when the payload does not
    /// have the documented shape.
    #[must_use]
    pub fn init_data(&self) -> Option<SystemInitData> {
        match self {
            Self::Other(message) if message.subtype == "init" => {
                SystemInitData::deserialize(&message.data).ok()
            }
            _ => None,
        }
    }

    pub(crate) fn set_data(&mut self, data: Value) {
        match self {
            Self::TaskStarted(message) => message.data = data,
            Self::TaskProgress(message) => message.data = data,
            Self::TaskNotification(message) => message.data = data,
            Self::TaskUpdated(message) => message.data = data,
            Self::MirrorError(message) => message.data = data,
            Self::HookEvent(message) => message.data = data,
            Self::Other(message) => message.data = data,
        }
    }
}

/// System message of a subtype without a dedicated type.
///
/// For subtype `init`, `data` is the session metadata frame described by
/// [`SystemInitData`].
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct GenericSystemMessage {
    /// Subtype of the frame.
    pub subtype: String,
    /// Raw frame as sent by the CLI.
    pub data: Value,
}

/// An MCP server listed in [`SystemInitData`].
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
#[non_exhaustive]
pub struct SystemInitMcpServer {
    /// Server name.
    pub name: String,
    /// Connection status.
    pub status: String,
    /// Where the definition came from: `sdk`, `plugin`, or a config scope.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

/// A loaded plugin listed in [`SystemInitData`].
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
#[non_exhaustive]
pub struct SystemInitPlugin {
    /// Plugin name.
    pub name: String,
    /// Plugin path.
    pub path: String,
    /// Version declared in the plugin manifest, verbatim.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

/// A plugin load failure listed in [`SystemInitData`].
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
#[non_exhaustive]
pub struct SystemInitPluginError {
    /// `name@marketplace`, or a positional tag such as `inline[0]`.
    pub plugin: String,
    /// Failure category, from an open set (`type` on the wire).
    #[serde(rename = "type")]
    pub kind: String,
    /// Description of the failure.
    pub message: String,
    /// Path of the entry that did not load at all, resolved against the
    /// working directory.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

wire_enum! {
    /// Fast mode state reported in [`SystemInitData`].
    pub enum FastModeState {
        /// Fast mode is off.
        Off => "off",
        /// Fast mode is cooling down.
        Cooldown => "cooldown",
        /// Fast mode is on.
        On => "on",
    }
}

wire_enum! {
    /// Why fast mode cannot serve right now, reported in [`SystemInitData`].
    pub enum FastModeDisabledReason {
        /// The plan is free.
        Free => "free",
        /// Disabled by user preference.
        Preference => "preference",
        /// Extra usage is disabled.
        ExtraUsageDisabled => "extra_usage_disabled",
        /// A network error occurred.
        NetworkError => "network_error",
        /// The reason is unknown to the CLI.
        UnknownReason => "unknown",
        /// The provider is not first party.
        NotFirstParty => "not_first_party",
        /// Disabled by an environment variable.
        DisabledByEnv => "disabled_by_env",
        /// The selected model does not allow it.
        ModelNotAllowed => "model_not_allowed",
        /// The SDK host must opt in.
        SdkOptInRequired => "sdk_opt_in_required",
        /// The check is still pending.
        Pending => "pending",
    }
}

/// Shape of [`SystemMessage::data`] for an `init` frame.
///
/// The CLI emits this session metadata at the start of each turn. The parser
/// does not validate the payload, so this describes what the CLI sends
/// rather than a guarantee: a missing field takes its default value. Obtain
/// it with [`SystemMessage::init_data`].
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
#[non_exhaustive]
pub struct SystemInitData {
    /// Unique id of the frame.
    pub uuid: String,
    /// Session the frame belongs to.
    pub session_id: String,
    /// Names of the available agents.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agents: Option<Vec<String>>,
    /// Where the API credential came from, or `none`.
    #[serde(rename = "apiKeySource")]
    pub api_key_source: String,
    /// Enabled beta features.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub betas: Option<Vec<String>>,
    /// Version of the CLI.
    pub claude_code_version: String,
    /// Working directory of the session.
    pub cwd: String,
    /// Names of the available tools.
    pub tools: Vec<String>,
    /// MCP servers and their status.
    pub mcp_servers: Vec<SystemInitMcpServer>,
    /// Model in use.
    pub model: String,
    /// Permission mode in effect.
    #[serde(rename = "permissionMode")]
    pub permission_mode: PermissionMode,
    /// Available slash commands.
    pub slash_commands: Vec<String>,
    /// Slash commands bound to the local terminal; present when non-empty.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub terminal_slash_commands: Option<Vec<String>>,
    /// Output style in effect.
    pub output_style: String,
    /// Names of the available skills.
    pub skills: Vec<String>,
    /// Loaded plugins.
    pub plugins: Vec<SystemInitPlugin>,
    /// Plugin load failures. An absent value does not assert a clean load.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plugin_errors: Option<Vec<SystemInitPluginError>>,
    /// Fast mode state.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fast_mode_state: Option<FastModeState>,
    /// The check that blocks fast mode, when something does.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fast_mode_disabled_reason: Option<FastModeDisabledReason>,
    /// Protocol capabilities of this CLI, for feature detection. Open set:
    /// ignore unknown values.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<Vec<String>>,
}

/// Usage statistics reported by task progress and notification messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
#[non_exhaustive]
pub struct TaskUsage {
    /// Tokens consumed so far.
    pub total_tokens: u64,
    /// Number of tool calls so far.
    pub tool_uses: u64,
    /// Elapsed time in milliseconds.
    pub duration_ms: u64,
}

wire_enum! {
    /// Status carried by a [`TaskNotificationMessage`].
    pub enum TaskNotificationStatus {
        /// The task completed.
        Completed => "completed",
        /// The task failed.
        Failed => "failed",
        /// The task was stopped.
        Stopped => "stopped",
    }
}

wire_enum! {
    /// Status carried by the patch of a [`TaskUpdatedMessage`].
    ///
    /// `pending`, `running`, and `paused` are not terminal; `completed`,
    /// `failed`, and `killed` are. The raw `killed` appears here, while a
    /// [`TaskNotificationMessage`] reports it as `stopped`.
    pub enum TaskUpdatedStatus {
        /// Not started yet.
        Pending => "pending",
        /// Running.
        Running => "running",
        /// Paused.
        Paused => "paused",
        /// Completed.
        Completed => "completed",
        /// Failed.
        Failed => "failed",
        /// Killed.
        Killed => "killed",
    }
}

/// Task statuses that mean the task has finished.
///
/// The set spans both vocabularies: a [`TaskNotificationMessage`] reports
/// `stopped` while a [`TaskUpdatedMessage`] reports the raw `killed`.
pub const TERMINAL_TASK_STATUSES: [&str; 4] = ["completed", "failed", "stopped", "killed"];

/// Returns whether `status` is one of [`TERMINAL_TASK_STATUSES`].
#[must_use]
pub fn is_terminal_task_status(status: &str) -> bool {
    TERMINAL_TASK_STATUSES.contains(&status)
}

impl TaskNotificationStatus {
    /// Whether the status is one of [`TERMINAL_TASK_STATUSES`].
    #[must_use]
    pub fn is_terminal(&self) -> bool {
        is_terminal_task_status(self.as_str())
    }
}

impl TaskUpdatedStatus {
    /// Whether the status is one of [`TERMINAL_TASK_STATUSES`].
    #[must_use]
    pub fn is_terminal(&self) -> bool {
        is_terminal_task_status(self.as_str())
    }
}

/// System message emitted when a task starts.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct TaskStartedMessage {
    /// Subtype of the frame, `task_started`.
    pub subtype: String,
    /// Raw frame as sent by the CLI.
    pub data: Value,
    /// Id of the task.
    pub task_id: String,
    /// Description of the task.
    pub description: String,
    /// Unique id of the frame.
    pub uuid: String,
    /// Session the frame belongs to.
    pub session_id: String,
    /// Id of the tool call that started the task.
    pub tool_use_id: Option<String>,
    /// Kind of task, such as `local_agent`.
    pub task_type: Option<String>,
}

/// System message emitted while a task is in progress.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct TaskProgressMessage {
    /// Subtype of the frame, `task_progress`.
    pub subtype: String,
    /// Raw frame as sent by the CLI.
    pub data: Value,
    /// Id of the task.
    pub task_id: String,
    /// Description of the current step.
    pub description: String,
    /// Usage so far.
    pub usage: TaskUsage,
    /// Unique id of the frame.
    pub uuid: String,
    /// Session the frame belongs to.
    pub session_id: String,
    /// Id of the tool call that started the task.
    pub tool_use_id: Option<String>,
    /// Name of the last tool the task called.
    pub last_tool_name: Option<String>,
}

/// System message emitted when a task completes, fails, or is stopped.
///
/// Not every terminal task emits this message: a background task may report
/// completion only through a [`TaskUpdatedMessage`] whose status is
/// terminal. Clear tracked task ids on a terminal status from either
/// message.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct TaskNotificationMessage {
    /// Subtype of the frame, `task_notification`.
    pub subtype: String,
    /// Raw frame as sent by the CLI.
    pub data: Value,
    /// Id of the task.
    pub task_id: String,
    /// Final status.
    pub status: TaskNotificationStatus,
    /// Path of the file holding the task output.
    pub output_file: String,
    /// Summary of the outcome.
    pub summary: String,
    /// Unique id of the frame.
    pub uuid: String,
    /// Session the frame belongs to.
    pub session_id: String,
    /// Id of the tool call that started the task.
    pub tool_use_id: Option<String>,
    /// Final usage, when reported.
    pub usage: Option<TaskUsage>,
}

/// System message emitted when the state of a background task changes.
///
/// `patch` carries the changed fields; when its status is terminal the task
/// has finished. Parsing this message never fails: a missing `task_id`
/// becomes an empty string and a `patch` that is not an object becomes an
/// empty one.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct TaskUpdatedMessage {
    /// Subtype of the frame, `task_updated`.
    pub subtype: String,
    /// Raw frame as sent by the CLI.
    pub data: Value,
    /// Id of the task, empty when absent.
    pub task_id: String,
    /// Changed fields.
    pub patch: Map<String, Value>,
    /// Status found in the patch, when it is a string.
    pub status: Option<TaskUpdatedStatus>,
    /// Session the frame belongs to.
    pub session_id: Option<String>,
    /// Unique id of the frame.
    pub uuid: Option<String>,
}

/// System message emitted when a session store append fails.
///
/// The failure is not fatal: the local transcript is already durable and
/// the session continues. The failed batch was dropped, so the external
/// store may be missing it.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct MirrorErrorMessage {
    /// Subtype of the frame, `mirror_error`.
    pub subtype: String,
    /// Raw frame.
    pub data: Value,
    /// Key of the transcript whose append failed.
    pub key: Option<SessionKey>,
    /// Error text, empty when absent.
    pub error: String,
}

/// Hook lifecycle event, emitted when hook events are included in the
/// stream.
///
/// The frame arrives as a system message of subtype `hook_started` or
/// `hook_response`; the latter carries `output`, `exit_code`, and `outcome`
/// in `data`.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct HookEventMessage {
    /// Lifecycle phase: `hook_started` or `hook_response`.
    pub subtype: String,
    /// Raw frame as sent by the CLI.
    pub data: Value,
    /// Name of the hook event, such as `PreToolUse`; empty when absent.
    pub hook_event_name: String,
    /// Session the event belongs to.
    pub session_id: Option<String>,
    /// Unique id of the event.
    pub uuid: Option<String>,
}

/// Tool call deferred by a `PreToolUse` hook that answered `defer`.
///
/// The run stops and the result message carries the deferred call so the
/// caller can inspect it and decide whether to resume.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct DeferredToolUse {
    /// Identifier of the tool call.
    pub id: String,
    /// Name of the tool.
    pub name: String,
    /// Tool input, normally a JSON object.
    pub input: Value,
}

/// Token usage of a turn or of a run, as the CLI reports it.
///
/// It holds the JSON the CLI sent, so no key is ever lost (the Python SDK
/// passes the same dictionary through), and reads the counts the Messages
/// API documents. A count that is missing or not a number reads as `None`;
/// a fraction is truncated, as the message parser reads counts.
///
/// ```
/// use claude_agent_rust_sdk::Usage;
///
/// let usage = Usage::from(serde_json::json!({"input_tokens": 12, "service_tier": "standard"}));
/// assert_eq!(usage.input_tokens(), Some(12));
/// assert_eq!(usage.output_tokens(), None);
/// assert_eq!(usage.service_tier(), Some("standard"));
/// ```
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Usage(Value);

impl Usage {
    fn count(&self, key: &str) -> Option<u64> {
        let value = self.0.get(key)?;
        value
            .as_u64()
            .or_else(|| value.as_f64().map(|number| number as u64))
    }

    /// Input tokens not read from or written to the cache.
    #[must_use]
    pub fn input_tokens(&self) -> Option<u64> {
        self.count("input_tokens")
    }

    /// Output tokens.
    #[must_use]
    pub fn output_tokens(&self) -> Option<u64> {
        self.count("output_tokens")
    }

    /// Input tokens written to the cache.
    #[must_use]
    pub fn cache_creation_input_tokens(&self) -> Option<u64> {
        self.count("cache_creation_input_tokens")
    }

    /// Input tokens read from the cache.
    #[must_use]
    pub fn cache_read_input_tokens(&self) -> Option<u64> {
        self.count("cache_read_input_tokens")
    }

    /// Service tier that served the request, such as `standard`.
    #[must_use]
    pub fn service_tier(&self) -> Option<&str> {
        self.0.get("service_tier").and_then(Value::as_str)
    }

    /// The value of any key, modeled or not.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.0.get(key)
    }

    /// The JSON the CLI sent.
    #[must_use]
    pub fn as_value(&self) -> &Value {
        &self.0
    }

    /// Returns the JSON the CLI sent.
    #[must_use]
    pub fn into_value(self) -> Value {
        self.0
    }
}

impl From<Value> for Usage {
    fn from(value: Value) -> Self {
        Self(value)
    }
}

impl From<Usage> for Value {
    fn from(usage: Usage) -> Self {
        usage.0
    }
}

/// Token usage and cost of one model.
///
/// The wire keys are camelCase; a missing key takes its default value, and
/// a key the struct does not model lands in [`extra`](Self::extra).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
#[non_exhaustive]
pub struct ModelUsage {
    /// Input tokens.
    pub input_tokens: u64,
    /// Output tokens.
    pub output_tokens: u64,
    /// Input tokens read from the cache.
    pub cache_read_input_tokens: u64,
    /// Input tokens written to the cache.
    pub cache_creation_input_tokens: u64,
    /// Web search requests.
    pub web_search_requests: u64,
    /// Cost in US dollars (`costUSD` on the wire).
    #[serde(rename = "costUSD")]
    pub cost_usd: f64,
    /// Context window of the model.
    pub context_window: u64,
    /// Maximum output tokens of the model.
    pub max_output_tokens: u64,
    /// Canonical model id used for the pricing lookup.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub canonical_model: Option<String>,
    /// API provider that served the model.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// Keys this version of the crate does not model, kept as the CLI sent
    /// them (the Python SDK passes the whole dictionary through) and written
    /// back on serialization.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// Result message with cost and usage information.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct ResultMessage {
    /// Result subtype, such as `success` or `error_max_turns`.
    pub subtype: String,
    /// Total duration in milliseconds.
    pub duration_ms: u64,
    /// Time spent in API calls, in milliseconds.
    pub duration_api_ms: u64,
    /// Whether the run ended with an error.
    pub is_error: bool,
    /// Number of turns.
    pub num_turns: u64,
    /// Session the result belongs to.
    pub session_id: String,
    /// Stop reason of the final assistant turn.
    pub stop_reason: Option<String>,
    /// Total cost in US dollars.
    pub total_cost_usd: Option<f64>,
    /// Aggregate token usage.
    pub usage: Option<Usage>,
    /// Result text.
    pub result: Option<String>,
    /// Structured output, when an output format was requested.
    pub structured_output: Option<Value>,
    /// Usage per model, keyed by model name (`modelUsage` on the wire). An
    /// entry that is not a JSON object is skipped.
    pub model_usage: Option<IndexMap<String, ModelUsage>>,
    /// Permission denials, as raw JSON.
    pub permission_denials: Option<Vec<Value>>,
    /// Tool call deferred by a hook.
    pub deferred_tool_use: Option<DeferredToolUse>,
    /// Error strings reported by the CLI.
    pub errors: Option<Vec<String>>,
    /// HTTP status of the failing API call, when `is_error` is true and the
    /// subtype is `success`.
    pub api_error_status: Option<i64>,
    /// Unique id of the message.
    pub uuid: Option<String>,
    /// Why the query loop ended, such as `completed` or `max_turns`. The
    /// values `aborted_streaming` and `aborted_tools` mean the turn was
    /// cancelled.
    pub terminal_reason: Option<String>,
    /// Origin of the user message that started this turn.
    pub origin: Option<MessageOrigin>,
    /// `uuid` of the user message that started this turn; on an injected
    /// turn, the last user message taken in while it was running. Set on
    /// error results too.
    pub user_message_uuid: Option<String>,
    /// `uuid` of every user message this turn took in, in order. The CLI
    /// lists at most 64.
    pub user_message_uuids: Option<Vec<String>>,
    /// Keys of the frame the crate does not model, as the CLI sent them.
    pub extra: Map<String, Value>,
}

impl ResultMessage {
    /// Creates a result message with only its required fields set.
    #[must_use]
    pub fn new(
        subtype: impl Into<String>,
        duration_ms: u64,
        duration_api_ms: u64,
        is_error: bool,
        num_turns: u64,
        session_id: impl Into<String>,
    ) -> Self {
        Self {
            subtype: subtype.into(),
            duration_ms,
            duration_api_ms,
            is_error,
            num_turns,
            session_id: session_id.into(),
            stop_reason: None,
            total_cost_usd: None,
            usage: None,
            result: None,
            structured_output: None,
            model_usage: None,
            permission_denials: None,
            deferred_tool_use: None,
            errors: None,
            api_error_status: None,
            uuid: None,
            terminal_reason: None,
            origin: None,
            user_message_uuid: None,
            user_message_uuids: None,
            extra: Map::new(),
        }
    }
}

/// Stream event for partial message updates.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct StreamEvent {
    /// Unique id of the event.
    pub uuid: String,
    /// Session the event belongs to.
    pub session_id: String,
    /// Raw API stream event.
    pub event: Value,
    /// Id of the tool call that spawned the subagent this event belongs to.
    pub parent_tool_use_id: Option<String>,
    /// `uuid` of the user message this turn is answering.
    pub user_message_uuid: Option<String>,
    /// `uuid` of every user message this turn has taken in so far.
    pub user_message_uuids: Option<Vec<String>>,
    /// Keys of the frame the crate does not model, as the CLI sent them.
    pub extra: Map<String, Value>,
}

wire_enum! {
    /// Rate limit status.
    pub enum RateLimitStatus {
        /// Within the limit.
        Allowed => "allowed",
        /// Approaching the limit.
        AllowedWarning => "allowed_warning",
        /// The limit has been hit.
        Rejected => "rejected",
    }
}

wire_enum! {
    /// Rate limit window.
    pub enum RateLimitType {
        /// Five-hour window.
        FiveHour => "five_hour",
        /// Seven-day window.
        SevenDay => "seven_day",
        /// Seven-day window for Opus models.
        SevenDayOpus => "seven_day_opus",
        /// Seven-day window for Sonnet models.
        SevenDaySonnet => "seven_day_sonnet",
        /// Overage usage.
        Overage => "overage",
    }
}

/// Rate limit state emitted by the CLI when it changes.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct RateLimitInfo {
    /// Current status.
    pub status: RateLimitStatus,
    /// Unix timestamp at which the window resets.
    pub resets_at: Option<i64>,
    /// Which window applies.
    pub rate_limit_type: Option<RateLimitType>,
    /// Fraction of the limit consumed, from 0.0 to 1.0.
    pub utilization: Option<f64>,
    /// Status of overage usage, when applicable.
    pub overage_status: Option<RateLimitStatus>,
    /// Unix timestamp at which the overage window resets.
    pub overage_resets_at: Option<i64>,
    /// Why overage is unavailable, when it is rejected.
    pub overage_disabled_reason: Option<String>,
    /// Raw object from the CLI, including fields not modeled above.
    pub raw: Value,
}

/// Rate limit event, emitted whenever the rate limit status changes.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct RateLimitEvent {
    /// The new rate limit state.
    pub rate_limit_info: RateLimitInfo,
    /// Unique id of the event.
    pub uuid: String,
    /// Session the event belongs to.
    pub session_id: String,
}

/// Emitted when the conversation is replaced without ending the connection,
/// for example after `/clear`.
///
/// A reset clears the history and zeroes the running totals reported on
/// later result messages.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ConversationResetMessage {
    /// Opaque identifier of the fresh conversation. This is not the session
    /// id of later messages.
    pub new_conversation_id: String,
    /// Unique id of this message.
    pub uuid: String,
    /// Id of the session that was reset.
    pub session_id: String,
}
