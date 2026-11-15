#![cfg(feature = "testing")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::time::Duration;

use claude_agent_kit::testing::MockTransport;
use claude_agent_kit::{
    ClaudeAgentOptions, ClaudeSdkClient, Error, InputMessage, Message, PermissionResult, Prompt,
    SdkMcpTool, ToolResult, create_sdk_mcp_server, query_with_transport,
};
use futures_core::Stream;
use futures_util::{FutureExt, StreamExt, stream};
use serde_json::{Value, json};

use common::mcp::initialize_params;
use common::mock::{
    Recorder, assistant, cli, cli_answering, drain, is_control_request, options_with_hook,
    responses, result, result_texts, session_state, settle, silent_cli, success, system_subtypes,
    written,
};

fn answering_prompts() -> MockTransport {
    cli_answering(|frame| {
        if frame["type"] == "user" {
            vec![assistant("4"), result("4")]
        } else {
            Vec::new()
        }
    })
}

fn staying_open() -> MockTransport {
    let transport = MockTransport::new();
    transport.respond_with(|frame| {
        if frame["type"] == "control_request" {
            vec![success(frame, json!({}))]
        } else {
            Vec::new()
        }
    });
    transport
}

fn error_result(fields: Value) -> Value {
    let mut frame = json!({
        "type": "result",
        "subtype": "success",
        "duration_ms": 100,
        "duration_api_ms": 80,
        "is_error": true,
        "num_turns": 1,
        "session_id": "test",
    });
    frame
        .as_object_mut()
        .unwrap()
        .extend(fields.as_object().unwrap().clone());
    frame
}

fn exit(code: i32) -> Error {
    Error::process(
        "Command failed",
        Some(code),
        Some("Check stderr output for details".to_owned()),
    )
}

async fn run_ending_with(frames: Vec<Value>, end: Error) -> Vec<Result<Message, Error>> {
    let transport = staying_open();
    let stream = query_with_transport("Hello", ClaudeAgentOptions::default(), transport.clone());
    let messages = tokio::spawn(drain(stream));
    settle().await;
    for frame in frames {
        transport.push_frame(frame);
    }
    transport.push_error(end);
    messages.await.unwrap()
}

fn one_user_message(text: &str) -> Prompt {
    Prompt::stream(stream::iter([json!({
        "type": "user",
        "message": {"role": "user", "content": text},
    })]))
}

fn greeter_options() -> ClaudeAgentOptions {
    let greet = SdkMcpTool::raw(
        "greet",
        "Greet a user",
        json!({"type": "object", "properties": {"name": {"type": "string"}}}),
        |arguments| async move {
            let name = arguments["name"].as_str().unwrap_or_default().to_owned();
            Ok(ToolResult::text(format!("Hi {name}")))
        },
    )
    .unwrap();
    ClaudeAgentOptions::builder()
        .mcp_server(
            "greeter",
            create_sdk_mcp_server("greeter", None, [greet]).unwrap(),
        )
        .build()
        .unwrap()
}

fn greeter_request(request_id: &str, id: u64, method: &str, params: Value) -> Value {
    json!({
        "type": "control_request",
        "request_id": request_id,
        "request": {
            "subtype": "mcp_message",
            "server_name": "greeter",
            "message": {"jsonrpc": "2.0", "id": id, "method": method, "params": params},
        },
    })
}

fn assert_one_assistant_then_one_result(messages: &[Result<Message, Error>]) {
    assert_eq!(messages.len(), 2);
    assert!(matches!(messages[0], Ok(Message::Assistant(_))));
    assert!(matches!(messages[1], Ok(Message::Result(_))));
}

fn requires_stream(_: &impl Stream<Item = Result<Message, Error>>) {}

