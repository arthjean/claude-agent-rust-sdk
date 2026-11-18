#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use claude_agent_rust_sdk::{
    AssistantMessage, AssistantMessageError, ContentBlock, Error, Message, MessageOriginKind,
    RateLimitEvent, RateLimitStatus, RateLimitType, ResultMessage, ServerToolName, SessionKey,
    StreamEvent, SystemMessage, TERMINAL_TASK_STATUSES, TaskNotificationOriginSubkind,
    TaskNotificationStatus, TaskUpdatedMessage, TaskUpdatedStatus, TaskUsage, ToolResultContent,
    UserContent, UserMessage, is_terminal_task_status, parse_message,
};
use serde_json::{Value, json};

fn user(data: Value) -> UserMessage {
    match parse_message(data).unwrap() {
        Message::User(message) => message,
        other => panic!("expected a user message, got {other:?}"),
    }
}

fn assistant(data: Value) -> AssistantMessage {
    match parse_message(data).unwrap() {
        Message::Assistant(message) => message,
        other => panic!("expected an assistant message, got {other:?}"),
    }
}

fn system(data: Value) -> SystemMessage {
    match parse_message(data).unwrap() {
        Message::System(message) => message,
        other => panic!("expected a system message, got {other:?}"),
    }
}

fn result(data: Value) -> ResultMessage {
    match parse_message(data).unwrap() {
        Message::Result(message) => message,
        other => panic!("expected a result message, got {other:?}"),
    }
}

fn stream_event(data: Value) -> StreamEvent {
    match parse_message(data).unwrap() {
        Message::StreamEvent(message) => message,
        other => panic!("expected a stream event, got {other:?}"),
    }
}

fn rate_limit(data: Value) -> RateLimitEvent {
    match parse_message(data).unwrap() {
        Message::RateLimit(message) => message,
        other => panic!("expected a rate limit event, got {other:?}"),
    }
}

fn task_updated(data: Value) -> TaskUpdatedMessage {
    match system(data) {
        SystemMessage::TaskUpdated(message) => message,
        other => panic!("expected a task_updated message, got {other:?}"),
    }
}

fn parse_error(data: Value) -> (String, Option<Value>) {
    match parse_message(data) {
        Err(Error::MessageParse { message, data }) => (message, data),
        other => panic!("expected a parse error, got {other:?}"),
    }
}

fn blocks(content: &UserContent) -> &[ContentBlock] {
    content.as_blocks().expect("content should be a block list")
}

fn base_result() -> Value {
    json!({
        "type": "result",
        "subtype": "success",
        "duration_ms": 1000,
        "duration_api_ms": 500,
        "is_error": false,
        "num_turns": 2,
        "session_id": "session_123",
    })
}

fn with(mut base: Value, extra: Value) -> Value {
    let target = base.as_object_mut().unwrap();
    for (key, value) in extra.as_object().unwrap() {
        target.insert(key.clone(), value.clone());
    }
    base
}

#[test]
fn parse_valid_user_message() {
    let message = user(json!({
        "type": "user",
        "message": {"content": [{"type": "text", "text": "Hello"}]},
    }));
    let content = blocks(&message.content);
    assert_eq!(content.len(), 1);
    assert_eq!(content[0].as_text(), Some("Hello"));
}

#[test]
fn parse_user_message_with_string_content_yields_string_content() {
    let message = user(json!({"type": "user", "message": {"content": "Hello"}}));
    assert_eq!(message.content, UserContent::Text("Hello".to_owned()));
    assert_eq!(message.content.as_text(), Some("Hello"));
}

#[test]
fn parse_user_message_with_uuid() {
    let message = user(json!({
        "type": "user",
        "uuid": "msg-abc123-def456",
        "message": {"content": [{"type": "text", "text": "Hello"}]},
    }));
    assert_eq!(message.uuid.as_deref(), Some("msg-abc123-def456"));
    assert_eq!(blocks(&message.content).len(), 1);
}

#[test]
fn parse_user_message_origin() {
    let peer = json!({
        "kind": "peer",
        "from": "peer-addr",
        "name": "other-session",
        "verifiedPeerPid": 4242,
        "someFutureField": true,
    });
    for content in [json!("hi"), json!([{"type": "text", "text": "hi"}])] {
        let message = user(json!({
            "type": "user",
            "message": {"content": content},
            "origin": peer,
        }));
        let origin = message.origin.expect("origin should be parsed");
        assert_eq!(Value::Object(origin.raw.clone()), peer);
        assert_eq!(origin.kind, MessageOriginKind::Peer);
        assert_eq!(origin.from(), Some("peer-addr"));
        assert_eq!(origin.name(), Some("other-session"));
        assert_eq!(origin.verified_peer_pid(), Some(4242));
        assert_eq!(origin.raw["someFutureField"], true);
    }
}

#[test]
fn parse_user_message_origin_absent_or_malformed() {
    for extra in [
        json!({}),
        json!({"origin": null}),
        json!({"origin": "human"}),
        json!({"origin": {}}),
    ] {
        let message = user(with(
            json!({"type": "user", "message": {"content": "hi"}}),
            extra.clone(),
        ));
        assert_eq!(message.origin, None, "{extra}");
    }
}

#[test]
fn parse_user_message_with_tool_use() {
    let message = user(json!({
        "type": "user",
        "message": {
            "content": [
                {"type": "text", "text": "Let me read this file"},
                {
                    "type": "tool_use",
                    "id": "tool_456",
                    "name": "Read",
                    "input": {"file_path": "/example.txt"},
                },
            ]
        },
    }));
    let content = blocks(&message.content);
    assert_eq!(content.len(), 2);
    assert!(matches!(content[0], ContentBlock::Text(_)));
    let ContentBlock::ToolUse(tool_use) = &content[1] else {
        panic!("expected a tool use block");
    };
    assert_eq!(tool_use.id, "tool_456");
    assert_eq!(tool_use.name, "Read");
    assert_eq!(tool_use.input, json!({"file_path": "/example.txt"}));
}

#[test]
fn parse_user_message_with_tool_result() {
    let message = user(json!({
        "type": "user",
        "message": {
            "content": [
                {
                    "type": "tool_result",
                    "tool_use_id": "tool_789",
                    "content": "File contents here",
                }
            ]
        },
    }));
    let content = blocks(&message.content);
    assert_eq!(content.len(), 1);
    let ContentBlock::ToolResult(tool_result) = &content[0] else {
        panic!("expected a tool result block");
    };
    assert_eq!(tool_result.tool_use_id, "tool_789");
    assert_eq!(
        tool_result.content,
        Some(ToolResultContent::Text("File contents here".to_owned()))
    );
    assert_eq!(tool_result.is_error, None);
}

#[test]
fn parse_user_message_with_tool_result_error() {
    let message = user(json!({
        "type": "user",
        "message": {
            "content": [
                {
                    "type": "tool_result",
                    "tool_use_id": "tool_error",
                    "content": "File not found",
                    "is_error": true,
                }
            ]
        },
    }));
    let ContentBlock::ToolResult(tool_result) = &blocks(&message.content)[0] else {
        panic!("expected a tool result block");
    };
    assert_eq!(tool_result.tool_use_id, "tool_error");
    assert_eq!(
        tool_result.content.as_ref().and_then(|c| c.as_text()),
        Some("File not found")
    );
    assert_eq!(tool_result.is_error, Some(true));
}

const PERSISTED_OUTPUT_PREFIX: &str = "<persisted-output>";
const INLINE_RESULT_LIMIT_CHARS: usize = 50_000;

fn tool_result_frame(content: &str, is_error: bool) -> Value {
    json!({
        "type": "user",
        "message": {
            "role": "user",
            "content": [
                {
                    "type": "tool_result",
                    "tool_use_id": "toolu_01ABC",
                    "content": content,
                    "is_error": is_error,
                }
            ],
        },
        "parent_tool_use_id": null,
        "tool_use_result": null,
        "uuid": "test-uuid-1234",
    })
}

fn persisted_output() -> String {
    format!(
        "{PERSISTED_OUTPUT_PREFIX}\nOutput too large (73.0KB). Full output saved to: \
         /tmp/.claude/tool-results/abc123.txt\n\nPreview (first 2KB):\n{}\n...\n\
         </persisted-output>",
        "x".repeat(2000)
    )
}

