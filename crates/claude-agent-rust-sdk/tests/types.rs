#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::time::Duration;

use claude_agent_rust_sdk::ErrorKind;
use claude_agent_rust_sdk::{
    AgentDefinition, AgentEffort, AgentMcpServer, AgentMemory, AssistantMessage, AsyncHookOutput,
    BackgroundTaskSummary, CanUseTool, ContentBlock, ContextUsageResponse, EffortLevel,
    ElicitationAction, ElicitationHookSpecificOutput, FastModeDisabledReason, FastModeState,
    HookCallback, HookContext, HookDecision, HookEffort, HookEvent, HookFailureMode, HookInput,
    HookMatcher, HookOutput, HookPermissionDecision, HookSpecificOutput, McpServerConnectionStatus,
    McpServerStatus, McpServerStatusConfig, McpStatusResponse, PermissionBehavior, PermissionMode,
    PermissionRequestHookSpecificOutput, PermissionResult, PermissionRuleValue, PermissionUpdate,
    PermissionUpdateDestination, PermissionUpdateKind, PostToolUseHookSpecificOutput,
    PreModelSwitchHookSpecificOutput, PreToolUseHookSpecificOutput, ResultMessage,
    SessionCronSummary, SessionStartHookSpecificOutput, SyncHookOutput, TextBlock, ThinkingBlock,
    ToolPermissionContext, ToolResultBlock, ToolResultContent, ToolUseBlock, UserMessage,
    UserPromptSubmitHookSpecificOutput, WorktreeCreateHookSpecificOutput,
};
use serde_json::{Map, Value, json};

fn base_hook_input(event: &str, extra: Value) -> Value {
    let mut input = json!({
        "session_id": "sess-1",
        "transcript_path": "/tmp/transcript",
        "cwd": "/home/user",
        "hook_event_name": event,
    });
    let target = input.as_object_mut().unwrap();
    for (key, value) in extra.as_object().unwrap() {
        target.insert(key.clone(), value.clone());
    }
    input
}

fn keys(value: &Value) -> Vec<&str> {
    let mut keys: Vec<&str> = value
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    keys
}

#[test]
fn effort_level_has_the_five_levels() {
    assert_eq!(
        EffortLevel::KNOWN,
        ["low", "medium", "high", "xhigh", "max"]
    );
    assert_eq!(EffortLevel::from("xhigh"), EffortLevel::Xhigh);
    assert_eq!(
        serde_json::to_value(EffortLevel::Max).unwrap(),
        json!("max")
    );
}

#[test]
fn permission_mode_has_the_six_values() {
    assert_eq!(
        PermissionMode::KNOWN,
        [
            "default",
            "acceptEdits",
            "plan",
            "bypassPermissions",
            "dontAsk",
            "auto"
        ]
    );
    let modes = [
        PermissionMode::Default,
        PermissionMode::AcceptEdits,
        PermissionMode::Plan,
        PermissionMode::BypassPermissions,
        PermissionMode::DontAsk,
        PermissionMode::Auto,
    ];
    for (mode, wire) in modes.iter().zip(PermissionMode::KNOWN) {
        assert_eq!(mode.as_str(), *wire);
        assert_eq!(&PermissionMode::from(*wire), mode);
        assert_eq!(serde_json::to_value(mode).unwrap(), json!(wire));
        assert_eq!(
            &serde_json::from_value::<PermissionMode>(json!(wire)).unwrap(),
            mode
        );
    }
    let future = PermissionMode::from("futureMode");
    assert!(matches!(&future, PermissionMode::Unknown(value) if value == "futureMode"));
    assert_eq!(future.as_str(), "futureMode");
}

#[test]
fn a_wire_enum_value_equals_another_exactly_when_their_spellings_do() {
    use std::collections::HashSet;

    let read: HookEvent = serde_json::from_value(json!("PreToolUse")).unwrap();
    assert_eq!(
        read,
        HookEvent::PreToolUse,
        "a known spelling is never Unknown"
    );
    assert_eq!(
        HookEvent::from("PreToolUse".to_owned()),
        HookEvent::PreToolUse
    );
    let future: HookEvent = serde_json::from_value(json!("FutureEvent")).unwrap();
    assert_eq!(future, HookEvent::from("FutureEvent"));
    assert_ne!(future, HookEvent::from("OtherEvent"));
    let set: HashSet<HookEvent> = [read, HookEvent::PreToolUse, future.clone(), future]
        .into_iter()
        .collect();
    assert_eq!(set.len(), 2);
    for value in &set {
        assert_eq!(serde_json::to_value(value).unwrap(), json!(value.as_str()));
    }
}

#[test]
fn hook_event_has_the_ten_values() {
    assert_eq!(
        HookEvent::KNOWN,
        [
            "PreToolUse",
            "PostToolUse",
            "PostToolUseFailure",
            "UserPromptSubmit",
            "Stop",
            "SubagentStop",
            "PreCompact",
            "Notification",
            "SubagentStart",
            "PermissionRequest",
        ]
    );
    let events = [
        HookEvent::PreToolUse,
        HookEvent::PostToolUse,
        HookEvent::PostToolUseFailure,
        HookEvent::UserPromptSubmit,
        HookEvent::Stop,
        HookEvent::SubagentStop,
        HookEvent::PreCompact,
        HookEvent::Notification,
        HookEvent::SubagentStart,
        HookEvent::PermissionRequest,
    ];
    for (event, wire) in events.iter().zip(HookEvent::KNOWN) {
        assert_eq!(event.as_str(), *wire);
    }
}

#[test]
fn permission_update_roundtrip_add_rules() {
    let wire = json!({
        "type": "addRules",
        "destination": "localSettings",
        "behavior": "allow",
        "rules": [
            {"toolName": "Bash", "ruleContent": "npm *"},
            {"toolName": "Read"},
        ],
    });
    let update = PermissionUpdate::from_wire(&wire).unwrap();
    assert_eq!(update.kind, PermissionUpdateKind::AddRules);
    assert_eq!(
        update.destination,
        Some(PermissionUpdateDestination::LocalSettings)
    );
    assert_eq!(update.behavior, Some(PermissionBehavior::Allow));
    assert_eq!(
        update.rules,
        Some(vec![
            PermissionRuleValue::new("Bash", Some("npm *".to_owned())),
            PermissionRuleValue::new("Read", None),
        ])
    );
    assert_eq!(update.to_wire().unwrap(), wire);
    assert_eq!(serde_json::to_value(&update).unwrap(), wire);
    assert_eq!(
        serde_json::from_value::<PermissionUpdate>(wire).unwrap(),
        update
    );
}

#[test]
fn permission_update_serializes_tool_name_and_rule_content_in_camel_case() {
    let update = PermissionUpdate::new(PermissionUpdateKind::ReplaceRules)
        .rules([PermissionRuleValue::new("Bash", Some("ls".to_owned()))])
        .behavior(PermissionBehavior::Deny);
    let wire = update.to_wire().unwrap();
    assert_eq!(wire["rules"][0]["toolName"], "Bash");
    assert_eq!(wire["rules"][0]["ruleContent"], "ls");
    assert_eq!(
        PermissionUpdate::from_wire(&wire).unwrap(),
        update.destination(PermissionUpdateDestination::Session)
    );
}

#[test]
fn whole_tool_rule_has_no_rule_content_key() {
    let update = PermissionUpdate::new(PermissionUpdateKind::AddRules)
        .rules([PermissionRuleValue::new("WebSearch", None)])
        .behavior(PermissionBehavior::Allow)
        .destination(PermissionUpdateDestination::LocalSettings);
    assert_eq!(
        update.to_wire().unwrap(),
        json!({
            "type": "addRules",
            "destination": "localSettings",
            "behavior": "allow",
            "rules": [{"toolName": "WebSearch"}],
        })
    );
}

