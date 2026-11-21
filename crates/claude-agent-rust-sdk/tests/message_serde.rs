#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use claude_agent_rust_sdk::{
    AssistantMessage, ContentBlock, Message, ResultMessage, SystemMessage, TextBlock, ToolUseBlock,
    UserMessage, parse_message,
};
use serde_json::{Value, json};

/// Frames of every message type, written as the CLI writes them.
fn frames() -> Vec<Value> {
    vec![
        json!({
            "type": "user",
            "message": {"role": "user", "content": "Hello"},
            "parent_tool_use_id": null,
            "session_id": "s",
            "uuid": "u1",
        }),
        json!({
            "type": "user",
            "message": {"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": "t1", "content": [{"type": "text", "text": "4"}], "is_error": false},
                {"type": "image", "source": {"type": "base64", "data": "AA=="}},
            ]},
            "parent_tool_use_id": "p1",
            "tool_use_result": {"stdout": "4"},
            "origin": {"kind": "peer", "from": "agent-2"},
            "uuid": "u2",
            "timestamp": "2026-11-21T10:00:00Z",
        }),
        json!({
            "type": "assistant",
            "message": {
                "id": "msg_1",
                "type": "message",
                "role": "assistant",
                "model": "claude-opus-5-5",
                "content": [
                    {"type": "thinking", "thinking": "Hmm", "signature": "sig"},
                    {"type": "text", "text": "Paris.", "citations": [{"url": "https://example.test"}]},
                    {"type": "tool_use", "id": "t1", "name": "Bash", "input": {"command": "ls"}},
                    {"type": "server_tool_use", "id": "s1", "name": "web_search", "input": {"query": "x"}},
                    {"type": "advisor_tool_result", "tool_use_id": "s1", "content": {"ok": true}},
                    {"type": "redacted_thinking", "data": "xyz"},
                ],
                "stop_reason": "tool_use",
                "stop_sequence": null,
                "usage": {"input_tokens": 3, "output_tokens": 9, "service_tier": "standard"},
            },
            "parent_tool_use_id": null,
            "error": "rate_limit",
            "session_id": "s",
            "uuid": "a1",
            "user_message_uuids": ["u1", "u2"],
            "request_id": "req_1",
        }),
        json!({
            "type": "system",
            "subtype": "init",
            "cwd": "/w",
            "session_id": "s",
            "tools": ["Bash"],
            "mcp_servers": [{"name": "m", "status": "connected"}],
            "model": "claude-opus-5-5",
            "permissionMode": "default",
            "apiKeySource": "none",
            "uuid": "i1",
        }),
        json!({
            "type": "system", "subtype": "task_started", "task_id": "k1",
            "description": "Run", "uuid": "x1", "session_id": "s", "tool_use_id": "t1",
            "task_type": "local_bash", "prompt": "ls",
        }),
        json!({
            "type": "system", "subtype": "task_progress", "task_id": "k1",
            "description": "Run", "usage": {"total_tokens": 10, "tool_uses": 1, "duration_ms": 2.5},
            "uuid": "x2", "session_id": "s", "last_tool_name": "Bash",
        }),
        json!({
            "type": "system", "subtype": "task_notification", "task_id": "k1",
            "status": "completed", "output_file": "/o", "summary": "done",
            "uuid": "x3", "session_id": "s", "usage": {"total_tokens": 1, "tool_uses": 0, "duration_ms": 9},
        }),
        json!({
            "type": "system", "subtype": "task_updated", "task_id": "k1",
            "patch": {"status": "killed", "end_time": 5}, "session_id": "s", "uuid": "x4",
        }),
        json!({
            "type": "system", "subtype": "mirror_error",
            "key": {"project_key": "p", "session_id": "s", "subpath": "subagents/agent-a"},
            "error": "append failed",
        }),
        json!({
            "type": "system", "subtype": "hook_started", "hook_id": "h",
            "hook_name": "PreToolUse:Bash", "hook_event": "PreToolUse",
            "session_id": "s", "uuid": "x5",
        }),
        json!({
            "type": "result",
            "subtype": "success",
            "is_error": false,
            "duration_ms": 1200,
            "duration_api_ms": 900,
            "num_turns": 2,
            "result": "Paris.",
            "stop_reason": "end_turn",
            "session_id": "s",
            "total_cost_usd": 0.0125,
            "usage": {"input_tokens": 3},
            "modelUsage": {"claude-opus-5-5": {
                "inputTokens": 3, "outputTokens": 9, "cacheReadInputTokens": 0,
                "cacheCreationInputTokens": 0, "webSearchRequests": 0, "costUSD": 0.0125,
                "contextWindow": 200_000, "maxOutputTokens": 32_000, "speed": "fast",
            }},
            "permission_denials": [{"tool_name": "Bash"}],
            "deferred_tool_use": {"id": "t9", "name": "Bash", "input": {}},
            "errors": ["boom"],
            "api_error_status": 529,
            "terminal_reason": "completed",
            "uuid": "r1",
            "fast_mode_state": "off",
        }),
        json!({
            "type": "stream_event",
            "uuid": "e1",
            "session_id": "s",
            "event": {"type": "content_block_delta", "delta": {"type": "text_delta", "text": "Pa"}},
            "parent_tool_use_id": null,
            "ttft_ms": 12,
        }),
        json!({
            "type": "rate_limit_event",
            "rate_limit_info": {
                "status": "allowed_warning", "resetsAt": 1_700_000_000,
                "rateLimitType": "five_hour", "utilization": 0.91, "isUsingOverage": false,
            },
            "uuid": "l1",
            "session_id": "s",
        }),
        json!({
            "type": "conversation_reset", "new_conversation_id": "c2",
            "uuid": "z1", "session_id": "s",
        }),
        json!({"type": "a_future_frame", "payload": [1, 2]}),
    ]
}

