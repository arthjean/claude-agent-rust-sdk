//! Permission modes, permission updates, and the tool-permission callback.

use std::fmt;
use std::future::Future;
use std::sync::Arc;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value, json};

use super::BoxFuture;
use super::macros::wire_enum;
use crate::error::{BoxError, Error};

wire_enum! {
    /// Permission mode of a session.
    #[derive(Default)]
    pub enum PermissionMode {
        /// Standard behavior: prompts for dangerous operations.
        #[default]
        Default => "default",
        /// File edits are accepted automatically.
        AcceptEdits => "acceptEdits",
        /// Planning mode: tools are not executed.
        Plan => "plan",
        /// Every permission check is bypassed.
        BypassPermissions => "bypassPermissions",
        /// Never prompts: a call that is not approved in advance is denied.
        DontAsk => "dontAsk",
        /// A model classifier approves or denies each tool call.
        Auto => "auto",
    }
}

wire_enum! {
    /// Where a permission update is stored.
    pub enum PermissionUpdateDestination {
        /// Global user settings.
        UserSettings => "userSettings",
        /// Project settings.
        ProjectSettings => "projectSettings",
        /// Local settings.
        LocalSettings => "localSettings",
        /// The current session only.
        Session => "session",
    }
}

wire_enum! {
    /// Behavior of a permission rule.
    pub enum PermissionBehavior {
        /// Allow matching calls.
        Allow => "allow",
        /// Deny matching calls.
        Deny => "deny",
        /// Ask before matching calls.
        Ask => "ask",
    }
}

wire_enum! {
    /// Kind of a [`PermissionUpdate`].
    pub enum PermissionUpdateKind {
        /// Add rules.
        AddRules => "addRules",
        /// Replace rules.
        ReplaceRules => "replaceRules",
        /// Remove rules.
        RemoveRules => "removeRules",
        /// Change the permission mode.
        SetMode => "setMode",
        /// Add directories.
        AddDirectories => "addDirectories",
        /// Remove directories.
        RemoveDirectories => "removeDirectories",
    }
}

/// One permission rule: a tool name and an optional rule content.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct PermissionRuleValue {
    /// Name of the tool (`toolName` on the wire).
    pub tool_name: String,
    /// What the rule matches within the tool, such as `"npm test"` for
    /// Bash (`ruleContent` on the wire). `None`, or an empty string, makes a
    /// rule that covers every use of the tool; `None` leaves the key out.
    pub rule_content: Option<String>,
}

impl PermissionRuleValue {
    /// Creates a rule.
    #[must_use]
    pub fn new(tool_name: impl Into<String>, rule_content: Option<String>) -> Self {
        Self {
            tool_name: tool_name.into(),
            rule_content,
        }
    }
}

/// A change to the permission configuration.
///
/// The wire form depends on [`PermissionUpdate::kind`]: rule kinds carry
/// `rules` and `behavior`, `setMode` carries `mode`, and directory kinds
/// carry `directories`. Fields that do not apply to the kind are not sent.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct PermissionUpdate {
    /// Kind of update (`type` on the wire).
    pub kind: PermissionUpdateKind,
    /// Rules, for the rule kinds.
    pub rules: Option<Vec<PermissionRuleValue>>,
    /// Behavior of the rules, for the rule kinds.
    pub behavior: Option<PermissionBehavior>,
    /// Mode, for `setMode`.
    pub mode: Option<PermissionMode>,
    /// Directories, for the directory kinds.
    pub directories: Option<Vec<String>>,
    /// Where the update applies and is kept. `None` is sent as
    /// [`PermissionUpdateDestination::Session`].
    pub destination: Option<PermissionUpdateDestination>,
}

impl PermissionUpdate {
    /// Creates an update of the given kind with every other field unset.
    #[must_use]
    pub fn new(kind: impl Into<PermissionUpdateKind>) -> Self {
        Self {
            kind: kind.into(),
            rules: None,
            behavior: None,
            mode: None,
            directories: None,
            destination: None,
        }
    }

    /// Sets the rules.
    #[must_use]
    pub fn rules(mut self, rules: impl IntoIterator<Item = PermissionRuleValue>) -> Self {
        self.rules = Some(rules.into_iter().collect());
        self
    }

    /// Sets the behavior.
    #[must_use]
    pub fn behavior(mut self, behavior: impl Into<PermissionBehavior>) -> Self {
        self.behavior = Some(behavior.into());
        self
    }

    /// Sets the mode.
    #[must_use]
    pub fn mode(mut self, mode: impl Into<PermissionMode>) -> Self {
        self.mode = Some(mode.into());
        self
    }

    /// Sets the directories.
    #[must_use]
    pub fn directories(mut self, directories: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.directories = Some(directories.into_iter().map(Into::into).collect());
        self
    }

    /// Sets the destination.
    #[must_use]
    pub fn destination(mut self, destination: impl Into<PermissionUpdateDestination>) -> Self {
        self.destination = Some(destination.into());
        self
    }

