#![cfg(feature = "testing")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use claude_agent_rust_sdk::ErrorKind;
use claude_agent_rust_sdk::{
    AgentDefinition, AsyncHookOutput, ClaudeAgentOptions, ClaudeSdkClient, Error, HookCallback,
    HookEvent, HookFailureMode, HookInput, HookMatcher, HookOutput, Message, PermissionResult,
    PermissionRuleValue, PermissionUpdate, PermissionUpdateDestination, PermissionUpdateKind,
    Skills, SyncHookOutput, SystemPromptCustom, SystemPromptPreset, ToolPermissionContext,
    query_with_transport,
};
use futures_util::StreamExt;
use serde_json::{Value, json};

use common::capture::Warnings;
use common::mock::{
    Recorder, assistant, cli, cli_answering, cli_answering_only_initialize, connected, drain,
    hook_request, initialize_request, is_control_request, options_with_hook, passive_hook, refusal,
    requests, response_to, responses, result, result_texts, settle, success, written,
};

fn default_options() -> ClaudeAgentOptions {
    ClaudeAgentOptions::default()
}

fn answering_prompts() -> claude_agent_rust_sdk::testing::MockTransport {
    cli_answering(|frame| {
        if frame["type"] == "user" {
            vec![assistant("Hello!"), result("Hello!")]
        } else {
            Vec::new()
        }
    })
}

#[tokio::test(start_paused = true)]
async fn control_responses_complete_the_request_with_the_same_id() {
    let transport = cli_answering(|frame| {
        if is_control_request(frame, "mcp_status") {
            vec![success(
                frame,
                json!({"mcpServers": [{"name": "files", "status": "connected"}]}),
            )]
        } else if is_control_request(frame, "get_context_usage") {
            vec![success(
                frame,
                json!({"totalTokens": 42, "maxTokens": 200_000}),
            )]
        } else {
            Vec::new()
        }
    });
    let client = connected(&transport, default_options()).await;
    transport.push_frame(json!({
        "type": "control_response",
        "response": {"subtype": "success", "request_id": "req_999_deadbeef", "response": {"stray": true}},
    }));
    transport.push_frame(json!({"type": "control_response"}));

    let (status, usage) = tokio::join!(client.get_mcp_status(), client.get_context_usage());
    let status = status.unwrap();
    assert_eq!(status.mcp_servers.len(), 1);
    assert_eq!(status.mcp_servers[0].name, "files");
    assert_eq!(usage.unwrap().total_tokens, 42);

    client.disconnect().await.unwrap();
    let leaked: Vec<_> = client.receive_messages().collect().await;
    assert_eq!(leaked.len(), 1);
    assert!(
        leaked[0]
            .as_ref()
            .err()
            .is_some_and(Error::is_cli_connection)
    );
}

#[tokio::test(start_paused = true)]
async fn outgoing_control_requests_use_the_upstream_envelope() {
    let transport = cli();
    let client = connected(&transport, default_options()).await;
    client.interrupt().await.unwrap();
    client.set_model(Some("claude-opus-5")).await.unwrap();

    let frames = written(&transport, "control_request");
    assert_eq!(frames.len(), 3);
    for (index, frame) in frames.iter().enumerate() {
        let object = frame.as_object().unwrap();
        let mut keys: Vec<&str> = object.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(keys, ["request", "request_id", "type"]);

        let request_id = frame["request_id"].as_str().unwrap();
        let parts: Vec<&str> = request_id.split('_').collect();
        assert_eq!(parts.len(), 3, "{request_id}");
        assert_eq!(parts[0], "req");
        assert_eq!(parts[1], (index + 1).to_string());
        assert_eq!(parts[2].len(), 8);
        assert!(
            parts[2]
                .chars()
                .all(|digit| digit.is_ascii_hexdigit() && !digit.is_ascii_uppercase())
        );
    }
    assert_eq!(frames[0]["request"]["subtype"], "initialize");
    assert_eq!(frames[1]["request"], json!({"subtype": "interrupt"}));
    assert_eq!(
        frames[2]["request"],
        json!({"subtype": "set_model", "model": "claude-opus-5"})
    );
    let ids: Vec<&str> = frames
        .iter()
        .map(|frame| frame["request_id"].as_str().unwrap())
        .collect();
    assert_ne!(ids[1].split('_').nth(2), ids[2].split('_').nth(2));
}