#[tokio::test(start_paused = true)]
async fn text_prompt_is_written_as_one_user_message_and_messages_are_typed() {
    let transport = answering_prompts();
    let stream = query_with_transport(
        "What is 2 + 2?",
        ClaudeAgentOptions::default(),
        transport.clone(),
    );
    requires_stream(&stream);
    settle().await;
    assert_eq!(transport.connect_count(), 0);

    let messages = drain(stream).await;
    assert_eq!(
        written(&transport, "user"),
        [json!({
            "type": "user",
            "session_id": "",
            "message": {"role": "user", "content": "What is 2 + 2?"},
            "parent_tool_use_id": null,
        })]
    );
    assert_eq!(messages.len(), 2);
    let Ok(Message::Assistant(answer)) = &messages[0] else {
        panic!("expected an assistant message");
    };
    assert_eq!(answer.content[0].as_text(), Some("4"));
    let Ok(Message::Result(outcome)) = &messages[1] else {
        panic!("expected a result message");
    };
    assert_eq!(outcome.result.as_deref(), Some("4"));
    assert!(!outcome.is_error);

    assert!(transport.input_ended());
    assert!(transport.is_closed());
    assert_eq!(transport.close_count(), 1);
}

#[tokio::test(start_paused = true)]
async fn stream_prompt_writes_every_input_message_as_given() {
    let transport = answering_prompts();
    let raw =
        json!({"type": "user", "message": {"role": "user", "content": "second"}, "custom": 1});
    let prompt = Prompt::stream(stream::iter([
        Value::from(InputMessage::user("first").session_id("main")),
        raw.clone(),
    ]));
    let messages = drain(query_with_transport(
        prompt,
        ClaudeAgentOptions::default(),
        transport.clone(),
    ))
    .await;

    assert_eq!(
        written(&transport, "user"),
        [
            json!({
                "type": "user",
                "message": {"role": "user", "content": "first"},
                "parent_tool_use_id": null,
                "session_id": "main",
            }),
            raw,
        ]
    );
    assert_eq!(result_texts(&messages), ["4", "4"]);
}

#[tokio::test(start_paused = true)]
async fn verbatim_prompts_mark_every_user_frame_as_client_composed() {
    let verbatim = || {
        ClaudeAgentOptions::builder()
            .verbatim_prompts(true)
            .build()
            .unwrap()
    };
    let inputs = || {
        Prompt::stream(stream::iter([
            json!({"type": "user", "message": {"role": "user", "content": "one"}}),
            json!({"type": "user", "message": {"role": "user", "content": "two"}, "client_composed": false}),
        ]))
    };

    let transport = answering_prompts();
    drain(query_with_transport(
        "@notes.md /compact",
        verbatim(),
        transport.clone(),
    ))
    .await;
    assert_eq!(written(&transport, "user")[0]["client_composed"], true);
    assert_eq!(
        written(&transport, "user")[0]["message"]["content"],
        "@notes.md /compact"
    );

    let transport = answering_prompts();
    drain(query_with_transport(
        inputs(),
        verbatim(),
        transport.clone(),
    ))
    .await;
    let frames = written(&transport, "user");
    assert_eq!(frames.len(), 2);
    assert!(frames.iter().all(|frame| frame["client_composed"] == true));

    let transport = answering_prompts();
    drain(query_with_transport(
        "plain",
        ClaudeAgentOptions::default(),
        transport.clone(),
    ))
    .await;
    assert!(
        written(&transport, "user")[0]
            .get("client_composed")
            .is_none()
    );

    let transport = answering_prompts();
    drain(query_with_transport(
        inputs(),
        ClaudeAgentOptions::default(),
        transport.clone(),
    ))
    .await;
    let frames = written(&transport, "user");
    assert!(frames[0].get("client_composed").is_none());
    assert_eq!(frames[1]["client_composed"], false);
}

#[tokio::test(start_paused = true)]
async fn text_prompt_with_a_hook_delivers_more_messages_than_the_queue_holds_before_the_result() {
    let transport = cli_answering(|frame| {
        if frame["type"] == "user" {
            let mut turn: Vec<Value> = (0..150)
                .map(|index| assistant(&format!("step {index}")))
                .collect();
            turn.push(result("done"));
            turn
        } else {
            Vec::new()
        }
    });
    let mut stream = query_with_transport("Hello", options_with_hook(), transport.clone());
    let first = tokio::time::timeout(Duration::from_secs(3600), stream.next())
        .await
        .expect("the query stalled before its first message");
    let Some(Ok(Message::Assistant(first))) = first else {
        panic!("expected an assistant message");
    };
    assert_eq!(first.content[0].as_text(), Some("step 0"));
    settle().await;
    assert_eq!(transport.pending_frames(), 49);
    assert!(!transport.input_ended());

    let rest = drain(stream).await;
    assert_eq!(rest.len(), 150);
    assert!(
        rest[..149]
            .iter()
            .all(|message| matches!(message, Ok(Message::Assistant(_))))
    );
    assert_eq!(result_texts(&rest), ["done"]);
    assert!(transport.input_ended());
}

