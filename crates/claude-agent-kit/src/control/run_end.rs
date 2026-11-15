//! Run-end tracking: when the standard input of the CLI can close.
//!
//! A `result` message ends one turn, not necessarily the run. Background
//! agents keep running past it, or have just finished and still wake the
//! session for a follow-up turn, and those turns need the standard input
//! for the answers to hook, permission, and in-process MCP requests. The
//! run ends on the session state `idle` after a result or, from a CLI that
//! reports no session state, on the first result with no tracked task in
//! flight. A ceiling bounds the wait between turns.

use std::collections::HashSet;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::time::Duration;

use indexmap::IndexMap;
use serde_json::Value;
use tokio::runtime::Handle;
use tokio::sync::watch;
use tokio::task::AbortHandle;

use crate::types::is_terminal_task_status;

pub(crate) const RUN_END_CEILING_VARIABLE: &str = "CLAUDE_CODE_PRINT_BG_WAIT_CEILING_MS";
pub(crate) const DEFAULT_RUN_END_CEILING_MS: u64 = 600_000;
pub(crate) const MAX_RUN_END_CEILING_MS: u64 = 2_147_483_647;

/// Task types whose completion runs a follow-up turn. Shells, monitors,
/// teammates, and remote agents are left out on purpose: they may never
/// reach a terminal status, so waiting on one would hold the standard
/// input open forever.
const DEFERRING_TASK_TYPES: [&str; 2] = ["local_agent", "local_workflow"];

const STATE_IDLE: &str = "idle";
const STATE_REQUIRES_ACTION: &str = "requires_action";

/// Reads the run-end ceiling as the CLI will see it: the session
/// environment overrides the environment of this process. `0` means no
/// limit, and a value that is not a non-negative integer gives the default.
pub(crate) fn run_end_ceiling_ms(options_env: &IndexMap<String, String>) -> u64 {
    match options_env.get(RUN_END_CEILING_VARIABLE) {
        Some(raw) => parse_ceiling_ms(Some(raw)),
        None => parse_ceiling_ms(std::env::var(RUN_END_CEILING_VARIABLE).ok().as_deref()),
    }
}

pub(crate) fn parse_ceiling_ms(raw: Option<&str>) -> u64 {
    let Some(raw) = raw else {
        return DEFAULT_RUN_END_CEILING_MS;
    };
    let text = raw.trim();
    let (negative, digits) = match text.strip_prefix('-') {
        Some(digits) => (true, digits),
        None => (false, text.strip_prefix('+').unwrap_or(text)),
    };
    let well_formed = digits.starts_with(|first: char| first.is_ascii_digit())
        && digits.ends_with(|last: char| last.is_ascii_digit())
        && !digits.contains("__")
        && digits
            .chars()
            .all(|character| character.is_ascii_digit() || character == '_');
    if !well_formed {
        return DEFAULT_RUN_END_CEILING_MS;
    }
    let magnitude = digits
        .chars()
        .filter_map(|character| character.to_digit(10))
        .fold(0_u64, |total, digit| {
            total.saturating_mul(10).saturating_add(u64::from(digit))
        });
    if negative && magnitude != 0 {
        return DEFAULT_RUN_END_CEILING_MS;
    }
    magnitude
}

struct Event(watch::Sender<bool>);

impl Event {
    fn new() -> Self {
        Self(watch::Sender::new(false))
    }

    fn set(&self) {
        self.0.send_replace(true);
    }

    fn is_set(&self) -> bool {
        *self.0.borrow()
    }

    fn subscribe(&self) -> watch::Receiver<bool> {
        self.0.subscribe()
    }
}

/// Shared handle to the run-end state of one session.
#[derive(Clone)]
pub(crate) struct RunEndTracker(Arc<Mutex<RunEnd>>);

struct RunEnd {
    this: Weak<Mutex<RunEnd>>,
    ended: Event,
    result_received: bool,
    session_state: Option<String>,
    turn_in_progress: bool,
    run_final: bool,
    closed: bool,
    inflight_tasks: HashSet<String>,
    ceiling: Option<Duration>,
    ceiling_generation: u64,
    ceiling_timer: Option<AbortHandle>,
    bidirectional: bool,
}

fn lock(state: &Mutex<RunEnd>) -> MutexGuard<'_, RunEnd> {
    state.lock().unwrap_or_else(PoisonError::into_inner)
}