fn tool_result_text(message: &UserMessage) -> &str {
    let content = blocks(&message.content);
    assert_eq!(content.len(), 1);
    let ContentBlock::ToolResult(tool_result) = &content[0] else {
        panic!("expected a tool result block");
    };
    tool_result
        .content
        .as_ref()
        .and_then(ToolResultContent::as_text)
        .expect("the tool result content should be a string")
}

#[test]
fn persisted_output_tool_result_is_parsed_unchanged_so_a_caller_can_detect_it_by_its_prefix() {
    let persisted = persisted_output();
    let message = user(tool_result_frame(&persisted, false));
    let text = tool_result_text(&message);
    assert!(text.starts_with(PERSISTED_OUTPUT_PREFIX), "{text:.100}");
    assert_eq!(text, persisted);

    let inline = "x".repeat(1000);
    let message = user(tool_result_frame(&inline, false));
    let text = tool_result_text(&message);
    assert_eq!(text, inline);
    assert!(!text.starts_with(PERSISTED_OUTPUT_PREFIX));
}

#[test]
fn persisted_output_tool_result_keeps_the_preview_length_and_is_not_expanded() {
    let persisted = persisted_output();
    let message = user(tool_result_frame(&persisted, false));
    let text = tool_result_text(&message);
    assert_eq!(text.len(), persisted.len());
    assert!(text.len() < INLINE_RESULT_LIMIT_CHARS, "{}", text.len());
}

#[test]
fn tool_result_with_an_explicit_false_error_flag_is_parsed_as_not_an_error() {
    let message = user(tool_result_frame(&"x".repeat(1000), false));
    let ContentBlock::ToolResult(tool_result) = &blocks(&message.content)[0] else {
        panic!("expected a tool result block");
    };
    assert_eq!(tool_result.is_error, Some(false));

    let message = user(tool_result_frame("tool failed", true));
    let ContentBlock::ToolResult(tool_result) = &blocks(&message.content)[0] else {
        panic!("expected a tool result block");
    };
    assert_eq!(tool_result.is_error, Some(true));
}

#[test]
fn parse_tool_result_content_shapes() {
    let message = user(json!({
        "type": "user",
        "message": {
            "content": [
                {"type": "tool_result", "tool_use_id": "a", "content": [{"type": "text", "text": "x"}]},
                {"type": "tool_result", "tool_use_id": "b", "content": null},
                {"type": "tool_result", "tool_use_id": "c"},
                {"type": "tool_result", "tool_use_id": "d", "content": {"odd": 1}},
            ]
        },
    }));
    let contents: Vec<Option<ToolResultContent>> = blocks(&message.content)
        .iter()
        .map(|block| match block {
            ContentBlock::ToolResult(tool_result) => tool_result.content.clone(),
            other => panic!("expected a tool result block, got {other:?}"),
        })
        .collect();
    assert_eq!(
        contents,
        [
            Some(ToolResultContent::Blocks(vec![
                json!({"type": "text", "text": "x"})
            ])),
            None,
            None,
            Some(ToolResultContent::Other(json!({"odd": 1}))),
        ]
    );
}

#[test]
fn parse_user_message_with_mixed_content() {
    let message = user(json!({
        "type": "user",
        "message": {
            "content": [
                {"type": "text", "text": "Here's what I found:"},
                {"type": "tool_use", "id": "use_1", "name": "Search", "input": {"query": "test"}},
                {"type": "tool_result", "tool_use_id": "use_1", "content": "Search results"},
                {"type": "text", "text": "What do you think?"},
            ]
        },
    }));
    let content = blocks(&message.content);
    assert_eq!(content.len(), 4);
    assert!(matches!(content[0], ContentBlock::Text(_)));
    assert!(matches!(content[1], ContentBlock::ToolUse(_)));
    assert!(matches!(content[2], ContentBlock::ToolResult(_)));
    assert!(matches!(content[3], ContentBlock::Text(_)));
}

#[test]
fn parse_user_message_inside_subagent() {
    let message = user(json!({
        "type": "user",
        "message": {"content": [{"type": "text", "text": "Hello"}]},
        "parent_tool_use_id": "toolu_01Xrwd5Y13sEHtzScxR77So8",
    }));
    assert_eq!(
        message.parent_tool_use_id.as_deref(),
        Some("toolu_01Xrwd5Y13sEHtzScxR77So8")
    );
}

#[test]
fn parse_user_message_with_tool_use_result() {
    let tool_result_data = json!({
        "filePath": "/path/to/file.py",
        "oldString": "old code",
        "newString": "new code",
        "originalFile": "full file contents",
        "structuredPatch": [
            {
                "oldStart": 33,
                "oldLines": 7,
                "newStart": 33,
                "newLines": 7,
                "lines": ["   # comment", "-      old line", "+      new line"],
            }
        ],
        "userModified": false,
        "replaceAll": false,
    });
    let message = user(json!({
        "type": "user",
        "message": {
            "role": "user",
            "content": [
                {
                    "tool_use_id": "toolu_vrtx_01KXWexk3NJdwkjWzPMGQ2F1",
                    "type": "tool_result",
                    "content": "The file has been updated.",
                }
            ],
        },
        "parent_tool_use_id": null,
        "session_id": "84afb479-17ae-49af-8f2b-666ac2530c3a",
        "uuid": "2ace3375-1879-48a0-a421-6bce25a9295a",
        "tool_use_result": tool_result_data,
    }));
    let tool_use_result = message.tool_use_result.expect("tool_use_result");
    assert_eq!(tool_use_result, tool_result_data);
    assert_eq!(tool_use_result["filePath"], "/path/to/file.py");
    assert_eq!(tool_use_result["structuredPatch"][0]["oldStart"], 33);
    assert_eq!(
        message.uuid.as_deref(),
        Some("2ace3375-1879-48a0-a421-6bce25a9295a")
    );
    assert_eq!(message.parent_tool_use_id, None);
}

#[test]
fn parse_user_message_with_string_content_and_tool_use_result() {
    let tool_result_data = json!({"filePath": "/path/to/file.py", "userModified": true});
    let message = user(json!({
        "type": "user",
        "message": {"content": "Simple string content"},
        "tool_use_result": tool_result_data,
    }));
    assert_eq!(message.content.as_text(), Some("Simple string content"));
    assert_eq!(message.tool_use_result, Some(tool_result_data));
}

#[test]
fn parse_valid_assistant_message() {
    let message = assistant(json!({
        "type": "assistant",
        "message": {
            "content": [
                {"type": "text", "text": "Hello"},
                {"type": "tool_use", "id": "tool_123", "name": "Read", "input": {"file_path": "/test.txt"}},
            ],
            "model": "claude-opus-4-1-20250805",
        },
    }));
    assert_eq!(message.content.len(), 2);
    assert!(matches!(message.content[0], ContentBlock::Text(_)));
    assert!(matches!(message.content[1], ContentBlock::ToolUse(_)));
    assert_eq!(message.model, "claude-opus-4-1-20250805");
}

#[test]
fn parse_assistant_message_with_thinking() {
    let message = assistant(json!({
        "type": "assistant",
        "message": {
            "content": [
                {"type": "thinking", "thinking": "I'm thinking about the answer...", "signature": "sig-123"},
                {"type": "text", "text": "Here's my response"},
            ],
            "model": "claude-opus-4-1-20250805",
        },
    }));
    assert_eq!(message.content.len(), 2);
    let ContentBlock::Thinking(thinking) = &message.content[0] else {
        panic!("expected a thinking block");
    };
    assert_eq!(thinking.thinking, "I'm thinking about the answer...");
    assert_eq!(thinking.signature, "sig-123");
    assert_eq!(message.content[1].as_text(), Some("Here's my response"));
}

#[test]
fn parse_assistant_message_with_server_tool_use() {
    let message = assistant(json!({
        "type": "assistant",
        "message": {
            "content": [
                {"type": "server_tool_use", "id": "srvtoolu_01ABC", "name": "advisor", "input": {}},
            ],
            "model": "claude-sonnet-4-5",
        },
    }));
    assert_eq!(message.content.len(), 1);
    let ContentBlock::ServerToolUse(server_tool_use) = &message.content[0] else {
        panic!("expected a server tool use block");
    };
    assert_eq!(server_tool_use.id, "srvtoolu_01ABC");
    assert_eq!(server_tool_use.name, ServerToolName::Advisor);
    assert_eq!(server_tool_use.input, json!({}));
}