#[test]
fn empty_rule_content_is_sent_as_is() {
    let update = PermissionUpdate::new(PermissionUpdateKind::AddRules)
        .rules([PermissionRuleValue::new("Bash", Some(String::new()))])
        .behavior(PermissionBehavior::Deny)
        .destination(PermissionUpdateDestination::Session);
    assert_eq!(
        update.to_wire().unwrap()["rules"],
        json!([{"toolName": "Bash", "ruleContent": ""}])
    );
}

fn read_rule() -> PermissionRuleValue {
    PermissionRuleValue::new("Read", None)
}

#[test]
fn missing_destination_is_sent_as_session() {
    let updates = [
        PermissionUpdate::new(PermissionUpdateKind::AddRules)
            .rules([read_rule()])
            .behavior(PermissionBehavior::Allow),
        PermissionUpdate::new(PermissionUpdateKind::ReplaceRules)
            .rules([read_rule()])
            .behavior(PermissionBehavior::Deny),
        PermissionUpdate::new(PermissionUpdateKind::RemoveRules)
            .rules([read_rule()])
            .behavior(PermissionBehavior::Allow),
        PermissionUpdate::new(PermissionUpdateKind::SetMode).mode(PermissionMode::AcceptEdits),
        PermissionUpdate::new(PermissionUpdateKind::AddDirectories).directories(["/tmp/a"]),
        PermissionUpdate::new(PermissionUpdateKind::RemoveDirectories).directories(["/tmp/a"]),
    ];
    for update in updates {
        assert_eq!(
            update.to_wire().unwrap()["destination"],
            "session",
            "{:?}",
            update.kind
        );
    }
}

#[test]
fn missing_required_field_fails() {
    let rules = |kind| PermissionUpdate::new(kind).rules([read_rule()]);
    let cases = [
        (
            PermissionUpdate::new(PermissionUpdateKind::AddRules).behavior("allow"),
            "rules",
        ),
        (
            PermissionUpdate::new(PermissionUpdateKind::ReplaceRules).behavior("deny"),
            "rules",
        ),
        (
            PermissionUpdate::new(PermissionUpdateKind::RemoveRules).behavior("ask"),
            "rules",
        ),
        (rules(PermissionUpdateKind::AddRules), "behavior"),
        (rules(PermissionUpdateKind::ReplaceRules), "behavior"),
        (rules(PermissionUpdateKind::RemoveRules), "behavior"),
        (PermissionUpdate::new(PermissionUpdateKind::SetMode), "mode"),
        (
            PermissionUpdate::new(PermissionUpdateKind::AddDirectories),
            "directories",
        ),
        (
            PermissionUpdate::new(PermissionUpdateKind::RemoveDirectories),
            "directories",
        ),
    ];
    for (update, missing) in cases {
        let error = update.to_wire().unwrap_err();
        assert_eq!(error.kind(), ErrorKind::Config, "{error:?}");
        assert!(
            error.to_string().contains(&format!("has no {missing}")),
            "{error}"
        );
        assert!(serde_json::to_value(&update).is_err());
    }
}

#[test]
fn unknown_kind_fails() {
    let update = PermissionUpdate::new("addSomething").destination("session");
    let error = update.to_wire().unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Config, "{error:?}");
    assert!(
        error
            .to_string()
            .contains("type 'addSomething', which this version"),
        "{error}"
    );
}

#[test]
fn cli_suggestion_round_trips_unchanged() {
    let wires = [
        json!({
            "type": "addRules",
            "destination": "localSettings",
            "behavior": "allow",
            "rules": [{"toolName": "WebSearch"}],
        }),
        json!({
            "type": "replaceRules",
            "destination": "projectSettings",
            "behavior": "deny",
            "rules": [{"toolName": "Bash", "ruleContent": "rm -rf *"}],
        }),
        json!({
            "type": "removeRules",
            "destination": "userSettings",
            "behavior": "ask",
            "rules": [{"toolName": "mcp__server__tool"}],
        }),
        json!({"type": "setMode", "mode": "plan", "destination": "session"}),
        json!({
            "type": "removeDirectories",
            "directories": ["/tmp/a"],
            "destination": "localSettings",
        }),
    ];
    for wire in wires {
        assert_eq!(
            PermissionUpdate::from_wire(&wire)
                .unwrap()
                .to_wire()
                .unwrap(),
            wire
        );
    }
}

#[test]
fn permission_update_set_mode() {
    let wire = json!({"type": "setMode", "mode": "acceptEdits", "destination": "session"});
    let update = PermissionUpdate::from_wire(&wire).unwrap();
    assert_eq!(update.mode, Some(PermissionMode::AcceptEdits));
    assert_eq!(update.rules, None);
    assert_eq!(update.to_wire().unwrap(), wire);
}

#[test]
fn permission_update_directories() {
    let wire = json!({
        "type": "addDirectories",
        "directories": ["/tmp/a", "/tmp/b"],
        "destination": "userSettings",
    });
    let update = PermissionUpdate::from_wire(&wire).unwrap();
    assert_eq!(
        update.directories,
        Some(vec!["/tmp/a".to_owned(), "/tmp/b".to_owned()])
    );
    assert_eq!(update.to_wire().unwrap(), wire);
}

#[test]
fn permission_update_sends_only_the_fields_of_its_kind() {
    let update = PermissionUpdate::new(PermissionUpdateKind::SetMode)
        .mode(PermissionMode::Plan)
        .directories(["/ignored"])
        .rules([PermissionRuleValue::new("Read", None)]);
    assert_eq!(
        update.to_wire().unwrap(),
        json!({"type": "setMode", "destination": "session", "mode": "plan"})
    );
}

#[test]
fn permission_update_from_wire_rejects_malformed_values() {
    assert!(PermissionUpdate::from_wire(&json!("addRules")).is_err());
    assert!(PermissionUpdate::from_wire(&json!({"rules": []})).is_err());
    assert!(
        PermissionUpdate::from_wire(&json!({"type": "addRules", "rules": [{"ruleContent": "x"}]}))
            .is_err()
    );
}

#[test]
fn permission_result_is_allow_or_deny() {
    assert!(matches!(
        PermissionResult::allow(),
        PermissionResult::Allow {
            updated_input: None,
            updated_permissions: None,
            ..
        }
    ));
    let PermissionResult::Allow {
        updated_input,
        updated_permissions,
        ..
    } = PermissionResult::allow_with_input(json!({"safe": true}))
        .updated_permissions([PermissionUpdate::new(PermissionUpdateKind::SetMode).mode("plan")])
    else {
        panic!("expected an allow");
    };
    assert_eq!(updated_input, Some(json!({"safe": true})));
    assert_eq!(updated_permissions.map(|updates| updates.len()), Some(1));
    assert!(matches!(
        PermissionResult::deny("not allowed"),
        PermissionResult::Deny { message, interrupt: false, .. } if message == "not allowed"
    ));
    assert!(matches!(
        PermissionResult::deny_and_interrupt("stop").updated_permissions([]),
        PermissionResult::Deny {
            interrupt: true,
            ..
        }
    ));
}

