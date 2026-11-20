//! Scriptable stand-in for the Claude CLI.
//!
//! The program plays the scenario file named by the `FAKE_CLAUDE_SCENARIO`
//! environment variable; the format is documented in the `scenario`
//! module. Without that variable it behaves as a minimal CLI: it emits an
//! init frame, answers every control request with a success response, and
//! exits with code 0 when stdin closes.
//!
//! The init frame carries `fake_args` (the arguments received), `fake_env`
//! (the variables a scenario asks to report), `fake_pid` (the identifier of
//! the process) and, on Windows, `fake_console_window` (whether the process
//! has a console window).
//!
//! When `FAKE_CLAUDE_RECORD` names a file, the program also records the
//! arguments, the working directory, selected environment variables, and
//! every stdin line it receives; the format and the selection rule are
//! documented in the `record` module.
//!
//! `--version` or `-v` prints the version and exits. A scenario that cannot
//! be loaded exits with code 64, an expectation that is not met in time
//! with code 65, stdin closing during an expectation with code 66, and a
//! record file that cannot be created with code 67; each prints its reason
//! to stderr.

mod record;
mod scenario;

use std::io::{BufRead, Write};
use std::process::{Command, ExitCode, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use serde_json::{Map, Value, json};

use record::{RECORD_VARIABLE, Recorder};
use scenario::{DEFAULT_VERSION, Scenario, StdinPause, Step, matches};

const SCENARIO_VARIABLE: &str = "FAKE_CLAUDE_SCENARIO";
const DESCENDANT_SLEEP_VARIABLE: &str = "FAKE_CLAUDE_DESCENDANT_SLEEP_MS";
const EMIT_SESSION_STATE_VARIABLE: &str = "CLAUDE_CODE_EMIT_SESSION_STATE_EVENTS";
const SDK_READS_SESSION_STATE_VARIABLE: &str = "CLAUDE_CODE_SDK_READS_SESSION_STATE";
const HOOK_CALLBACK_PLACEHOLDER: &str = "$hook_callback_id";

const EXIT_BAD_SCENARIO: u8 = 64;
const EXIT_UNMET_EXPECTATION: u8 = 65;
const EXIT_INPUT_CLOSED: u8 = 66;
const EXIT_RECORD_UNWRITABLE: u8 = 67;

fn main() -> ExitCode {
    if let Some(sleep) = std::env::var_os(DESCENDANT_SLEEP_VARIABLE) {
        let milliseconds = sleep.to_string_lossy().parse().unwrap_or(0);
        std::thread::sleep(Duration::from_millis(milliseconds));
        return ExitCode::SUCCESS;
    }
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let recorder = match std::env::var_os(RECORD_VARIABLE) {
        None => Recorder::default(),
        Some(path) => match Recorder::open(path.as_ref()) {
            Ok(recorder) => recorder,
            Err(error) => {
                eprintln!(
                    "fake-claude: cannot create record file {}: {error}",
                    path.to_string_lossy()
                );
                return ExitCode::from(EXIT_RECORD_UNWRITABLE);
            }
        },
    };
    recorder.spawn(&arguments);
    if arguments
        .iter()
        .any(|argument| argument == "--version" || argument == "-v")
    {
        println!("{DEFAULT_VERSION} (fake-claude)");
        return ExitCode::SUCCESS;
    }

    let scenario = match std::env::var_os(SCENARIO_VARIABLE) {
        None => Scenario::stand_in(),
        Some(path) => {
            let loaded = std::fs::read_to_string(&path)
                .map_err(|error| error.to_string())
                .and_then(|text| Scenario::parse(&text));
            match loaded {
                Ok(scenario) => scenario,
                Err(error) => {
                    eprintln!(
                        "fake-claude: cannot load scenario {}: {error}",
                        path.to_string_lossy()
                    );
                    return ExitCode::from(EXIT_BAD_SCENARIO);
                }
            }
        }
    };
    ExitCode::from(Player::start(&scenario, arguments, recorder).play(&scenario.steps))
}

#[derive(Default)]
struct Output {
    hook_callback_id: Option<String>,
}

type SharedOutput = Arc<Mutex<Output>>;

fn emit(output: &SharedOutput, frame: &Value) {
    let _guard = output.lock().unwrap_or_else(PoisonError::into_inner);
    let mut stdout = std::io::stdout().lock();
    let _ = writeln!(stdout, "{frame}");
    let _ = stdout.flush();
}

enum Input {
    Frame(Value),
    Closed,
}

struct Player {
    output: SharedOutput,
    input: Receiver<Input>,
    input_closed: bool,
    last_request_id: Option<Value>,
}

impl Player {
    fn start(scenario: &Scenario, arguments: Vec<String>, recorder: Recorder) -> Self {
        let output = SharedOutput::default();
        if scenario.init {
            emit(&output, &init_frame(scenario, arguments));
        }
        let (sender, input) = channel();
        if scenario.read_stdin {
            let reader_output = Arc::clone(&output);
            let auto_reply = scenario.auto_reply;
            let pause = scenario.stdin_pause;
            std::thread::spawn(move || {
                read_input(&reader_output, &sender, auto_reply, pause, &recorder);
            });
        }
        Self {
            output,
            input,
            input_closed: false,
            last_request_id: None,
        }
    }

    fn play(mut self, steps: &[Step]) -> u8 {
        for step in steps {
            if let Some(code) = self.run(step) {
                return code;
            }
        }
        self.wait_for_eof();
        0
    }

    fn run(&mut self, step: &Step) -> Option<u8> {
        match step {
            Step::Emit(frame) => {
                let hook_callback_id = self.hook_callback_id();
                emit(
                    &self.output,
                    &substitute(frame, hook_callback_id.as_deref()),
                );
            }
            Step::EmitRaw(text) => {
                let _guard = self.output.lock().unwrap_or_else(PoisonError::into_inner);
                let mut stdout = std::io::stdout().lock();
                let _ = stdout.write_all(text.as_bytes());
                let _ = stdout.flush();
            }
            Step::Stderr(text) => {
                let mut stderr = std::io::stderr().lock();
                let _ = stderr.write_all(text.as_bytes());
                let _ = stderr.flush();
            }
            Step::Expect { pattern, timeout } => return self.expect(pattern, *timeout),
            Step::ControlResponse(payload) => self.respond(json!({
                "subtype": "success",
                "response": payload,
            })),
            Step::ControlError(message) => self.respond(json!({
                "subtype": "error",
                "error": message,
            })),
            Step::SessionState(state) => self.session_state(state),
            Step::Sleep(duration) => std::thread::sleep(*duration),
            Step::SpawnDescendant { sleep } => self.spawn_descendant(*sleep),
            Step::WaitForEof => self.wait_for_eof(),
            Step::Exit(code) => return Some(*code),
        }
        None
    }

    fn hook_callback_id(&self) -> Option<String> {
        self.output
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .hook_callback_id
            .clone()
    }

    fn expect(&mut self, pattern: &Value, timeout: Duration) -> Option<u8> {
        let deadline = Instant::now() + timeout;
        loop {
            if self.input_closed {
                eprintln!("fake-claude: stdin closed while waiting for: {pattern}");
                return Some(EXIT_INPUT_CLOSED);
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            match self.input.recv_timeout(remaining) {
                Ok(Input::Frame(frame)) => {
                    if matches(pattern, &frame) {
                        self.last_request_id = frame.get("request_id").cloned();
                        return None;
                    }
                }
                Ok(Input::Closed) | Err(RecvTimeoutError::Disconnected) => {
                    self.input_closed = true;
                }
                Err(RecvTimeoutError::Timeout) => {
                    eprintln!(
                        "fake-claude: expectation not met after {} ms: {pattern}",
                        timeout.as_millis()
                    );
                    return Some(EXIT_UNMET_EXPECTATION);
                }
            }
        }
    }

    fn respond(&self, mut response: Value) {
        if let Some(fields) = response.as_object_mut() {
            fields.insert(
                "request_id".to_owned(),
                self.last_request_id.clone().unwrap_or(Value::Null),
            );
        }
        emit(
            &self.output,
            &json!({"type": "control_response", "response": response}),
        );
    }

    fn session_state(&self, state: &str) {
        let mut frame = json!({
            "type": "system",
            "subtype": "session_state_changed",
            "state": state,
            "uuid": format!("u-{state}"),
            "session_id": "s",
        });
        if truthy(EMIT_SESSION_STATE_VARIABLE) {
            emit(&self.output, &frame);
        } else if truthy(SDK_READS_SESSION_STATE_VARIABLE) {
            frame["sdk_host_only"] = Value::Bool(true);
            emit(&self.output, &frame);
        }
    }

    fn spawn_descendant(&self, sleep: Duration) {
        let spawned = std::env::current_exe().and_then(|program| {
            Command::new(program)
                .env(DESCENDANT_SLEEP_VARIABLE, sleep.as_millis().to_string())
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
        });
        match spawned {
            Ok(descendant) => emit(
                &self.output,
                &json!({"type": "fake", "subtype": "descendant", "pid": descendant.id()}),
            ),
            Err(error) => eprintln!("fake-claude: cannot spawn a descendant: {error}"),
        }
    }

    fn wait_for_eof(&mut self) {
        while !self.input_closed {
            match self.input.recv() {
                Ok(Input::Frame(_)) => {}
                Ok(Input::Closed) | Err(_) => self.input_closed = true,
            }
        }
    }
}

fn init_frame(scenario: &Scenario, arguments: Vec<String>) -> Value {
    let reported: Map<String, Value> = scenario
        .report_env
        .iter()
        .map(|name| {
            let value = std::env::var(name).map_or(Value::Null, Value::String);
            (name.clone(), value)
        })
        .collect();
    let mut frame = json!({
        "type": "system",
        "subtype": "init",
        "session_id": "s",
        "model": "m",
        "cwd": ".",
        "tools": [],
        "mcp_servers": [],
        "permissionMode": "default",
        "apiKeySource": "none",
        "fake_args": arguments,
        "fake_env": reported,
        "fake_pid": std::process::id(),
    });
    if let Some(version) = &scenario.claude_code_version {
        frame["claude_code_version"] = Value::String(version.clone());
    }
    if let Some(has_window) = has_console_window() {
        frame["fake_console_window"] = Value::Bool(has_window);
    }
    frame
}

/// Whether this process owns or shares a console window. Windows only.
#[cfg(windows)]
fn has_console_window() -> Option<bool> {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        safe fn GetConsoleWindow() -> *mut std::ffi::c_void;
    }
    Some(!GetConsoleWindow().is_null())
}

#[cfg(not(windows))]
fn has_console_window() -> Option<bool> {
    None
}

fn read_input(
    output: &SharedOutput,
    sender: &Sender<Input>,
    auto_reply: bool,
    pause: Option<StdinPause>,
    recorder: &Recorder,
) {
    for (index, line) in std::io::stdin().lock().lines().enumerate() {
        let Ok(line) = line else {
            break;
        };
        let delivered = read_line(output, sender, auto_reply, recorder, &line);
        if let Some(pause) = pause
            && index + 1 == pause.after_lines
        {
            std::thread::sleep(pause.duration);
        }
        if !delivered {
            return;
        }
    }
    recorder.stdin_eof();
    let _ = sender.send(Input::Closed);
}

/// Records and handles one stdin line. Returns `false` once the player no
/// longer listens.
fn read_line(
    output: &SharedOutput,
    sender: &Sender<Input>,
    auto_reply: bool,
    recorder: &Recorder,
    line: &str,
) -> bool {
    let parsed = serde_json::from_str::<Value>(line.trim()).ok();
    recorder.stdin_line(line, parsed.as_ref());
    let Some(frame) = parsed else {
        return true;
    };
    if frame.get("type").and_then(Value::as_str) == Some("control_request") {
        remember_hook_callback(output, &frame);
        if auto_reply {
            emit(
                output,
                &json!({
                    "type": "control_response",
                    "response": {
                        "subtype": "success",
                        "request_id": frame.get("request_id").cloned().unwrap_or(Value::Null),
                        "response": {},
                    },
                }),
            );
        }
    }
    sender.send(Input::Frame(frame)).is_ok()
}

fn remember_hook_callback(output: &SharedOutput, frame: &Value) {
    let request = &frame["request"];
    if request.get("subtype").and_then(Value::as_str) != Some("initialize") {
        return;
    }
    let first = request
        .get("hooks")
        .and_then(Value::as_object)
        .into_iter()
        .flat_map(Map::values)
        .filter_map(Value::as_array)
        .flatten()
        .find_map(|matcher| matcher["hookCallbackIds"][0].as_str());
    if let Some(first) = first {
        let mut output = output.lock().unwrap_or_else(PoisonError::into_inner);
        output
            .hook_callback_id
            .get_or_insert_with(|| first.to_owned());
    }
}

fn substitute(frame: &Value, hook_callback_id: Option<&str>) -> Value {
    match frame {
        Value::String(text) if text == HOOK_CALLBACK_PLACEHOLDER => {
            hook_callback_id.map_or(Value::Null, |id| Value::String(id.to_owned()))
        }
        Value::Array(items) => items
            .iter()
            .map(|item| substitute(item, hook_callback_id))
            .collect(),
        Value::Object(fields) => fields
            .iter()
            .map(|(key, value)| (key.clone(), substitute(value, hook_callback_id)))
            .collect::<Map<String, Value>>()
            .into(),
        other => other.clone(),
    }
}

fn truthy(variable: &str) -> bool {
    std::env::var(variable).is_ok_and(|value| {
        matches!(
            value.trim().to_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        EXIT_BAD_SCENARIO, EXIT_INPUT_CLOSED, EXIT_RECORD_UNWRITABLE, EXIT_UNMET_EXPECTATION,
    };
    use super::{SharedOutput, remember_hook_callback, substitute};

    #[test]
    fn exit_codes_are_distinct_and_non_zero() {
        let codes = [
            EXIT_BAD_SCENARIO,
            EXIT_UNMET_EXPECTATION,
            EXIT_INPUT_CLOSED,
            EXIT_RECORD_UNWRITABLE,
        ];
        for (index, code) in codes.iter().enumerate() {
            assert_ne!(*code, 0);
            assert!(!codes[..index].contains(code));
        }
    }

    #[test]
    fn first_hook_callback_of_initialize_is_remembered_and_substituted() {
        let output = SharedOutput::default();
        remember_hook_callback(
            &output,
            &json!({
                "type": "control_request",
                "request_id": "r1",
                "request": {
                    "subtype": "initialize",
                    "hooks": {"PreToolUse": [{"matcher": null, "hookCallbackIds": ["hook_0", "hook_1"]}]},
                },
            }),
        );
        let remembered = output.lock().unwrap().hook_callback_id.clone();
        assert_eq!(remembered.as_deref(), Some("hook_0"));

        let frame = json!({"request": {"callback_id": "$hook_callback_id", "list": ["$hook_callback_id", "x"]}});
        assert_eq!(
            substitute(&frame, remembered.as_deref()),
            json!({"request": {"callback_id": "hook_0", "list": ["hook_0", "x"]}})
        );
        assert_eq!(
            substitute(&json!("$hook_callback_id"), None),
            serde_json::Value::Null
        );
    }
}
