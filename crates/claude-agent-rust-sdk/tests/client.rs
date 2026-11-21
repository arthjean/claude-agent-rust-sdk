#![cfg(feature = "testing")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::sync::Arc;

use claude_agent_rust_sdk::ErrorKind;
use claude_agent_rust_sdk::testing::MockTransport;
use claude_agent_rust_sdk::{
    ClaudeAgentOptions, ClaudeSdkClient, Error, InputMessage, McpServerConnectionStatus, Message,
    PermissionMode, Prompt,
};
use futures_util::{StreamExt, stream};
use serde_json::{Value, json};

use common::mock::{
    Recorder, assistant, cli, cli_answering, connected, initialize_request, is_control_request,
    options_with_hook, requests, result, settle, success, written,
};

fn default_options() -> ClaudeAgentOptions {
    ClaudeAgentOptions::default()
}

fn assert_not_connected<T: std::fmt::Debug>(outcome: Result<T, Error>, method: &str) {
    match outcome {
        Err(error) if error.kind() == ErrorKind::NotConnected => {
            assert!(
                error.to_string().starts_with("Not connected"),
                "{method}: {error}"
            );
        }
        other => panic!("{method}: expected a not-connected error, got {other:?}"),
    }
}

fn text_of(message: &Message) -> String {
    match message {
        Message::Assistant(assistant) => assistant.content[0].as_text().unwrap().to_owned(),
        Message::Result(result) => result.result.clone().unwrap_or_default(),
        other => panic!("unexpected message {other:?}"),
    }
}

#[tokio::test(start_paused = true)]
async fn every_method_but_connect_and_disconnect_needs_a_connection() {
    let client = ClaudeSdkClient::new(default_options());
    assert_not_connected(client.query("Hello").await, "query");
    assert_not_connected(
        client.query_with_session_id("Hello", "main").await,
        "query_with_session_id",
    );
    assert_not_connected(client.interrupt().await, "interrupt");
    assert_not_connected(
        client.set_permission_mode(PermissionMode::Plan).await,
        "set_permission_mode",
    );
    assert_not_connected(client.set_model(None).await, "set_model");
    assert_not_connected(client.rewind_files("uuid").await, "rewind_files");
    assert_not_connected(
        client.reconnect_mcp_server("files").await,
        "reconnect_mcp_server",
    );
    assert_not_connected(
        client.toggle_mcp_server("files", true).await,
        "toggle_mcp_server",
    );
    assert_not_connected(client.stop_task("task").await, "stop_task");
    assert_not_connected(client.mcp_status().await, "mcp_status");
    assert_not_connected(client.context_usage().await, "context_usage");
    assert_not_connected(client.server_info(), "server_info");

    let mut messages: Vec<_> = client.receive_messages().collect().await;
    assert_eq!(messages.len(), 1);
    assert_not_connected(messages.pop().unwrap(), "receive_messages");
    let mut response: Vec<_> = client.receive_response().collect().await;
    assert_eq!(response.len(), 1);
    assert_not_connected(response.pop().unwrap(), "receive_response");
}

#[tokio::test(start_paused = true)]
async fn disconnect_is_safe_without_a_connection_and_idempotent() {
    let transport = cli();
    let client = ClaudeSdkClient::with_transport(default_options(), transport.clone());
    client.disconnect().await.unwrap();
    assert_eq!(transport.close_count(), 0);

    client.connect().await.unwrap();
    assert_eq!(initialize_request(&transport)["subtype"], "initialize");
    client.disconnect().await.unwrap();
    client.disconnect().await.unwrap();
    assert_eq!(transport.close_count(), 1);
    assert_not_connected(client.interrupt().await, "interrupt");
    settle().await;
    assert_eq!(transport.close_count(), 1);
}