#[tokio::test(start_paused = true)]
async fn control_request_without_response_times_out_after_sixty_seconds() {
    let transport = cli_answering_only_initialize();
    let client = connected(&transport, default_options()).await;

    let started = tokio::time::Instant::now();
    let error = client.interrupt().await.unwrap_err();
    assert_eq!(started.elapsed(), Duration::from_secs(60));
    assert!(error.control_subtype() == Some("interrupt"));
    assert_eq!(error.to_string(), "Control request timeout: interrupt");

    let late = success(&requests(&transport, "interrupt")[0], json!({}));
    transport.push_frame(late);
    settle().await;
    transport.respond_with(|frame| vec![success(frame, json!({}))]);
    client.interrupt().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn initialize_without_response_times_out_and_closes_the_transport() {
    let transport = common::mock::silent_cli();
    let client = ClaudeSdkClient::with_transport(default_options(), transport.clone());

    let started = tokio::time::Instant::now();
    let error = client.connect().await.unwrap_err();
    assert_eq!(started.elapsed(), Duration::from_secs(60));
    assert!(error.control_subtype() == Some("initialize"));
    assert!(transport.is_closed());
    assert!(
        client
            .interrupt()
            .await
            .as_ref()
            .err()
            .is_some_and(Error::is_cli_connection)
    );
}

#[tokio::test(start_paused = true)]
async fn error_responses_fail_the_request_with_the_text_of_the_cli() {
    let transport = cli_answering(|frame| {
        if is_control_request(frame, "mcp_reconnect") {
            vec![refusal(frame, "Server not found: files")]
        } else if is_control_request(frame, "stop_task") {
            vec![json!({
                "type": "control_response",
                "response": {"subtype": "error", "request_id": frame["request_id"]},
            })]
        } else {
            Vec::new()
        }
    });
    let client = connected(&transport, default_options()).await;

    let error = client.reconnect_mcp_server("files").await.unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Control);
    assert_eq!(error.to_string(), "Server not found: files");

    let error = client.stop_task("task-1").await.unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Control);
    assert_eq!(error.to_string(), "Unknown error");

    client.interrupt().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn reader_error_fails_every_pending_request_with_that_error() {
    let transport = cli_answering_only_initialize();
    let client = connected(&transport, default_options()).await;

    let fail = async {
        settle().await;
        assert_eq!(written(&transport, "control_request").len(), 3);
        transport.push_error(Error::process("Command failed", Some(3), None));
    };
    let (interrupt, status, ()) = tokio::join!(client.interrupt(), client.get_mcp_status(), fail);
    for error in [interrupt.unwrap_err(), status.unwrap_err()] {
        assert_eq!(error.kind(), ErrorKind::Process);
        assert_eq!(error.exit_code(), Some(3));
        assert!(error.to_string().starts_with("Command failed"), "{error}");
    }

    let mut messages = client.receive_messages();
    let streamed = messages.next().await.unwrap().unwrap_err();
    assert_eq!(streamed.exit_code(), Some(3));
    assert!(messages.next().await.is_none());

    assert_eq!(requests(&transport, "set_model").len(), 0);
}

#[tokio::test(start_paused = true)]
async fn a_control_request_after_the_cli_died_is_a_connection_error() {
    let transport = cli_answering_only_initialize();
    let client = connected(&transport, default_options()).await;
    transport.push_error(Error::process("Command failed", Some(3), None));
    settle().await;

    let later = client.set_model(None).await.unwrap_err();
    assert_eq!(later.kind(), ErrorKind::ConnectionClosed);
    assert!(later.is_cli_connection());
    assert!(later.is_retryable());
    assert!(
        later
            .to_string()
            .starts_with("Cannot write to process that exited with error: Command failed"),
        "{later}"
    );
    let cause = std::error::Error::source(&later)
        .and_then(|source| source.downcast_ref::<Error>())
        .unwrap();
    assert_eq!(cause.exit_code(), Some(3));
    assert_eq!(requests(&transport, "set_model").len(), 0);
}

#[tokio::test(start_paused = true)]
async fn clean_end_of_output_fails_pending_requests_at_once() {
    let transport = cli_answering_only_initialize();
    let client = connected(&transport, default_options()).await;

    let finish = async {
        settle().await;
        transport.finish();
    };
    let started = tokio::time::Instant::now();
    let (interrupt, ()) = tokio::join!(client.interrupt(), finish);
    assert!(
        interrupt
            .as_ref()
            .err()
            .is_some_and(Error::is_cli_connection)
    );
    assert!(started.elapsed() < Duration::from_secs(1));
}