#[test]
fn parse_assistant_message_with_server_tool_result() {
    let message = assistant(json!({
        "type": "assistant",
        "message": {
            "content": [
                {
                    "type": "advisor_tool_result",
                    "tool_use_id": "srvtoolu_01ABC",
                    "content": {
                        "type": "advisor_result",
                        "text": "Consider edge cases around empty input.",
                    },
                },
            ],
            "model": "claude-sonnet-4-5",
        },
    }));
    assert_eq!(message.content.len(), 1);
    let ContentBlock::ServerToolResult(server_tool_result) = &message.content[0] else {
        panic!("expected a server tool result block");
    };
    assert_eq!(server_tool_result.tool_use_id, "srvtoolu_01ABC");
    assert_eq!(
        server_tool_result.content,
        json!({"type": "advisor_result", "text": "Consider edge cases around empty input."})
    );
}

#[test]
fn parse_assistant_message_with_redacted_advisor_result() {
    let message = assistant(json!({
        "type": "assistant",
        "message": {
            "content": [
                {
                    "type": "advisor_tool_result",
                    "tool_use_id": "srvtoolu_01ABC",
                    "content": {
                        "type": "advisor_redacted_result",
                        "encrypted_content": "EuYDCioIDhgC...",
                    },
                },
            ],
            "model": "claude-sonnet-4-5",
        },
    }));
    let ContentBlock::ServerToolResult(server_tool_result) = &message.content[0] else {
        panic!("expected a server tool result block");
    };
    assert_eq!(
        server_tool_result.content["type"],
        "advisor_redacted_result"
    );
    assert_eq!(
        server_tool_result.content["encrypted_content"],
        "EuYDCioIDhgC..."
    );
}

#[test]
fn unknown_content_block_is_kept_as_raw_json() {
    let future_block = json!({"type": "hologram", "frames": [1, 2, 3]});
    let message = assistant(json!({
        "type": "assistant",
        "message": {
            "content": [{"type": "text", "text": "before"}, future_block, {"type": "text", "text": "after"}],
            "model": "m",
        },
    }));
    assert_eq!(message.content.len(), 3);
    assert_eq!(
        message.content[1],
        ContentBlock::Unknown(future_block.clone())
    );

    let message = user(json!({"type": "user", "message": {"content": [future_block]}}));
    assert_eq!(
        blocks(&message.content),
        [ContentBlock::Unknown(future_block)]
    );
}

#[test]
fn parse_assistant_message_with_usage() {
    let usage = json!({
        "input_tokens": 100,
        "output_tokens": 50,
        "cache_read_input_tokens": 2000,
        "cache_creation_input_tokens": 500,
    });
    let message = assistant(json!({
        "type": "assistant",
        "message": {
            "content": [{"type": "text", "text": "hi"}],
            "model": "claude-opus-4-5",
            "usage": usage,
        },
    }));
    assert_eq!(message.usage, Some(usage));
}

#[test]
fn parse_assistant_message_without_usage() {
    let message = assistant(json!({
        "type": "assistant",
        "message": {"content": [{"type": "text", "text": "hi"}], "model": "claude-opus-4-5"},
    }));
    assert_eq!(message.usage, None);
}

#[test]
fn parse_valid_system_message() {
    let message = system(json!({"type": "system", "subtype": "start"}));
    assert!(matches!(message, SystemMessage::Other(_)));
    assert_eq!(message.subtype(), "start");
}

#[test]
fn parse_task_started_message() {
    let data = json!({
        "type": "system",
        "subtype": "task_started",
        "task_id": "task-abc",
        "tool_use_id": "toolu_01",
        "description": "Reticulating splines",
        "task_type": "background",
        "uuid": "uuid-1",
        "session_id": "session-1",
    });
    let SystemMessage::TaskStarted(message) = system(data.clone()) else {
        panic!("expected a task_started message");
    };
    assert_eq!(message.task_id, "task-abc");
    assert_eq!(message.description, "Reticulating splines");
    assert_eq!(message.uuid, "uuid-1");
    assert_eq!(message.session_id, "session-1");
    assert_eq!(message.tool_use_id.as_deref(), Some("toolu_01"));
    assert_eq!(message.task_type.as_deref(), Some("background"));
    assert_eq!(message.subtype, "task_started");
    assert_eq!(message.data, data);
}

#[test]
fn parse_task_started_message_optional_fields_absent() {
    let SystemMessage::TaskStarted(message) = system(json!({
        "type": "system",
        "subtype": "task_started",
        "task_id": "task-abc",
        "description": "Working",
        "uuid": "uuid-1",
        "session_id": "session-1",
    })) else {
        panic!("expected a task_started message");
    };
    assert_eq!(message.tool_use_id, None);
    assert_eq!(message.task_type, None);
}

#[test]
fn parse_task_progress_message() {
    let SystemMessage::TaskProgress(message) = system(json!({
        "type": "system",
        "subtype": "task_progress",
        "task_id": "task-abc",
        "tool_use_id": "toolu_01",
        "description": "Halfway there",
        "usage": {"total_tokens": 1234, "tool_uses": 5, "duration_ms": 9876},
        "last_tool_name": "Read",
        "uuid": "uuid-2",
        "session_id": "session-1",
    })) else {
        panic!("expected a task_progress message");
    };
    assert_eq!(message.task_id, "task-abc");
    assert_eq!(message.description, "Halfway there");
    assert_eq!(
        (
            message.usage.total_tokens,
            message.usage.tool_uses,
            message.usage.duration_ms
        ),
        (1234, 5, 9876)
    );
    assert_eq!(message.last_tool_name.as_deref(), Some("Read"));
    assert_eq!(message.tool_use_id.as_deref(), Some("toolu_01"));
    assert_eq!(message.uuid, "uuid-2");
    assert_eq!(message.session_id, "session-1");
}

#[test]
fn parse_task_notification_message() {
    let SystemMessage::TaskNotification(message) = system(json!({
        "type": "system",
        "subtype": "task_notification",
        "task_id": "task-abc",
        "tool_use_id": "toolu_01",
        "status": "completed",
        "output_file": "/tmp/out.md",
        "summary": "All done",
        "usage": {"total_tokens": 2000, "tool_uses": 7, "duration_ms": 12345},
        "uuid": "uuid-3",
        "session_id": "session-1",
    })) else {
        panic!("expected a task_notification message");
    };
    assert_eq!(message.task_id, "task-abc");
    assert_eq!(message.status, TaskNotificationStatus::Completed);
    assert!(message.status.is_terminal());
    assert_eq!(message.output_file, "/tmp/out.md");
    assert_eq!(message.summary, "All done");
    let usage: TaskUsage = message.usage.expect("usage");
    assert_eq!(
        (usage.total_tokens, usage.tool_uses, usage.duration_ms),
        (2000, 7, 12345)
    );
    assert_eq!(message.tool_use_id.as_deref(), Some("toolu_01"));
    assert_eq!(message.uuid, "uuid-3");
    assert_eq!(message.session_id, "session-1");
}

#[test]
fn parse_task_notification_message_optional_fields_absent() {
    let SystemMessage::TaskNotification(message) = system(json!({
        "type": "system",
        "subtype": "task_notification",
        "task_id": "task-abc",
        "status": "failed",
        "output_file": "/tmp/out.md",
        "summary": "Boom",
        "uuid": "uuid-3",
        "session_id": "session-1",
    })) else {
        panic!("expected a task_notification message");
    };
    assert_eq!(message.status, TaskNotificationStatus::Failed);
    assert_eq!(message.usage, None);
    assert_eq!(message.tool_use_id, None);
}

#[test]
fn typed_task_messages_require_their_fields() {
    let cases = [
        (
            json!({"type": "system", "subtype": "task_started", "description": "d", "uuid": "u", "session_id": "s"}),
            "task_id",
        ),
        (
            json!({"type": "system", "subtype": "task_started", "task_id": "t", "uuid": "u", "session_id": "s"}),
            "description",
        ),
        (
            json!({"type": "system", "subtype": "task_started", "task_id": "t", "description": "d", "session_id": "s"}),
            "uuid",
        ),
        (
            json!({"type": "system", "subtype": "task_started", "task_id": "t", "description": "d", "uuid": "u"}),
            "session_id",
        ),
        (
            json!({"type": "system", "subtype": "task_progress", "task_id": "t", "description": "d", "uuid": "u", "session_id": "s"}),
            "usage",
        ),
        (
            json!({"type": "system", "subtype": "task_notification", "task_id": "t", "output_file": "/o", "summary": "s", "uuid": "u", "session_id": "s"}),
            "status",
        ),
        (
            json!({"type": "system", "subtype": "task_notification", "task_id": "t", "status": "completed", "summary": "s", "uuid": "u", "session_id": "s"}),
            "output_file",
        ),
        (
            json!({"type": "system", "subtype": "task_notification", "task_id": "t", "status": "completed", "output_file": "/o", "uuid": "u", "session_id": "s"}),
            "summary",
        ),
    ];
    for (data, field) in cases {
        let (message, _) = parse_error(data);
        assert_eq!(
            message,
            format!("Missing required field in system message: '{field}'")
        );
    }
}