#[test]
fn every_message_read_from_the_cli_serializes_back_to_its_frame() {
    for frame in frames() {
        let message = parse_message(frame.clone()).unwrap();
        assert_eq!(serde_json::to_value(&message).unwrap(), frame);

        let text = serde_json::to_string(&message).unwrap();
        let back: Message = serde_json::from_str(&text).unwrap();
        assert_eq!(back, message, "{text}");
    }
}

#[test]
fn a_message_type_serializes_alone_as_the_frame_its_variant_writes() {
    for frame in frames() {
        let message = parse_message(frame.clone()).unwrap();
        let alone = match &message {
            Message::User(inner) => serde_json::to_value(inner),
            Message::Assistant(inner) => serde_json::to_value(inner),
            Message::System(inner) => serde_json::to_value(inner),
            Message::Result(inner) => serde_json::to_value(inner),
            Message::StreamEvent(inner) => serde_json::to_value(inner),
            Message::RateLimit(inner) => serde_json::to_value(inner),
            Message::ConversationReset(inner) => serde_json::to_value(inner),
            _ => continue,
        };
        assert_eq!(alone.unwrap(), frame);
    }
    let blocks = &frames()[2]["message"]["content"];
    for block in blocks.as_array().unwrap() {
        let read: ContentBlock = serde_json::from_value(block.clone()).unwrap();
        assert_eq!(&serde_json::to_value(&read).unwrap(), block);
    }
}

#[test]
fn a_value_the_typed_field_could_not_hold_is_written_back_until_the_field_changes() {
    let frame = json!({
        "type": "result", "subtype": "error_during_execution", "is_error": true,
        "duration_ms": 1, "duration_api_ms": 1, "num_turns": 1, "session_id": "s",
        "api_error_status": "529", "errors": ["boom", {"code": 529}],
    });
    let Message::Result(mut result) = parse_message(frame.clone()).unwrap() else {
        panic!("a result message");
    };
    assert_eq!(serde_json::to_value(&result).unwrap(), frame);

    result.api_error_status = Some(503);
    result.errors = None;
    let written = serde_json::to_value(&result).unwrap();
    assert_eq!(written["api_error_status"], 503);
    assert!(written.get("errors").is_none(), "{written}");
}

