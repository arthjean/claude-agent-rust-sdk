//! When a one-shot query closes the input of the CLI.
//!
//! A query with hooks, a permission callback or in-process tools holds
//! the input open so the CLI can ask the SDK questions while it works. A
//! result only ends a turn: a background subagent that settles just
//! before the result still wakes the parent for a follow-up turn, whose
//! requests need the input. The SDK keeps it open until the CLI reports
//! the session idle.

use std::future::Future;
use std::path::Path;

use claude_agent_kit::{
    AgentDefinition, ClaudeAgentOptionsBuilder, ContentBlock, HookCallback, HookMatcher,
    HookPermissionDecision, Message, MessageStream, PreToolUseHookSpecificOutput, SettingSource,
    SubprocessTransport, SyncHookOutput, Transport, query_with_transport,
};
use futures_util::StreamExt;
use serde_json::Value;

use crate::suite::{self, Log, TURN_LIMIT};

const STATE_FRAME: &str = "session_state_changed";
const CALLER_OPT_IN: &str = "CLAUDE_CODE_EMIT_SESSION_STATE_EVENTS";
const SDK_OPT_IN: &str = "CLAUDE_CODE_SDK_READS_SESSION_STATE";

fn options(cwd: &Path) -> ClaudeAgentOptionsBuilder {
    suite::options()
        .cwd(cwd)
        .model("haiku")
        .setting_sources(Vec::<SettingSource>::new())
        .extra_arg("strict-mcp-config", None)
        .system_prompt("Be terse. Follow the user's steps exactly.")
}

fn record_and_allow(asked: &Log<String>) -> HookCallback {
    let asked = asked.clone();
    suite::hook(move |input| {
        if let Some((tool, _)) = suite::tool_call(input) {
            asked.lock().unwrap().push(tool);
        }
        SyncHookOutput::default().hook_specific_output(
            PreToolUseHookSpecificOutput::default()
                .permission_decision(HookPermissionDecision::Allow),
        )
    })
}

fn state_frames(messages: &[Message]) -> Vec<&Value> {
    messages
        .iter()
        .filter_map(|message| match message {
            Message::System(system) if system.subtype() == STATE_FRAME => Some(system.data()),
            _ => None,
        })
        .collect()
}

struct Recorder {
    process: SubprocessTransport,
    frames: Log<Value>,
}

impl Transport for Recorder {
    fn connect(&self) -> impl Future<Output = claude_agent_kit::Result<()>> + Send {
        Transport::connect(&self.process)
    }

    fn write(&self, data: &str) -> impl Future<Output = claude_agent_kit::Result<()>> + Send {
        Transport::write(&self.process, data)
    }

    fn read_messages(&self) -> MessageStream {
        let frames = self.frames.clone();
        Box::pin(
            Transport::read_messages(&self.process).inspect(move |item| {
                if let Ok(frame) = item
                    && frame["subtype"] == STATE_FRAME
                {
                    frames.lock().unwrap().push(frame.clone());
                }
            }),
        )
    }

    fn close(&self) -> impl Future<Output = claude_agent_kit::Result<()>> + Send {
        Transport::close(&self.process)
    }

    fn is_ready(&self) -> bool {
        Transport::is_ready(&self.process)
    }

    fn end_input(&self) -> impl Future<Output = claude_agent_kit::Result<()>> + Send {
        Transport::end_input(&self.process)
    }
}

#[tokio::test]
#[ignore = "runs the real CLI; run with `cargo xtask e2e`"]
async fn a_text_prompt_with_a_hook_ends_with_one_result() {
    let (_directory, cwd) = suite::workspace();
    let asked = suite::log::<String>();
    let options = options(&cwd)
        .tools(Vec::<String>::new())
        .hook(
            "PreToolUse",
            HookMatcher::new().hook(record_and_allow(&asked)),
        )
        .build()
        .unwrap();

    let messages = suite::run("Reply with OK.", options).await;
    suite::forget(&cwd);

    assert!(!suite::result(&messages).is_error, "{messages:#?}");
    assert_eq!(state_frames(&messages), Vec::<&Value>::new());
}