#[tokio::test(start_paused = true)]
async fn stream_prompt_answers_the_in_process_server_requests_that_follow_the_user_message() {
    let transport = cli_answering(|frame| {
        if frame["type"] == "user" {
            return vec![greeter_request(
                "mcp_init_1",
                1,
                "initialize",
                initialize_params(),
            )];
        }
        if frame["type"] != "control_response" {
            return Vec::new();
        }
        match frame["response"]["request_id"].as_str() {
            Some("mcp_init_1") => vec![greeter_request("mcp_init_2", 2, "tools/list", json!({}))],
            Some("mcp_init_2") => vec![assistant("Hello!"), result("Hello!")],
            _ => Vec::new(),
        }
    });
    let messages = drain(query_with_transport(
        one_user_message("Greet Alice"),
        greeter_options(),
        transport.clone(),
    ))
    .await;

    assert_one_assistant_then_one_result(&messages);
    assert_eq!(written(&transport, "user").len(), 1);
    let answers = responses(&transport);
    assert_eq!(answers.len(), 2);
    assert_eq!(answers[0]["request_id"], "mcp_init_1");
    assert_eq!(answers[0]["subtype"], "success");
    assert_eq!(
        answers[0]["response"]["mcp_response"]["result"]["serverInfo"]["name"],
        "greeter"
    );
    assert_eq!(answers[1]["request_id"], "mcp_init_2");
    assert_eq!(answers[1]["subtype"], "success");
    let tools: Vec<&str> = answers[1]["response"]["mcp_response"]["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    assert_eq!(tools, ["greet"]);
    assert!(transport.input_ended());
}

#[tokio::test(start_paused = true)]
async fn stream_prompt_with_only_a_permission_callback_answers_the_request_before_stdin_closes() {
    let asked = Recorder::<String>::new();
    let options = {
        let asked = asked.clone();
        ClaudeAgentOptions::builder()
            .can_use_tool(move |tool_name, _input, _context| {
                asked.push(tool_name);
                async { Ok(PermissionResult::allow()) }
            })
            .build()
            .unwrap()
    };
    let stdin_open_at_the_answer = Recorder::<bool>::new();
    let transport = silent_cli();
    {
        let observed = transport.clone();
        let seen = stdin_open_at_the_answer.clone();
        transport.respond_with(move |frame| {
            if frame["type"] == "control_request" {
                return vec![success(frame, json!({}))];
            }
            if frame["type"] == "user" {
                return vec![json!({
                    "type": "control_request",
                    "request_id": "perm_1",
                    "request": {
                        "subtype": "can_use_tool",
                        "tool_name": "Write",
                        "input": {"file_path": "/tmp/x", "content": "hi"},
                        "tool_use_id": "toolu_1",
                    },
                })];
            }
            if frame["type"] == "control_response" {
                seen.push(!observed.input_ended());
                return vec![assistant("Hello!"), result("Hello!")];
            }
            Vec::new()
        });
    }
    let messages = drain(query_with_transport(
        one_user_message("write it"),
        options,
        transport.clone(),
    ))
    .await;

    assert_eq!(asked.all(), ["Write"]);
    let answers = responses(&transport);
    assert_eq!(answers.len(), 1);
    assert_eq!(answers[0]["request_id"], "perm_1");
    assert_eq!(answers[0]["subtype"], "success");
    assert_eq!(answers[0]["response"]["behavior"], "allow");
    assert_eq!(stdin_open_at_the_answer.all(), [true]);
    assert_one_assistant_then_one_result(&messages);
    assert!(transport.input_ended());
}

#[tokio::test(start_paused = true)]
async fn dropping_the_stream_closes_the_session() {
    let transport = cli_answering(|frame| {
        if frame["type"] == "user" {
            vec![assistant("thinking")]
        } else {
            Vec::new()
        }
    });
    let mut stream = query_with_transport("Hello", options_with_hook(), transport.clone());
    assert!(matches!(
        stream.next().await,
        Some(Ok(Message::Assistant(_)))
    ));
    assert!(!transport.is_closed());
    drop(stream);
    settle().await;
    assert!(transport.is_closed());
    assert_eq!(transport.close_count(), 1);

    let unanswered = silent_cli();
    let mut stream =
        query_with_transport("Hello", ClaudeAgentOptions::default(), unanswered.clone());
    assert!(stream.next().now_or_never().is_none());
    settle().await;
    assert_eq!(unanswered.connect_count(), 1);
    drop(stream);
    settle().await;
    assert!(unanswered.is_closed());
}

#[tokio::test(start_paused = true)]
async fn error_result_followed_by_a_failing_exit_ends_with_a_result_error() {
    let cases = [
        (
            json!({"subtype": "error_max_turns", "errors": ["Reached maximum number of turns (1)", " second cause "]}),
            "Reached maximum number of turns (1); second cause",
        ),
        (
            json!({"subtype": "success", "errors": [], "result": " API Error: 529 Overloaded ", "api_error_status": 529}),
            "API Error: 529 Overloaded",
        ),
        (
            json!({"subtype": "error_during_execution", "errors": ["  ", ""], "result": ""}),
            "error_during_execution",
        ),
        (
            json!({"subtype": "success", "api_error_status": 529}),
            "API error (HTTP 529)",
        ),
        (json!({"subtype": "success"}), "unknown error"),
        (
            json!({"subtype": "error_during_execution", "errors": 42, "result": "the tool crashed"}),
            "the tool crashed",
        ),
    ];
    for (fields, text) in cases {
        let frame = error_result(fields);
        let mut items = run_ending_with(vec![assistant("working"), frame.clone()], exit(1)).await;
        assert_eq!(items.len(), 3, "{text}");
        assert!(matches!(items[1], Ok(Message::Result(ref outcome)) if outcome.is_error));

        let last = items.pop().unwrap().unwrap_err();
        assert_eq!(
            last.to_string(),
            format!("Claude Code returned an error result: {text} (exit code: 1)")
        );
        let Error::ResultError {
            message,
            data,
            exit_code,
        } = last
        else {
            panic!("expected a result error for {text}");
        };
        assert_eq!(
            message,
            format!("Claude Code returned an error result: {text}")
        );
        assert_eq!(exit_code, Some(1));
        assert_eq!(Value::Object(data.into_inner()), frame);
    }
}

#[tokio::test(start_paused = true)]
async fn failing_exit_is_kept_when_the_last_result_is_not_the_cause() {
    let failed = error_result(json!({"subtype": "error_max_turns", "errors": ["too many turns"]}));
    let cases = [
        vec![assistant("working")],
        vec![assistant("working"), result("fine")],
        vec![failed.clone(), result("fine")],
        vec![failed.clone(), assistant("a new turn")],
        vec![
            failed.clone(),
            json!({"type": "system", "subtype": "status"}),
        ],
    ];
    for frames in cases {
        let count = frames.len();
        let mut items = run_ending_with(frames, exit(1)).await;
        assert_eq!(items.len(), count + 1);
        let last = items.pop().unwrap().unwrap_err();
        assert!(matches!(
            &last,
            Error::Process { message, exit_code: Some(1), .. } if message == "Command failed"
        ));
    }

    let mut visible_state = session_state("idle");
    visible_state
        .as_object_mut()
        .unwrap()
        .remove("sdk_host_only");
    for marker in [session_state("idle"), visible_state] {
        let mut items = run_ending_with(vec![failed.clone(), marker], exit(1)).await;
        let last = items.pop().unwrap().unwrap_err();
        assert!(matches!(last, Error::ResultError { .. }));
    }

    let broken: serde_json::Error = serde_json::from_str::<Value>("{\"type\": ").unwrap_err();
    let mut items = run_ending_with(vec![failed], Error::json_decode("{\"type\": ", broken)).await;
    let last = items.pop().unwrap().unwrap_err();
    assert!(matches!(last, Error::JsonDecode { .. }));
}

#[tokio::test(start_paused = true)]
async fn pending_control_requests_fail_with_the_result_error() {
    let failed = error_result(json!({
        "subtype": "error_during_execution",
        "errors": ["No conversation found with session ID: 123"],
    }));

    let refused_resume = silent_cli();
    let stream = query_with_transport(
        "Hello",
        ClaudeAgentOptions::default(),
        refused_resume.clone(),
    );
    let items = tokio::spawn(drain(stream));
    settle().await;
    refused_resume.push_frame(failed.clone());
    refused_resume.push_error(exit(1));
    let items = items.await.unwrap();
    assert_eq!(items.len(), 1);
    let error = items[0].as_ref().unwrap_err();
    assert!(matches!(error, Error::ResultError { .. }));
    assert!(
        error
            .to_string()
            .contains("No conversation found with session ID: 123")
    );
    assert!(refused_resume.is_closed());

    let transport = MockTransport::new();
    transport.respond_with(|frame| {
        if is_control_request(frame, "initialize") {
            vec![success(frame, json!({}))]
        } else {
            Vec::new()
        }
    });
    let client = ClaudeSdkClient::with_transport(ClaudeAgentOptions::default(), transport.clone());
    client.connect().await.unwrap();
    let fail = async {
        settle().await;
        transport.push_frame(failed.clone());
        transport.push_error(exit(1));
    };
    let (status, ()) = tokio::join!(client.get_mcp_status(), fail);
    let error = status.unwrap_err();
    assert!(matches!(
        error,
        Error::ResultError {
            exit_code: Some(1),
            ..
        }
    ));
}

#[tokio::test(start_paused = true)]
async fn message_that_does_not_parse_ends_the_stream_with_an_error() {
    let transport = cli_answering(|frame| {
        if frame["type"] == "user" {
            vec![
                json!({"type": "weather", "forecast": "rain"}),
                json!({"type": "assistant"}),
                result("never read"),
            ]
        } else {
            Vec::new()
        }
    });
    let items = drain(query_with_transport(
        "Hello",
        ClaudeAgentOptions::default(),
        transport.clone(),
    ))
    .await;
    assert_eq!(items.len(), 2);
    assert!(matches!(&items[0], Ok(Message::Unknown(raw)) if raw["forecast"] == "rain"));
    assert!(matches!(items[1], Err(Error::MessageParse { .. })));
    assert!(transport.is_closed());
}

#[tokio::test(start_paused = true)]
async fn failures_before_the_first_message_are_the_only_item() {
    let unreachable = cli();
    unreachable.fail_connect("The service is unreachable");
    let items = drain(query_with_transport(
        "Hello",
        ClaudeAgentOptions::default(),
        unreachable.clone(),
    ))
    .await;
    assert_eq!(items.len(), 1);
    assert!(matches!(&items[0], Err(Error::CliConnection { message })
        if message == "The service is unreachable"));
    assert!(written(&unreachable, "control_request").is_empty());

    let mut conflicting = ClaudeAgentOptions::builder()
        .can_use_tool(async |_tool_name, _input, _context| Ok(PermissionResult::allow()))
        .build()
        .unwrap();
    conflicting.permission_prompt_tool_name = Some("mcp__approver__ask".to_owned());
    let untouched = cli();
    let items = drain(query_with_transport(
        "Hello",
        conflicting,
        untouched.clone(),
    ))
    .await;
    assert_eq!(items.len(), 1);
    assert!(matches!(items[0], Err(Error::Config { .. })));
    assert_eq!(untouched.connect_count(), 0);

    let rejecting_writes = cli_answering(|_| Vec::new());
    let stream = query_with_transport(
        "Hello",
        ClaudeAgentOptions::default(),
        rejecting_writes.clone(),
    );
    rejecting_writes.fail_writes("The pipe is broken");
    let items = drain(stream).await;
    assert_eq!(items.len(), 1);
    assert!(matches!(items[0], Err(Error::CliConnection { .. })));
    assert!(rejecting_writes.is_closed());
}

#[tokio::test(start_paused = true)]
async fn system_messages_are_delivered_with_their_subtype() {
    let transport = cli_answering(|frame| {
        if frame["type"] == "user" {
            vec![
                json!({"type": "system", "subtype": "init", "session_id": "test"}),
                json!({"type": "system", "subtype": "compact_boundary"}),
                result("done"),
            ]
        } else {
            Vec::new()
        }
    });
    let items = drain(query_with_transport(
        "Hello",
        ClaudeAgentOptions::default(),
        transport,
    ))
    .await;
    assert_eq!(system_subtypes(&items), ["init", "compact_boundary"]);
    assert_eq!(result_texts(&items), ["done"]);
}