#[tokio::test]
async fn can_use_tool_callback_receives_name_input_and_context() {
    let callback = CanUseTool::new(
        async |tool_name: String, input: Value, context: ToolPermissionContext| {
            if tool_name == "Bash" {
                Ok(PermissionResult::deny_and_interrupt(format!(
                    "{} blocked {}",
                    context.tool_use_id.unwrap_or_default(),
                    input["command"]
                )))
            } else {
                Ok(PermissionResult::allow())
            }
        },
    );
    let mut context = ToolPermissionContext::default();
    context.tool_use_id = Some("toolu_1".to_owned());
    context.suggestions = vec![PermissionUpdate::new(PermissionUpdateKind::SetMode)];
    let denied = callback
        .call("Bash".to_owned(), json!({"command": "rm"}), context)
        .await
        .unwrap();
    assert_eq!(
        denied,
        PermissionResult::deny_and_interrupt("toolu_1 blocked \"rm\"")
    );
    let allowed = callback
        .clone()
        .call(
            "Read".to_owned(),
            json!({}),
            ToolPermissionContext::default(),
        )
        .await
        .unwrap();
    assert_eq!(allowed, PermissionResult::allow());

    let failing = CanUseTool::new(async |_, _, _| Err("policy backend down".into()));
    let error = failing
        .call(
            "Read".to_owned(),
            json!({}),
            ToolPermissionContext::default(),
        )
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "policy backend down");
}

#[test]
fn message_types_are_constructible() {
    let message = UserMessage::new("Hello, Claude!");
    assert_eq!(message.content.as_text(), Some("Hello, Claude!"));

    let message = AssistantMessage::new(
        vec![ContentBlock::Text(TextBlock::new("Hello, human!"))],
        "claude-opus-4-1-20250805",
    );
    assert_eq!(message.content.len(), 1);
    assert_eq!(message.content[0].as_text(), Some("Hello, human!"));

    let thinking = ThinkingBlock::new("I'm thinking...", "sig-123");
    assert_eq!(thinking.thinking, "I'm thinking...");
    assert_eq!(thinking.signature, "sig-123");

    let tool_use = ToolUseBlock::new("tool-123", "Read", json!({"file_path": "/test.txt"}));
    assert_eq!(tool_use.id, "tool-123");
    assert_eq!(tool_use.name, "Read");
    assert_eq!(tool_use.input["file_path"], "/test.txt");

    let tool_result = ToolResultBlock::new(
        "tool-123",
        Some(ToolResultContent::from("File contents here")),
        Some(false),
    );
    assert_eq!(tool_result.tool_use_id, "tool-123");
    assert_eq!(
        tool_result.content.unwrap().as_text(),
        Some("File contents here")
    );
    assert_eq!(tool_result.is_error, Some(false));

    let mut result = ResultMessage::new("success", 1500, 1200, false, 1, "session-123");
    result.total_cost_usd = Some(0.01);
    assert_eq!(result.subtype, "success");
    assert_eq!(result.total_cost_usd, Some(0.01));
    assert_eq!(result.session_id, "session-123");
}

#[test]
fn notification_hook_input() {
    let wire = base_hook_input(
        "Notification",
        json!({"message": "Task completed", "notification_type": "info"}),
    );
    let HookInput::Notification(input) = HookInput::from_wire(wire.clone()) else {
        panic!("expected a Notification input");
    };
    assert_eq!(input.base.session_id, "sess-1");
    assert_eq!(input.base.transcript_path, "/tmp/transcript");
    assert_eq!(input.base.cwd, "/home/user");
    assert_eq!(input.message, "Task completed");
    assert_eq!(input.notification_type, "info");
    assert_eq!(input.title, None);

    let with_title = base_hook_input(
        "Notification",
        json!({"message": "Task completed", "notification_type": "info", "title": "Success"}),
    );
    let HookInput::Notification(input) = HookInput::from_wire(with_title) else {
        panic!("expected a Notification input");
    };
    assert_eq!(input.title.as_deref(), Some("Success"));
}

#[test]
fn subagent_start_hook_input() {
    let wire = base_hook_input(
        "SubagentStart",
        json!({"agent_id": "agent-42", "agent_type": "researcher"}),
    );
    let input = HookInput::from_wire(wire);
    assert_eq!(input.hook_event_name(), "SubagentStart");
    assert_eq!(input.agent_id(), Some("agent-42"));
    let HookInput::SubagentStart(input) = input else {
        panic!("expected a SubagentStart input");
    };
    assert_eq!(input.base.agent_id.as_deref(), Some("agent-42"));
    assert_eq!(input.base.agent_type.as_deref(), Some("researcher"));
}

#[test]
fn every_hook_input_variant_carries_the_subagent_context() {
    let cases = [
        (
            "PreToolUse",
            json!({"tool_name": "Bash", "tool_input": {}, "tool_use_id": "t"}),
        ),
        (
            "PostToolUse",
            json!({"tool_name": "Bash", "tool_input": {}, "tool_response": {}, "tool_use_id": "t"}),
        ),
        (
            "PostToolUseFailure",
            json!({"tool_name": "Bash", "tool_input": {}, "tool_use_id": "t", "error": "e"}),
        ),
        ("UserPromptSubmit", json!({"prompt": "hi"})),
        ("Stop", json!({"stop_hook_active": false})),
        (
            "SubagentStop",
            json!({"stop_hook_active": false, "agent_transcript_path": "/tmp/agent"}),
        ),
        (
            "PreCompact",
            json!({"trigger": "manual", "custom_instructions": null}),
        ),
        (
            "Notification",
            json!({"message": "m", "notification_type": "info"}),
        ),
        ("SubagentStart", json!({})),
        (
            "PermissionRequest",
            json!({"tool_name": "Bash", "tool_input": {}}),
        ),
    ];
    assert_eq!(cases.len(), HookEvent::KNOWN.len());
    for (event, extra) in cases {
        let mut wire = base_hook_input(event, extra);
        wire["agent_id"] = json!("agent-42");
        wire["agent_type"] = json!("researcher");
        wire["permission_mode"] = json!("default");
        let input = HookInput::from_wire(wire.clone());
        assert!(!matches!(input, HookInput::Unknown(_)), "{event}");
        assert_eq!(input.hook_event_name(), event);
        let base = input.base().expect("typed inputs have a base");
        assert_eq!(base.agent_id.as_deref(), Some("agent-42"), "{event}");
        assert_eq!(base.agent_type.as_deref(), Some("researcher"), "{event}");
        assert_eq!(base.permission_mode.as_deref(), Some("default"), "{event}");
        assert!(base.extra.is_empty(), "{event}: {:?}", base.extra);
        assert_eq!(input.agent_id(), Some("agent-42"));
        assert_eq!(input.to_wire(), wire, "{event}");
        assert_eq!(serde_json::to_value(&input).unwrap(), wire, "{event}");
        assert_eq!(
            serde_json::from_value::<HookInput>(wire).unwrap(),
            input,
            "{event}"
        );
    }
}

#[test]
fn pre_tool_use_hook_input_with_and_without_agent_id() {
    let wire = base_hook_input(
        "PreToolUse",
        json!({
            "tool_name": "Bash",
            "tool_input": {"command": "echo hello"},
            "tool_use_id": "toolu_abc123",
            "agent_id": "agent-42",
            "agent_type": "researcher",
        }),
    );
    let HookInput::PreToolUse(input) = HookInput::from_wire(wire) else {
        panic!("expected a PreToolUse input");
    };
    assert_eq!(input.tool_name, "Bash");
    assert_eq!(input.tool_input, json!({"command": "echo hello"}));
    assert_eq!(input.tool_use_id, "toolu_abc123");
    assert_eq!(input.base.agent_id.as_deref(), Some("agent-42"));
    assert_eq!(input.base.agent_type.as_deref(), Some("researcher"));

    let main_thread = base_hook_input(
        "PreToolUse",
        json!({
            "tool_name": "Bash",
            "tool_input": {"command": "echo hello"},
            "tool_use_id": "toolu_def456",
        }),
    );
    let input = HookInput::from_wire(main_thread);
    assert_eq!(input.agent_id(), None);
    assert!(matches!(input, HookInput::PreToolUse(_)));
}