    /// Converts the update to its control-protocol form.
    ///
    /// The CLI rejects an update that lacks a field its kind needs, or whose
    /// `ruleContent` is `null`, and then ignores every update of the same
    /// response. So this method:
    ///
    /// - leaves `ruleContent` out when [`PermissionRuleValue::rule_content`]
    ///   is `None`, which is how the CLI expects a rule for the whole tool;
    /// - sends `"session"` when [`PermissionUpdate::destination`] is `None`,
    ///   so the update changes only the current session;
    /// - fails when a field the kind needs is `None`: `rules` and `behavior`
    ///   for the rule kinds, `mode` for `setMode`, `directories` for the
    ///   directory kinds;
    /// - fails when the kind is [`PermissionUpdateKind::Unknown`].
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::Config`](crate::ErrorKind::Config) in the two failing cases above.
    pub fn to_wire(&self) -> Result<Value, Error> {
        let kind = self.kind.as_str();
        let missing = |field: &str, hint: &str| {
            Error::config(format!(
                "PermissionUpdate(type='{kind}') has no {field}. {hint}"
            ))
        };
        let mut wire = Map::new();
        wire.insert("type".to_owned(), json!(kind));
        wire.insert(
            "destination".to_owned(),
            json!(
                self.destination
                    .as_ref()
                    .map_or("session", PermissionUpdateDestination::as_str)
            ),
        );
        match self.kind {
            PermissionUpdateKind::AddRules
            | PermissionUpdateKind::ReplaceRules
            | PermissionUpdateKind::RemoveRules => {
                let rules = self.rules.as_ref().ok_or_else(|| {
                    missing("rules", "Set rules to a list of PermissionRuleValue.")
                })?;
                let behavior = self.behavior.as_ref().ok_or_else(|| {
                    missing("behavior", "Set behavior to 'allow', 'deny' or 'ask'.")
                })?;
                let rules: Vec<Value> = rules
                    .iter()
                    .map(|rule| {
                        let mut wire_rule = Map::new();
                        wire_rule.insert("toolName".to_owned(), json!(rule.tool_name));
                        if let Some(content) = &rule.rule_content {
                            wire_rule.insert("ruleContent".to_owned(), json!(content));
                        }
                        Value::Object(wire_rule)
                    })
                    .collect();
                wire.insert("rules".to_owned(), Value::Array(rules));
                wire.insert("behavior".to_owned(), json!(behavior.as_str()));
            }
            PermissionUpdateKind::SetMode => {
                let mode = self.mode.as_ref().ok_or_else(|| {
                    missing(
                        "mode",
                        "Set mode to a permission mode such as 'acceptEdits'.",
                    )
                })?;
                wire.insert("mode".to_owned(), json!(mode.as_str()));
            }
            PermissionUpdateKind::AddDirectories | PermissionUpdateKind::RemoveDirectories => {
                let directories = self
                    .directories
                    .as_ref()
                    .ok_or_else(|| missing("directories", "Set directories to a list of paths."))?;
                wire.insert("directories".to_owned(), json!(directories));
            }
            PermissionUpdateKind::Unknown(_) => {
                return Err(Error::config(format!(
                    "PermissionUpdate has type '{kind}', which this version of the SDK \
                     cannot send. Use one of addRules, replaceRules, removeRules, setMode, \
                     addDirectories or removeDirectories, or upgrade the SDK if this update \
                     came from the CLI."
                )));
            }
        }
        Ok(Value::Object(wire))
    }

    /// Builds an update from its control-protocol form.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::MessageParse`](crate::ErrorKind::MessageParse) when `wire` is not an object, when
    /// `type` is missing, or when a rule lacks `toolName`.
    pub fn from_wire(wire: &Value) -> Result<Self, Error> {
        let invalid = |message: &str| Error::message_parse(message, Some(wire.clone()));
        let object = wire
            .as_object()
            .ok_or_else(|| invalid("Invalid permission update (expected an object)"))?;
        let kind = object
            .get("type")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid("Missing required field in permission update: 'type'"))?;
        let rules = match object.get("rules") {
            None | Some(Value::Null) => None,
            Some(Value::Array(items)) => {
                let mut rules = Vec::with_capacity(items.len());
                for item in items {
                    let tool_name =
                        item.get("toolName")
                            .and_then(Value::as_str)
                            .ok_or_else(|| {
                                invalid("Missing required field in permission rule: 'toolName'")
                            })?;
                    let rule_content = item
                        .get("ruleContent")
                        .and_then(Value::as_str)
                        .map(str::to_owned);
                    rules.push(PermissionRuleValue::new(tool_name, rule_content));
                }
                Some(rules)
            }
            Some(_) => return Err(invalid("Invalid permission update (rules must be a list)")),
        };
        let string = |key: &str| object.get(key).and_then(Value::as_str);
        let directories = object
            .get("directories")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            });
        Ok(Self {
            kind: PermissionUpdateKind::from(kind),
            rules,
            behavior: string("behavior").map(PermissionBehavior::from),
            mode: string("mode").map(PermissionMode::from),
            directories,
            destination: string("destination").map(PermissionUpdateDestination::from),
        })
    }
}

