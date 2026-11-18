#![cfg(feature = "testing")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::time::Duration;

use claude_agent_kit::testing::MockTransport;
use claude_agent_kit::{
    ClaudeAgentOptions, Error, HookMatcher, InputMessage, Message, PermissionResult, Prompt,
    create_sdk_mcp_server, query_with_transport,
};
use futures_util::{StreamExt, stream};
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use common::mock::{
    assistant, cli, options_with_hook, passive_hook, result, result_texts, session_state, settle,
    subagent_assistant, task_notification, task_started, task_updated, visible_session_state,
    written,
};

const CEILING_VARIABLE: &str = "CLAUDE_CODE_PRINT_BG_WAIT_CEILING_MS";
const DEFAULT_CEILING: Duration = Duration::from_millis(600_000);

struct Run {
    transport: MockTransport,
    messages: JoinHandle<Vec<Result<Message, Error>>>,
}

impl Run {
    async fn start(options: ClaudeAgentOptions) -> Self {
        Self::start_with("Hello", options).await
    }

    async fn start_with(prompt: impl Into<Prompt>, options: ClaudeAgentOptions) -> Self {
        let transport = cli();
        let stream = query_with_transport(prompt, options, transport.clone());
        let messages = tokio::spawn(stream.collect::<Vec<_>>());
        settle().await;
        Self {
            transport,
            messages,
        }
    }

    async fn emit(&self, frames: impl IntoIterator<Item = Value>) {
        for frame in frames {
            self.transport.push_frame(frame);
        }
        settle().await;
    }

    fn stdin_is_open(&self) -> bool {
        !self.transport.input_ended()
    }

    async fn messages(self) -> Vec<Result<Message, Error>> {
        assert!(!self.stdin_is_open(), "the run never closed stdin");
        self.messages.await.unwrap()
    }
}

fn with_ceiling(value: &str) -> ClaudeAgentOptions {
    ClaudeAgentOptions::builder()
        .hook("PreToolUse", HookMatcher::new().hook(passive_hook()))
        .env_var(CEILING_VARIABLE, value)
        .build()
        .unwrap()
}

fn ambient_ceiling_is_unset() -> bool {
    std::env::var_os(CEILING_VARIABLE).is_none()
}

fn held_open_input() -> (mpsc::UnboundedSender<Value>, Prompt) {
    let (sender, mut receiver) = mpsc::unbounded_channel::<Value>();
    let prompt = Prompt::stream(stream::poll_fn(move |context| receiver.poll_recv(context)));
    (sender, prompt)
}

fn background_tasks_changed(tasks: Value) -> Value {
    json!({"type": "system", "subtype": "background_tasks_changed", "tasks": tasks})
}

fn state_frames(messages: &[Result<Message, Error>]) -> Vec<String> {
    messages
        .iter()
        .filter_map(|message| match message {
            Ok(Message::System(system)) if system.subtype() == "session_state_changed" => {
                Some(system.data()["state"].as_str().unwrap().to_owned())
            }
            _ => None,
        })
        .collect()
}

#[tokio::test(start_paused = true)]
async fn without_callbacks_stdin_closes_as_soon_as_the_input_is_written() {
    let run = Run::start(ClaudeAgentOptions::default()).await;
    assert_eq!(written(&run.transport, "user").len(), 1);
    assert!(!run.stdin_is_open());
    assert!(run.messages().await.is_empty());

    let prompt = Prompt::stream(stream::iter([
        InputMessage::user("first"),
        InputMessage::user("second"),
    ]));
    let run = Run::start_with(prompt, ClaudeAgentOptions::default()).await;
    assert_eq!(written(&run.transport, "user").len(), 2);
    assert!(!run.stdin_is_open());
}