#[test]
fn post_tool_use_hook_input_with_agent_id_and_duration() {
    let wire = base_hook_input(
        "PostToolUse",
        json!({
            "tool_name": "Bash",
            "tool_input": {"command": "echo hello"},
            "tool_response": {"content": [{"type": "text", "text": "hello"}]},
            "tool_use_id": "toolu_abc123",
            "agent_id": "agent-42",
            "duration_ms": 12,
        }),
    );
    let HookInput::PostToolUse(input) = HookInput::from_wire(wire) else {
        panic!("expected a PostToolUse input");
    };
    assert_eq!(input.base.agent_id.as_deref(), Some("agent-42"));
    assert_eq!(input.duration_ms, Some(12));
    assert_eq!(input.tool_response["content"][0]["text"], "hello");
}

#[test]
fn a_hook_input_the_cli_schema_allows_stays_typed() {
    let wire = base_hook_input(
        "PostToolUse",
        json!({
            "tool_name": "Bash",
            "tool_input": {"command": "true"},
            "tool_use_id": "toolu_1",
            "duration_ms": 12.5,
            "effort": "high",
        }),
    );
    let HookInput::PostToolUse(input) = HookInput::from_wire(wire) else {
        panic!("a missing tool_response and a fractional duration are allowed by the CLI schema");
    };
    assert_eq!(input.duration_ms, Some(12));
    assert_eq!(input.tool_response, Value::Null);
    assert_eq!(
        input.base.effort.map(|effort| effort.level).as_deref(),
        Some("high")
    );
}

#[test]
fn a_hook_input_that_does_not_match_its_event_is_unknown_and_logged() {
    let (warnings, _guard) = common::capture::Warnings::install_at(tracing::Level::DEBUG);
    let wire = base_hook_input("PostToolUse", json!({"tool_name": 7}));
    assert_eq!(HookInput::from_wire(wire.clone()), HookInput::Unknown(wire));
    assert!(
        warnings
            .all()
            .iter()
            .any(|message| message.contains("does not match the type of its event")),
        "{:?}",
        warnings.all()
    );
}

#[test]
fn post_tool_use_failure_hook_input_optional_fields() {
    let wire = base_hook_input(
        "PostToolUseFailure",
        json!({
            "tool_name": "Bash",
            "tool_input": {"command": "false"},
            "tool_use_id": "toolu_1",
            "error": "exit 1",
        }),
    );
    let HookInput::PostToolUseFailure(input) = HookInput::from_wire(wire) else {
        panic!("expected a PostToolUseFailure input");
    };
    assert_eq!(input.error, "exit 1");
    assert_eq!(input.is_interrupt, None);
    assert_eq!(input.duration_ms, None);
}

#[test]
fn permission_request_hook_input() {
    let wire = base_hook_input(
        "PermissionRequest",
        json!({"tool_name": "Bash", "tool_input": {"command": "ls"}}),
    );
    let HookInput::PermissionRequest(input) = HookInput::from_wire(wire) else {
        panic!("expected a PermissionRequest input");
    };
    assert_eq!(input.tool_name, "Bash");
    assert_eq!(input.tool_input, json!({"command": "ls"}));
    assert_eq!(input.permission_suggestions, None);

    let with_suggestions = base_hook_input(
        "PermissionRequest",
        json!({
            "tool_name": "Bash",
            "tool_input": {"command": "ls"},
            "permission_suggestions": [{"type": "allow", "rule": "Bash(*)"}],
        }),
    );
    let HookInput::PermissionRequest(input) = HookInput::from_wire(with_suggestions) else {
        panic!("expected a PermissionRequest input");
    };
    assert_eq!(input.permission_suggestions.unwrap().len(), 1);
}

#[test]
fn base_hook_input_prompt_id_and_effort() {
    let wire = base_hook_input(
        "PreToolUse",
        json!({
            "prompt_id": "550e8400-e29b-41d4-a716-446655440000",
            "effort": {"level": "high"},
            "tool_name": "Bash",
            "tool_input": {"command": "ls"},
            "tool_use_id": "toolu_1",
        }),
    );
    let input = HookInput::from_wire(wire);
    let base = input.base().unwrap();
    assert_eq!(
        base.prompt_id.as_deref(),
        Some("550e8400-e29b-41d4-a716-446655440000")
    );
    assert_eq!(base.effort.as_ref().unwrap().level, "high");

    let without =
        HookInput::from_wire(base_hook_input("UserPromptSubmit", json!({"prompt": "hi"})));
    let base = without.base().unwrap();
    assert_eq!(base.prompt_id, None);
    assert_eq!(base.effort, None);
}

#[test]
fn stop_hook_input_background_work() {
    let wire = base_hook_input(
        "Stop",
        json!({
            "stop_hook_active": false,
            "last_assistant_message": "Done.",
            "background_tasks": [
                {
                    "id": "task-1",
                    "type": "shell",
                    "status": "running",
                    "description": "npm test",
                    "command": "npm test",
                }
            ],
            "session_crons": [
                {"id": "cron-1", "schedule": "0 9 * * 1-5", "recurring": true, "prompt": "check CI"}
            ],
        }),
    );
    let HookInput::Stop(input) = HookInput::from_wire(wire.clone()) else {
        panic!("expected a Stop input");
    };
    assert!(!input.stop_hook_active);
    assert_eq!(input.last_assistant_message.as_deref(), Some("Done."));
    let task = &input.background_tasks.as_ref().unwrap()[0];
    assert_eq!(task.kind, "shell");
    assert_eq!(task.command.as_deref(), Some("npm test"));
    assert_eq!(task.agent_type, None);
    let cron = &input.session_crons.as_ref().unwrap()[0];
    assert!(cron.recurring);
    assert_eq!(cron.schedule, "0 9 * * 1-5");
    assert_eq!(HookInput::Stop(input).to_wire(), wire);
}

#[test]
fn stop_and_subagent_stop_optional_fields_default_to_none() {
    let HookInput::Stop(stop) =
        HookInput::from_wire(base_hook_input("Stop", json!({"stop_hook_active": true})))
    else {
        panic!("expected a Stop input");
    };
    assert_eq!(stop.last_assistant_message, None);
    assert_eq!(stop.background_tasks, None);
    assert_eq!(stop.session_crons, None);

    let HookInput::SubagentStop(subagent_stop) = HookInput::from_wire(base_hook_input(
        "SubagentStop",
        json!({
            "stop_hook_active": true,
            "agent_id": "agent-1",
            "agent_type": "researcher",
            "agent_transcript_path": "/tmp/agent.jsonl",
        }),
    )) else {
        panic!("expected a SubagentStop input");
    };
    assert_eq!(subagent_stop.agent_transcript_path, "/tmp/agent.jsonl");
    assert_eq!(subagent_stop.base.agent_id.as_deref(), Some("agent-1"));
    assert_eq!(subagent_stop.last_assistant_message, None);
    assert_eq!(subagent_stop.background_tasks, None);
    assert_eq!(subagent_stop.session_crons, None);
}

fn without(value: &Value, key: &str) -> Value {
    let mut value = value.clone();
    value.as_object_mut().unwrap().shift_remove(key);
    value
}