impl Serialize for PermissionUpdate {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.to_wire()
            .map_err(serde::ser::Error::custom)?
            .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for PermissionUpdate {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let wire = Value::deserialize(deserializer)?;
        Self::from_wire(&wire).map_err(serde::de::Error::custom)
    }
}

/// Context handed to the tool-permission callback.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[non_exhaustive]
pub struct ToolPermissionContext {
    /// Permission updates suggested by the CLI.
    pub suggestions: Vec<PermissionUpdate>,
    /// Identifier of this tool call within the assistant message.
    pub tool_use_id: Option<String>,
    /// Id of the subagent, when the call comes from one.
    pub agent_id: Option<String>,
    /// File path that triggered the request, when applicable.
    pub blocked_path: Option<String>,
    /// Why this request was triggered.
    pub decision_reason: Option<String>,
    /// Full prompt sentence, to use as the primary prompt text when present.
    pub title: Option<String>,
    /// Short noun phrase for the tool action, suitable for a button label.
    pub display_name: Option<String>,
    /// Subtitle for the permission prompt.
    pub description: Option<String>,
}

/// Decision returned by the tool-permission callback.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum PermissionResult {
    /// Allow the tool call. Built with [`PermissionResult::allow`] and the
    /// methods that refine it; new fields may be added, so a pattern needs
    /// `..`.
    #[non_exhaustive]
    Allow {
        /// Replacement input; the original input is used when absent.
        updated_input: Option<Value>,
        /// Permission updates to apply along with this allow. If any of them
        /// cannot be sent ([`PermissionUpdate::to_wire`] fails), none are
        /// sent and a warning is logged; the tool call is still allowed.
        updated_permissions: Option<Vec<PermissionUpdate>>,
    },
    /// Deny the tool call. Built with [`PermissionResult::deny`] or
    /// [`PermissionResult::deny_and_interrupt`].
    #[non_exhaustive]
    Deny {
        /// Explanation given to the model.
        message: String,
        /// Whether to interrupt the run.
        interrupt: bool,
    },
}

impl PermissionResult {
    /// Allows the call with its original input.
    #[must_use]
    pub fn allow() -> Self {
        Self::Allow {
            updated_input: None,
            updated_permissions: None,
        }
    }

    /// Allows the call with a replacement input.
    #[must_use]
    pub fn allow_with_input(updated_input: Value) -> Self {
        Self::Allow {
            updated_input: Some(updated_input),
            updated_permissions: None,
        }
    }

    /// Denies the call without interrupting the run.
    #[must_use]
    pub fn deny(message: impl Into<String>) -> Self {
        Self::Deny {
            message: message.into(),
            interrupt: false,
        }
    }

    /// Denies the call and interrupts the run.
    #[must_use]
    pub fn deny_and_interrupt(message: impl Into<String>) -> Self {
        Self::Deny {
            message: message.into(),
            interrupt: true,
        }
    }

    /// Sets the permission updates to apply along with an allow. It has no
    /// effect on a deny, which carries none.
    #[must_use]
    pub fn updated_permissions(
        mut self,
        updates: impl IntoIterator<Item = PermissionUpdate>,
    ) -> Self {
        if let Self::Allow {
            updated_permissions,
            ..
        } = &mut self
        {
            *updated_permissions = Some(updates.into_iter().collect());
        }
        self
    }
}

type CanUseToolFn = dyn Fn(
        String,
        Value,
        ToolPermissionContext,
    ) -> BoxFuture<'static, Result<PermissionResult, BoxError>>
    + Send
    + Sync;

/// Tool-permission callback.
///
/// It receives the tool name, the tool input, and a
/// [`ToolPermissionContext`], and answers with a [`PermissionResult`]. It
/// replaces the interactive permission prompt, so it runs only for calls
/// that would otherwise prompt.
///
/// Any async closure converts into this type:
///
/// ```
/// use claude_agent_rust_sdk::{CanUseTool, PermissionResult};
///
/// let callback = CanUseTool::new(async |_tool_name, _input, _context| {
///     Ok(PermissionResult::allow())
/// });
/// # let _ = callback;
/// ```
#[derive(Clone)]
pub struct CanUseTool(Arc<CanUseToolFn>);

impl CanUseTool {
    /// Wraps an async closure.
    pub fn new<F, Fut>(callback: F) -> Self
    where
        F: Fn(String, Value, ToolPermissionContext) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<PermissionResult, BoxError>> + Send + 'static,
    {
        Self(Arc::new(move |tool_name, input, context| {
            Box::pin(callback(tool_name, input, context))
        }))
    }

    /// Invokes the callback.
    pub fn call(
        &self,
        tool_name: String,
        input: Value,
        context: ToolPermissionContext,
    ) -> BoxFuture<'static, Result<PermissionResult, BoxError>> {
        (self.0)(tool_name, input, context)
    }
}

impl<F, Fut> From<F> for CanUseTool
where
    F: Fn(String, Value, ToolPermissionContext) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<PermissionResult, BoxError>> + Send + 'static,
{
    fn from(callback: F) -> Self {
        Self::new(callback)
    }
}

impl fmt::Debug for CanUseTool {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("CanUseTool(..)")
    }
}