#[tokio::test(start_paused = true)]
async fn with_a_callback_stdin_stays_open_until_the_run_has_ended() {
    let permission = ClaudeAgentOptions::builder()
        .can_use_tool(async |_tool_name, _input, _context| Ok(PermissionResult::allow()))
        .build()
        .unwrap();
    let in_process_server = ClaudeAgentOptions::builder()
        .mcp_server(
            "greeter",
            create_sdk_mcp_server("greeter", None, []).unwrap(),
        )
        .build()
        .unwrap();
    for options in [options_with_hook(), permission, in_process_server] {
        let run = Run::start(options).await;
        assert!(run.stdin_is_open());
        run.emit([assistant("Hello!")]).await;
        tokio::time::sleep(Duration::from_secs(3600)).await;
        assert!(run.stdin_is_open());
        run.emit([result("Hello!")]).await;
        assert!(!run.stdin_is_open());
        assert_eq!(result_texts(&run.messages().await), ["Hello!"]);
    }
}

#[tokio::test(start_paused = true)]
async fn result_with_a_tracked_task_in_flight_keeps_stdin_open() {
    let drains = [
        task_notification("task-1"),
        task_updated("task-1", "completed"),
    ];
    for task_type in ["local_agent", "local_workflow"] {
        for drain_frame in drains.clone() {
            let run = Run::start(options_with_hook()).await;
            run.emit([
                assistant("Launching"),
                task_started("task-1", task_type),
                result("first"),
            ])
            .await;
            assert!(run.stdin_is_open(), "{task_type}");
            run.emit([task_updated("task-1", "running")]).await;
            assert!(run.stdin_is_open(), "{task_type}");

            run.emit([drain_frame, result("second")]).await;
            assert!(!run.stdin_is_open(), "{task_type}");
            assert_eq!(result_texts(&run.messages().await), ["first", "second"]);
        }
    }
}

#[tokio::test(start_paused = true)]
async fn shells_and_monitors_never_defer_the_close() {
    for task_type in ["local_bash", "monitor_mcp", "remote_agent"] {
        let run = Run::start(options_with_hook()).await;
        run.emit([task_started("task-1", task_type), result("done")])
            .await;
        assert!(!run.stdin_is_open(), "{task_type}");
    }
}

#[tokio::test(start_paused = true)]
async fn background_snapshot_neither_adds_nor_removes_a_tracked_task() {
    let run = Run::start(options_with_hook()).await;
    run.emit([
        background_tasks_changed(json!([
            {"task_id": "task-1", "task_type": "local_agent"},
            {"task_id": "task-2", "task_type": "local_agent"},
        ])),
        result("done"),
    ])
    .await;
    assert!(!run.stdin_is_open());

    let run = Run::start(options_with_hook()).await;
    run.emit([
        task_started("task-1", "local_agent"),
        background_tasks_changed(json!([{"task_id": "shell-1", "task_type": "local_bash"}])),
        result("first"),
    ])
    .await;
    assert!(run.stdin_is_open());
    run.emit([background_tasks_changed(json!([])), result("second")])
        .await;
    assert!(run.stdin_is_open());
    run.emit([task_notification("task-1"), result("third")])
        .await;
    assert!(!run.stdin_is_open());
    assert_eq!(
        result_texts(&run.messages().await),
        ["first", "second", "third"]
    );
}

#[tokio::test(start_paused = true)]
async fn stream_prompt_with_an_in_process_server_keeps_stdin_open_until_the_result() {
    let options = ClaudeAgentOptions::builder()
        .mcp_server(
            "greeter",
            create_sdk_mcp_server("greeter", None, []).unwrap(),
        )
        .build()
        .unwrap();
    let input = json!({"type": "user", "message": {"role": "user", "content": "Hello"}});
    let prompt = Prompt::stream(stream::iter([input.clone()]));
    let run = Run::start_with(prompt, options).await;
    assert_eq!(written(&run.transport, "user"), [input]);
    assert!(run.stdin_is_open());
    run.emit([assistant("Hello!")]).await;
    tokio::time::sleep(Duration::from_secs(3600)).await;
    assert!(run.stdin_is_open());
    run.emit([result("Hello!")]).await;
    assert!(!run.stdin_is_open());

    let messages = run.messages().await;
    assert_eq!(messages.len(), 2);
    assert!(matches!(messages[0], Ok(Message::Assistant(_))));
    assert!(matches!(messages[1], Ok(Message::Result(_))));
}