impl RunEndTracker {
    /// `bidirectional` tells whether the CLI may still send control
    /// requests that need an answer: hooks, a tool-permission callback, or
    /// an in-process MCP server are configured.
    pub(crate) fn new(ceiling_ms: u64, bidirectional: bool) -> Self {
        let ceiling =
            (ceiling_ms > 0).then(|| Duration::from_millis(ceiling_ms.min(MAX_RUN_END_CEILING_MS)));
        Self(Arc::new_cyclic(|this| {
            Mutex::new(RunEnd {
                this: this.clone(),
                ended: Event::new(),
                result_received: false,
                session_state: None,
                turn_in_progress: false,
                run_final: false,
                closed: false,
                inflight_tasks: HashSet::new(),
                ceiling,
                ceiling_generation: 0,
                ceiling_timer: None,
                bidirectional,
            })
        }))
    }

    /// Observes a `system` frame. Returns whether the frame must be hidden
    /// from the application: a session state frame the CLI sent only
    /// because the SDK asked for it.
    pub(crate) fn observe_system(&self, frame: &Value) -> bool {
        let mut run = lock(&self.0);
        let had_tasks_in_flight = !run.inflight_tasks.is_empty();
        run.track_task_lifecycle(frame);
        if had_tasks_in_flight && run.inflight_tasks.is_empty() {
            run.rearm_ceiling_between_turns();
        }
        if frame.get("subtype").and_then(Value::as_str) != Some("session_state_changed") {
            return false;
        }
        run.on_session_state(frame.get("state"));
        frame.get("sdk_host_only") == Some(&Value::Bool(true))
    }

    /// Observes a `result` frame: one turn ended.
    pub(crate) fn observe_result(&self) {
        let mut run = lock(&self.0);
        run.result_received = true;
        run.turn_in_progress = false;
        let state = run.session_state.as_deref();
        let no_turn_is_owed = matches!(state, None | Some(STATE_IDLE)) || !run.bidirectional;
        let answering_a_request = state == Some(STATE_REQUIRES_ACTION);
        if no_turn_is_owed {
            run.maybe_end_run();
        } else if !answering_a_request {
            run.arm_ceiling();
        }
    }

    /// Observes an `assistant` or `stream_event` frame of the main thread:
    /// a turn is under way, so the run reopens and the ceiling stops.
    pub(crate) fn observe_main_thread_activity(&self) {
        let mut run = lock(&self.0);
        run.turn_in_progress = true;
        run.reopen_run();
        run.clear_ceiling();
    }

    /// A prompt is about to be written: it owes a run of its own, result
    /// included.
    pub(crate) fn begin_prompt(&self) {
        let mut run = lock(&self.0);
        run.reopen_run();
        run.result_received = false;
        run.clear_ceiling();
    }

    /// The reader is gone: the run is over for good.
    pub(crate) fn finish(&self) {
        let mut run = lock(&self.0);
        run.run_final = true;
        run.end_run();
    }

    /// The session is closing: no ceiling may be armed anymore.
    pub(crate) fn mark_closed(&self) {
        let mut run = lock(&self.0);
        run.closed = true;
        run.clear_ceiling();
    }

    /// The standard input is about to close: the run stays ended.
    pub(crate) fn seal(&self) {
        let mut run = lock(&self.0);
        run.run_final = true;
        run.clear_ceiling();
    }

    /// Waits for the end of the run when control requests may still need
    /// an answer; returns at once otherwise.
    pub(crate) async fn wait_for_end(&self) {
        let ended = {
            let run = lock(&self.0);
            if !run.bidirectional {
                return;
            }
            run.ended.subscribe()
        };
        let mut ended = ended;
        let _ = ended.wait_for(|ended| *ended).await;
    }

    #[cfg(test)]
    pub(crate) fn has_ended(&self) -> bool {
        lock(&self.0).ended.is_set()
    }

    #[cfg(test)]
    pub(crate) fn tasks_in_flight(&self) -> Vec<String> {
        let mut tasks: Vec<String> = lock(&self.0).inflight_tasks.iter().cloned().collect();
        tasks.sort();
        tasks
    }

    #[cfg(test)]
    pub(crate) fn ceiling_is_armed(&self) -> bool {
        lock(&self.0).ceiling_timer.is_some()
    }
}

impl RunEnd {
    fn track_task_lifecycle(&mut self, frame: &Value) {
        let Some(task_id) = frame
            .get("task_id")
            .and_then(Value::as_str)
            .filter(|task_id| !task_id.is_empty())
        else {
            return;
        };
        match frame.get("subtype").and_then(Value::as_str) {
            Some("task_started") => {
                let defers = frame
                    .get("task_type")
                    .and_then(Value::as_str)
                    .is_some_and(|task_type| DEFERRING_TASK_TYPES.contains(&task_type));
                if defers {
                    self.inflight_tasks.insert(task_id.to_owned());
                }
            }
            Some("task_notification") => {
                self.inflight_tasks.remove(task_id);
            }
            Some("task_updated") => {
                let terminal = frame
                    .get("patch")
                    .and_then(|patch| patch.get("status"))
                    .and_then(Value::as_str)
                    .is_some_and(is_terminal_task_status);
                if terminal {
                    self.inflight_tasks.remove(task_id);
                }
            }
            _ => {}
        }
    }