#[tokio::test(start_paused = true)]
async fn unsupported_control_request_gets_an_error_and_the_session_continues() {
    let transport = cli();
    let client = connected(&transport, default_options()).await;
    transport.push_frame(json!({
        "type": "control_request",
        "request_id": "cli-1",
        "request": {"subtype": "teleport", "where": "anywhere"},
    }));
    transport.push_frame(json!({
        "type": "control_request",
        "request_id": "cli-2",
        "request": {},
    }));
    settle().await;

    assert_eq!(
        written(&transport, "control_response")[0],
        json!({
            "type": "control_response",
            "response": {
                "subtype": "error",
                "request_id": "cli-1",
                "error": "Unsupported control request subtype: teleport",
            },
        })
    );
    assert_eq!(
        response_to(&transport, "cli-2").unwrap()["error"],
        "Unsupported control request subtype: null"
    );
    client.interrupt().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn cancel_request_aborts_the_handler_which_writes_no_response() {
    let started = Arc::new(AtomicBool::new(false));
    let dropped = Arc::new(AtomicBool::new(false));
    struct SetOnDrop(Arc<AtomicBool>);
    impl Drop for SetOnDrop {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }
    let blocking = {
        let (started, dropped) = (Arc::clone(&started), Arc::clone(&dropped));
        HookCallback::new(move |_input, _tool_use_id, _context| {
            let guard = SetOnDrop(Arc::clone(&dropped));
            started.store(true, Ordering::SeqCst);
            async move {
                let _guard = guard;
                std::future::pending::<()>().await;
                Ok(HookOutput::default())
            }
        })
    };
    let options = ClaudeAgentOptions::builder()
        .hook(
            "PreToolUse",
            HookMatcher::new().hook(blocking).hook(passive_hook()),
        )
        .build()
        .unwrap();
    let transport = cli();
    let client = connected(&transport, options).await;

    transport.push_frame(hook_request("hook-1", "hook_0"));
    settle().await;
    assert!(started.load(Ordering::SeqCst));
    assert!(!dropped.load(Ordering::SeqCst));

    transport.push_frame(json!({"type": "control_cancel_request", "request_id": "hook-1"}));
    transport.push_frame(json!({"type": "control_cancel_request", "request_id": "never-seen"}));
    transport.push_frame(json!({"type": "control_cancel_request"}));
    settle().await;
    assert!(dropped.load(Ordering::SeqCst));
    assert!(response_to(&transport, "hook-1").is_none());

    transport.push_frame(hook_request("hook-2", "hook_1"));
    settle().await;
    assert_eq!(
        response_to(&transport, "hook-2").unwrap()["subtype"],
        "success"
    );
    assert_eq!(responses(&transport).len(), 1);
    client.disconnect().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn full_queue_of_one_hundred_messages_blocks_the_reader() {
    let transport = cli();
    let client = connected(&transport, options_with_hook()).await;
    for index in 0..150 {
        transport.push_frame(json!({"type": "system", "subtype": "status", "index": index}));
    }
    transport.push_frame(hook_request("hook-1", "hook_0"));
    settle().await;

    assert_eq!(transport.pending_frames(), 50);
    assert!(response_to(&transport, "hook-1").is_none());

    let mut messages = client.receive_messages();
    for index in 0..150 {
        let Message::System(system) = messages.next().await.unwrap().unwrap() else {
            panic!("expected a system message");
        };
        assert_eq!(system.data()["index"], index);
    }
    settle().await;
    assert_eq!(transport.pending_frames(), 0);
    assert_eq!(
        response_to(&transport, "hook-1").unwrap()["subtype"],
        "success"
    );
}

#[tokio::test(start_paused = true)]
async fn control_and_mirror_frames_never_reach_the_message_stream() {
    let transport = cli_answering(|frame| {
        if frame["type"] == "user" {
            vec![
                json!({
                    "type": "transcript_mirror",
                    "filePath": "/tmp/projects/p/s.jsonl",
                    "entries": [{"type": "user", "uuid": "u1"}],
                }),
                json!({"type": "control_response", "response": {"request_id": "unknown"}}),
                json!({"type": "control_cancel_request", "request_id": "unknown"}),
                json!({
                    "type": "control_request",
                    "request_id": "cli-1",
                    "request": {"subtype": "can_use_tool", "tool_name": "Bash", "input": {}},
                }),
                assistant("Hello!"),
                result("Hello!"),
            ]
        } else {
            Vec::new()
        }
    });
    let messages = drain(query_with_transport("Hello", default_options(), transport)).await;

    assert_eq!(messages.len(), 2);
    assert!(matches!(messages[0], Ok(Message::Assistant(_))));
    assert!(matches!(messages[1], Ok(Message::Result(_))));
}

#[tokio::test(start_paused = true)]
async fn initialize_is_sent_before_the_prompt_with_every_configured_field() {
    let options = ClaudeAgentOptions::builder()
        .agent(
            "reviewer",
            AgentDefinition::new("Reviews code", "You review code.")
                .tools(["Read", "Grep"])
                .max_turns(3),
        )
        .system_prompt(
            SystemPromptPreset::new()
                .exclude_dynamic_sections(true)
                .snapshot(false),
        )
        .skills(Skills::list(["pdf", "docx"]))
        .forward_subagent_text(true)
        .build()
        .unwrap();
    let transport = answering_prompts();
    let messages = drain(query_with_transport("Hello", options, transport.clone())).await;
    assert_eq!(result_texts(&messages), ["Hello!"]);

    let frames = transport.written_frames();
    assert_eq!(frames[0]["type"], "control_request");
    assert_eq!(frames[1]["type"], "user");
    assert_eq!(
        frames[0]["request"],
        json!({
            "subtype": "initialize",
            "hooks": null,
            "agents": {
                "reviewer": {
                    "description": "Reviews code",
                    "prompt": "You review code.",
                    "tools": ["Read", "Grep"],
                    "maxTurns": 3,
                },
            },
            "excludeDynamicSections": true,
            "systemPromptSnapshot": false,
            "skills": ["pdf", "docx"],
            "forwardSubagentText": true,
        })
    );
}

#[tokio::test(start_paused = true)]
async fn initialize_leaves_out_what_is_not_set() {
    let bare = cli();
    connected(&bare, default_options()).await;
    assert_eq!(
        initialize_request(&bare),
        json!({"subtype": "initialize", "hooks": null})
    );

    let every_skill = cli();
    let options = ClaudeAgentOptions::builder()
        .skills(Skills::All)
        .system_prompt("You are terse.")
        .hooks([(HookEvent::Stop, Vec::new())])
        .build()
        .unwrap();
    connected(&every_skill, options).await;
    assert_eq!(
        initialize_request(&every_skill),
        json!({"subtype": "initialize", "hooks": null})
    );

    let custom = cli();
    let options = ClaudeAgentOptions::builder()
        .system_prompt(SystemPromptCustom::new("You are terse.").snapshot(true))
        .skills(Skills::list(Vec::<String>::new()))
        .build()
        .unwrap();
    connected(&custom, options).await;
    assert_eq!(
        initialize_request(&custom),
        json!({
            "subtype": "initialize",
            "hooks": null,
            "systemPromptSnapshot": true,
            "skills": [],
        })
    );
}

#[tokio::test(start_paused = true)]
async fn hooks_get_ids_from_one_counter_and_matchers_use_the_wire_shape() {
    let called = Recorder::<&'static str>::new();
    let named = |name: &'static str| {
        let called = called.clone();
        HookCallback::new(move |_input, _tool_use_id, _context| {
            called.push(name);
            async { Ok(HookOutput::default()) }
        })
    };
    let options = ClaudeAgentOptions::builder()
        .hook(
            "PreToolUse",
            HookMatcher::new()
                .matcher("Bash")
                .hook(named("first"))
                .hook(named("second"))
                .timeout(Duration::from_secs(30))
                .on_failure(HookFailureMode::Continue),
        )
        .hook("PreToolUse", HookMatcher::new().matcher("Write|Edit"))
        .hook(
            "PostToolUse",
            HookMatcher::new()
                .hook(named("third"))
                .timeout(Duration::from_millis(1500)),
        )
        .build()
        .unwrap();
    let transport = cli();
    let client = connected(&transport, options).await;

    assert_eq!(
        initialize_request(&transport)["hooks"],
        json!({
            "PreToolUse": [
                {
                    "matcher": "Bash",
                    "hookCallbackIds": ["hook_0", "hook_1"],
                    "timeout": 30,
                    "onFailure": "continue",
                },
                {"matcher": "Write|Edit", "hookCallbackIds": []},
            ],
            "PostToolUse": [
                {"matcher": null, "hookCallbackIds": ["hook_2"], "timeout": 1.5},
            ],
        })
    );

    for (request_id, callback_id) in [("h-3", "hook_2"), ("h-1", "hook_0"), ("h-2", "hook_1")] {
        transport.push_frame(hook_request(request_id, callback_id));
        settle().await;
    }
    assert_eq!(called.all(), ["third", "first", "second"]);
    client.disconnect().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn hook_callback_receives_the_typed_input_and_answers_with_wire_names() {
    let seen = Recorder::<(HookInput, Option<String>)>::new();
    let synchronous = {
        let seen = seen.clone();
        HookCallback::new(move |input, tool_use_id, _context| {
            seen.push((input, tool_use_id));
            async {
                Ok(SyncHookOutput::default()
                    .continue_(false)
                    .stop_reason("Policy forbids writes")
                    .suppress_output(true)
                    .into())
            }
        })
    };
    let deferred = HookCallback::new(async |_input, _tool_use_id, _context| {
        Ok(AsyncHookOutput::new().async_timeout(5000).into())
    });
    let options = ClaudeAgentOptions::builder()
        .hook(
            "PreToolUse",
            HookMatcher::new().hook(synchronous).hook(deferred),
        )
        .build()
        .unwrap();
    let transport = cli();
    let _client = connected(&transport, options).await;

    transport.push_frame(hook_request("hook-1", "hook_0"));
    let mut without_tool_use = hook_request("hook-2", "hook_1");
    without_tool_use["request"]
        .as_object_mut()
        .unwrap()
        .shift_remove("tool_use_id");
    transport.push_frame(without_tool_use);
    settle().await;

    let seen = seen.all();
    assert_eq!(seen.len(), 1);
    let (input, tool_use_id) = &seen[0];
    let HookInput::PreToolUse(input) = input else {
        panic!("expected a PreToolUse input, got {input:?}");
    };
    assert_eq!(input.tool_name, "Write");
    assert_eq!(input.tool_input, json!({"file_path": "a.txt"}));
    assert_eq!(input.base.cwd, "/work");
    assert_eq!(tool_use_id.as_deref(), Some("toolu_1"));

    assert_eq!(
        response_to(&transport, "hook-1").unwrap(),
        json!({
            "subtype": "success",
            "request_id": "hook-1",
            "response": {
                "continue": false,
                "stopReason": "Policy forbids writes",
                "suppressOutput": true,
            },
        })
    );
    assert_eq!(
        response_to(&transport, "hook-2").unwrap()["response"],
        json!({"async": true, "asyncTimeout": 5000})
    );
}

#[tokio::test(start_paused = true)]
async fn failing_or_unknown_hook_gets_an_error_response_and_the_session_continues() {
    let failing = HookCallback::new(async |_input, _tool_use_id, _context| {
        Err("the policy service is down".into())
    });
    let panicking = HookCallback::new(
        async |_input,
               _tool_use_id,
               _context|
               -> Result<HookOutput, claude_agent_rust_sdk::BoxError> {
            panic!("hook exploded")
        },
    );
    let options = ClaudeAgentOptions::builder()
        .hook(
            "PreToolUse",
            HookMatcher::new()
                .hook(failing)
                .hook(passive_hook())
                .hook(panicking),
        )
        .build()
        .unwrap();
    let transport = cli();
    let client = connected(&transport, options).await;

    transport.push_frame(hook_request("hook-1", "hook_0"));
    transport.push_frame(hook_request("hook-2", "hook_99"));
    transport.push_frame(hook_request("hook-4", "hook_2"));
    settle().await;
    assert_eq!(
        response_to(&transport, "hook-1").unwrap(),
        json!({
            "subtype": "error",
            "request_id": "hook-1",
            "error": "the policy service is down",
        })
    );
    assert_eq!(
        response_to(&transport, "hook-2").unwrap(),
        json!({
            "subtype": "error",
            "request_id": "hook-2",
            "error": "No hook callback found for ID: hook_99",
        })
    );
    assert_eq!(
        response_to(&transport, "hook-4").unwrap()["subtype"],
        "error"
    );

    transport.push_frame(hook_request("hook-3", "hook_1"));
    settle().await;
    assert_eq!(
        response_to(&transport, "hook-3").unwrap(),
        json!({"subtype": "success", "request_id": "hook-3", "response": {}})
    );
    client.interrupt().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn initialize_response_is_stored_and_returned_as_server_info() {
    let transport = cli_answering(|frame| {
        if is_control_request(frame, "initialize") {
            vec![success(
                frame,
                json!({"commands": [{"name": "compact"}], "output_style": "default"}),
            )]
        } else {
            Vec::new()
        }
    });
    let client = ClaudeSdkClient::with_transport(default_options(), transport.clone());
    assert!(
        client
            .get_server_info()
            .as_ref()
            .err()
            .is_some_and(Error::is_cli_connection)
    );
    client.connect().await.unwrap();
    assert_eq!(
        client.get_server_info().unwrap(),
        Some(json!({"commands": [{"name": "compact"}], "output_style": "default"}))
    );
}

fn blocking_options(mode: HookFailureMode) -> ClaudeAgentOptions {
    ClaudeAgentOptions::builder()
        .hook(
            "PreToolUse",
            HookMatcher::new().hook(passive_hook()).on_failure(mode),
        )
        .build()
        .unwrap()
}

fn initialize_answering(response: Value) -> claude_agent_rust_sdk::testing::MockTransport {
    cli_answering(move |frame| {
        if is_control_request(frame, "initialize") {
            vec![success(frame, response.clone())]
        } else {
            Vec::new()
        }
    })
}

#[tokio::test(start_paused = true)]
async fn block_on_failure_is_refused_when_the_cli_cannot_honor_it() {
    for mode in [
        HookFailureMode::Block,
        HookFailureMode::Unknown("halt".to_owned()),
    ] {
        let transport = initialize_answering(json!({}));
        let client = ClaudeSdkClient::with_transport(blocking_options(mode), transport.clone());
        let error = client.connect().await.unwrap_err();
        assert!(
            error.is_cli_connection()
                && error
                    .to_string()
                    .contains("does not honor on_failure on hook matchers")
        );
        assert!(transport.is_closed());
        assert!(
            client
                .get_server_info()
                .as_ref()
                .err()
                .is_some_and(Error::is_cli_connection)
        );
    }

    let transport =
        initialize_answering(json!({"hook_on_failure_supported": true, "hooks_applied": false}));
    let client = ClaudeSdkClient::with_transport(
        blocking_options(HookFailureMode::Block),
        transport.clone(),
    );
    let error = client.connect().await.unwrap_err();
    assert!(error.is_cli_connection() && error.to_string().contains("did not register your hooks"));
    assert!(transport.is_closed());

    let stream = query_with_transport(
        "Hello",
        blocking_options(HookFailureMode::Block),
        initialize_answering(json!({})),
    );
    let items = drain(stream).await;
    assert_eq!(items.len(), 1);
    assert!(
        items[0]
            .as_ref()
            .err()
            .is_some_and(Error::is_cli_connection)
    );
}

#[tokio::test(start_paused = true)]
async fn block_on_failure_is_accepted_when_the_cli_honors_it() {
    let transport = initialize_answering(json!({"hook_on_failure_supported": true}));
    let client = ClaudeSdkClient::with_transport(
        blocking_options(HookFailureMode::Block),
        transport.clone(),
    );
    client.connect().await.unwrap();
    assert_eq!(
        initialize_request(&transport)["hooks"]["PreToolUse"][0]["onFailure"],
        "block"
    );

    let lenient = initialize_answering(json!({}));
    let client =
        ClaudeSdkClient::with_transport(blocking_options(HookFailureMode::Continue), lenient);
    client.connect().await.unwrap();
}

fn permission_request(request_id: &str) -> Value {
    json!({
        "type": "control_request",
        "request_id": request_id,
        "request": {
            "subtype": "can_use_tool",
            "tool_name": "Bash",
            "input": {"command": "rm -rf build"},
            "permission_suggestions": [
                {
                    "type": "addRules",
                    "rules": [{"toolName": "Bash", "ruleContent": "rm:*"}],
                    "behavior": "allow",
                    "destination": "session",
                },
            ],
            "tool_use_id": "toolu_7",
            "agent_id": "agent-3",
            "blocked_path": "/work/build",
            "decision_reason": "Path is outside allowed directories",
            "title": "Claude wants to delete build",
            "display_name": "Delete build",
            "description": "Removes the build directory",
        },
    })
}

#[tokio::test(start_paused = true)]
async fn permission_callback_receives_the_tool_the_input_and_the_full_context() {
    let seen = Recorder::<(String, Value, ToolPermissionContext)>::new();
    let options = {
        let seen = seen.clone();
        ClaudeAgentOptions::builder()
            .can_use_tool(move |tool_name, input, context| {
                seen.push((tool_name, input, context));
                async { Ok(PermissionResult::allow()) }
            })
            .build()
            .unwrap()
    };
    let transport = cli();
    let _client = connected(&transport, options).await;

    transport.push_frame(permission_request("perm-1"));
    let mut bare = permission_request("perm-2");
    bare["request"] =
        json!({"subtype": "can_use_tool", "tool_name": "Read", "input": {"file_path": "a"}});
    transport.push_frame(bare);
    settle().await;

    let seen = seen.all();
    assert_eq!(seen.len(), 2);
    let (tool_name, input, context) = &seen[0];
    assert_eq!(tool_name, "Bash");
    assert_eq!(input, &json!({"command": "rm -rf build"}));
    assert_eq!(
        context.suggestions,
        [PermissionUpdate::new(PermissionUpdateKind::AddRules)
            .rules([PermissionRuleValue::new("Bash", Some("rm:*".to_owned()))])
            .behavior("allow")
            .destination(PermissionUpdateDestination::Session)]
    );
    assert_eq!(context.tool_use_id.as_deref(), Some("toolu_7"));
    assert_eq!(context.agent_id.as_deref(), Some("agent-3"));
    assert_eq!(context.blocked_path.as_deref(), Some("/work/build"));
    assert_eq!(
        context.decision_reason.as_deref(),
        Some("Path is outside allowed directories")
    );
    assert_eq!(
        context.title.as_deref(),
        Some("Claude wants to delete build")
    );
    assert_eq!(context.display_name.as_deref(), Some("Delete build"));
    assert_eq!(
        context.description.as_deref(),
        Some("Removes the build directory")
    );
    assert_eq!(seen[1].2, ToolPermissionContext::default());

    assert_eq!(
        response_to(&transport, "perm-1").unwrap(),
        json!({
            "subtype": "success",
            "request_id": "perm-1",
            "response": {"behavior": "allow", "updatedInput": {"command": "rm -rf build"}},
        })
    );
    assert_eq!(
        response_to(&transport, "perm-2").unwrap()["response"],
        json!({"behavior": "allow", "updatedInput": {"file_path": "a"}})
    );
}

#[tokio::test(start_paused = true)]
async fn permission_results_use_the_wire_shapes() {
    let options = ClaudeAgentOptions::builder()
        .can_use_tool(async |tool_name: String, input: Value, _context| {
            Ok(match tool_name.as_str() {
                "Write" => PermissionResult::Allow {
                    updated_input: Some(json!({"file_path": "safe.txt", "was": input})),
                    updated_permissions: Some(vec![
                        PermissionUpdate::new(PermissionUpdateKind::SetMode)
                            .mode("acceptEdits")
                            .destination(PermissionUpdateDestination::Session),
                    ]),
                },
                "Bash" => PermissionResult::deny("Shell access is off"),
                _ => PermissionResult::Deny {
                    message: "Stop everything".to_owned(),
                    interrupt: true,
                },
            })
        })
        .build()
        .unwrap();
    let transport = cli();
    let _client = connected(&transport, options).await;

    for (request_id, tool_name) in [("p-1", "Write"), ("p-2", "Bash"), ("p-3", "WebFetch")] {
        transport.push_frame(json!({
            "type": "control_request",
            "request_id": request_id,
            "request": {"subtype": "can_use_tool", "tool_name": tool_name, "input": {"a": 1}},
        }));
    }
    settle().await;

    assert_eq!(
        response_to(&transport, "p-1").unwrap()["response"],
        json!({
            "behavior": "allow",
            "updatedInput": {"file_path": "safe.txt", "was": {"a": 1}},
            "updatedPermissions": [
                {"type": "setMode", "mode": "acceptEdits", "destination": "session"},
            ],
        })
    );
    assert_eq!(
        response_to(&transport, "p-2").unwrap()["response"],
        json!({"behavior": "deny", "message": "Shell access is off"})
    );
    assert_eq!(
        response_to(&transport, "p-3").unwrap()["response"],
        json!({"behavior": "deny", "message": "Stop everything", "interrupt": true})
    );
}

#[tokio::test(start_paused = true)]
async fn permission_callback_echoes_whole_tool_suggestions() {
    let options = ClaudeAgentOptions::builder()
        .can_use_tool(async |_tool_name, _input, context: ToolPermissionContext| {
            Ok(PermissionResult::Allow {
                updated_input: None,
                updated_permissions: Some(context.suggestions),
            })
        })
        .build()
        .unwrap();
    let transport = cli();
    let _client = connected(&transport, options).await;

    let suggestions = json!([
        {
            "type": "addRules",
            "destination": "localSettings",
            "behavior": "allow",
            "rules": [{"toolName": "WebSearch"}],
        },
        {
            "type": "addRules",
            "destination": "session",
            "behavior": "allow",
            "rules": [{"toolName": "Bash", "ruleContent": "git status"}],
        },
        {"type": "setMode", "mode": "acceptEdits", "destination": "session"},
    ]);
    transport.push_frame(json!({
        "type": "control_request",
        "request_id": "test-whole-tool",
        "request": {
            "subtype": "can_use_tool",
            "tool_name": "WebSearch",
            "input": {"query": "weather"},
            "permission_suggestions": suggestions,
        },
    }));
    settle().await;

    let response = response_to(&transport, "test-whole-tool").unwrap();
    assert_eq!(response["subtype"], "success");
    assert_eq!(response["response"]["updatedPermissions"], suggestions);
}

#[tokio::test(start_paused = true)]
async fn permission_callback_sends_no_updates_if_one_is_malformed() {
    let (warnings, _guard) = Warnings::install();
    let options = ClaudeAgentOptions::builder()
        .can_use_tool(async |_tool_name, _input, _context| {
            Ok(PermissionResult::Allow {
                updated_input: None,
                updated_permissions: Some(vec![
                    PermissionUpdate::new(PermissionUpdateKind::AddRules)
                        .rules([PermissionRuleValue::new("Read", None)])
                        .behavior("allow")
                        .destination(PermissionUpdateDestination::Session),
                    PermissionUpdate::new(PermissionUpdateKind::AddRules)
                        .rules([PermissionRuleValue::new("Bash", Some("ls".to_owned()))]),
                    PermissionUpdate::new(PermissionUpdateKind::SetMode).mode("acceptEdits"),
                ]),
            })
        })
        .build()
        .unwrap();
    let transport = cli();
    let _client = connected(&transport, options).await;

    transport.push_frame(json!({
        "type": "control_request",
        "request_id": "test-malformed-update",
        "request": {
            "subtype": "can_use_tool",
            "tool_name": "Read",
            "input": {"file_path": "/tmp/x"},
            "permission_suggestions": [],
        },
    }));
    settle().await;

    let response = response_to(&transport, "test-malformed-update").unwrap();
    assert_eq!(response["subtype"], "success");
    assert_eq!(response["response"]["behavior"], "allow");
    assert!(response["response"].get("updatedPermissions").is_none());
    let warnings = warnings.all().join("\n");
    assert!(
        warnings.contains("updated_permissions[1] cannot be sent"),
        "{warnings}"
    );
    assert!(warnings.contains("has no behavior"), "{warnings}");
}

#[tokio::test(start_paused = true)]
async fn permission_request_without_a_callback_gets_the_upstream_error() {
    let transport = cli();
    let client = connected(&transport, options_with_hook()).await;
    transport.push_frame(permission_request("perm-1"));
    settle().await;

    assert_eq!(
        response_to(&transport, "perm-1").unwrap(),
        json!({
            "subtype": "error",
            "request_id": "perm-1",
            "error": "canUseTool callback is not provided",
        })
    );
    client.interrupt().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn failing_permission_callback_or_malformed_request_gets_an_error_response() {
    let options = ClaudeAgentOptions::builder()
        .can_use_tool(async |_tool_name, _input, _context| Err("the approver is offline".into()))
        .build()
        .unwrap();
    let transport = cli();
    let client = connected(&transport, options).await;

    transport.push_frame(permission_request("perm-1"));
    transport.push_frame(json!({
        "type": "control_request",
        "request_id": "perm-2",
        "request": {"subtype": "can_use_tool", "input": {}},
    }));
    let mut bad_suggestion = permission_request("perm-3");
    bad_suggestion["request"]["permission_suggestions"] = json!([{"rules": []}]);
    transport.push_frame(bad_suggestion);
    settle().await;

    assert_eq!(
        response_to(&transport, "perm-1").unwrap(),
        json!({
            "subtype": "error",
            "request_id": "perm-1",
            "error": "the approver is offline",
        })
    );
    assert_eq!(
        response_to(&transport, "perm-2").unwrap()["error"],
        "Missing required field in can_use_tool request: 'tool_name'"
    );
    assert_eq!(
        response_to(&transport, "perm-3").unwrap()["subtype"],
        "error"
    );
    client.interrupt().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn sixty_four_concurrent_sessions_keep_their_control_channels_apart() {
    let sessions = (0..64).map(|index| async move {
        let options = ClaudeAgentOptions::builder()
            .hook(
                "PreToolUse",
                HookMatcher::new().hook(HookCallback::new(
                    move |_input, _tool_use_id, _context| async move {
                        Ok(SyncHookOutput::default()
                            .system_message(format!("session {index}"))
                            .into())
                    },
                )),
            )
            .build()
            .unwrap();
        let transport = cli_answering(move |frame| {
            if frame["type"] == "user" {
                vec![hook_request("hook-1", "hook_0")]
            } else if frame["type"] == "control_response" {
                let text = frame["response"]["response"]["systemMessage"]
                    .as_str()
                    .unwrap();
                vec![assistant(text), result(text)]
            } else {
                Vec::new()
            }
        });
        let prompt = format!("prompt {index}");
        let messages = drain(query_with_transport(
            prompt.as_str(),
            options,
            transport.clone(),
        ))
        .await;
        assert_eq!(result_texts(&messages), [format!("session {index}")]);
        assert_eq!(
            written(&transport, "user")[0]["message"]["content"],
            json!(prompt)
        );
    });
    futures_util::future::join_all(sessions).await;
}

#[tokio::test(start_paused = true)]
async fn hooks_under_notification_subagent_start_and_permission_request_are_registered() {
    let events = ["Notification", "SubagentStart", "PermissionRequest"];
    let mut builder = ClaudeAgentOptions::builder();
    for event in events {
        builder = builder.hook(event, HookMatcher::new().hook(passive_hook()));
    }
    let options = builder.build().unwrap();
    let registered: Vec<String> = options
        .hooks
        .as_ref()
        .unwrap()
        .keys()
        .map(|event| event.as_str().to_owned())
        .collect();
    assert_eq!(registered, events);

    let transport = cli();
    let client = connected(&transport, options).await;
    let hooks = initialize_request(&transport)["hooks"].clone();
    let sent: Vec<&str> = hooks
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(sent.len(), 3, "{hooks}");
    for event in events {
        assert!(sent.contains(&event), "{event}: {hooks}");
        assert_eq!(
            hooks[event][0]["hookCallbackIds"].as_array().unwrap().len(),
            1
        );
    }
    client.disconnect().await.unwrap();
}