#[tokio::test(start_paused = true)]
async fn connect_on_a_connected_client_closes_the_previous_session_first() {
    let transport = MockTransport::new();
    let closes_seen_at_initialize = Recorder::<usize>::new();
    {
        let observed = transport.clone();
        let seen = closes_seen_at_initialize.clone();
        transport.respond_with(move |frame| {
            if is_control_request(frame, "initialize") {
                seen.push(observed.close_count());
            }
            if frame["type"] == "control_request" {
                vec![success(frame, json!({}))]
            } else {
                Vec::new()
            }
        });
    }
    let client = ClaudeSdkClient::with_transport(default_options(), transport.clone());
    client.connect().await.unwrap();
    client.connect().await.unwrap();

    assert_eq!(closes_seen_at_initialize.all(), [0, 1]);
    assert_eq!(transport.connect_count(), 2);
    assert_eq!(transport.close_count(), 1);
    client.interrupt().await.unwrap();
    client.disconnect().await.unwrap();
    assert_eq!(transport.close_count(), 2);
}

#[tokio::test(start_paused = true)]
async fn connect_with_a_prompt_sends_it_in_the_default_session() {
    let transport = cli();
    let client = ClaudeSdkClient::with_transport(default_options(), transport.clone());
    client.connect_with_prompt("Hello").await.unwrap();
    let frames = transport.written_frames();
    assert_eq!(frames[0]["request"]["subtype"], "initialize");
    assert_eq!(
        frames[1],
        json!({
            "type": "user",
            "message": {"role": "user", "content": "Hello"},
            "parent_tool_use_id": null,
            "session_id": "default",
        })
    );
    settle().await;
    assert!(!transport.input_ended());

    let streamed = cli();
    let client = ClaudeSdkClient::with_transport(default_options(), streamed.clone());
    let prompt = Prompt::stream(stream::iter([
        InputMessage::user("one"),
        InputMessage::user("two"),
    ]));
    client.connect_with_prompt(prompt).await.unwrap();
    settle().await;
    assert_eq!(written(&streamed, "user").len(), 2);
    assert!(streamed.input_ended());
}

#[tokio::test(start_paused = true)]
async fn query_uses_the_default_session_unless_one_is_given_and_keeps_stdin_open() {
    let transport = cli();
    let client = connected(&transport, default_options()).await;
    client.query("first").await.unwrap();
    client
        .query_with_session_id("second", "side")
        .await
        .unwrap();
    let prompt = Prompt::stream(stream::iter([
        Value::from(InputMessage::user("third")),
        Value::from(InputMessage::user("fourth").session_id("own")),
    ]));
    client.query_with_session_id(prompt, "batch").await.unwrap();
    settle().await;

    let frames = written(&transport, "user");
    let sessions: Vec<&str> = frames
        .iter()
        .map(|frame| frame["session_id"].as_str().unwrap())
        .collect();
    assert_eq!(sessions, ["default", "side", "batch", "own"]);
    assert_eq!(
        frames[0],
        json!({
            "type": "user",
            "message": {"role": "user", "content": "first"},
            "parent_tool_use_id": null,
            "session_id": "default",
        })
    );
    assert!(!transport.input_ended());

    let failing = Prompt::try_stream(stream::iter([
        Ok(Value::from(InputMessage::user("fifth"))),
        Err("the source went away"),
    ]));
    let error = client.query(failing).await.unwrap_err();
    assert_eq!(error.kind(), ErrorKind::PromptStream);
    assert_eq!(error.to_string(), "Prompt stream failed");
    assert_eq!(
        std::error::Error::source(&error).unwrap().to_string(),
        "the source went away"
    );
    assert_eq!(written(&transport, "user").len(), 5);
    assert!(!transport.input_ended());
}

#[tokio::test(start_paused = true)]
async fn verbatim_prompts_mark_every_frame_the_client_writes() {
    let options = ClaudeAgentOptions::builder()
        .verbatim_prompts(true)
        .build()
        .unwrap();
    let transport = cli();
    let client = ClaudeSdkClient::with_transport(options, transport.clone());
    client.connect_with_prompt("@file.md").await.unwrap();
    client.query("/compact").await.unwrap();
    client
        .query(Prompt::stream(stream::iter([InputMessage::user(
            "streamed",
        )])))
        .await
        .unwrap();
    let frames = written(&transport, "user");
    assert_eq!(frames.len(), 3);
    assert!(frames.iter().all(|frame| frame["client_composed"] == true));

    let plain = cli();
    let client = connected(&plain, default_options()).await;
    client.query("/compact").await.unwrap();
    assert!(written(&plain, "user")[0].get("client_composed").is_none());
}