    fn on_session_state(&mut self, state: Option<&Value>) {
        self.session_state = match state {
            None | Some(Value::Null) => None,
            Some(Value::String(state)) => Some(state.clone()),
            Some(other) => Some(other.to_string()),
        };
        match self.session_state.as_deref() {
            Some(STATE_IDLE) => {
                if self.result_received {
                    self.maybe_end_run();
                }
            }
            Some(STATE_REQUIRES_ACTION) => {
                self.reopen_run();
                self.clear_ceiling();
            }
            _ => {
                self.reopen_run();
                self.rearm_ceiling_between_turns();
            }
        }
    }

    fn maybe_end_run(&mut self) {
        if !self.inflight_tasks.is_empty() {
            tracing::debug!(
                "Turn ended with {} task(s) in flight; keeping stdin open",
                self.inflight_tasks.len()
            );
            return;
        }
        self.end_run();
    }

    fn end_run(&mut self) {
        self.clear_ceiling();
        self.ended.set();
    }

    fn reopen_run(&mut self) {
        if self.ended.is_set() && !self.run_final {
            self.ended = Event::new();
        }
    }

    fn arm_ceiling(&mut self) {
        self.clear_ceiling();
        let Some(wait) = self.ceiling else {
            return;
        };
        let stands_down = self.ended.is_set()
            || self.run_final
            || self.closed
            || self.turn_in_progress
            || !self.bidirectional;
        if stands_down {
            return;
        }
        let Ok(runtime) = Handle::try_current() else {
            return;
        };
        let timer = runtime.spawn(end_run_at_ceiling(
            self.this.clone(),
            self.ceiling_generation,
            wait,
        ));
        self.ceiling_timer = Some(timer.abort_handle());
    }

    fn rearm_ceiling_between_turns(&mut self) {
        let waiting_on_the_cli = !matches!(
            self.session_state.as_deref(),
            None | Some(STATE_IDLE | STATE_REQUIRES_ACTION)
        );
        if self.result_received && waiting_on_the_cli {
            self.arm_ceiling();
        }
    }

    fn clear_ceiling(&mut self) {
        self.ceiling_generation += 1;
        if let Some(timer) = self.ceiling_timer.take() {
            timer.abort();
        }
    }
}