#[test]
fn background_work_summaries_and_hook_effort_have_the_upstream_required_and_optional_keys() {
    let required_task = json!({
        "id": "task-1",
        "type": "shell",
        "status": "running",
        "description": "npm test",
    });
    let task: BackgroundTaskSummary = serde_json::from_value(required_task.clone()).unwrap();
    assert_eq!(serde_json::to_value(&task).unwrap(), required_task);
    for key in ["id", "type", "status", "description"] {
        let outcome = serde_json::from_value::<BackgroundTaskSummary>(without(&required_task, key));
        assert!(
            outcome.is_err(),
            "a task summary without {key} was accepted"
        );
    }

    let mut full_task = required_task.clone();
    for key in ["command", "agent_type", "server", "tool", "name"] {
        let mut with_one = required_task.clone();
        with_one[key] = json!(format!("{key} value"));
        let task: BackgroundTaskSummary = serde_json::from_value(with_one.clone()).unwrap();
        assert_eq!(serde_json::to_value(&task).unwrap(), with_one);
        full_task[key] = json!(format!("{key} value"));
    }
    let task: BackgroundTaskSummary = serde_json::from_value(full_task.clone()).unwrap();
    assert_eq!(task.kind, "shell");
    assert_eq!(task.command.as_deref(), Some("command value"));
    assert_eq!(task.agent_type.as_deref(), Some("agent_type value"));
    assert_eq!(task.server.as_deref(), Some("server value"));
    assert_eq!(task.tool.as_deref(), Some("tool value"));
    assert_eq!(task.name.as_deref(), Some("name value"));
    let wire = serde_json::to_value(&task).unwrap();
    assert_eq!(wire, full_task);
    assert_eq!(
        keys(&wire),
        [
            "agent_type",
            "command",
            "description",
            "id",
            "name",
            "server",
            "status",
            "tool",
            "type"
        ]
    );

    let required_cron = json!({
        "id": "cron-1",
        "schedule": "0 9 * * 1-5",
        "recurring": true,
        "prompt": "check CI",
    });
    let cron: SessionCronSummary = serde_json::from_value(required_cron.clone()).unwrap();
    let wire = serde_json::to_value(&cron).unwrap();
    assert_eq!(wire, required_cron);
    assert_eq!(keys(&wire), ["id", "prompt", "recurring", "schedule"]);
    for key in ["id", "schedule", "recurring", "prompt"] {
        let outcome = serde_json::from_value::<SessionCronSummary>(without(&required_cron, key));
        assert!(
            outcome.is_err(),
            "a cron summary without {key} was accepted"
        );
    }

    let effort: HookEffort = serde_json::from_value(json!({"level": "high"})).unwrap();
    assert_eq!(effort.level, "high");
    assert_eq!(
        serde_json::to_value(&effort).unwrap(),
        json!({"level": "high"})
    );
    assert!(serde_json::from_value::<HookEffort>(json!({})).is_err());
}

#[test]
fn unrecognized_hook_event_name_is_unknown_without_error() {
    let wire = base_hook_input("FromTheFuture", json!({"payload": 1}));
    let input: HookInput = serde_json::from_value(wire.clone()).unwrap();
    assert_eq!(input, HookInput::Unknown(wire.clone()));
    assert_eq!(input.hook_event_name(), "FromTheFuture");
    assert!(input.base().is_none());
    assert_eq!(input.to_wire(), wire);

    let nameless = json!({"test": "data"});
    let input = HookInput::from_wire(nameless.clone());
    assert_eq!(input, HookInput::Unknown(nameless));
    assert_eq!(input.hook_event_name(), "");
}

#[test]
fn known_hook_event_with_an_unexpected_shape_is_unknown_without_error() {
    let wire = base_hook_input("PreToolUse", json!({"tool_name": "Bash"}));
    assert_eq!(HookInput::from_wire(wire.clone()), HookInput::Unknown(wire));
}

#[test]
fn hook_input_keeps_fields_this_crate_does_not_model() {
    let wire = base_hook_input(
        "UserPromptSubmit",
        json!({"prompt": "hi", "future_field": {"nested": true}}),
    );
    let input = HookInput::from_wire(wire.clone());
    assert_eq!(
        input.base().unwrap().extra.get("future_field"),
        Some(&json!({"nested": true}))
    );
    assert_eq!(input.to_wire(), wire);
}

#[test]
fn sync_hook_output_uses_the_continue_wire_name() {
    let output = HookOutput::Sync(
        SyncHookOutput::default()
            .continue_(true)
            .suppress_output(false)
            .stop_reason("Test stop reason")
            .decision(HookDecision::Block)
            .system_message("Test system message")
            .reason("Test reason for blocking")
            .hook_specific_output(
                PreToolUseHookSpecificOutput::default()
                    .permission_decision(HookPermissionDecision::Deny)
                    .permission_decision_reason("Security policy violation")
                    .updated_input(json!({"modified": "input"})),
            ),
    );
    let expected = json!({
        "continue": true,
        "suppressOutput": false,
        "stopReason": "Test stop reason",
        "decision": "block",
        "systemMessage": "Test system message",
        "reason": "Test reason for blocking",
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": "deny",
            "permissionDecisionReason": "Security policy violation",
            "updatedInput": {"modified": "input"},
        },
    });
    assert_eq!(output.to_wire(), expected);
    assert_eq!(serde_json::to_value(&output).unwrap(), expected);
    assert!(
        !output
            .to_wire()
            .as_object()
            .unwrap()
            .contains_key("continue_")
    );
    assert_eq!(
        serde_json::from_value::<HookOutput>(expected).unwrap(),
        output
    );
}

#[test]
fn async_hook_output_uses_the_async_wire_name() {
    let output = HookOutput::Async(AsyncHookOutput::new().async_timeout(5000));
    let expected = json!({"async": true, "asyncTimeout": 5000});
    assert_eq!(output.to_wire(), expected);
    assert!(!output.to_wire().as_object().unwrap().contains_key("async_"));
    assert_eq!(
        serde_json::from_value::<HookOutput>(expected).unwrap(),
        output
    );

    assert_eq!(
        HookOutput::from(AsyncHookOutput::new()).to_wire(),
        json!({"async": true})
    );
}

#[test]
fn default_hook_output_is_an_empty_object() {
    assert_eq!(HookOutput::default().to_wire(), json!({}));
    assert_eq!(
        HookOutput::from(SyncHookOutput::block("no")).to_wire(),
        json!({"decision": "block", "reason": "no"})
    );
}

#[test]
fn sync_hook_output_passes_extra_keys_through() {
    let mut output = SyncHookOutput::default();
    output.extra.insert("processed".to_owned(), json!(true));
    let wire = HookOutput::Sync(output.clone()).to_wire();
    assert_eq!(wire, json!({"processed": true}));
    assert_eq!(
        serde_json::from_value::<HookOutput>(wire).unwrap(),
        HookOutput::Sync(output)
    );
}

#[test]
fn hook_specific_output_has_the_21_typed_variants() {
    assert_eq!(
        HookSpecificOutput::EVENT_NAMES,
        [
            "PreToolUse",
            "PostToolUse",
            "PostToolUseFailure",
            "UserPromptSubmit",
            "SessionStart",
            "Notification",
            "SubagentStart",
            "PermissionRequest",
            "UserPromptExpansion",
            "PostToolBatch",
            "Stop",
            "SubagentStop",
            "PermissionDenied",
            "PreModelSwitch",
            "PostModelSwitch",
            "Elicitation",
            "ElicitationResult",
            "CwdChanged",
            "FileChanged",
            "WorktreeCreate",
            "MessageDisplay",
        ]
    );
    let mut distinct = HookSpecificOutput::EVENT_NAMES.to_vec();
    distinct.sort_unstable();
    distinct.dedup();
    assert_eq!(distinct.len(), 21);
}