#[test]
fn parse_task_updated_message_terminal() {
    let message = task_updated(json!({
        "type": "system",
        "subtype": "task_updated",
        "task_id": "task-abc",
        "patch": {"status": "completed", "end_time": 1780405729183_i64},
        "uuid": "uuid-4",
        "session_id": "session-1",
    }));
    assert_eq!(message.task_id, "task-abc");
    assert_eq!(
        Value::Object(message.patch.clone()),
        json!({"status": "completed", "end_time": 1780405729183_i64})
    );
    assert_eq!(message.status, Some(TaskUpdatedStatus::Completed));
    assert_eq!(message.uuid.as_deref(), Some("uuid-4"));
    assert_eq!(message.session_id.as_deref(), Some("session-1"));
    assert!(message.status.unwrap().is_terminal());
}

#[test]
fn parse_task_updated_message_minimal() {
    let message = task_updated(json!({
        "type": "system",
        "subtype": "task_updated",
        "task_id": "b1m21w89v",
        "patch": {"status": "completed", "end_time": 1780405729183_i64},
    }));
    assert_eq!(message.task_id, "b1m21w89v");
    assert_eq!(message.status, Some(TaskUpdatedStatus::Completed));
    assert_eq!(message.uuid, None);
    assert_eq!(message.session_id, None);
}

#[test]
fn parse_task_updated_message_non_terminal_statuses() {
    for status in ["pending", "running", "paused"] {
        let message = task_updated(json!({
            "type": "system",
            "subtype": "task_updated",
            "task_id": "task-abc",
            "patch": {"status": status},
        }));
        let parsed = message.status.expect("status");
        assert_eq!(parsed, status);
        assert!(!parsed.is_terminal());
        assert!(!is_terminal_task_status(status));
    }
}

#[test]
fn parse_task_updated_message_no_patch() {
    let message = task_updated(json!({
        "type": "system",
        "subtype": "task_updated",
        "task_id": "task-abc",
    }));
    assert!(message.patch.is_empty());
    assert_eq!(message.status, None);
}

#[test]
fn parse_task_updated_message_missing_task_id_becomes_empty() {
    let data = json!({"type": "system", "subtype": "task_updated"});
    let message = task_updated(data.clone());
    assert_eq!(message.task_id, "");
    assert!(message.patch.is_empty());
    assert_eq!(message.status, None);
    assert_eq!(message.data, data);
}

#[test]
fn parse_task_updated_message_patch_without_status() {
    let message = task_updated(json!({
        "type": "system",
        "subtype": "task_updated",
        "task_id": "task-abc",
        "patch": {"end_time": 1780405729183_i64},
    }));
    assert_eq!(
        Value::Object(message.patch),
        json!({"end_time": 1780405729183_i64})
    );
    assert_eq!(message.status, None);
}

#[test]
fn parse_task_updated_message_non_dict_patch() {
    for patch in [
        json!("completed"),
        json!(["completed"]),
        json!(42),
        json!(null),
    ] {
        let message = task_updated(json!({
            "type": "system",
            "subtype": "task_updated",
            "task_id": "task-abc",
            "patch": patch,
        }));
        assert!(message.patch.is_empty(), "{patch}");
        assert_eq!(message.status, None);
    }
}

#[test]
fn parse_task_updated_message_terminal_statuses() {
    for status in ["completed", "failed", "killed"] {
        let message = task_updated(json!({
            "type": "system",
            "subtype": "task_updated",
            "task_id": "task-abc",
            "patch": {"status": status},
        }));
        let parsed = message.status.expect("status");
        assert_eq!(parsed, status);
        assert!(parsed.is_terminal());
        assert!(TERMINAL_TASK_STATUSES.contains(&status));
    }
}

#[test]
fn parse_task_updated_killed_is_terminal() {
    let message = task_updated(json!({
        "type": "system",
        "subtype": "task_updated",
        "task_id": "bs2r8eew4",
        "patch": {"status": "killed", "end_time": 1780405729183_i64},
    }));
    assert_eq!(message.status, Some(TaskUpdatedStatus::Killed));
    assert!(message.status.unwrap().is_terminal());
}

#[test]
fn terminal_task_statuses_are_completed_failed_stopped_killed() {
    let mut statuses = TERMINAL_TASK_STATUSES.to_vec();
    statuses.sort_unstable();
    assert_eq!(statuses, ["completed", "failed", "killed", "stopped"]);
}

#[test]
fn typed_system_messages_keep_subtype_and_raw_data() {
    let frames = [
        json!({"type": "system", "subtype": "task_updated", "task_id": "t1", "patch": {"status": "failed"}, "uuid": "u1", "session_id": "s1"}),
        json!({"type": "system", "subtype": "task_started", "task_id": "t1", "description": "desc", "uuid": "u1", "session_id": "s1"}),
        json!({"type": "system", "subtype": "task_progress", "task_id": "t1", "description": "desc", "usage": {"total_tokens": 1, "tool_uses": 0, "duration_ms": 10}, "uuid": "u2", "session_id": "s1"}),
        json!({"type": "system", "subtype": "task_notification", "task_id": "t1", "status": "stopped", "output_file": "/o", "summary": "s", "uuid": "u3", "session_id": "s1"}),
        json!({"type": "system", "subtype": "mirror_error", "error": "boom"}),
        json!({"type": "system", "subtype": "hook_started", "hook_event": "Stop"}),
        json!({"type": "system", "subtype": "some_future_subtype", "foo": "bar"}),
    ];
    for frame in frames {
        let message = system(frame.clone());
        assert_eq!(message.subtype(), frame["subtype"].as_str().unwrap());
        assert_eq!(message.data(), &frame);
    }
}

#[test]
fn unknown_system_subtype_yields_generic() {
    let data = json!({"type": "system", "subtype": "some_future_subtype", "foo": "bar"});
    let SystemMessage::Other(message) = system(data.clone()) else {
        panic!("expected a generic system message");
    };
    assert_eq!(message.subtype, "some_future_subtype");
    assert_eq!(message.data, data);
}

#[test]
fn parse_mirror_error_message() {
    let SystemMessage::MirrorError(message) = system(json!({
        "type": "system",
        "subtype": "mirror_error",
        "key": {"project_key": "proj", "session_id": "sess", "subpath": "subagents/agent-1"},
        "error": "store unavailable",
    })) else {
        panic!("expected a mirror_error message");
    };
    assert_eq!(
        message.key,
        Some(SessionKey::new("proj", "sess").with_subpath("subagents/agent-1"))
    );
    assert_eq!(message.error, "store unavailable");

    let SystemMessage::MirrorError(message) =
        system(json!({"type": "system", "subtype": "mirror_error"}))
    else {
        panic!("expected a mirror_error message");
    };
    assert_eq!(message.key, None);
    assert_eq!(message.error, "");
}

#[test]
fn parse_assistant_message_inside_subagent() {
    let message = assistant(json!({
        "type": "assistant",
        "message": {
            "content": [
                {"type": "text", "text": "Hello"},
                {"type": "tool_use", "id": "tool_123", "name": "Read", "input": {"file_path": "/test.txt"}},
            ],
            "model": "claude-opus-4-1-20250805",
        },
        "parent_tool_use_id": "toolu_01Xrwd5Y13sEHtzScxR77So8",
    }));
    assert_eq!(
        message.parent_tool_use_id.as_deref(),
        Some("toolu_01Xrwd5Y13sEHtzScxR77So8")
    );
}

#[test]
fn parse_valid_result_message() {
    let message = result(base_result());
    assert_eq!(message.subtype, "success");
    assert_eq!(message.duration_ms, 1000);
    assert_eq!(message.duration_api_ms, 500);
    assert!(!message.is_error);
    assert_eq!(message.num_turns, 2);
    assert_eq!(message.session_id, "session_123");
    assert_eq!(message.stop_reason, None);
}