#[tokio::test(start_paused = true)]
async fn task_settled_before_the_result_keeps_stdin_open_until_idle() {
    let run = Run::start(with_ceiling("0")).await;
    run.emit([
        session_state("running"),
        task_started("task-1", "local_agent"),
        task_notification("task-1"),
        result("first"),
    ])
    .await;
    assert!(run.stdin_is_open());
    run.emit([result("second")]).await;
    assert!(run.stdin_is_open());
    run.emit([session_state("idle")]).await;
    assert!(!run.stdin_is_open());

    let messages = run.messages().await;
    assert_eq!(result_texts(&messages), ["first", "second"]);
    assert!(state_frames(&messages).is_empty());
}

#[tokio::test(start_paused = true)]
async fn only_frames_marked_for_the_sdk_are_hidden_and_both_kinds_drive_the_run_end() {
    type Frame = fn(&str) -> Value;
    let cases: [(Frame, &[&str]); 2] = [
        (session_state, &[]),
        (visible_session_state, &["running", "idle"]),
    ];
    for (frame, expected) in cases {
        let run = Run::start(with_ceiling("0")).await;
        run.emit([frame("running"), result("done")]).await;
        assert!(run.stdin_is_open());
        run.emit([frame("idle")]).await;
        assert!(!run.stdin_is_open());
        assert_eq!(state_frames(&run.messages().await), expected);
    }
}

#[tokio::test(start_paused = true)]
async fn idle_ends_the_run_only_after_a_result() {
    let run = Run::start(with_ceiling("0")).await;
    run.emit([session_state("running"), session_state("idle")])
        .await;
    assert!(run.stdin_is_open());
    run.emit([result("done")]).await;
    assert!(!run.stdin_is_open());

    let run = Run::start(with_ceiling("0")).await;
    run.emit([
        session_state("idle"),
        session_state("running"),
        result("done"),
    ])
    .await;
    assert!(run.stdin_is_open());
    run.emit([session_state("idle")]).await;
    assert!(!run.stdin_is_open());
}

#[tokio::test(start_paused = true)]
async fn idle_with_a_tracked_task_in_flight_keeps_stdin_open() {
    let run = Run::start(with_ceiling("0")).await;
    run.emit([
        session_state("running"),
        task_started("task-1", "local_agent"),
        result("first"),
        session_state("idle"),
    ])
    .await;
    assert!(run.stdin_is_open());
    run.emit([
        task_notification("task-1"),
        session_state("running"),
        result("second"),
        session_state("idle"),
    ])
    .await;
    assert!(!run.stdin_is_open());
}

#[tokio::test(start_paused = true)]
async fn stream_prompt_waits_for_the_run_of_its_last_message() {
    let (input, prompt) = held_open_input();
    let run = Run::start_with(prompt, with_ceiling("0")).await;
    input.send(InputMessage::user("first").into()).unwrap();
    settle().await;
    run.emit([
        session_state("running"),
        result("first"),
        session_state("idle"),
    ])
    .await;
    assert!(run.stdin_is_open());

    input.send(InputMessage::user("second").into()).unwrap();
    drop(input);
    settle().await;
    assert_eq!(written(&run.transport, "user").len(), 2);
    assert!(run.stdin_is_open());
    run.emit([session_state("running"), result("second")]).await;
    assert!(run.stdin_is_open());
    run.emit([session_state("idle")]).await;
    assert!(!run.stdin_is_open());
}

#[tokio::test(start_paused = true)]
async fn stream_reopens_for_work_the_cli_takes_up_after_idle() {
    for wake_state in ["running", "requires_action"] {
        let (input, prompt) = held_open_input();
        let run = Run::start_with(prompt, with_ceiling("0")).await;
        input.send(InputMessage::user("go").into()).unwrap();
        settle().await;
        run.emit([
            session_state("running"),
            result("first"),
            session_state("idle"),
        ])
        .await;
        run.emit([session_state(wake_state)]).await;
        drop(input);
        settle().await;
        assert!(run.stdin_is_open(), "{wake_state}");
        run.emit([result("second"), session_state("idle")]).await;
        assert!(!run.stdin_is_open(), "{wake_state}");
    }
}

