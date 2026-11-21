use std::sync::{Arc, Mutex};
use std::time::Duration;

use claude_agent_rust_sdk::testing::MockTransport;
use claude_agent_rust_sdk::{
    ClaudeAgentOptions, ClaudeSdkClient, Error, HookCallback, HookMatcher, HookOutput, Message,
    QueryStream,
};
use futures_util::StreamExt;
use serde_json::{Value, json};

pub fn is_control_request(frame: &Value, subtype: &str) -> bool {
    frame["type"] == "control_request" && frame["request"]["subtype"] == subtype
}

pub fn success(request: &Value, response: Value) -> Value {
    json!({
        "type": "control_response",
        "response": {
            "subtype": "success",
            "request_id": request["request_id"],
            "response": response,
        },
    })
}

pub fn refusal(request: &Value, error: &str) -> Value {
    json!({
        "type": "control_response",
        "response": {
            "subtype": "error",
            "request_id": request["request_id"],
            "error": error,
        },
    })
}

pub fn cli() -> MockTransport {
    cli_answering(|_| Vec::new())
}

pub fn cli_answering(
    on_frame: impl Fn(&Value) -> Vec<Value> + Send + Sync + 'static,
) -> MockTransport {
    let transport = MockTransport::new();
    transport.finish_when_input_ends();
    transport.respond_with(move |frame| {
        let mut replies = on_frame(frame);
        if replies.is_empty() && frame["type"] == "control_request" {
            replies.push(success(frame, json!({})));
        }
        replies
    });
    transport
}

pub fn silent_cli() -> MockTransport {
    let transport = MockTransport::new();
    transport.finish_when_input_ends();
    transport
}

pub fn cli_answering_only_initialize() -> MockTransport {
    let transport = silent_cli();
    transport.respond_with(|frame| {
        if is_control_request(frame, "initialize") {
            vec![success(frame, json!({}))]
        } else {
            Vec::new()
        }
    });
    transport
}

pub fn assistant(text: &str) -> Value {
    json!({
        "type": "assistant",
        "parent_tool_use_id": null,
        "session_id": "test",
        "message": {
            "role": "assistant",
            "model": "claude-sonnet-5",
            "content": [{"type": "text", "text": text}],
        },
    })
}

pub fn subagent_assistant(text: &str) -> Value {
    let mut frame = assistant(text);
    frame["parent_tool_use_id"] = json!("toolu_parent");
    frame
}

pub fn result(text: &str) -> Value {
    json!({
        "type": "result",
        "subtype": "success",
        "duration_ms": 100,
        "duration_api_ms": 80,
        "is_error": false,
        "num_turns": 1,
        "session_id": "test",
        "total_cost_usd": 0.001,
        "result": text,
    })
}

pub fn session_state(state: &str) -> Value {
    json!({
        "type": "system",
        "subtype": "session_state_changed",
        "state": state,
        "uuid": format!("uuid-state-{state}"),
        "session_id": "test",
        "sdk_host_only": true,
    })
}

pub fn visible_session_state(state: &str) -> Value {
    let mut frame = session_state(state);
    frame
        .as_object_mut()
        .unwrap()
        .retain(|name, _| name.as_str() != "sdk_host_only");
    frame
}

pub fn task_started(task_id: &str, task_type: &str) -> Value {
    json!({
        "type": "system",
        "subtype": "task_started",
        "task_id": task_id,
        "task_type": task_type,
        "description": "background work",
        "uuid": format!("uuid-start-{task_id}"),
        "session_id": "test",
    })
}

pub fn task_notification(task_id: &str) -> Value {
    json!({
        "type": "system",
        "subtype": "task_notification",
        "task_id": task_id,
        "status": "completed",
        "output_file": "/tmp/out",
        "summary": "done",
        "uuid": format!("uuid-end-{task_id}"),
        "session_id": "test",
    })
}

pub fn task_updated(task_id: &str, status: &str) -> Value {
    json!({
        "type": "system",
        "subtype": "task_updated",
        "task_id": task_id,
        "patch": {"status": status},
        "uuid": format!("uuid-update-{task_id}"),
        "session_id": "test",
    })
}

pub fn hook_request(request_id: &str, callback_id: &str) -> Value {
    json!({
        "type": "control_request",
        "request_id": request_id,
        "request": {
            "subtype": "hook_callback",
            "callback_id": callback_id,
            "input": {
                "hook_event_name": "PreToolUse",
                "session_id": "test",
                "transcript_path": "/tmp/transcript.jsonl",
                "cwd": "/work",
                "tool_name": "Write",
                "tool_input": {"file_path": "a.txt"},
                "tool_use_id": "toolu_1",
            },
            "tool_use_id": "toolu_1",
        },
    })
}

pub fn passive_hook() -> HookCallback {
    HookCallback::new(async |_input, _tool_use_id, _context| Ok(HookOutput::default()))
}

pub fn options_with_hook() -> ClaudeAgentOptions {
    ClaudeAgentOptions::builder()
        .hook("PreToolUse", HookMatcher::new().hook(passive_hook()))
        .build()
        .unwrap()
}

pub async fn settle() {
    tokio::time::sleep(Duration::from_millis(1)).await;
}

pub async fn connected(transport: &MockTransport, options: ClaudeAgentOptions) -> ClaudeSdkClient {
    let client = ClaudeSdkClient::with_transport(options, transport.clone());
    client.connect().await.unwrap();
    client
}

pub fn written(transport: &MockTransport, kind: &str) -> Vec<Value> {
    transport
        .written_frames()
        .into_iter()
        .filter(|frame| frame["type"] == kind)
        .collect()
}

pub fn responses(transport: &MockTransport) -> Vec<Value> {
    written(transport, "control_response")
        .into_iter()
        .map(|frame| frame["response"].clone())
        .collect()
}

pub fn response_to(transport: &MockTransport, request_id: &str) -> Option<Value> {
    responses(transport)
        .into_iter()
        .find(|response| response["request_id"] == request_id)
}

pub fn requests(transport: &MockTransport, subtype: &str) -> Vec<Value> {
    written(transport, "control_request")
        .into_iter()
        .filter(|frame| frame["request"]["subtype"] == subtype)
        .collect()
}

pub fn initialize_request(transport: &MockTransport) -> Value {
    requests(transport, "initialize")
        .first()
        .expect("no initialize request was written")["request"]
        .clone()
}

pub async fn drain(stream: QueryStream) -> Vec<Result<Message, Error>> {
    tokio::time::timeout(Duration::from_secs(3600), stream.collect())
        .await
        .expect("the query stream stalled")
}

pub fn result_texts(messages: &[Result<Message, Error>]) -> Vec<String> {
    messages
        .iter()
        .filter_map(|message| match message {
            Ok(Message::Result(result)) => Some(result.result.clone().unwrap_or_default()),
            _ => None,
        })
        .collect()
}

pub fn system_subtypes(messages: &[Result<Message, Error>]) -> Vec<String> {
    messages
        .iter()
        .filter_map(|message| match message {
            Ok(Message::System(system)) => Some(system.subtype().to_owned()),
            _ => None,
        })
        .collect()
}

#[derive(Clone, Default)]
pub struct Recorder<T>(Arc<Mutex<Vec<T>>>);

impl<T: Clone> Recorder<T> {
    pub fn new() -> Self {
        Self(Arc::new(Mutex::new(Vec::new())))
    }

    pub fn push(&self, value: T) {
        self.0.lock().unwrap().push(value);
    }

    pub fn all(&self) -> Vec<T> {
        self.0.lock().unwrap().clone()
    }
}