#[test]
fn parse_result_message_with_stop_reason() {
    let message = result(with(
        base_result(),
        json!({"stop_reason": "end_turn", "result": "Done"}),
    ));
    assert_eq!(message.stop_reason.as_deref(), Some("end_turn"));
    assert_eq!(message.result.as_deref(), Some("Done"));
}

#[test]
fn parse_result_message_with_null_stop_reason() {
    let message = result(with(
        base_result(),
        json!({"subtype": "error_max_turns", "is_error": true, "num_turns": 10, "stop_reason": null}),
    ));
    assert_eq!(message.stop_reason, None);
    assert!(message.is_error);
}

#[test]
fn parse_result_message_with_terminal_reason() {
    let message = result(with(
        base_result(),
        json!({"result": "", "terminal_reason": "aborted_tools"}),
    ));
    assert_eq!(message.terminal_reason.as_deref(), Some("aborted_tools"));
}

#[test]
fn parse_result_message_origin() {
    assert_eq!(result(base_result()).origin, None);

    let message = result(with(base_result(), json!({"origin": {"kind": "human"}})));
    let origin = message.origin.expect("origin");
    assert_eq!(origin.kind, MessageOriginKind::Human);
    assert_eq!(Value::Object(origin.raw), json!({"kind": "human"}));

    for (subkind, expected) in [
        (
            "scheduled-trigger",
            TaskNotificationOriginSubkind::ScheduledTrigger,
        ),
        (
            "peer-send-message",
            TaskNotificationOriginSubkind::PeerSendMessage,
        ),
    ] {
        let wire = json!({"kind": "task-notification", "subkind": subkind});
        let message = result(with(base_result(), json!({"origin": wire})));
        let origin = message.origin.expect("origin");
        assert_eq!(origin.kind, MessageOriginKind::TaskNotification);
        assert_eq!(origin.subkind(), Some(expected));
        assert_eq!(Value::Object(origin.raw), wire);
    }

    let message = result(with(
        base_result(),
        json!({"origin": {"kind": "unclassified"}}),
    ));
    assert_eq!(
        message.origin.expect("origin").kind,
        MessageOriginKind::Unclassified
    );

    let message = result(with(
        base_result(),
        json!({"origin": {"kind": "from-the-future"}}),
    ));
    assert_eq!(
        message.origin.expect("origin").kind,
        MessageOriginKind::Unknown("from-the-future".to_owned())
    );
}

#[test]
fn parse_user_message_uuids() {
    let frames = [
        json!({"type": "assistant", "message": {"content": [], "model": "claude-opus-4-6"}}),
        json!({"type": "stream_event", "uuid": "event-1", "session_id": "session_123", "event": {"type": "message_start"}}),
        base_result(),
        with(
            base_result(),
            json!({"subtype": "error_during_execution", "is_error": true}),
        ),
    ];
    let cases: [(Option<&str>, Option<Vec<&str>>); 3] = [
        (None, None),
        (Some("user-1"), None),
        (Some("user-2"), Some(vec!["user-1", "user-2"])),
    ];
    for frame in &frames {
        for (uuid, uuids) in &cases {
            let mut data = frame.clone();
            if let Some(uuid) = uuid {
                data["user_message_uuid"] = json!(uuid);
            }
            if let Some(uuids) = uuids {
                data["user_message_uuids"] = json!(uuids);
            }
            let (parsed_uuid, parsed_uuids) = match parse_message(data).unwrap() {
                Message::Assistant(message) => {
                    (message.user_message_uuid, message.user_message_uuids)
                }
                Message::StreamEvent(message) => {
                    (message.user_message_uuid, message.user_message_uuids)
                }
                Message::Result(message) => (message.user_message_uuid, message.user_message_uuids),
                other => panic!("unexpected message {other:?}"),
            };
            assert_eq!(parsed_uuid.as_deref(), *uuid);
            let expected: Option<Vec<String>> = uuids
                .as_ref()
                .map(|uuids| uuids.iter().map(ToString::to_string).collect());
            assert_eq!(parsed_uuids, expected);
        }
    }
}

#[test]
fn parse_result_message_empty_user_message_uuids() {
    let message = result(with(
        base_result(),
        json!({"subtype": "error_during_execution", "is_error": true, "user_message_uuids": []}),
    ));
    assert_eq!(message.user_message_uuid, None);
    assert_eq!(message.user_message_uuids, Some(Vec::new()));
}

#[test]
fn parse_result_message_missing_terminal_reason_is_none() {
    assert_eq!(result(base_result()).terminal_reason, None);
}

#[test]
fn parse_rate_limit_event() {
    let message = rate_limit(json!({
        "type": "rate_limit_event",
        "rate_limit_info": {
            "status": "allowed_warning",
            "resetsAt": 1700000000,
            "rateLimitType": "five_hour",
            "utilization": 0.91,
        },
        "uuid": "abc-123",
        "session_id": "session_xyz",
    }));
    assert_eq!(message.uuid, "abc-123");
    assert_eq!(message.session_id, "session_xyz");
    assert_eq!(
        message.rate_limit_info.status,
        RateLimitStatus::AllowedWarning
    );
    assert_eq!(message.rate_limit_info.resets_at, Some(1700000000));
    assert_eq!(
        message.rate_limit_info.rate_limit_type,
        Some(RateLimitType::FiveHour)
    );
    assert_eq!(message.rate_limit_info.utilization, Some(0.91));
}

#[test]
fn rate_limit_event_parsed_as_typed_message() {
    let info = json!({
        "status": "allowed_warning",
        "resetsAt": 1700000000,
        "rateLimitType": "five_hour",
        "utilization": 0.85,
        "isUsingOverage": false,
    });
    let message = rate_limit(json!({
        "type": "rate_limit_event",
        "rate_limit_info": info,
        "uuid": "550e8400-e29b-41d4-a716-446655440000",
        "session_id": "test-session-id",
    }));
    assert_eq!(message.uuid, "550e8400-e29b-41d4-a716-446655440000");
    assert_eq!(message.session_id, "test-session-id");
    let parsed = message.rate_limit_info;
    assert_eq!(parsed.status, "allowed_warning");
    assert_eq!(parsed.resets_at, Some(1700000000));
    assert_eq!(parsed.rate_limit_type, Some(RateLimitType::FiveHour));
    assert_eq!(parsed.utilization, Some(0.85));
    assert_eq!(parsed.raw["isUsingOverage"], false);
    assert_eq!(parsed.raw, info);
}

#[test]
fn rate_limit_event_rejected_parsed() {
    let message = rate_limit(json!({
        "type": "rate_limit_event",
        "rate_limit_info": {
            "status": "rejected",
            "resetsAt": 1700003600,
            "rateLimitType": "seven_day",
            "isUsingOverage": false,
            "overageStatus": "rejected",
            "overageResetsAt": 1700007200,
            "overageDisabledReason": "out_of_credits",
        },
        "uuid": "660e8400-e29b-41d4-a716-446655440001",
        "session_id": "test-session-id",
    }));
    let info = message.rate_limit_info;
    assert_eq!(info.status, RateLimitStatus::Rejected);
    assert_eq!(info.rate_limit_type, Some(RateLimitType::SevenDay));
    assert_eq!(info.overage_status, Some(RateLimitStatus::Rejected));
    assert_eq!(info.overage_resets_at, Some(1700007200));
    assert_eq!(
        info.overage_disabled_reason.as_deref(),
        Some("out_of_credits")
    );
}

#[test]
fn rate_limit_event_minimal_fields() {
    let message = rate_limit(json!({
        "type": "rate_limit_event",
        "rate_limit_info": {"status": "allowed"},
        "uuid": "770e8400-e29b-41d4-a716-446655440002",
        "session_id": "test-session-id",
    }));
    let info = message.rate_limit_info;
    assert_eq!(info.status, RateLimitStatus::Allowed);
    assert_eq!(info.resets_at, None);
    assert_eq!(info.rate_limit_type, None);
    assert_eq!(info.utilization, None);
    assert_eq!(info.overage_status, None);
    assert_eq!(info.overage_resets_at, None);
    assert_eq!(info.overage_disabled_reason, None);
}