#[tokio::test(start_paused = true)]
async fn connect_with_a_streamed_prompt_marks_every_frame_only_when_verbatim_prompts_is_on() {
    for enabled in [true, false] {
        let options = ClaudeAgentOptions::builder()
            .verbatim_prompts(enabled)
            .build()
            .unwrap();
        let transport = cli();
        let client = ClaudeSdkClient::with_transport(options, transport.clone());
        let prompt = Prompt::stream(stream::iter([
            InputMessage::user("one"),
            InputMessage::user("two"),
        ]));
        client.connect_with_prompt(prompt).await.unwrap();
        settle().await;

        let frames = written(&transport, "user");
        let contents: Vec<&Value> = frames
            .iter()
            .map(|frame| &frame["message"]["content"])
            .collect();
        assert_eq!(contents, ["one", "two"], "verbatim_prompts {enabled}");
        let marks: Vec<Option<&Value>> = frames
            .iter()
            .map(|frame| frame.get("client_composed"))
            .collect();
        let expected = enabled.then_some(&Value::Bool(true));
        assert_eq!(marks, [expected, expected], "verbatim_prompts {enabled}");
    }
}

#[tokio::test(start_paused = true)]
async fn receive_response_ends_after_the_first_result_message() {
    let transport = cli_answering(|frame| {
        if frame["type"] != "user" {
            return Vec::new();
        }
        let prompt = frame["message"]["content"].as_str().unwrap();
        vec![
            assistant(&format!("thinking about {prompt}")),
            result(&format!("answer to {prompt}")),
        ]
    });
    let client = connected(&transport, default_options()).await;
    client.query("one").await.unwrap();
    client.query("two").await.unwrap();

    let first: Vec<String> = client
        .receive_response()
        .map(|message| text_of(&message.unwrap()))
        .collect()
        .await;
    assert_eq!(first, ["thinking about one", "answer to one"]);

    let mut second = client.receive_response();
    assert_eq!(
        text_of(&second.next().await.unwrap().unwrap()),
        "thinking about two"
    );
    assert_eq!(
        text_of(&second.next().await.unwrap().unwrap()),
        "answer to two"
    );
    assert!(second.next().await.is_none());
    assert!(second.next().await.is_none());
}

#[tokio::test(start_paused = true)]
async fn receive_messages_ends_at_a_message_that_does_not_parse_and_a_new_call_resumes() {
    let transport = cli_answering(|frame| {
        if frame["type"] == "user" {
            vec![
                assistant("first"),
                result("first"),
                json!({"type": "assistant"}),
                assistant("second"),
                result("second"),
            ]
        } else {
            Vec::new()
        }
    });
    let client = connected(&transport, default_options()).await;
    client.query("go").await.unwrap();

    let texts = |messages: Vec<Result<Message, claude_agent_rust_sdk::Error>>| {
        messages
            .into_iter()
            .map(|message| match message {
                Ok(message) => text_of(&message),
                Err(error) if error.kind() == ErrorKind::MessageParse => "<unparsable>".to_owned(),
                Err(other) => panic!("unexpected error {other}"),
            })
            .collect::<Vec<_>>()
    };
    let first: Vec<_> = client.receive_messages().collect().await;
    assert_eq!(texts(first), ["first", "first", "<unparsable>"]);

    let mut messages = client.receive_messages();
    let mut rest = Vec::new();
    for _ in 0..2 {
        rest.push(messages.next().await.unwrap());
    }
    assert_eq!(texts(rest), ["second", "second"]);

    let end = async {
        settle().await;
        client.disconnect().await.unwrap();
    };
    let (next, ()) = tokio::join!(messages.next(), end);
    assert!(next.is_none());
}