#[test]
fn hook_specific_output_key_sets_match_upstream() {
    let cases: [(&str, &[&str], &[&str]); 21] = [
        (
            "PreToolUse",
            &[],
            &[
                "permissionDecision",
                "permissionDecisionReason",
                "updatedInput",
                "additionalContext",
            ],
        ),
        (
            "PostToolUse",
            &[],
            &[
                "additionalContext",
                "updatedToolOutput",
                "updatedMCPToolOutput",
            ],
        ),
        ("PostToolUseFailure", &[], &["additionalContext"]),
        (
            "UserPromptSubmit",
            &[],
            &[
                "additionalContext",
                "sessionTitle",
                "suppressOriginalPrompt",
            ],
        ),
        (
            "SessionStart",
            &[],
            &[
                "additionalContext",
                "initialUserMessage",
                "sessionTitle",
                "watchPaths",
                "reloadSkills",
            ],
        ),
        ("Notification", &[], &["additionalContext"]),
        ("SubagentStart", &[], &["additionalContext"]),
        ("PermissionRequest", &["decision"], &[]),
        (
            "UserPromptExpansion",
            &[],
            &["additionalContext", "suppressOriginalPrompt"],
        ),
        ("PostToolBatch", &[], &["additionalContext"]),
        ("Stop", &[], &["additionalContext"]),
        ("SubagentStop", &[], &["additionalContext"]),
        ("PermissionDenied", &[], &["retry"]),
        (
            "PreModelSwitch",
            &[],
            &["permissionDecision", "permissionDecisionReason"],
        ),
        ("PostModelSwitch", &[], &["additionalContext"]),
        ("Elicitation", &[], &["action", "content"]),
        ("ElicitationResult", &[], &["action", "content"]),
        ("CwdChanged", &[], &["watchPaths"]),
        ("FileChanged", &[], &["watchPaths"]),
        ("WorktreeCreate", &["worktreePath"], &[]),
        ("MessageDisplay", &[], &["displayContent"]),
    ];
    let sample = |key: &str| match key {
        "permissionDecision" => json!("allow"),
        "action" => json!("accept"),
        "watchPaths" => json!(["/repo/.envrc"]),
        "suppressOriginalPrompt" | "reloadSkills" | "retry" => json!(true),
        "decision" | "content" | "updatedInput" | "updatedToolOutput" | "updatedMCPToolOutput" => {
            json!({"k": "v"})
        }
        _ => json!("text"),
    };
    for (event, required, optional) in cases {
        let mut minimal = Map::new();
        minimal.insert("hookEventName".to_owned(), json!(event));
        for key in required {
            minimal.insert((*key).to_owned(), sample(key));
        }
        let minimal = Value::Object(minimal);
        let parsed: HookSpecificOutput = serde_json::from_value(minimal.clone())
            .unwrap_or_else(|error| panic!("{event} minimal: {error}"));
        assert_eq!(parsed.hook_event_name(), event);
        assert_eq!(serde_json::to_value(&parsed).unwrap(), minimal, "{event}");

        let mut full = minimal.clone();
        for key in optional {
            full[*key] = sample(key);
        }
        let parsed: HookSpecificOutput = serde_json::from_value(full.clone())
            .unwrap_or_else(|error| panic!("{event} full: {error}"));
        assert_eq!(serde_json::to_value(&parsed).unwrap(), full, "{event}");

        for key in required {
            let mut missing = minimal.clone();
            missing.as_object_mut().unwrap().shift_remove(*key);
            assert!(
                serde_json::from_value::<HookSpecificOutput>(missing).is_err(),
                "{event} should require {key}"
            );
        }
    }
}

#[test]
fn hook_specific_output_builders_produce_the_wire_names() {
    let output: HookSpecificOutput = PostToolUseHookSpecificOutput::default()
        .updated_mcp_tool_output(json!({"result": "modified"}))
        .updated_tool_output(json!({"stdout": "replaced", "stderr": "", "interrupted": false}))
        .into();
    assert_eq!(
        serde_json::to_value(&output).unwrap(),
        json!({
            "hookEventName": "PostToolUse",
            "updatedMCPToolOutput": {"result": "modified"},
            "updatedToolOutput": {"stdout": "replaced", "stderr": "", "interrupted": false},
        })
    );

    let output: HookSpecificOutput = UserPromptSubmitHookSpecificOutput::default()
        .session_title("Refactor auth")
        .suppress_original_prompt(true)
        .into();
    assert_eq!(
        serde_json::to_value(&output).unwrap(),
        json!({
            "hookEventName": "UserPromptSubmit",
            "sessionTitle": "Refactor auth",
            "suppressOriginalPrompt": true,
        })
    );

    let output: HookSpecificOutput = SessionStartHookSpecificOutput::default()
        .additional_context("ctx")
        .initial_user_message("Run the test suite")
        .session_title("auth-refactor")
        .watch_paths(["/repo/.envrc"])
        .reload_skills(true)
        .into();
    assert_eq!(
        serde_json::to_value(&output).unwrap(),
        json!({
            "hookEventName": "SessionStart",
            "additionalContext": "ctx",
            "initialUserMessage": "Run the test suite",
            "sessionTitle": "auth-refactor",
            "watchPaths": ["/repo/.envrc"],
            "reloadSkills": true,
        })
    );

    let output: HookSpecificOutput =
        PermissionRequestHookSpecificOutput::new(json!({"type": "allow"})).into();
    assert_eq!(
        serde_json::to_value(&output).unwrap(),
        json!({"hookEventName": "PermissionRequest", "decision": {"type": "allow"}})
    );

    let output: HookSpecificOutput = WorktreeCreateHookSpecificOutput::new("/repo/wt").into();
    assert_eq!(
        serde_json::to_value(&output).unwrap(),
        json!({"hookEventName": "WorktreeCreate", "worktreePath": "/repo/wt"})
    );

    let output: HookSpecificOutput = PreModelSwitchHookSpecificOutput::default()
        .permission_decision(PermissionBehavior::Ask)
        .into();
    assert_eq!(
        serde_json::to_value(&output).unwrap(),
        json!({"hookEventName": "PreModelSwitch", "permissionDecision": "ask"})
    );

    let output: HookSpecificOutput = ElicitationHookSpecificOutput::default()
        .action(ElicitationAction::Decline)
        .into();
    assert_eq!(
        serde_json::to_value(&output).unwrap(),
        json!({"hookEventName": "Elicitation", "action": "decline"})
    );
}

#[test]
fn output_literals_match_upstream() {
    assert_eq!(PermissionBehavior::KNOWN, ["allow", "deny", "ask"]);
    assert_eq!(
        HookPermissionDecision::KNOWN,
        ["allow", "deny", "ask", "defer"]
    );
    assert_eq!(ElicitationAction::KNOWN, ["accept", "decline", "cancel"]);
}

#[tokio::test]
async fn hook_callback_receives_typed_input_tool_use_id_and_context() {
    let callback = HookCallback::new(
        async |input: HookInput, tool_use_id: Option<String>, _context: HookContext| {
            let HookInput::PreToolUse(input) = input else {
                return Err("unexpected event".into());
            };
            Ok(HookOutput::Sync(SyncHookOutput::block(format!(
                "{} {}",
                input.tool_name,
                tool_use_id.unwrap_or_default()
            ))))
        },
    );
    let input = HookInput::from_wire(base_hook_input(
        "PreToolUse",
        json!({"tool_name": "Bash", "tool_input": {}, "tool_use_id": "toolu_1"}),
    ));
    let output = callback
        .call(input, Some("toolu_1".to_owned()), HookContext::default())
        .await
        .unwrap();
    assert_eq!(
        output.to_wire(),
        json!({"decision": "block", "reason": "Bash toolu_1"})
    );

    let error = callback
        .call(
            HookInput::from_wire(json!({"hook_event_name": "Other"})),
            None,
            HookContext::default(),
        )
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "unexpected event");
}