#[tokio::test(start_paused = true)]
async fn main_thread_activity_after_a_result_reopens_the_run() {
    let main_thread_stream_event = json!({
        "type": "stream_event",
        "uuid": "e1",
        "session_id": "test",
        "event": {"type": "message_start"},
        "parent_tool_use_id": null,
    });
    let subagent_stream_event = json!({
        "type": "stream_event",
        "uuid": "e2",
        "session_id": "test",
        "event": {"type": "message_start"},
        "parent_tool_use_id": "toolu_parent",
    });
    let cases = [
        (assistant("more"), true),
        (main_thread_stream_event, true),
        (subagent_assistant("more"), false),
        (subagent_stream_event, false),
        (json!({"type": "system", "subtype": "status"}), false),
    ];
    for (frame, reopens) in cases {
        let (input, prompt) = held_open_input();
        let run = Run::start_with(prompt, options_with_hook()).await;
        input.send(InputMessage::user("go").into()).unwrap();
        settle().await;
        run.emit([result("first")]).await;
        assert!(run.stdin_is_open());

        run.emit([frame.clone()]).await;
        drop(input);
        settle().await;
        assert_eq!(run.stdin_is_open(), reopens, "{frame}");
        if reopens {
            run.emit([result("second")]).await;
            assert!(!run.stdin_is_open(), "{frame}");
        }
    }
}

#[tokio::test(start_paused = true)]
async fn ceiling_ends_the_run_when_idle_never_comes() {
    let run = Run::start(with_ceiling("300")).await;
    run.emit([
        session_state("running"),
        assistant("Launching"),
        result("launched"),
    ])
    .await;
    tokio::time::sleep(Duration::from_millis(297)).await;
    assert!(run.stdin_is_open());
    tokio::time::sleep(Duration::from_millis(3)).await;
    assert!(!run.stdin_is_open());
    assert_eq!(result_texts(&run.messages().await), ["launched"]);
}

#[tokio::test(start_paused = true)]
async fn ceiling_defaults_to_ten_minutes_and_a_non_integer_gives_the_default() {
    if !ambient_ceiling_is_unset() {
        return;
    }
    for options in [
        options_with_hook(),
        with_ceiling("soon"),
        with_ceiling("-5"),
    ] {
        let run = Run::start(options).await;
        run.emit([session_state("running"), result("launched")])
            .await;
        tokio::time::sleep(DEFAULT_CEILING - Duration::from_millis(5)).await;
        assert!(run.stdin_is_open());
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert!(!run.stdin_is_open());
    }
}

#[tokio::test(start_paused = true)]
async fn ceiling_of_zero_waits_for_idle_and_a_huge_one_is_capped() {
    let run = Run::start(with_ceiling("0")).await;
    run.emit([session_state("running"), result("launched")])
        .await;
    tokio::time::sleep(Duration::from_secs(60 * 86_400)).await;
    assert!(run.stdin_is_open());
    run.emit([session_state("idle")]).await;
    assert!(!run.stdin_is_open());

    let run = Run::start(with_ceiling("99999999999999999999")).await;
    run.emit([session_state("running"), result("launched")])
        .await;
    tokio::time::sleep(Duration::from_millis(2_147_483_647 - 5)).await;
    assert!(run.stdin_is_open());
    tokio::time::sleep(Duration::from_millis(10)).await;
    assert!(!run.stdin_is_open());
}

#[tokio::test(start_paused = true)]
async fn main_thread_turn_stops_the_ceiling_and_subagent_messages_do_not() {
    let run = Run::start(with_ceiling("300")).await;
    run.emit([session_state("running"), result("launched")])
        .await;
    run.emit([assistant("a follow-up turn")]).await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert!(run.stdin_is_open());
    run.emit([result("follow-up")]).await;
    tokio::time::sleep(Duration::from_millis(297)).await;
    assert!(run.stdin_is_open());
    tokio::time::sleep(Duration::from_millis(3)).await;
    assert!(!run.stdin_is_open());

    let run = Run::start(with_ceiling("300")).await;
    run.emit([session_state("running"), result("launched")])
        .await;
    run.emit([subagent_assistant("still working")]).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!run.stdin_is_open());
}