#[tokio::test(start_paused = true)]
async fn control_methods_send_the_upstream_subtypes_and_field_names() {
    let transport = cli_answering(|frame| {
        if is_control_request(frame, "mcp_status") {
            vec![success(
                frame,
                json!({"mcpServers": [
                    {"name": "files", "status": "connected", "serverInfo": {"name": "files", "version": "1.0"}},
                    {"name": "search", "status": "failed", "error": "timeout"},
                ]}),
            )]
        } else if is_control_request(frame, "get_context_usage") {
            vec![success(
                frame,
                json!({
                    "categories": [{"name": "System prompt", "tokens": 3200, "color": "blue"}],
                    "totalTokens": 3200,
                    "maxTokens": 200_000,
                    "percentage": 1.6,
                    "model": "claude-sonnet-5",
                }),
            )]
        } else {
            Vec::new()
        }
    });
    let client = connected(&transport, default_options()).await;

    client.interrupt().await.unwrap();
    client
        .set_permission_mode(PermissionMode::AcceptEdits)
        .await
        .unwrap();
    client.set_model(Some("claude-opus-5")).await.unwrap();
    client.set_model(None).await.unwrap();
    client.rewind_files("msg-uuid-1").await.unwrap();
    client.reconnect_mcp_server("files").await.unwrap();
    client.toggle_mcp_server("files", false).await.unwrap();
    client.toggle_mcp_server("files", true).await.unwrap();
    client.stop_task("task-abc123").await.unwrap();
    let status = client.mcp_status().await.unwrap();
    let usage = client.context_usage().await.unwrap();

    let sent: Vec<Value> = written(&transport, "control_request")
        .into_iter()
        .skip(1)
        .map(|frame| frame["request"].clone())
        .collect();
    assert_eq!(
        sent,
        [
            json!({"subtype": "interrupt"}),
            json!({"subtype": "set_permission_mode", "mode": "acceptEdits"}),
            json!({"subtype": "set_model", "model": "claude-opus-5"}),
            json!({"subtype": "set_model", "model": null}),
            json!({"subtype": "rewind_files", "user_message_id": "msg-uuid-1"}),
            json!({"subtype": "mcp_reconnect", "serverName": "files"}),
            json!({"subtype": "mcp_toggle", "serverName": "files", "enabled": false}),
            json!({"subtype": "mcp_toggle", "serverName": "files", "enabled": true}),
            json!({"subtype": "stop_task", "task_id": "task-abc123"}),
            json!({"subtype": "mcp_status"}),
            json!({"subtype": "get_context_usage"}),
        ]
    );

    assert_eq!(status.mcp_servers.len(), 2);
    assert_eq!(
        status.mcp_servers[0].status,
        McpServerConnectionStatus::Connected
    );
    assert_eq!(status.mcp_servers[1].error.as_deref(), Some("timeout"));
    assert_eq!(usage.total_tokens, 3200);
    assert_eq!(usage.categories[0].name, "System prompt");
    assert_eq!(usage.model, "claude-sonnet-5");
}

#[tokio::test(start_paused = true)]
async fn sending_and_receiving_run_concurrently_on_a_shared_client() {
    let transport = cli_answering(|frame| {
        if frame["type"] == "user" {
            vec![assistant("streaming"), result("done")]
        } else {
            Vec::new()
        }
    });
    let client = Arc::new(connected(&transport, options_with_hook()).await);

    let receiver = {
        let client = Arc::clone(&client);
        tokio::spawn(async move {
            client
                .receive_response()
                .map(|message| text_of(&message.unwrap()))
                .collect::<Vec<_>>()
                .await
        })
    };
    settle().await;
    client.query("go").await.unwrap();
    client.interrupt().await.unwrap();
    assert_eq!(receiver.await.unwrap(), ["streaming", "done"]);
    assert_eq!(requests(&transport, "interrupt").len(), 1);
    assert!(!transport.input_ended());
}

#[tokio::test(start_paused = true)]
async fn dropping_a_connected_client_closes_the_session() {
    let transport = cli();
    let client = connected(&transport, default_options()).await;
    let messages = client.receive_messages();
    drop(messages);
    drop(client);
    settle().await;
    assert!(transport.is_closed());
    assert_eq!(transport.close_count(), 1);
}

#[tokio::test(start_paused = true)]
async fn failed_connect_leaves_nothing_running() {
    let transport = cli();
    transport.fail_connect("The service is unreachable");
    let client = ClaudeSdkClient::with_transport(default_options(), transport.clone());
    let error = client.connect().await.unwrap_err();
    assert!(error.is_cli_connection() && error.to_string() == "The service is unreachable");
    assert_not_connected(client.interrupt().await, "interrupt");
    assert_eq!(transport.close_count(), 1);
    client.disconnect().await.unwrap();
    assert_eq!(transport.close_count(), 1);
}