#[test]
fn hook_matcher_holds_matcher_hooks_timeout_and_on_failure() {
    let empty = HookMatcher::new();
    assert_eq!(empty.matcher, None);
    assert!(empty.hooks.is_empty());
    assert_eq!(empty.timeout, None);
    assert_eq!(empty.on_failure, None);

    let matcher = HookMatcher::new()
        .matcher("Write|MultiEdit|Edit")
        .hook(
            async |_input: HookInput, _tool_use_id: Option<String>, _context: HookContext| {
                Ok(HookOutput::default())
            },
        )
        .hook(HookCallback::new(async |_, _, _| Ok(HookOutput::default())))
        .timeout(Duration::from_secs(30))
        .on_failure(HookFailureMode::Block);
    assert_eq!(matcher.matcher.as_deref(), Some("Write|MultiEdit|Edit"));
    assert_eq!(matcher.hooks.len(), 2);
    assert_eq!(matcher.timeout, Some(Duration::from_secs(30)));
    assert_eq!(matcher.on_failure, Some(HookFailureMode::Block));
    assert_eq!(HookFailureMode::KNOWN, ["continue", "block"]);
}

#[test]
fn fast_mode_literals() {
    assert_eq!(FastModeState::KNOWN, ["off", "cooldown", "on"]);
    assert!(FastModeDisabledReason::KNOWN.contains(&"sdk_opt_in_required"));
}

#[test]
fn mcp_server_status_connected() {
    let status: McpServerStatus = serde_json::from_value(json!({
        "name": "my-server",
        "status": "connected",
        "serverInfo": {"name": "my-server", "version": "1.2.3"},
        "config": {"type": "http", "url": "https://example.com"},
        "scope": "project",
        "tools": [
            {
                "name": "greet",
                "description": "Greet a user",
                "annotations": {"readOnly": true, "destructive": false, "openWorld": false},
            }
        ],
    }))
    .unwrap();
    assert_eq!(status.name, "my-server");
    assert_eq!(status.status, McpServerConnectionStatus::Connected);
    assert_eq!(status.server_info.as_ref().unwrap().version, "1.2.3");
    let Some(McpServerStatusConfig::Http(config)) = &status.config else {
        panic!("expected an http config");
    };
    assert_eq!(config.url, "https://example.com");
    assert_eq!(status.scope.as_deref(), Some("project"));
    let tool = &status.tools.as_ref().unwrap()[0];
    assert_eq!(tool.name, "greet");
    assert_eq!(tool.annotations.as_ref().unwrap().read_only, Some(true));
    assert_eq!(tool.annotations.as_ref().unwrap().open_world, Some(false));
}

#[test]
fn mcp_server_status_minimal_and_failed() {
    let status: McpServerStatus =
        serde_json::from_value(json!({"name": "pending-server", "status": "pending"})).unwrap();
    assert_eq!(status.name, "pending-server");
    assert_eq!(status.status, McpServerConnectionStatus::Pending);
    assert_eq!(status.error, None);
    assert_eq!(status.config, None);
    assert_eq!(
        serde_json::to_value(&status).unwrap(),
        json!({"name": "pending-server", "status": "pending"})
    );

    let status: McpServerStatus = serde_json::from_value(
        json!({"name": "broken-server", "status": "failed", "error": "Connection refused"}),
    )
    .unwrap();
    assert_eq!(status.status, McpServerConnectionStatus::Failed);
    assert_eq!(status.error.as_deref(), Some("Connection refused"));
}

#[test]
fn mcp_server_status_config_variants() {
    let parse = |config: Value| {
        serde_json::from_value::<McpServerStatus>(
            json!({"name": "s", "status": "needs-auth", "config": config}),
        )
        .unwrap()
    };

    let status = parse(
        json!({"type": "claudeai-proxy", "url": "https://claude.ai/proxy", "id": "proxy-abc"}),
    );
    assert_eq!(status.status, McpServerConnectionStatus::NeedsAuth);
    let Some(McpServerStatusConfig::ClaudeAiProxy(config)) = &status.config else {
        panic!("expected a claudeai-proxy config");
    };
    assert_eq!(config.id, "proxy-abc");
    assert_eq!(
        status.config.as_ref().unwrap().type_name(),
        Some("claudeai-proxy")
    );

    let status = parse(json!({"command": "node", "args": ["server.js"]}));
    let Some(McpServerStatusConfig::Stdio(config)) = &status.config else {
        panic!("expected a stdio config");
    };
    assert_eq!(config.command, "node");
    assert_eq!(config.args.as_deref(), Some(&["server.js".to_owned()][..]));

    let status = parse(json!({"type": "sdk", "name": "calc"}));
    assert!(matches!(status.config, Some(McpServerStatusConfig::Sdk(_))));

    let future = json!({"type": "quantum", "qubits": 8});
    let status = parse(future.clone());
    assert_eq!(
        status.config,
        Some(McpServerStatusConfig::Unknown(future.clone()))
    );
    assert_eq!(serde_json::to_value(&status).unwrap()["config"], future);

    let sse = json!({"type": "sse", "url": "https://example.com/sse", "headers": {"X-Key": "v"}});
    let status = parse(sse.clone());
    assert!(matches!(status.config, Some(McpServerStatusConfig::Sse(_))));
    assert_eq!(serde_json::to_value(&status).unwrap()["config"], sse);
}

#[test]
fn mcp_status_response_wraps_servers() {
    let response: McpStatusResponse = serde_json::from_value(json!({
        "mcpServers": [
            {"name": "a", "status": "connected"},
            {"name": "b", "status": "disabled"},
        ]
    }))
    .unwrap();
    assert_eq!(response.mcp_servers.len(), 2);
    assert_eq!(
        response.mcp_servers[0].status,
        McpServerConnectionStatus::Connected
    );
    assert_eq!(
        response.mcp_servers[1].status,
        McpServerConnectionStatus::Disabled
    );
    assert_eq!(
        McpServerConnectionStatus::KNOWN,
        ["connected", "failed", "needs-auth", "pending", "disabled"]
    );
}

#[test]
fn context_usage_response_reads_the_wire_names() {
    let response: ContextUsageResponse = serde_json::from_value(json!({
        "categories": [{"name": "Messages", "tokens": 1200, "color": "blue", "isDeferred": false}],
        "totalTokens": 1200,
        "maxTokens": 180000,
        "rawMaxTokens": 200000,
        "percentage": 0.6,
        "model": "claude-sonnet-5",
        "isAutoCompactEnabled": true,
        "memoryFiles": [{"path": "CLAUDE.md"}],
        "mcpTools": [],
        "agents": [],
        "gridRows": [[{"color": "blue"}]],
        "autoCompactThreshold": 160000,
        "apiUsage": null,
    }))
    .unwrap();
    assert_eq!(response.categories[0].name, "Messages");
    assert_eq!(response.categories[0].tokens, 1200);
    assert_eq!(response.categories[0].is_deferred, Some(false));
    assert_eq!(response.total_tokens, 1200);
    assert_eq!(response.max_tokens, 180000);
    assert_eq!(response.raw_max_tokens, 200000);
    assert_eq!(response.percentage, 0.6);
    assert_eq!(response.model, "claude-sonnet-5");
    assert!(response.is_auto_compact_enabled);
    assert_eq!(response.memory_files.len(), 1);
    assert_eq!(response.grid_rows[0][0]["color"], "blue");
    assert_eq!(response.auto_compact_threshold, Some(160000));
    assert_eq!(response.api_usage, None);
    assert_eq!(response.system_tools, None);
}

#[test]
fn agent_definition_minimal_omits_unset_fields() {
    let agent = AgentDefinition::new("test", "You are a test");
    assert_eq!(
        serde_json::to_value(&agent).unwrap(),
        json!({"description": "test", "prompt": "You are a test"})
    );
}