#[test]
fn rate_limit_event_missing_fields() {
    let cases = [
        (
            json!({"type": "rate_limit_event", "uuid": "u", "session_id": "s"}),
            "rate_limit_info",
        ),
        (
            json!({"type": "rate_limit_event", "rate_limit_info": {}, "uuid": "u", "session_id": "s"}),
            "status",
        ),
        (
            json!({"type": "rate_limit_event", "rate_limit_info": {"status": "allowed"}, "session_id": "s"}),
            "uuid",
        ),
        (
            json!({"type": "rate_limit_event", "rate_limit_info": {"status": "allowed"}, "uuid": "u"}),
            "session_id",
        ),
    ];
    for (data, field) in cases {
        let (message, _) = parse_error(data);
        assert_eq!(
            message,
            format!("Missing required field in rate_limit_event message: '{field}'")
        );
    }
}

#[test]
fn parse_stream_event_requires_uuid_session_id_and_event() {
    let full = json!({
        "type": "stream_event",
        "uuid": "event-1",
        "session_id": "session_123",
        "event": {"type": "message_start"},
        "parent_tool_use_id": "toolu_1",
    });
    let message = stream_event(full.clone());
    assert_eq!(message.uuid, "event-1");
    assert_eq!(message.session_id, "session_123");
    assert_eq!(message.event, json!({"type": "message_start"}));
    assert_eq!(message.parent_tool_use_id.as_deref(), Some("toolu_1"));

    for field in ["uuid", "session_id", "event"] {
        let mut data = full.clone();
        data.as_object_mut().unwrap().remove(field);
        let (message, _) = parse_error(data);
        assert_eq!(
            message,
            format!("Missing required field in stream_event message: '{field}'")
        );
    }
}

#[test]
fn parse_conversation_reset() {
    let message = parse_message(json!({
        "type": "conversation_reset",
        "new_conversation_id": "d2f4a573-ca99-42a2-bb7a-905b40c908e8",
        "uuid": "msg-1",
        "session_id": "66694129-ce74-4ee1-9b0f-994155ac97ba",
    }))
    .unwrap();
    let Message::ConversationReset(message) = message else {
        panic!("expected a conversation reset");
    };
    assert_eq!(
        message.new_conversation_id,
        "d2f4a573-ca99-42a2-bb7a-905b40c908e8"
    );
    assert_eq!(message.uuid, "msg-1");
    assert_eq!(message.session_id, "66694129-ce74-4ee1-9b0f-994155ac97ba");
}

#[test]
fn parse_conversation_reset_missing_field() {
    let full = json!({
        "type": "conversation_reset",
        "new_conversation_id": "c",
        "uuid": "u",
        "session_id": "s",
    });
    for field in ["new_conversation_id", "uuid", "session_id"] {
        let mut data = full.clone();
        data.as_object_mut().unwrap().remove(field);
        let (message, _) = parse_error(data);
        assert_eq!(
            message,
            format!("Missing required field in conversation_reset message: '{field}'")
        );
    }
}

#[test]
fn parse_invalid_data_type() {
    let (message, data) = parse_error(json!("not a dict"));
    assert!(message.contains("Invalid message data type"));
    assert!(message.contains("expected dict, got str"));
    assert_eq!(data, Some(json!("not a dict")));
}

#[test]
fn parse_missing_type_field() {
    let (message, _) = parse_error(json!({"message": {"content": []}}));
    assert!(message.contains("Message missing 'type' field"));

    for empty_type in [json!(null), json!(""), json!(0), json!(false)] {
        let (message, _) = parse_error(json!({"type": empty_type}));
        assert_eq!(message, "Message missing 'type' field");
    }
}

#[test]
fn parse_unknown_message_type_is_kept_as_raw_json() {
    let data = json!({"type": "unknown_type"});
    assert_eq!(parse_message(data.clone()).unwrap(), Message::Unknown(data));

    let data = json!({
        "type": "some_future_event_type",
        "uuid": "880e8400-e29b-41d4-a716-446655440003",
        "session_id": "test-session-id",
    });
    assert_eq!(parse_message(data.clone()).unwrap(), Message::Unknown(data));

    let data = json!({"type": 7});
    assert_eq!(parse_message(data.clone()).unwrap(), Message::Unknown(data));
}

#[test]
fn known_message_types_still_parsed() {
    let message = assistant(json!({
        "type": "assistant",
        "message": {
            "content": [{"type": "text", "text": "hello"}],
            "model": "claude-sonnet-4-6-20250929",
        },
    }));
    assert_eq!(message.content[0].as_text(), Some("hello"));
}

#[test]
fn parse_user_message_missing_fields() {
    let (message, _) = parse_error(json!({"type": "user"}));
    assert_eq!(message, "Missing required field in user message: 'message'");

    let (message, _) = parse_error(json!({"type": "user", "message": {}}));
    assert_eq!(message, "Missing required field in user message: 'content'");
}

#[test]
fn parse_assistant_message_missing_fields() {
    let (message, _) = parse_error(json!({"type": "assistant"}));
    assert_eq!(
        message,
        "Missing required field in assistant message: 'message'"
    );

    let (message, _) = parse_error(json!({"type": "assistant", "message": {"content": []}}));
    assert_eq!(
        message,
        "Missing required field in assistant message: 'model'"
    );
}

#[test]
fn parse_assistant_non_list_content_is_an_error() {
    let (message, _) = parse_error(json!({
        "type": "assistant",
        "message": {"model": "m", "content": "hi"},
    }));
    assert_eq!(
        message,
        "Invalid assistant content (expected list, got str)"
    );
}

#[test]
fn non_dict_content_block_is_an_error() {
    for role in ["assistant", "user"] {
        let (message, _) = parse_error(json!({
            "type": role,
            "message": {"content": ["oops"], "model": "m"},
        }));
        assert_eq!(message, "Invalid content block (expected dict, got str)");
    }
}

#[test]
fn content_block_missing_required_field_is_an_error() {
    let cases = [
        (json!({"text": "no type"}), "type"),
        (json!({"type": "text"}), "text"),
        (json!({"type": "thinking", "thinking": "t"}), "signature"),
        (json!({"type": "tool_use", "id": "i", "name": "n"}), "input"),
        (json!({"type": "tool_result"}), "tool_use_id"),
        (
            json!({"type": "server_tool_use", "name": "advisor", "input": {}}),
            "id",
        ),
        (
            json!({"type": "advisor_tool_result", "tool_use_id": "t"}),
            "content",
        ),
    ];
    for (block, field) in cases {
        let (message, _) = parse_error(json!({
            "type": "assistant",
            "message": {"content": [block], "model": "m"},
        }));
        assert_eq!(
            message,
            format!("Missing required field in assistant message: '{field}'")
        );
    }
}

#[test]
fn required_field_of_the_wrong_type_is_an_error() {
    let (message, _) = parse_error(with(base_result(), json!({"duration_ms": "fast"})));
    assert_eq!(
        message,
        "Invalid field in result message: 'duration_ms' (expected int, got str)"
    );

    let (message, _) = parse_error(json!({
        "type": "assistant",
        "message": {"content": [], "model": 5},
    }));
    assert_eq!(
        message,
        "Invalid field in assistant message: 'model' (expected str, got int)"
    );
}

#[test]
fn user_content_that_is_neither_text_nor_a_list_is_an_error() {
    for (content, kind) in [(json!(7), "int"), (json!({"text": "hi"}), "dict")] {
        let (message, _) = parse_error(json!({
            "type": "user",
            "message": {"role": "user", "content": content},
        }));
        assert_eq!(
            message,
            format!("Invalid user content (expected str or list, got {kind})")
        );
    }
}

#[test]
fn integer_fields_accept_any_json_number() {
    let message = result(with(
        base_result(),
        json!({
            "duration_ms": 1234.9,
            "duration_api_ms": -5,
            "num_turns": 3.0,
            "api_error_status": 529.0,
            "modelUsage": {"m": {"inputTokens": 10.5, "outputTokens": -1}},
        }),
    ));
    assert_eq!(message.duration_ms, 1234);
    assert_eq!(message.duration_api_ms, 0);
    assert_eq!(message.num_turns, 3);
    assert_eq!(message.api_error_status, Some(529));
    let usage = &message.model_usage.expect("model_usage")["m"];
    assert_eq!(usage.input_tokens, 10);
    assert_eq!(usage.output_tokens, 0);

    let event = rate_limit(json!({
        "type": "rate_limit_event",
        "rate_limit_info": {"status": "allowed", "resetsAt": 1700000000.5},
        "uuid": "event-1",
        "session_id": "session-1",
    }));
    assert_eq!(event.rate_limit_info.resets_at, Some(1_700_000_000));

    let (message, _) = parse_error(with(base_result(), json!({"num_turns": true})));
    assert_eq!(
        message,
        "Invalid field in result message: 'num_turns' (expected int, got bool)"
    );
}