#[test]
fn a_changed_field_of_a_system_message_is_written_over_its_frame() {
    let frame = frames()[5].clone();
    let Message::System(SystemMessage::TaskProgress(mut progress)) =
        parse_message(frame.clone()).unwrap()
    else {
        panic!("a task progress message");
    };
    progress.description = "Run again".to_owned();
    progress.last_tool_name = None;
    let written = serde_json::to_value(progress_message(progress)).unwrap();
    let mut expected = frame;
    expected["description"] = json!("Run again");
    expected
        .as_object_mut()
        .unwrap()
        .retain(|name, _| name.as_str() != "last_tool_name");
    assert_eq!(written, expected);
}

fn progress_message(progress: claude_agent_rust_sdk::TaskProgressMessage) -> SystemMessage {
    SystemMessage::TaskProgress(progress)
}

#[test]
fn a_message_built_in_code_serializes_to_a_frame_that_reads_back_the_same() {
    let mut assistant = AssistantMessage::new(
        vec![
            ContentBlock::Text(TextBlock::new("Hi")),
            ContentBlock::ToolUse(ToolUseBlock::new("t1", "Bash", json!({"command": "ls"}))),
        ],
        "claude-opus-5-5",
    );
    assistant.session_id = Some("s".to_owned());
    let mut user = UserMessage::new("Hello");
    user.uuid = Some("u".to_owned());
    let mut result = ResultMessage::new("success", 5, 4, false, 1, "s");
    result.total_cost_usd = Some(0.5);

    for message in [
        Message::Assistant(assistant),
        Message::User(user),
        Message::Result(result),
    ] {
        let frame = serde_json::to_value(&message).unwrap();
        assert_eq!(parse_message(frame.clone()).unwrap(), message, "{frame}");
    }
    assert_eq!(
        serde_json::to_value(UserMessage::new("Hello")).unwrap(),
        json!({"type": "user", "message": {"content": "Hello"}, "parent_tool_use_id": null})
    );
}

#[test]
fn deserializing_a_frame_of_another_kind_or_an_invalid_frame_is_an_error() {
    let error = serde_json::from_value::<UserMessage>(frames()[2].clone()).unwrap_err();
    assert_eq!(error.to_string(), "expected a user message");
    let error = serde_json::from_value::<TextBlock>(
        json!({"type": "tool_use", "id": "t", "name": "n", "input": {}}),
    )
    .unwrap_err();
    assert_eq!(error.to_string(), "expected a text block");
    let error = serde_json::from_value::<Message>(json!({"type": "result"})).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("Missing required field in result message"),
        "{error}"
    );
}