#[test]
fn agent_definition_serializes_with_cli_keys() {
    let mut inline = Map::new();
    inline.insert(
        "local".to_owned(),
        json!({"command": "python", "args": ["server.py"]}),
    );
    let agent = AgentDefinition::new("test", "p")
        .tools(["Read"])
        .disallowed_tools(["Bash", "Write"])
        .model("claude-opus-4-5")
        .skills(["skill-a", "skill-b"])
        .memory(AgentMemory::Project)
        .mcp_servers([AgentMcpServer::from("slack"), AgentMcpServer::from(inline)])
        .initial_prompt("/review-pr 123")
        .max_turns(10)
        .background(true)
        .effort(AgentEffort::from(EffortLevel::High))
        .permission_mode(PermissionMode::BypassPermissions);
    let payload = serde_json::to_value(&agent).unwrap();
    assert_eq!(
        payload,
        json!({
            "description": "test",
            "prompt": "p",
            "tools": ["Read"],
            "disallowedTools": ["Bash", "Write"],
            "model": "claude-opus-4-5",
            "skills": ["skill-a", "skill-b"],
            "memory": "project",
            "mcpServers": ["slack", {"local": {"command": "python", "args": ["server.py"]}}],
            "initialPrompt": "/review-pr 123",
            "maxTurns": 10,
            "background": true,
            "effort": "high",
            "permissionMode": "bypassPermissions",
        })
    );
    for snake_case in [
        "disallowed_tools",
        "mcp_servers",
        "initial_prompt",
        "max_turns",
        "permission_mode",
    ] {
        assert!(payload.get(snake_case).is_none(), "{snake_case}");
    }
    assert_eq!(
        serde_json::from_value::<AgentDefinition>(payload).unwrap(),
        agent
    );
}

#[test]
fn agent_definition_effort_accepts_levels_and_integers() {
    let payload = |effort: AgentEffort| {
        serde_json::to_value(AgentDefinition::new("test", "p").effort(effort)).unwrap()["effort"]
            .clone()
    };
    assert_eq!(payload(EffortLevel::High.into()), json!("high"));
    assert_eq!(payload(EffortLevel::Xhigh.into()), json!("xhigh"));
    assert_eq!(payload(AgentEffort::Tokens(32000)), json!(32000));
}

#[test]
fn agent_definition_new_fields_omitted_when_none() {
    let payload = serde_json::to_value(AgentDefinition::new("test", "p")).unwrap();
    for key in ["background", "effort", "permissionMode"] {
        assert!(payload.get(key).is_none(), "{key}");
    }
}

/// Reads a control response captured from CLI 2.1.329 (see
/// `tests/fixtures/cli/README.md`).
fn captured(name: &str) -> Value {
    let path = format!("{}/tests/fixtures/cli/{name}", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
fn a_context_usage_answer_of_the_real_cli_is_read_and_written_back_whole() {
    let raw = captured("get_context_usage.json");
    let usage: ContextUsageResponse = serde_json::from_value(raw.clone()).unwrap();
    assert_eq!(usage.model, "claude-opus-5-5");
    assert_eq!(usage.max_tokens, 1_000_000);
    assert_eq!(usage.total_tokens, 13_793);
    assert!((usage.percentage - 1.0).abs() < f64::EPSILON);
    assert!(!usage.is_auto_compact_enabled);
    assert_eq!(usage.categories.len(), 7);
    assert_eq!(usage.categories[2].is_deferred, Some(true));
    assert_eq!(usage.categories[0].extra["id"], "system_prompt");
    assert_eq!(usage.skills.as_ref().unwrap()["totalSkills"], 14);
    assert!(usage.slash_commands.is_some() && usage.message_breakdown.is_some());
    assert_eq!(usage.api_usage, None, "the CLI sent null");
    for key in ["coldResumeQuota", "mcpInstructions", "autocompactSource"] {
        assert!(usage.extra.contains_key(key), "{key} is kept");
    }
    let mut written = serde_json::to_value(&usage).unwrap();
    assert_eq!(written["percentage"].as_f64(), Some(1.0));
    written["percentage"] = raw["percentage"].clone();
    written["apiUsage"] = Value::Null;
    assert_eq!(
        written, raw,
        "only the integer percentage and the null apiUsage change"
    );
}

#[test]
fn an_mcp_status_answer_of_the_real_cli_is_read_and_written_back_whole() {
    let raw = captured("mcp_status.json");
    let status: McpStatusResponse = serde_json::from_value(raw.clone()).unwrap();
    let server = &status.mcp_servers[0];
    assert_eq!(server.status, McpServerConnectionStatus::Connected);
    assert!(matches!(
        server.config,
        Some(McpServerStatusConfig::Stdio(_))
    ));
    let tools: Vec<_> = server
        .tools
        .iter()
        .flatten()
        .map(|tool| tool.name.as_str())
        .collect();
    assert_eq!(tools, ["echo", "ping"]);
    assert_eq!(server.extra["transport"], "stdio");
    assert_eq!(serde_json::to_value(&status).unwrap(), raw);
}

#[test]
fn the_real_cli_reports_no_tools_for_a_server_listing_a_schema_that_is_not_an_object() {
    let raw = captured("mcp_status_rejected_tools.json");
    let status: McpStatusResponse = serde_json::from_value(raw).unwrap();
    let server = &status.mcp_servers[0];
    assert_eq!(server.status, McpServerConnectionStatus::Connected);
    assert_eq!(server.error, None, "the CLI gives no reason");
    assert_eq!(
        server.tools.as_deref(),
        Some(&[][..]),
        "the valid tool is dropped too"
    );
}

#[test]
fn a_hook_output_for_an_event_the_crate_does_not_model_is_sent_as_written() {
    let setup = json!({"hookEventName": "Setup", "additionalContext": "ready"});
    let read: HookSpecificOutput = serde_json::from_value(setup.clone()).unwrap();
    assert_eq!(read, HookSpecificOutput::Unknown(setup.clone()));
    assert_eq!(read.hook_event_name(), "Setup");
    assert_eq!(serde_json::to_value(&read).unwrap(), setup);

    let output =
        SyncHookOutput::default().hook_specific_output(HookSpecificOutput::Unknown(setup.clone()));
    assert_eq!(
        serde_json::to_value(&output).unwrap()["hookSpecificOutput"],
        setup
    );
}

#[test]
fn a_hook_output_keeps_the_keys_of_a_known_event_the_crate_does_not_model() {
    let wire = json!({
        "hookEventName": "PostToolUse",
        "additionalContext": "noted",
        "classifierContext": {"verdict": "safe"},
    });
    let read: HookSpecificOutput = serde_json::from_value(wire.clone()).unwrap();
    let HookSpecificOutput::PostToolUse(post) = &read else {
        panic!("expected a typed PostToolUse output, got {read:?}");
    };
    assert_eq!(post.additional_context.as_deref(), Some("noted"));
    assert_eq!(post.extra["classifierContext"], json!({"verdict": "safe"}));
    assert_eq!(serde_json::to_value(&read).unwrap(), wire);

    let built = PostToolUseHookSpecificOutput::default()
        .additional_context("noted")
        .extra_field("classifierContext", json!({"verdict": "safe"}));
    assert_eq!(
        serde_json::to_value(HookSpecificOutput::from(built)).unwrap(),
        wire
    );
}

#[test]
fn usage_keeps_every_key_and_reads_the_documented_counts() {
    let wire = json!({
        "input_tokens": 7.9,
        "output_tokens": "many",
        "cache_creation": {"ephemeral_5m_input_tokens": 3},
        "server_tool_use": {"web_search_requests": 1},
    });
    let usage = claude_agent_rust_sdk::Usage::from(wire.clone());
    assert_eq!(usage.input_tokens(), Some(7));
    assert_eq!(usage.output_tokens(), None);
    assert_eq!(
        usage.get("server_tool_use"),
        Some(&json!({"web_search_requests": 1}))
    );
    assert_eq!(serde_json::to_value(&usage).unwrap(), wire);
}