async fn follow_up_turn_after_a_background_subagent(caller_opted_in: bool) {
    for variable in [CALLER_OPT_IN, SDK_OPT_IN] {
        assert!(
            std::env::var_os(variable).is_none(),
            "unset {variable}: this test chooses it through the options of the session"
        );
    }
    let (_directory, cwd) = suite::workspace();
    let asked = suite::log::<String>();
    let frames = suite::log::<Value>();
    let target = cwd.join("out.txt");
    let mut builder = options(&cwd)
        .tools(["Agent", "Write"])
        .hook(
            "PreToolUse",
            HookMatcher::new()
                .matcher("Write")
                .hook(record_and_allow(&asked)),
        )
        .agent(
            "worker",
            AgentDefinition::new(
                "Answers with one word.",
                "Reply with the single word DONE and nothing else.",
            )
            .tools(Vec::<String>::new())
            .model("haiku"),
        );
    if caller_opted_in {
        builder = builder.env_var(CALLER_OPT_IN, "1");
    }
    let options = builder.build().unwrap();
    let prompt = format!(
        "Step 1: call the Agent tool once with subagent_type `worker`, description `say done`, \
         prompt `Say DONE.` and run_in_background true, then end your turn right away with the \
         single word LAUNCHED. Do not wait for it. Step 2: when you are told the subagent \
         finished, use the Write tool to write its reply to {}, then answer FINISHED.",
        target.display()
    );
    let recorder = Recorder {
        process: SubprocessTransport::new(options.clone()),
        frames: frames.clone(),
    };

    let mut stream = query_with_transport(prompt, options, recorder);
    let mut messages = Vec::new();
    let collected = tokio::time::timeout(TURN_LIMIT, async {
        while let Some(item) = stream.next().await {
            match item {
                Ok(message) => messages.push(message),
                Err(error) => return Some(error),
            }
        }
        None
    })
    .await;
    suite::forget(&cwd);
    let error = collected
        .unwrap_or_else(|_| panic!("the query did not end within {TURN_LIMIT:?}: {messages:#?}"));

    let frames = suite::entries(&frames);
    if caller_opted_in {
        assert!(
            !frames.is_empty(),
            "this CLI sends no {STATE_FRAME} frames, so the run cannot be served past the first \
             result"
        );
    } else {
        assert!(
            frames.iter().any(|frame| frame["sdk_host_only"] == true),
            "this CLI predates {SDK_OPT_IN}: it sent no sdk_host_only {STATE_FRAME} frames, \
             got {frames:?}"
        );
    }
    if let Some(error) = error {
        panic!("the query failed: {error}\nmessages: {messages:#?}");
    }

    let states = state_frames(&messages);
    if caller_opted_in {
        assert!(
            !states.is_empty(),
            "the caller opted in, so it sees the state frames"
        );
        assert!(
            !states.iter().any(|state| state["sdk_host_only"] == true),
            "{states:?}"
        );
    } else {
        assert_eq!(states, Vec::<&Value>::new());
    }
    let results = suite::results(&messages);
    assert!(results.len() >= 2, "{messages:#?}");
    assert!(
        !results.iter().any(|result| result.is_error),
        "{results:#?}"
    );
    let first_result = messages
        .iter()
        .position(|message| matches!(message, Message::Result(_)))
        .unwrap();
    let first_write = messages.iter().position(|message| {
        matches!(message, Message::Assistant(assistant)
        if assistant.content.iter().any(|block| {
            matches!(block, ContentBlock::ToolUse(call) if call.name == "Write")
        }))
    });
    assert!(
        first_write.is_some_and(|write| write > first_result),
        "the Write should come from the follow-up turn: first result at {first_result}, first \
         write at {first_write:?}"
    );
    let asked = suite::entries(&asked);
    assert!(asked.contains(&"Write".to_owned()), "{asked:?}");
    assert!(
        target.exists(),
        "the write granted by the hook did not land"
    );
}

#[tokio::test]
#[ignore = "runs the real CLI; run with `cargo xtask e2e`"]
async fn the_follow_up_turn_is_served_when_the_sdk_asks_for_the_state_frames() {
    follow_up_turn_after_a_background_subagent(false).await;
}

#[tokio::test]
#[ignore = "runs the real CLI; run with `cargo xtask e2e`"]
async fn the_follow_up_turn_is_served_when_the_caller_asks_for_the_state_frames() {
    follow_up_turn_after_a_background_subagent(true).await;
}
