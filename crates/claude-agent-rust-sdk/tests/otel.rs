#![cfg(feature = "otel")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use claude_agent_rust_sdk::{ClaudeAgentOptionsBuilder, ClaudeSdkClient, Message, query};
use futures_util::StreamExt;
use opentelemetry::propagation::text_map_propagator::FieldIter;
use opentelemetry::propagation::{Extractor, Injector, TextMapPropagator};
use opentelemetry::trace::{SpanContext, SpanId, TraceContextExt, TraceFlags, TraceId, TraceState};
use opentelemetry::{Context, ContextGuard};
use opentelemetry_sdk::propagation::TraceContextPropagator;
use serde_json::{Value, json};
use tokio::sync::{Mutex, MutexGuard};
use tracing::Level;

use common::Fake;
use common::capture::Warnings;

const TRACE_ID: &str = "0af7651916cd43dd8448eb211c80319c";
const SPAN_ID: &str = "b7ad6b7169203331";
const TRACE_VARIABLES: [&str; 2] = ["TRACEPARENT", "TRACESTATE"];

static SERIAL: Mutex<()> = Mutex::const_new(());

struct Ambient {
    _serial: MutexGuard<'static, ()>,
}

impl Ambient {
    async fn with(variables: &[(&str, &str)]) -> Self {
        let serial = SERIAL.lock().await;
        for name in TRACE_VARIABLES {
            unsafe { std::env::remove_var(name) };
        }
        for (name, value) in variables {
            unsafe { std::env::set_var(name, value) };
        }
        opentelemetry::global::set_text_map_propagator(TraceContextPropagator::new());
        Self { _serial: serial }
    }
}

impl Drop for Ambient {
    fn drop(&mut self) {
        for name in TRACE_VARIABLES {
            unsafe { std::env::remove_var(name) };
        }
    }
}

fn active_span(state: &str) -> ContextGuard {
    let span = SpanContext::new(
        TraceId::from_hex(TRACE_ID).unwrap(),
        SpanId::from_hex(SPAN_ID).unwrap(),
        TraceFlags::SAMPLED,
        true,
        state.parse::<TraceState>().unwrap(),
    );
    Context::new().with_remote_span_context(span).attach()
}

fn trace_parent() -> String {
    format!("00-{TRACE_ID}-{SPAN_ID}-01")
}

fn scenario() -> Fake {
    Fake::new(json!({
        "report_env": TRACE_VARIABLES,
        "auto_reply": true,
        "steps": [
            {"expect": {"type": "user"}},
            {"emit": {"type": "result", "subtype": "success", "duration_ms": 1, "duration_api_ms": 1,
                      "is_error": false, "num_turns": 1, "session_id": "s", "result": "ok"}},
        ],
    }))
}

async fn environment_seen_by_the_cli(options: ClaudeAgentOptionsBuilder) -> Value {
    let mut messages = query("Hello", options.build().unwrap());
    let mut reported = None;
    while let Some(message) = messages.next().await {
        if let Message::System(system) = message.unwrap()
            && system.subtype() == "init"
        {
            reported = Some(system.data()["fake_env"].clone());
        }
    }
    reported.expect("the fake CLI sent no init frame")
}

#[tokio::test]
async fn the_active_context_reaches_the_cli_with_upper_case_names() {
    let _ambient = Ambient::with(&[]).await;
    let fake = scenario();
    let _span = active_span("vendor=value");

    let reported = environment_seen_by_the_cli(fake.options()).await;

    assert_eq!(reported["TRACEPARENT"], trace_parent());
    assert_eq!(reported["TRACESTATE"], "vendor=value");
}

#[tokio::test]
async fn inherited_trace_variables_are_replaced_by_the_fresh_values() {
    let _ambient = Ambient::with(&[
        (
            "TRACEPARENT",
            "00-11111111111111111111111111111111-2222222222222222-00",
        ),
        ("TRACESTATE", "stale=1"),
    ])
    .await;
    let fake = scenario();
    let _span = active_span("");

    let reported = environment_seen_by_the_cli(fake.options()).await;

    assert_eq!(reported["TRACEPARENT"], trace_parent());
    assert_ne!(reported["TRACESTATE"], "stale=1");
    assert!(
        reported["TRACESTATE"] == Value::Null || reported["TRACESTATE"] == "",
        "{reported}"
    );
}

#[tokio::test]
async fn trace_variables_set_in_the_options_are_never_overwritten() {
    let _ambient = Ambient::with(&[("TRACESTATE", "stale=1")]).await;
    let fake = scenario();
    let _span = active_span("vendor=value");
    let explicit = "00-33333333333333333333333333333333-4444444444444444-01";

    let reported =
        environment_seen_by_the_cli(fake.options().env_var("TRACEPARENT", explicit)).await;

    assert_eq!(reported["TRACEPARENT"], explicit);
    assert_eq!(reported["TRACESTATE"], "vendor=value");
}

#[tokio::test]
async fn without_an_active_span_inherited_trace_variables_are_left_untouched() {
    let inherited = "00-55555555555555555555555555555555-6666666666666666-01";
    let _ambient =
        Ambient::with(&[("TRACEPARENT", inherited), ("TRACESTATE", "inherited=1")]).await;
    let fake = scenario();

    let reported = environment_seen_by_the_cli(fake.options()).await;

    assert_eq!(reported["TRACEPARENT"], inherited);
    assert_eq!(reported["TRACESTATE"], "inherited=1");
}

#[tokio::test]
async fn without_an_active_span_no_trace_variable_is_added() {
    let _ambient = Ambient::with(&[]).await;
    let fake = scenario();

    let reported = environment_seen_by_the_cli(fake.options()).await;

    assert_eq!(reported["TRACEPARENT"], Value::Null);
    assert_eq!(reported["TRACESTATE"], Value::Null);
}

#[derive(Debug)]
struct PanickingPropagator;

impl TextMapPropagator for PanickingPropagator {
    fn inject_context(&self, _context: &Context, _injector: &mut dyn Injector) {
        panic!("propagator failure");
    }

    fn extract_with_context(&self, context: &Context, _extractor: &dyn Extractor) -> Context {
        context.clone()
    }

    fn fields(&self) -> FieldIter<'_> {
        FieldIter::new(&[])
    }
}

#[tokio::test]
async fn an_injection_failure_is_logged_and_the_connection_proceeds() {
    let inherited = "00-77777777777777777777777777777777-8888888888888888-01";
    let _ambient = Ambient::with(&[("TRACEPARENT", inherited)]).await;
    opentelemetry::global::set_text_map_propagator(PanickingPropagator);
    let previous_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let (logs, _guard) = Warnings::install_at(Level::DEBUG);
    let fake = scenario();
    let _span = active_span("");

    let client = ClaudeSdkClient::new(fake.build());
    let connected = client.connect().await;
    let observed = scenario();
    let reported = environment_seen_by_the_cli(observed.options()).await;
    std::panic::set_hook(previous_hook);

    connected.unwrap();
    assert_eq!(reported["TRACEPARENT"], inherited);
    assert_eq!(reported["TRACESTATE"], Value::Null);
    let server_info = client.server_info().unwrap();
    client.disconnect().await.unwrap();
    assert!(server_info.is_some());
    assert!(
        logs.all()
            .iter()
            .any(|message| message == "OTEL trace context injection failed"),
        "{:?}",
        logs.all()
    );
}