#[tokio::test(start_paused = true)]
async fn requires_action_stops_the_ceiling_until_the_cli_runs_again() {
    let run = Run::start(with_ceiling("300")).await;
    run.emit([session_state("running"), result("launched")])
        .await;
    run.emit([session_state("requires_action")]).await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert!(run.stdin_is_open());

    run.emit([session_state("running")]).await;
    tokio::time::sleep(Duration::from_millis(297)).await;
    assert!(run.stdin_is_open());
    tokio::time::sleep(Duration::from_millis(3)).await;
    assert!(!run.stdin_is_open());

    let run = Run::start(with_ceiling("300")).await;
    run.emit([session_state("requires_action"), result("launched")])
        .await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert!(run.stdin_is_open());
    run.emit([session_state("idle")]).await;
    assert!(!run.stdin_is_open());
}

#[tokio::test(start_paused = true)]
async fn ceiling_is_not_armed_while_a_turn_is_under_way() {
    let run = Run::start(with_ceiling("300")).await;
    run.emit([session_state("running"), result("launched")])
        .await;
    run.emit([assistant("a follow-up turn"), session_state("running")])
        .await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert!(run.stdin_is_open());
    run.emit([result("follow-up"), session_state("idle")]).await;
    assert!(!run.stdin_is_open());
}

#[tokio::test(start_paused = true)]
async fn ceiling_leaves_a_tracked_agent_alone_and_restarts_once_it_settles() {
    let run = Run::start(with_ceiling("300")).await;
    run.emit([
        session_state("running"),
        task_started("task-1", "local_agent"),
        result("launched"),
    ])
    .await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert!(run.stdin_is_open());

    run.emit([task_notification("task-1")]).await;
    tokio::time::sleep(Duration::from_millis(297)).await;
    assert!(run.stdin_is_open());
    tokio::time::sleep(Duration::from_millis(3)).await;
    assert!(!run.stdin_is_open());
}

#[tokio::test(start_paused = true)]
async fn stream_ended_by_the_ceiling_reopens_for_a_main_thread_turn() {
    let (input, prompt) = held_open_input();
    let run = Run::start_with(prompt, with_ceiling("300")).await;
    input.send(InputMessage::user("go").into()).unwrap();
    settle().await;
    run.emit([session_state("running"), result("launched")])
        .await;
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert!(run.stdin_is_open());

    run.emit([assistant("woken by a background task")]).await;
    drop(input);
    settle().await;
    assert!(run.stdin_is_open());
    run.emit([result("follow-up"), session_state("idle")]).await;
    assert!(!run.stdin_is_open());
}

#[tokio::test(start_paused = true)]
async fn empty_or_failing_input_stream_still_closes_stdin() {
    let empty = Prompt::stream(stream::empty::<Value>());
    let run = Run::start_with(empty, options_with_hook()).await;
    assert!(written(&run.transport, "user").is_empty());
    assert!(!run.stdin_is_open());

    let fails_at_once = Prompt::try_stream(stream::iter([Err::<Value, _>("no prompt today")]));
    let run = Run::start_with(fails_at_once, options_with_hook()).await;
    assert!(written(&run.transport, "user").is_empty());
    assert!(!run.stdin_is_open());

    let fails_after_one = Prompt::try_stream(stream::iter([
        Ok(Value::from(InputMessage::user("first"))),
        Err("the source went away"),
    ]));
    let run = Run::start_with(fails_after_one, options_with_hook()).await;
    assert_eq!(written(&run.transport, "user").len(), 1);
    assert!(run.stdin_is_open());
    run.emit([assistant("Hello!"), result("first")]).await;
    assert!(!run.stdin_is_open());
    assert_eq!(result_texts(&run.messages().await), ["first"]);
}