#[test]
fn optional_field_of_the_wrong_type_reads_as_absent() {
    let message = assistant(json!({
        "type": "assistant",
        "message": {"content": [], "model": "m", "id": 12, "stop_reason": ["x"]},
        "uuid": 5,
        "session_id": {"a": 1},
        "user_message_uuids": "not-a-list",
    }));
    assert_eq!(message.message_id, None);
    assert_eq!(message.stop_reason, None);
    assert_eq!(message.uuid, None);
    assert_eq!(message.session_id, None);
    assert_eq!(message.user_message_uuids, None);
}

#[test]
fn parse_system_message_missing_fields() {
    let (message, _) = parse_error(json!({"type": "system"}));
    assert_eq!(
        message,
        "Missing required field in system message: 'subtype'"
    );
}

#[test]
fn parse_result_message_missing_fields() {
    let (message, _) = parse_error(json!({"type": "result", "subtype": "success"}));
    assert!(message.contains("Missing required field in result message"));

    for field in [
        "subtype",
        "duration_ms",
        "duration_api_ms",
        "is_error",
        "num_turns",
        "session_id",
    ] {
        let mut data = base_result();
        data.as_object_mut().unwrap().remove(field);
        let (message, _) = parse_error(data);
        assert_eq!(
            message,
            format!("Missing required field in result message: '{field}'")
        );
    }
}

#[test]
fn message_parse_error_contains_data() {
    let data = json!({"type": "assistant"});
    let (_, carried) = parse_error(data.clone());
    assert_eq!(carried, Some(data));
}

#[test]
fn parse_assistant_message_without_error() {
    let message = assistant(json!({
        "type": "assistant",
        "message": {
            "content": [{"type": "text", "text": "Hello"}],
            "model": "claude-opus-4-5-20251101",
        },
    }));
    assert_eq!(message.error, None);
}

#[test]
fn parse_assistant_message_with_authentication_error() {
    let message = assistant(json!({
        "type": "assistant",
        "message": {
            "content": [{"type": "text", "text": "Invalid API key · Fix external API key"}],
            "model": "<synthetic>",
        },
        "session_id": "test-session",
        "error": "authentication_failed",
    }));
    assert_eq!(
        message.error,
        Some(AssistantMessageError::AuthenticationFailed)
    );
    assert_eq!(message.content.len(), 1);
    assert!(matches!(message.content[0], ContentBlock::Text(_)));
}

#[test]
fn parse_assistant_message_with_unknown_error() {
    let message = assistant(json!({
        "type": "assistant",
        "message": {
            "content": [
                {
                    "type": "text",
                    "text": "API Error: 500 {\"type\":\"error\",\"error\":{\"type\":\"api_error\",\"message\":\"Internal server error\"}}",
                }
            ],
            "model": "<synthetic>",
        },
        "session_id": "test-session",
        "error": "unknown",
    }));
    assert_eq!(message.error, Some(AssistantMessageError::UnknownError));
    assert_eq!(message.error.unwrap(), "unknown");
}

#[test]
fn parse_assistant_message_with_rate_limit_error() {
    let message = assistant(json!({
        "type": "assistant",
        "message": {
            "content": [{"type": "text", "text": "Rate limit exceeded"}],
            "model": "<synthetic>",
        },
        "error": "rate_limit",
    }));
    assert_eq!(message.error, Some(AssistantMessageError::RateLimit));
}

#[test]
fn parse_assistant_message_with_all_fields() {
    let message = assistant(json!({
        "type": "assistant",
        "message": {
            "content": [{"type": "text", "text": "Hello"}],
            "model": "claude-sonnet-4-5-20250929",
            "id": "msg_01HRq7YZE3apPqSHydvG77Ve",
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 10, "output_tokens": 5},
        },
        "session_id": "fdf2d90a-fd9e-4736-ae35-806edd13643f",
        "uuid": "0dbd2453-1209-4fe9-bd51-4102f64e33df",
        "parent_tool_use_id": "toolu_parent",
    }));
    assert_eq!(
        message.message_id.as_deref(),
        Some("msg_01HRq7YZE3apPqSHydvG77Ve")
    );
    assert_eq!(message.stop_reason.as_deref(), Some("end_turn"));
    assert_eq!(
        message.session_id.as_deref(),
        Some("fdf2d90a-fd9e-4736-ae35-806edd13643f")
    );
    assert_eq!(
        message.uuid.as_deref(),
        Some("0dbd2453-1209-4fe9-bd51-4102f64e33df")
    );
    assert_eq!(
        message.usage,
        Some(json!({"input_tokens": 10, "output_tokens": 5}))
    );
    assert_eq!(message.parent_tool_use_id.as_deref(), Some("toolu_parent"));
}

#[test]
fn parse_assistant_message_optional_fields_absent() {
    let message = assistant(json!({
        "type": "assistant",
        "message": {"content": [{"type": "text", "text": "hi"}], "model": "claude-opus-4-5"},
    }));
    assert_eq!(message.message_id, None);
    assert_eq!(message.stop_reason, None);
    assert_eq!(message.session_id, None);
    assert_eq!(message.uuid, None);
    assert_eq!(message.usage, None);
    assert_eq!(message.error, None);
    assert_eq!(message.parent_tool_use_id, None);
}

#[test]
fn parse_result_message_with_model_usage() {
    let message = result(json!({
        "type": "result",
        "subtype": "success",
        "duration_ms": 3000,
        "duration_api_ms": 2000,
        "is_error": false,
        "num_turns": 1,
        "session_id": "fdf2d90a-fd9e-4736-ae35-806edd13643f",
        "stop_reason": "end_turn",
        "total_cost_usd": 0.0106,
        "usage": {"input_tokens": 3, "output_tokens": 24},
        "result": "Hello",
        "modelUsage": {
            "claude-sonnet-4-5-20250929": {
                "inputTokens": 3,
                "outputTokens": 24,
                "cacheReadInputTokens": 20012,
                "costUSD": 0.0106,
                "contextWindow": 200000,
                "maxOutputTokens": 64000,
            }
        },
        "permission_denials": [],
        "uuid": "d379c496-f33a-4ea4-b920-3c5483baa6f7",
    }));
    let model_usage = message.model_usage.expect("model_usage");
    let usage = &model_usage["claude-sonnet-4-5-20250929"];
    assert_eq!(usage.cost_usd, 0.0106);
    assert_eq!(usage.input_tokens, 3);
    assert_eq!(usage.output_tokens, 24);
    assert_eq!(usage.cache_read_input_tokens, 20012);
    assert_eq!(usage.cache_creation_input_tokens, 0);
    assert_eq!(usage.context_window, 200000);
    assert_eq!(usage.max_output_tokens, 64000);
    assert_eq!(message.permission_denials, Some(Vec::new()));
    assert_eq!(
        message.uuid.as_deref(),
        Some("d379c496-f33a-4ea4-b920-3c5483baa6f7")
    );
    assert_eq!(message.total_cost_usd, Some(0.0106));
    assert_eq!(
        message.usage,
        Some(json!({"input_tokens": 3, "output_tokens": 24}))
    );
}

#[test]
fn parse_result_message_optional_fields_absent() {
    let message = result(base_result());
    assert_eq!(message.model_usage, None);
    assert_eq!(message.permission_denials, None);
    assert_eq!(message.deferred_tool_use, None);
    assert_eq!(message.errors, None);
    assert_eq!(message.api_error_status, None);
    assert_eq!(message.uuid, None);
    assert_eq!(message.total_cost_usd, None);
    assert_eq!(message.usage, None);
    assert_eq!(message.result, None);
    assert_eq!(message.structured_output, None);
}

#[test]
fn parse_result_message_with_deferred_tool_use() {
    let message = result(with(
        base_result(),
        json!({
            "deferred_tool_use": {
                "id": "toolu_01abc",
                "name": "Bash",
                "input": {"command": "rm -rf /tmp/scratch"},
            },
        }),
    ));
    let deferred = message.deferred_tool_use.expect("deferred_tool_use");
    assert_eq!(deferred.id, "toolu_01abc");
    assert_eq!(deferred.name, "Bash");
    assert_eq!(deferred.input, json!({"command": "rm -rf /tmp/scratch"}));

    let (message, _) = parse_error(with(
        base_result(),
        json!({"deferred_tool_use": {"id": "toolu_01abc", "input": {}}}),
    ));
    assert_eq!(message, "Missing required field in result message: 'name'");
}