/// Every public message, hook input, and session type can be built from
/// outside the crate, as upstream's dataclasses can, and what is built
/// round-trips through the wire form.
#[test]
fn messages_and_hook_inputs_built_in_code_round_trip_through_the_wire() {
    use claude_agent_rust_sdk::{
        CompactTrigger, ConversationResetMessage, DeferredToolUse, GenericSystemMessage,
        HookEventMessage, HookInput, HookInputBase, MirrorErrorMessage, NotificationHookInput,
        PermissionRequestHookInput, PostToolUseFailureHookInput, PostToolUseHookInput,
        PreCompactHookInput, PreToolUseHookInput, RateLimitEvent, RateLimitInfo, RateLimitStatus,
        SessionMessage, SessionMessageType, StopHookInput, StreamEvent, SubagentStartHookInput,
        SubagentStopHookInput, TaskNotificationMessage, TaskNotificationStatus,
        TaskProgressMessage, TaskStartedMessage, TaskUpdatedMessage, TaskUsage,
        UserPromptSubmitHookInput,
    };

    let mut started = TaskStartedMessage::new("t1", "Explore", "u1", "s");
    started.task_type = Some("local_agent".to_owned());
    let mut usage = TaskUsage::default();
    usage.total_tokens = 10;
    let mut patch = serde_json::Map::new();
    patch.insert("status".to_owned(), json!("completed"));
    let updated = TaskUpdatedMessage::new("t1", patch);
    assert_eq!(
        updated.status.as_ref().map(|status| status.as_str()),
        Some("completed")
    );
    let messages = [
        Message::System(SystemMessage::TaskStarted(started)),
        Message::System(SystemMessage::TaskProgress(TaskProgressMessage::new(
            "t1", "Explore", usage, "u2", "s",
        ))),
        Message::System(SystemMessage::TaskNotification(
            TaskNotificationMessage::new(
                "t1",
                TaskNotificationStatus::Completed,
                "/out",
                "done",
                "u3",
                "s",
            ),
        )),
        Message::System(SystemMessage::TaskUpdated(updated)),
        Message::System(SystemMessage::MirrorError(MirrorErrorMessage::new(
            "store down",
        ))),
        Message::System(SystemMessage::HookEvent(HookEventMessage::new(
            "hook_started",
            "PreToolUse",
        ))),
        Message::System(SystemMessage::Other(GenericSystemMessage::new(
            "status",
            json!({"type": "system", "subtype": "status", "status": "compacting"}),
        ))),
        Message::StreamEvent(StreamEvent::new(
            "u4",
            "s",
            json!({"type": "message_start"}),
        )),
        Message::RateLimit(RateLimitEvent::new(
            RateLimitInfo::new(RateLimitStatus::Allowed),
            "u5",
            "s",
        )),
        Message::ConversationReset(ConversationResetMessage::new("c2", "u6", "s")),
    ];
    for message in messages {
        let wire = serde_json::to_value(&message).unwrap();
        let read = parse_message(wire.clone()).unwrap();
        assert_eq!(serde_json::to_value(&read).unwrap(), wire);
        assert_eq!(
            std::mem::discriminant(&read),
            std::mem::discriminant(&message)
        );
    }

    let base = HookInputBase::new("s", "/t.jsonl", "/w");
    let inputs = [
        HookInput::PreToolUse(PreToolUseHookInput::new(
            base.clone(),
            "Bash",
            json!({"command": "ls"}),
            "tu1",
        )),
        HookInput::PostToolUse(PostToolUseHookInput::new(
            base.clone(),
            "Bash",
            json!({}),
            json!("ok"),
            "tu1",
        )),
        HookInput::PostToolUseFailure(PostToolUseFailureHookInput::new(
            base.clone(),
            "Bash",
            json!({}),
            "tu1",
            "boom",
        )),
        HookInput::UserPromptSubmit(UserPromptSubmitHookInput::new(base.clone(), "hi")),
        HookInput::Stop(StopHookInput::new(base.clone(), false)),
        HookInput::SubagentStop(SubagentStopHookInput::new(base.clone(), true, "/a.jsonl")),
        HookInput::PreCompact(PreCompactHookInput::new(
            base.clone(),
            CompactTrigger::Manual,
        )),
        HookInput::Notification(NotificationHookInput::new(
            base.clone(),
            "waiting",
            "idle_prompt",
        )),
        HookInput::SubagentStart(SubagentStartHookInput::new(base.clone())),
        HookInput::PermissionRequest(PermissionRequestHookInput::new(
            base,
            "Write",
            json!({"path": "/x"}),
        )),
    ];
    for input in inputs {
        let wire = serde_json::to_value(&input).unwrap();
        let read: HookInput = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(read, input, "{wire}");
    }

    let deferred = DeferredToolUse::new("tu1", "Bash", json!({}));
    assert_eq!(deferred.name, "Bash");
    let stored = SessionMessage::new(SessionMessageType::User, "u1", "s", json!({"role": "user"}));
    assert_eq!(stored.parent_tool_use_id, None);
    assert_eq!(stored.parent_agent_id, None);
}
