#![cfg(feature = "testing")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::cell::RefCell;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, Once};

use claude_agent_rust_sdk::{
    ClaudeAgentOptions, HookCallback, HookMatcher, HookOutput, InputMessage, PermissionResult,
    Prompt, SdkMcpTool, SessionKey, SessionStore, SessionStoreEntry, SessionStoreError,
    StderrCallback, SubprocessTransport, ToolResult, create_sdk_mcp_server, query_with_transport,
};
use futures_util::{StreamExt, stream};
use serde_json::json;
use tracing::span::{Attributes, Id, Record};
use tracing::subscriber::Interest;
use tracing::{Event, Instrument, Metadata, Subscriber};
use tracing_core::span::Current;

use common::Fake;
use common::mcp::SdkMcpClient;
use common::mock::{Recorder, cli, connected, hook_request, response_to, result, settle};

thread_local! {
    /// The spans entered on this thread, the innermost last.
    static ENTERED: RefCell<Vec<u64>> = const { RefCell::new(Vec::new()) };
}

static NEXT_SPAN: AtomicU64 = AtomicU64::new(1);
static SPANS: Mutex<Vec<(u64, &'static Metadata<'static>)>> = Mutex::new(Vec::new());

/// A subscriber that only tracks which span each thread is in, which is
/// what `Span::current` and `Instrument::in_current_span` read.
struct SpanTracker;

fn metadata_of(id: u64) -> Option<&'static Metadata<'static>> {
    SPANS
        .lock()
        .unwrap()
        .iter()
        .find(|(known, _)| *known == id)
        .map(|(_, metadata)| *metadata)
}

impl Subscriber for SpanTracker {
    fn register_callsite(&self, _metadata: &'static Metadata<'static>) -> Interest {
        Interest::always()
    }

    fn enabled(&self, _metadata: &Metadata<'_>) -> bool {
        true
    }

    fn new_span(&self, attributes: &Attributes<'_>) -> Id {
        let id = NEXT_SPAN.fetch_add(1, Ordering::SeqCst);
        SPANS.lock().unwrap().push((id, attributes.metadata()));
        Id::from_u64(id)
    }