async fn end_run_at_ceiling(run: Weak<Mutex<RunEnd>>, generation: u64, wait: Duration) {
    tokio::time::sleep(wait).await;
    let Some(run) = run.upgrade() else {
        return;
    };
    let mut run = lock(&run);
    if generation != run.ceiling_generation {
        return;
    }
    run.ceiling_timer = None;
    if !run.inflight_tasks.is_empty() {
        tracing::debug!(
            "No idle state {} ms after the last result, but {} tracked task(s) still in flight; \
             keeping stdin open",
            wait.as_millis(),
            run.inflight_tasks.len()
        );
        return;
    }
    tracing::debug!(
        "No idle state {} ms after the last result; ending the run",
        wait.as_millis()
    );
    run.end_run();
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::{
        DEFAULT_RUN_END_CEILING_MS, MAX_RUN_END_CEILING_MS, RunEndTracker, lock, parse_ceiling_ms,
    };

    fn task(subtype: &str, task_id: &str, extra: Value) -> Value {
        let mut frame = json!({"type": "system", "subtype": subtype, "task_id": task_id});
        if let (Some(frame), Some(extra)) = (frame.as_object_mut(), extra.as_object()) {
            frame.extend(extra.clone());
        }
        frame
    }

    #[test]
    fn ceiling_parses_like_the_cli_reads_it() {
        let cases: [(Option<&str>, u64); 15] = [
            (None, DEFAULT_RUN_END_CEILING_MS),
            (Some("300"), 300),
            (Some(" 1500 "), 1500),
            (Some("+42"), 42),
            (Some("1_000"), 1000),
            (Some("0"), 0),
            (Some("-0"), 0),
            (Some("-5"), DEFAULT_RUN_END_CEILING_MS),
            (Some(""), DEFAULT_RUN_END_CEILING_MS),
            (Some("abc"), DEFAULT_RUN_END_CEILING_MS),
            (Some("1e6"), DEFAULT_RUN_END_CEILING_MS),
            (Some("12.5"), DEFAULT_RUN_END_CEILING_MS),
            (Some("1__0"), DEFAULT_RUN_END_CEILING_MS),
            (Some("_1"), DEFAULT_RUN_END_CEILING_MS),
            (Some("99999999999999999999999999"), u64::MAX),
        ];
        for (raw, expected) in cases {
            assert_eq!(parse_ceiling_ms(raw), expected, "raw {raw:?}");
        }
        assert_eq!(DEFAULT_RUN_END_CEILING_MS, 600_000);
        assert_eq!(MAX_RUN_END_CEILING_MS, 2_147_483_647);
    }

    #[test]
    fn ledger_adds_on_start_and_clears_only_on_terminal() {
        let run = RunEndTracker::new(0, true);
        let agent = json!({"task_type": "local_agent"});
        run.observe_system(&task("task_started", "a", agent.clone()));
        run.observe_system(&task(
            "task_started",
            "w",
            json!({"task_type": "local_workflow"}),
        ));
        assert_eq!(run.tasks_in_flight(), ["a", "w"]);

        run.observe_system(&task(
            "task_updated",
            "a",
            json!({"patch": {"status": "running"}}),
        ));
        run.observe_system(&task("task_updated", "a", json!({"patch": "broken"})));
        run.observe_system(&task("task_progress", "a", json!({})));
        assert_eq!(run.tasks_in_flight(), ["a", "w"]);

        run.observe_system(&task(
            "task_updated",
            "a",
            json!({"patch": {"status": "killed"}}),
        ));
        run.observe_system(&task("task_notification", "w", json!({})));
        run.observe_system(&task("task_notification", "w", json!({})));
        assert!(run.tasks_in_flight().is_empty());

        run.observe_system(
            &json!({"type": "system", "subtype": "task_started", "task_type": "local_agent"}),
        );
        run.observe_system(&task("task_started", "", agent));
        assert!(run.tasks_in_flight().is_empty());
    }

    #[test]
    fn shells_monitors_and_other_task_types_never_defer_the_close() {
        let run = RunEndTracker::new(0, true);
        for task_type in [
            "local_bash",
            "monitor_mcp",
            "in_process_teammate",
            "remote_agent",
        ] {
            run.observe_system(&task(
                "task_started",
                task_type,
                json!({"task_type": task_type}),
            ));
        }
        run.observe_system(&task("task_started", "untyped", json!({})));
        run.observe_system(&json!({
            "type": "system",
            "subtype": "background_tasks_changed",
            "tasks": [{"task_id": "x", "task_type": "local_agent"}],
        }));
        assert!(run.tasks_in_flight().is_empty());
        run.observe_result();
        assert!(run.has_ended());
    }

    #[test]
    fn a_result_ends_the_run_without_state_and_waits_for_idle_with_it() {
        let run = RunEndTracker::new(0, true);
        run.observe_result();
        assert!(run.has_ended());

        let run = RunEndTracker::new(0, true);
        let state = |state: &str| json!({"type": "system", "subtype": "session_state_changed", "state": state, "sdk_host_only": true});
        assert!(run.observe_system(&state("idle")));
        assert!(!run.has_ended());
        run.observe_system(&state("running"));
        run.observe_result();
        assert!(!run.has_ended());
        run.observe_system(&state("idle"));
        assert!(run.has_ended());

        run.observe_system(&state("running"));
        assert!(!run.has_ended());
        run.seal();
        run.finish();
        run.observe_system(&state("running"));
        assert!(run.has_ended());
    }

    #[test]
    fn only_frames_marked_for_the_sdk_are_hidden() {
        let run = RunEndTracker::new(0, true);
        let visible =
            json!({"type": "system", "subtype": "session_state_changed", "state": "running"});
        assert!(!run.observe_system(&visible));
        let marked_false = json!({
            "type": "system", "subtype": "session_state_changed", "state": "running", "sdk_host_only": false,
        });
        assert!(!run.observe_system(&marked_false));
        assert!(
            !run.observe_system(
                &json!({"type": "system", "subtype": "init", "sdk_host_only": true})
            )
        );
    }

    #[test]
    fn without_control_needs_a_result_ends_the_run_whatever_the_state() {
        let run = RunEndTracker::new(600_000, false);
        run.observe_system(
            &json!({"type": "system", "subtype": "session_state_changed", "state": "running"}),
        );
        run.observe_result();
        assert!(run.has_ended());
        assert!(!run.ceiling_is_armed());
    }

    #[tokio::test(start_paused = true)]
    async fn no_ceiling_is_armed_once_stdin_is_closed() {
        let run = RunEndTracker::new(50, true);
        let state = |state: &str| json!({"type": "system", "subtype": "session_state_changed", "state": state});
        run.observe_system(&state("running"));
        run.observe_result();
        assert!(run.ceiling_is_armed());
        assert!(!run.has_ended());

        run.observe_system(&state("idle"));
        run.wait_for_end().await;
        run.seal();
        assert!(!run.ceiling_is_armed());

        run.observe_system(&state("running"));
        lock(&run.0).arm_ceiling();
        assert!(!run.ceiling_is_armed());
        assert!(run.has_ended());
    }
}
