#![cfg(feature = "testing")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::time::Duration;

use claude_agent_kit::{
    ClaudeAgentOptions, ClaudeSdkClient, Error, HookMatcher, query_with_transport,
};
use futures_util::StreamExt;

use common::mock::{cli, passive_hook, result, session_state, settle, silent_cli};

const STREAM_CLOSE_TIMEOUT_VARIABLE: &str = "CLAUDE_CODE_STREAM_CLOSE_TIMEOUT";
const CEILING_VARIABLE: &str = "CLAUDE_CODE_PRINT_BG_WAIT_CEILING_MS";

fn hook_options() -> claude_agent_kit::ClaudeAgentOptionsBuilder {
    ClaudeAgentOptions::builder().hook("PreToolUse", HookMatcher::new().hook(passive_hook()))
}

async fn stdin_closes_after(options: ClaudeAgentOptions) -> Duration {
    let transport = cli();
    let stream = query_with_transport("Hello", options, transport.clone());
    let messages = tokio::spawn(stream.collect::<Vec<_>>());
    settle().await;
    transport.push_frame(session_state("running"));
    transport.push_frame(result("launched"));
    let started = tokio::time::Instant::now();
    messages.await.unwrap();
    assert!(transport.input_ended());
    started.elapsed()
}

async fn environment_of_the_process_is_honored() {
    let transport = silent_cli();
    let client = ClaudeSdkClient::with_transport(ClaudeAgentOptions::default(), transport);
    let started = tokio::time::Instant::now();
    let error = client.connect().await.unwrap_err();
    assert!(matches!(&error, Error::ControlTimeout { subtype } if subtype == "initialize"));
    assert_eq!(started.elapsed(), Duration::from_millis(120_000));

    let from_process = stdin_closes_after(hook_options().build().unwrap()).await;
    assert_eq!(from_process, Duration::from_millis(250));

    let from_options = stdin_closes_after(
        hook_options()
            .env_var(CEILING_VARIABLE, "700")
            .build()
            .unwrap(),
    )
    .await;
    assert_eq!(from_options, Duration::from_millis(700));
}

#[test]
fn initialize_timeout_and_run_end_ceiling_read_the_process_environment() {
    unsafe {
        std::env::set_var(STREAM_CLOSE_TIMEOUT_VARIABLE, "120000");
        std::env::set_var(CEILING_VARIABLE, "250");
    }
    tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .start_paused(true)
        .build()
        .unwrap()
        .block_on(environment_of_the_process_is_honored());
}