    fn record(&self, _span: &Id, _values: &Record<'_>) {}

    fn record_follows_from(&self, _span: &Id, _follows: &Id) {}

    fn event(&self, _event: &Event<'_>) {}

    fn enter(&self, span: &Id) {
        ENTERED.with(|entered| entered.borrow_mut().push(span.into_u64()));
    }

    fn exit(&self, _span: &Id) {
        ENTERED.with(|entered| entered.borrow_mut().pop());
    }

    fn current_span(&self) -> Current {
        let innermost = ENTERED.with(|entered| entered.borrow().last().copied());
        match innermost.and_then(|id| metadata_of(id).map(|metadata| (id, metadata))) {
            Some((id, metadata)) => Current::new(Id::from_u64(id), metadata),
            None => Current::none(),
        }
    }
}

fn track_spans() {
    static INSTALL: Once = Once::new();
    INSTALL.call_once(|| {
        tracing::subscriber::set_global_default(SpanTracker).unwrap();
    });
}

fn current_span_name() -> Option<&'static str> {
    tracing::Span::current()
        .metadata()
        .map(|metadata| metadata.name())
}

#[tokio::test(start_paused = true)]
async fn callbacks_run_in_the_span_of_the_caller_that_connected() {
    track_spans();
    let seen = Recorder::<(&'static str, Option<&'static str>)>::new();
    let options = {
        let permission = seen.clone();
        let hook = seen.clone();
        ClaudeAgentOptions::builder()
            .can_use_tool(move |_tool_name, _input, _context| {
                permission.push(("can_use_tool", current_span_name()));
                async { Ok(PermissionResult::allow()) }
            })
            .hook(
                "PreToolUse",
                HookMatcher::new().hook(HookCallback::new(
                    move |_input, _tool_use_id, _context| {
                        hook.push(("hook", current_span_name()));
                        async { Ok(HookOutput::default()) }
                    },
                )),
            )
            .build()
            .unwrap()
    };
    let transport = cli();
    let client = connected(&transport, options)
        .instrument(tracing::info_span!("caller"))
        .await;

    transport.push_frame(json!({
        "type": "control_request",
        "request_id": "perm-1",
        "request": {"subtype": "can_use_tool", "tool_name": "Bash", "input": {}},
    }));
    transport.push_frame(hook_request("hook-1", "hook_0"));
    settle().await;

    assert!(response_to(&transport, "perm-1").is_some());
    assert!(response_to(&transport, "hook-1").is_some());
    let mut seen = seen.all();
    seen.sort_unstable();
    assert_eq!(
        seen,
        [("can_use_tool", Some("caller")), ("hook", Some("caller"))]
    );
    client.disconnect().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn in_process_tools_run_in_the_span_of_the_caller_that_connected() {
    track_spans();
    let seen = Recorder::<Option<&'static str>>::new();
    let tool = {
        let seen = seen.clone();
        SdkMcpTool::raw(
            "probe",
            "Records the span it runs in",
            json!({"type": "object", "properties": {}}),
            move |_arguments| {
                seen.push(current_span_name());
                async { Ok(ToolResult::default()) }
            },
        )
        .unwrap()
    };
    let server = create_sdk_mcp_server("srv", None, [tool]).unwrap();
    let client = SdkMcpClient::start([("srv", server)])
        .instrument(tracing::info_span!("caller"))
        .await;
    client.initialize("srv").await;

    let answer = client.call_tool("srv", "probe", json!({})).await;

    assert_eq!(answer["isError"], false);
    assert_eq!(seen.all(), [Some("caller")]);
    client.close().await;
}

/// A store that records the span each append runs in.
#[derive(Clone)]
struct SpanStore(Recorder<Option<&'static str>>);

impl SessionStore for SpanStore {
    async fn append(
        &self,
        _key: &SessionKey,
        _entries: &[SessionStoreEntry],
    ) -> Result<(), SessionStoreError> {
        self.0.push(current_span_name());
        Ok(())
    }

    async fn load(
        &self,
        _key: &SessionKey,
    ) -> Result<Option<Vec<SessionStoreEntry>>, SessionStoreError> {
        Ok(None)
    }
}

#[tokio::test(start_paused = true)]
async fn the_session_store_runs_in_the_span_of_the_caller_that_connected() {
    track_spans();
    let seen = Recorder::<Option<&'static str>>::new();
    let config = tempfile::tempdir().unwrap();
    let options = ClaudeAgentOptions::builder()
        .session_store(SpanStore(seen.clone()))
        .env_var("CLAUDE_CONFIG_DIR", config.path().to_string_lossy())
        .build()
        .unwrap();
    let transport = cli();
    let client = connected(&transport, options)
        .instrument(tracing::info_span!("caller"))
        .await;
    let file = config.path().join("projects").join("proj").join("s.jsonl");
    transport.push_frame(json!({
        "type": "transcript_mirror",
        "filePath": file.to_string_lossy(),
        "entries": [{"type": "user", "uuid": "u1", "sessionId": "s"}],
    }));
    transport.push_frame(result("done"));
    let mut messages = client.receive_response();
    while let Some(message) = messages.next().await {
        message.unwrap();
    }
    drop(messages);
    assert_eq!(seen.all(), [Some("caller")]);
    client.disconnect().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn the_input_stream_is_read_in_the_span_of_the_caller_that_reads_the_query() {
    track_spans();
    let seen = Recorder::<Option<&'static str>>::new();
    let probe = seen.clone();
    let prompt = Prompt::stream(stream::iter([()]).map(move |()| {
        probe.push(current_span_name());
        InputMessage::user("Hello")
    }));
    let transport = cli();
    let stream = query_with_transport(prompt, ClaudeAgentOptions::default(), transport.clone());
    let reader = tokio::spawn(
        stream
            .collect::<Vec<_>>()
            .instrument(tracing::info_span!("caller")),
    );
    settle().await;
    transport.push_frame(result("done"));
    let messages = reader.await.unwrap();
    assert!(messages.iter().all(Result::is_ok), "{messages:?}");
    assert_eq!(seen.all(), [Some("caller")]);
}

#[tokio::test]
async fn the_stderr_callback_runs_in_the_span_of_the_caller_that_connected() {
    track_spans();
    let seen = Recorder::<Option<&'static str>>::new();
    let probe = seen.clone();
    let callback = StderrCallback::from_sync(move |_line| probe.push(current_span_name()));
    let fake = Fake::new(json!({
        "steps": [{"stderr": "one line\n"}, {"wait_for_eof": true}],
    }));
    let transport = SubprocessTransport::new(fake.options().stderr(callback).build().unwrap());
    transport
        .connect()
        .instrument(tracing::info_span!("caller"))
        .await
        .unwrap();
    let deadline = tokio::time::Instant::now() + common::STEP_LIMIT;
    while seen.all().is_empty() {
        assert!(tokio::time::Instant::now() < deadline, "no stderr line");
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    transport.close().await.unwrap();
    assert_eq!(seen.all(), [Some("caller")]);
}