#[tokio::test(start_paused = true)]
async fn a_consumer_waiting_in_receive_messages_ends_when_the_client_disconnects() {
    let transport = cli();
    let client = connected(&transport, default_options()).await;
    let consume = async {
        let mut messages = client.receive_messages();
        let mut count = 0;
        while let Some(message) = messages.next().await {
            message.unwrap();
            count += 1;
        }
        count
    };
    let disconnect = async {
        settle().await;
        client.disconnect().await.unwrap();
    };
    let (count, ()) = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        tokio::join!(consume, disconnect)
    })
    .await
    .expect("the consumer is released by the disconnect");
    assert_eq!(count, 0);
    assert_eq!(transport.close_count(), 1);
}

#[tokio::test(start_paused = true)]
async fn disconnect_during_a_connect_closes_the_session_being_opened_at_once() {
    let transport = common::mock::silent_cli();
    let client = ClaudeSdkClient::with_transport(default_options(), transport.clone());
    let started = tokio::time::Instant::now();
    let disconnect = async {
        settle().await;
        client.disconnect().await
    };
    let (connected, disconnected) = tokio::join!(client.connect(), disconnect);
    assert!(
        started.elapsed() < std::time::Duration::from_secs(1),
        "{:?}",
        started.elapsed()
    );
    let error = connected.unwrap_err();
    assert!(error.is_cli_connection(), "{error}");
    disconnected.unwrap();
    assert!(transport.is_closed());
    assert_not_connected(client.interrupt().await, "interrupt");
}

/// Upstream `async with` awaits `disconnect()` on leaving the block, even
/// on an exception; `scoped` does on both outcomes, before it returns.
#[tokio::test(start_paused = true)]
async fn scoped_closes_the_session_before_returning_on_success_and_on_error() {
    let transport = cli();
    let client = ClaudeSdkClient::with_transport(default_options(), transport.clone());
    let answer = client
        .scoped(async |client| {
            client.interrupt().await?;
            Ok::<_, Error>(42)
        })
        .await
        .unwrap();
    assert_eq!(answer, 42);
    assert_eq!(transport.close_count(), 1);
    assert_not_connected(client.interrupt().await, "interrupt");

    let transport = cli();
    let client = ClaudeSdkClient::with_transport(default_options(), transport.clone());
    let failed = client
        .scoped(async |_client| {
            Err::<(), claude_agent_rust_sdk::BoxError>("the body failed".into())
        })
        .await
        .unwrap_err();
    assert_eq!(failed.to_string(), "the body failed");
    assert_eq!(transport.close_count(), 1);
}

/// Upstream's `receive_messages` is an async generator: the connection is
/// checked at the first iteration, so an iterator created before `connect`
/// reads the session opened since.
#[tokio::test(start_paused = true)]
async fn a_message_stream_created_before_connect_reads_the_session_connected_since() {
    let transport = cli();
    let client = ClaudeSdkClient::with_transport(default_options(), transport.clone());
    let mut messages = client.receive_messages();
    client.connect().await.unwrap();
    transport.push_frame(assistant("hello"));
    let first = messages.next().await.unwrap().unwrap();
    assert!(matches!(first, Message::Assistant(_)), "{first:?}");
    drop(messages);

    let unconnected = ClaudeSdkClient::with_transport(default_options(), cli());
    let mut messages = unconnected.receive_response();
    assert_not_connected(messages.next().await.unwrap(), "receive_response");
    assert!(messages.next().await.is_none());
}

/// Upstream returns `None` from `get_server_info` while `connect` waits
/// for the initialize response, and exposes the options it was given.
#[tokio::test(start_paused = true)]
async fn server_info_is_none_while_connecting_and_the_options_stay_readable() {
    let options = ClaudeAgentOptions::builder().max_turns(3).build().unwrap();
    let client = Arc::new(ClaudeSdkClient::with_transport(
        options,
        common::mock::silent_cli(),
    ));
    assert_eq!(client.options().max_turns, Some(3));
    assert_not_connected(client.server_info(), "server_info");
    let connecting = {
        let client = Arc::clone(&client);
        tokio::spawn(async move { client.connect().await })
    };
    settle().await;
    assert_eq!(client.server_info().unwrap(), None);
    client.disconnect().await.unwrap();
    assert!(connecting.await.unwrap().is_err());
    assert_not_connected(client.server_info(), "server_info");
}