#[test]
fn parse_result_message_with_structured_output() {
    let message = result(with(
        base_result(),
        json!({"structured_output": {"answer": 42}}),
    ));
    assert_eq!(message.structured_output, Some(json!({"answer": 42})));
}

#[test]
fn parse_result_message_with_errors() {
    let message = result(json!({
        "type": "result",
        "subtype": "error_during_execution",
        "duration_ms": 5000,
        "duration_api_ms": 3000,
        "is_error": true,
        "num_turns": 3,
        "session_id": "session_456",
        "errors": [
            "Tool execution failed: permission denied",
            "Unable to write to /etc/hosts",
        ],
        "uuid": "err-uuid-789",
    }));
    assert_eq!(
        message.errors,
        Some(vec![
            "Tool execution failed: permission denied".to_owned(),
            "Unable to write to /etc/hosts".to_owned(),
        ])
    );
    assert!(message.is_error);
    assert_eq!(message.subtype, "error_during_execution");
    assert_eq!(message.uuid.as_deref(), Some("err-uuid-789"));
}

#[test]
fn parse_result_message_with_api_error_status() {
    let message = result(with(
        base_result(),
        json!({"is_error": true, "api_error_status": 529}),
    ));
    assert_eq!(message.api_error_status, Some(529));
    assert!(message.is_error);
    assert_eq!(message.subtype, "success");
}

#[test]
fn parse_result_message_success_no_errors() {
    let message = result(with(
        base_result(),
        json!({"result": "Task completed successfully"}),
    ));
    assert_eq!(message.errors, None);
    assert_eq!(
        message.result.as_deref(),
        Some("Task completed successfully")
    );
}

#[test]
fn parse_hook_event_message() {
    let data = json!({
        "type": "system",
        "subtype": "hook_started",
        "hook_event": "PreToolUse",
        "hook_name": "PreToolUse",
        "session_id": "sess-123",
        "uuid": "uuid-456",
        "tool_name": "Bash",
        "tool_input": {"command": "ls"},
    });
    let SystemMessage::HookEvent(message) = system(data.clone()) else {
        panic!("expected a hook event");
    };
    assert_eq!(message.subtype, "hook_started");
    assert_eq!(message.hook_event_name, "PreToolUse");
    assert_eq!(message.session_id.as_deref(), Some("sess-123"));
    assert_eq!(message.uuid.as_deref(), Some("uuid-456"));
    assert_eq!(message.data, data);
}

#[test]
fn parse_hook_event_message_response() {
    let SystemMessage::HookEvent(message) = system(json!({
        "type": "system",
        "subtype": "hook_response",
        "hook_event": "PostToolUse",
        "hook_name": "PostToolUse",
        "session_id": "sess-123",
        "uuid": "uuid-789",
        "output": "",
        "exit_code": 0,
        "outcome": "success",
    })) else {
        panic!("expected a hook event");
    };
    assert_eq!(message.subtype, "hook_response");
    assert_eq!(message.hook_event_name, "PostToolUse");
    assert_eq!(message.session_id.as_deref(), Some("sess-123"));
    assert_eq!(message.uuid.as_deref(), Some("uuid-789"));
    assert_eq!(message.data["output"], "");
    assert_eq!(message.data["exit_code"], 0);
    assert_eq!(message.data["outcome"], "success");
}

#[test]
fn parse_hook_event_message_minimal() {
    let SystemMessage::HookEvent(message) =
        system(json!({"type": "system", "subtype": "hook_started", "hook_name": "Stop"}))
    else {
        panic!("expected a hook event");
    };
    assert_eq!(message.subtype, "hook_started");
    assert_eq!(message.hook_event_name, "Stop");
    assert_eq!(message.session_id, None);
    assert_eq!(message.uuid, None);
}

#[test]
fn hook_event_name_precedence_is_hook_event_then_hook_name_then_hook_event_name() {
    let name = |extra: Value| match system(with(
        json!({"type": "system", "subtype": "hook_response"}),
        extra,
    )) {
        SystemMessage::HookEvent(message) => message.hook_event_name,
        other => panic!("expected a hook event, got {other:?}"),
    };
    assert_eq!(
        name(json!({"hook_event": "A", "hook_name": "B", "hook_event_name": "C"})),
        "A"
    );
    assert_eq!(name(json!({"hook_name": "B", "hook_event_name": "C"})), "B");
    assert_eq!(name(json!({"hook_event_name": "C"})), "C");
    assert_eq!(name(json!({"hook_event": "", "hook_name": "B"})), "B");
    assert_eq!(name(json!({})), "");
}

#[test]
fn init_frame_parses_to_a_generic_system_message_with_a_typed_view() {
    let data = json!({
        "type": "system",
        "subtype": "init",
        "uuid": "u1",
        "session_id": "s1",
        "apiKeySource": "none",
        "claude_code_version": "2.1.284",
        "cwd": "/repo",
        "tools": ["Bash"],
        "mcp_servers": [{"name": "docs", "status": "connected"}],
        "model": "claude-sonnet-5",
        "permissionMode": "default",
        "slash_commands": ["compact"],
        "output_style": "default",
        "skills": [],
        "plugins": [],
    });
    let message = system(data.clone());
    assert!(matches!(message, SystemMessage::Other(_)));
    assert_eq!(message.data(), &data);

    let init = message.init_data().expect("init data");
    assert_eq!(init.uuid, "u1");
    assert_eq!(init.session_id, "s1");
    assert_eq!(init.api_key_source, "none");
    assert_eq!(init.claude_code_version, "2.1.284");
    assert_eq!(init.cwd, "/repo");
    assert_eq!(init.tools, ["Bash"]);
    assert_eq!(init.mcp_servers[0].name, "docs");
    assert_eq!(init.mcp_servers[0].status, "connected");
    assert_eq!(init.mcp_servers[0].source, None);
    assert_eq!(init.model, "claude-sonnet-5");
    assert_eq!(init.permission_mode, "default");
    assert_eq!(init.slash_commands, ["compact"]);
    assert_eq!(init.output_style, "default");
    assert!(init.skills.is_empty());
    assert!(init.plugins.is_empty());
    assert_eq!(init.capabilities, None);
    assert_eq!(init.agents, None);
    assert_eq!(init.fast_mode_state, None);

    assert!(
        system(json!({"type": "system", "subtype": "start"}))
            .init_data()
            .is_none()
    );
}

#[test]
fn init_frame_optional_fields_are_typed() {
    let message = system(json!({
        "type": "system",
        "subtype": "init",
        "plugins": [{"name": "p", "path": "/p", "version": "1.2.3"}],
        "plugin_errors": [{"plugin": "inline[0]", "type": "path-not-found", "message": "missing", "path": "/x"}],
        "fast_mode_state": "cooldown",
        "fast_mode_disabled_reason": "sdk_opt_in_required",
        "capabilities": ["interrupt_receipt_v1"],
        "terminal_slash_commands": ["exit"],
        "agents": ["general-purpose"],
        "betas": ["context-1m-2025-08-07"],
        "mcp_servers": [{"name": "docs", "status": "connected", "source": "plugin"}],
    }));
    let init = message.init_data().expect("init data");
    assert_eq!(init.plugins[0].version.as_deref(), Some("1.2.3"));
    let plugin_error = &init.plugin_errors.as_ref().unwrap()[0];
    assert_eq!(plugin_error.plugin, "inline[0]");
    assert_eq!(plugin_error.kind, "path-not-found");
    assert_eq!(plugin_error.path.as_deref(), Some("/x"));
    assert_eq!(init.fast_mode_state.unwrap(), "cooldown");
    assert_eq!(
        init.fast_mode_disabled_reason.unwrap(),
        "sdk_opt_in_required"
    );
    assert_eq!(init.capabilities.unwrap(), ["interrupt_receipt_v1"]);
    assert_eq!(init.terminal_slash_commands.unwrap(), ["exit"]);
    assert_eq!(init.agents.unwrap(), ["general-purpose"]);
    assert_eq!(init.mcp_servers[0].source.as_deref(), Some("plugin"));
}
