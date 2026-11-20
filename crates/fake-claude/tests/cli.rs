#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

const FAKE_CLAUDE: &str = env!("CARGO_BIN_EXE_fake-claude");
const SCENARIO_VARIABLE: &str = "FAKE_CLAUDE_SCENARIO";

struct Scenario {
    _directory: tempfile::TempDir,
    path: PathBuf,
}

fn scenario(value: Value) -> Scenario {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("scenario.json");
    std::fs::write(&path, value.to_string()).unwrap();
    Scenario {
        _directory: directory,
        path,
    }
}

fn spawn(scenario: Option<&Path>, arguments: &[&str]) -> Child {
    let mut command = Command::new(FAKE_CLAUDE);
    command
        .args(arguments)
        .env_remove(SCENARIO_VARIABLE)
        .env_remove("CLAUDE_CODE_EMIT_SESSION_STATE_EVENTS")
        .env_remove("CLAUDE_CODE_SDK_READS_SESSION_STATE")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(scenario) = scenario {
        command.env(SCENARIO_VARIABLE, scenario);
    }
    command.spawn().unwrap()
}

fn run(scenario: Option<&Path>, arguments: &[&str], input: &str) -> Output {
    let mut child = spawn(scenario, arguments);
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

fn frames(output: &Output) -> Vec<Value> {
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

#[test]
fn version_flag_prints_the_version() {
    for flag in ["--version", "-v"] {
        let output = run(None, &[flag], "");
        assert!(output.status.success());
        assert_eq!(
            String::from_utf8_lossy(&output.stdout).trim(),
            "2.1.329 (fake-claude)"
        );
    }
}

#[test]
fn without_a_scenario_it_speaks_the_protocol_until_stdin_closes() {
    let request = json!({"type": "control_request", "request_id": "1"});
    let output = run(
        None,
        &["--input-format", "stream-json"],
        &format!("{request}\n"),
    );
    assert_eq!(output.status.code(), Some(0));
    let frames = frames(&output);
    assert_eq!(frames.len(), 2);
    assert_eq!(frames[0]["type"], "system");
    assert_eq!(frames[0]["subtype"], "init");
    assert_eq!(frames[0]["claude_code_version"], "2.1.329");
    assert_eq!(
        frames[0]["fake_args"],
        json!(["--input-format", "stream-json"])
    );
    assert_eq!(
        frames[1],
        json!({
            "type": "control_response",
            "response": {"subtype": "success", "request_id": "1", "response": {}},
        })
    );
}

#[test]
fn the_reported_version_is_configurable() {
    let custom = scenario(json!({"claude_code_version": "9.9.9", "steps": [{"exit": 0}]}));
    let frames_custom = frames(&run(Some(&custom.path), &[], ""));
    assert_eq!(frames_custom[0]["claude_code_version"], "9.9.9");

    let absent = scenario(json!({"claude_code_version": null, "steps": [{"exit": 0}]}));
    let frames_absent = frames(&run(Some(&absent.path), &[], ""));
    assert_eq!(frames_absent[0]["subtype"], "init");
    assert!(frames_absent[0].get("claude_code_version").is_none());

    let silent = scenario(json!({"init": false, "steps": [{"exit": 0}]}));
    assert!(frames(&run(Some(&silent.path), &[], "")).is_empty());
}

#[test]
fn a_scenario_plays_emit_expect_reply_sleep_and_exit() {
    let played = scenario(json!({
        "init": false,
        "steps": [
            {"emit": {"type": "assistant", "text": "ready"}},
            {"emit_raw": "not json\n{\"type\": \"split\","},
            {"emit_raw": " \"rest\": true}\n"},
            {"stderr": "a warning\n"},
            {"expect": {"type": "control_request", "request": {"subtype": "interrupt"}}},
            {"control_response": {"interrupted": true}},
            {"expect": {"type": "control_request", "request": {"subtype": "set_model"}}},
            {"control_error": "unknown model"},
            {"sleep_ms": 20},
            {"emit": {"type": "result", "subtype": "success"}},
            {"exit": 7},
        ],
    }));
    let input = format!(
        "{}\n\n{}\n{}\n",
        json!({"type": "user", "message": "skipped: does not match"}),
        json!({"type": "control_request", "request_id": "a", "request": {"subtype": "interrupt"}}),
        json!({"type": "control_request", "request_id": "b", "request": {"subtype": "set_model"}}),
    );
    let output = run(Some(&played.path), &[], &input);
    assert_eq!(output.status.code(), Some(7));
    assert_eq!(String::from_utf8_lossy(&output.stderr), "a warning\n");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines[1], "not json");
    assert_eq!(
        lines
            .iter()
            .filter(|line| line.starts_with('{'))
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .collect::<Vec<_>>(),
        [
            json!({"type": "assistant", "text": "ready"}),
            json!({"type": "split", "rest": true}),
            json!({
                "type": "control_response",
                "response": {"subtype": "success", "request_id": "a",
                             "response": {"interrupted": true}},
            }),
            json!({
                "type": "control_response",
                "response": {"subtype": "error", "request_id": "b", "error": "unknown model"},
            }),
            json!({"type": "result", "subtype": "success"}),
        ]
    );
}

#[test]
fn an_expectation_that_is_never_met_exits_with_a_distinct_code_after_ten_seconds() {
    let waiting = scenario(json!({
        "init": false,
        "steps": [{"expect": {"type": "user", "never": "sent"}}],
    }));
    let started = Instant::now();
    let mut child = spawn(Some(&waiting.path), &[]);
    let mut stdin = child.stdin.take().unwrap();
    writeln!(stdin, "{}", json!({"type": "user"})).unwrap();
    let status = child.wait().unwrap();
    let elapsed = started.elapsed();
    drop(stdin);

    assert_eq!(status.code(), Some(65));
    assert!(elapsed >= Duration::from_secs(10), "{elapsed:?}");
    assert!(elapsed < Duration::from_secs(25), "{elapsed:?}");
    let mut stderr = String::new();
    BufReader::new(child.stderr.take().unwrap())
        .read_line(&mut stderr)
        .unwrap();
    assert!(
        stderr.contains("expectation not met after 10000 ms"),
        "{stderr}"
    );
    assert!(stderr.contains("\"never\":\"sent\""), "{stderr}");
}

#[test]
fn the_expectation_limit_can_be_shortened_per_step() {
    let waiting = scenario(json!({
        "init": false,
        "steps": [{"expect": {"type": "user"}, "timeout_ms": 150}],
    }));
    let mut child = spawn(Some(&waiting.path), &[]);
    let stdin = child.stdin.take().unwrap();
    let status = child.wait().unwrap();
    drop(stdin);
    assert_eq!(status.code(), Some(65));
}

#[test]
fn stdin_closing_during_an_expectation_exits_with_its_own_code() {
    let waiting = scenario(json!({"init": false, "steps": [{"expect": {"type": "user"}}]}));
    let output = run(Some(&waiting.path), &[], "");
    assert_eq!(output.status.code(), Some(66));
    assert!(String::from_utf8_lossy(&output.stderr).contains("stdin closed while waiting for"));
}

#[test]
fn an_unreadable_or_malformed_scenario_exits_with_its_own_code() {
    let directory = tempfile::tempdir().unwrap();
    let missing = run(Some(&directory.path().join("missing.json")), &[], "");
    assert_eq!(missing.status.code(), Some(64));
    assert!(String::from_utf8_lossy(&missing.stderr).contains("cannot load scenario"));

    let malformed = scenario(json!({"steps": [{"dance": true}]}));
    let output = run(Some(&malformed.path), &[], "");
    assert_eq!(output.status.code(), Some(64));
    assert!(String::from_utf8_lossy(&output.stderr).contains("unknown action `dance`"));
}

#[test]
fn environment_variables_are_reported_in_the_init_frame() {
    let reporting = scenario(json!({
        "report_env": ["FAKE_CLAUDE_TEST_SET", "FAKE_CLAUDE_TEST_UNSET"],
        "steps": [{"exit": 0}],
    }));
    let output = Command::new(FAKE_CLAUDE)
        .env(SCENARIO_VARIABLE, &reporting.path)
        .env("FAKE_CLAUDE_TEST_SET", "value")
        .env_remove("FAKE_CLAUDE_TEST_UNSET")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(
        frames(&output)[0]["fake_env"],
        json!({"FAKE_CLAUDE_TEST_SET": "value", "FAKE_CLAUDE_TEST_UNSET": null})
    );
}

#[test]
fn session_state_frames_follow_the_cli_contract() {
    let stateful = scenario(json!({
        "init": false,
        "steps": [{"session_state": "running"}, {"exit": 0}],
    }));
    let run_with = |variables: &[(&str, &str)]| {
        let mut command = Command::new(FAKE_CLAUDE);
        command
            .env(SCENARIO_VARIABLE, &stateful.path)
            .env_remove("CLAUDE_CODE_EMIT_SESSION_STATE_EVENTS")
            .env_remove("CLAUDE_CODE_SDK_READS_SESSION_STATE")
            .stdin(Stdio::null());
        for (name, value) in variables {
            command.env(name, value);
        }
        frames(&command.output().unwrap())
    };

    assert!(run_with(&[]).is_empty());
    let plain = run_with(&[("CLAUDE_CODE_EMIT_SESSION_STATE_EVENTS", "1")]);
    assert_eq!(plain[0]["state"], "running");
    assert!(plain[0].get("sdk_host_only").is_none());
    let hidden = run_with(&[("CLAUDE_CODE_SDK_READS_SESSION_STATE", "true")]);
    assert_eq!(hidden[0]["subtype"], "session_state_changed");
    assert_eq!(hidden[0]["sdk_host_only"], true);
    assert!(run_with(&[("CLAUDE_CODE_SDK_READS_SESSION_STATE", "0")]).is_empty());
}

#[test]
fn the_run_end_scenario_matches_the_upstream_stand_in() {
    let wake = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("scenarios")
        .join("run_end_wake.json");
    let mut child = spawn(Some(&wake), &[]);
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let mut read = || {
        let mut line = String::new();
        stdout.read_line(&mut line).unwrap();
        serde_json::from_str::<Value>(&line).unwrap()
    };

    let init = read();
    assert_eq!(init["claude_code_version"], "2.1.999");
    writeln!(
        stdin,
        "{}",
        json!({
            "type": "control_request",
            "request_id": "init-1",
            "request": {
                "subtype": "initialize",
                "hooks": {"PreToolUse": [{"matcher": null, "hookCallbackIds": ["hook_0"]}]},
            },
        })
    )
    .unwrap();
    assert_eq!(read()["response"]["request_id"], "init-1");

    writeln!(stdin, "{}", json!({"type": "user", "message": "go"})).unwrap();
    assert_eq!(read()["message"]["content"][0]["text"], "LAUNCHED");
    assert_eq!(read()["result"], "LAUNCHED");
    assert_eq!(read()["message"]["content"][0]["text"], "writing");
    let hook = read();
    assert_eq!(hook["request_id"], "hook-1");
    assert_eq!(hook["request"]["callback_id"], "hook_0");

    writeln!(
        stdin,
        "{}",
        json!({
            "type": "control_response",
            "response": {"subtype": "success", "request_id": "hook-1", "response": {}},
        })
    )
    .unwrap();
    assert_eq!(read()["message"]["content"][0]["text"], "FINISHED");
    assert_eq!(read()["result"], "FINISHED");

    drop(stdin);
    assert_eq!(child.wait().unwrap().code(), Some(0));
}

const RECORD_VARIABLE: &str = "FAKE_CLAUDE_RECORD";

fn records(path: &Path) -> Vec<Value> {
    std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .collect()
}

fn without_pid(records: &[Value]) -> Vec<Value> {
    records
        .iter()
        .map(|record| {
            let mut record = record.clone();
            assert!(record["pid"].as_u64().is_some(), "{record}");
            record
                .as_object_mut()
                .unwrap()
                .retain(|key, _| key != "pid");
            record
        })
        .collect()
}

fn run_recorded(
    scenario: Option<&Path>,
    record: &Path,
    environment: &[(&str, &str)],
    arguments: &[&str],
    input: &str,
) -> Output {
    let mut command = Command::new(FAKE_CLAUDE);
    command
        .args(arguments)
        .env_remove(SCENARIO_VARIABLE)
        .env(RECORD_VARIABLE, record)
        .envs(environment.iter().copied())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(scenario) = scenario {
        command.env(SCENARIO_VARIABLE, scenario);
    }
    let mut child = command.spawn().unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

#[test]
fn a_recorded_run_writes_the_spawn_and_every_stdin_line_in_order() {
    let directory = tempfile::tempdir().unwrap();
    let record = directory.path().join("record.jsonl");
    let initialize =
        json!({"type": "control_request", "request_id": "r1", "request": {"subtype": "initialize"}})
            .to_string();
    let user = json!({"type": "user", "message": {"role": "user", "content": "hi"}}).to_string();
    let input = format!("{initialize}\nnot json\n{user}\n");
    let output = run_recorded(
        None,
        &record,
        &[
            ("CLAUDE_CODE_ENTRYPOINT", "sdk-rs"),
            ("ANTHROPIC_BASE_URL", "http://localhost"),
            ("MAX_THINKING_TOKENS", "8"),
            ("TRACEPARENT", "00-a-b-01"),
            ("TRACESTATE", "k=v"),
            ("PWD", "/somewhere"),
            ("UNRELATED_VARIABLE", "hidden"),
        ],
        &["--output-format", "stream-json", "--verbose"],
        &input,
    );
    assert_eq!(output.status.code(), Some(0));

    let records = records(&record);
    let spawn = &records[0];
    assert_eq!(spawn["record"], "spawn");
    assert_eq!(
        spawn["args"],
        json!(["--output-format", "stream-json", "--verbose"])
    );
    assert_eq!(
        spawn["cwd"],
        std::env::current_dir().unwrap().to_string_lossy().as_ref()
    );
    assert!(records.iter().all(|record| record["pid"] == spawn["pid"]));
    let env = spawn["env"].as_object().unwrap();
    assert_eq!(env["CLAUDE_CODE_ENTRYPOINT"], "sdk-rs");
    assert_eq!(env["ANTHROPIC_BASE_URL"], "http://localhost");
    assert_eq!(env["MAX_THINKING_TOKENS"], "8");
    assert_eq!(env["TRACEPARENT"], "00-a-b-01");
    assert_eq!(env["TRACESTATE"], "k=v");
    assert_eq!(env["PWD"], "/somewhere");
    assert!(!env.contains_key("UNRELATED_VARIABLE"));
    assert!(!env.contains_key(RECORD_VARIABLE));
    assert!(env.keys().all(|name| {
        name.starts_with("CLAUDE_")
            || name.starts_with("ANTHROPIC_")
            || ["PWD", "MAX_THINKING_TOKENS", "TRACEPARENT", "TRACESTATE"].contains(&name.as_str())
    }));

    assert_eq!(
        without_pid(&records[1..]),
        [
            json!({"record": "stdin", "frame": {"type": "control_request", "request_id": "r1", "request": {"subtype": "initialize"}}, "text": initialize}),
            json!({"record": "stdin_raw", "text": "not json"}),
            json!({"record": "stdin", "frame": {"type": "user", "message": {"role": "user", "content": "hi"}}, "text": user}),
            json!({"record": "stdin_eof"}),
        ]
    );
}

#[test]
fn records_are_flushed_as_they_are_written() {
    let directory = tempfile::tempdir().unwrap();
    let record = directory.path().join("record.jsonl");
    let scenario = scenario(json!({"steps": [{"expect": {"type": "user"}}, {"sleep_ms": 60_000}]}));
    let mut child = Command::new(FAKE_CLAUDE)
        .env(SCENARIO_VARIABLE, &scenario.path)
        .env(RECORD_VARIABLE, &record)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    writeln!(stdin, "{}", json!({"type": "user"})).unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let mut init = String::new();
    stdout.read_line(&mut init).unwrap();

    let deadline = Instant::now() + Duration::from_secs(10);
    while records(&record).len() < 2 {
        assert!(Instant::now() < deadline, "the stdin record never appeared");
        std::thread::sleep(Duration::from_millis(20));
    }
    child.kill().unwrap();
    child.wait().unwrap();

    let records = records(&record);
    assert_eq!(records[0]["record"], "spawn");
    assert_eq!(
        without_pid(&records[1..2]),
        [
            json!({"record": "stdin", "frame": {"type": "user"}, "text": json!({"type": "user"}).to_string()})
        ]
    );
    assert_eq!(records.len(), 2);
}

#[test]
fn every_process_started_with_the_variable_appends_its_own_records() {
    let directory = tempfile::tempdir().unwrap();
    let record = directory.path().join("record.jsonl");
    let version = run_recorded(None, &record, &[], &["--version"], "");
    assert_eq!(version.status.code(), Some(0));
    let session = run_recorded(None, &record, &[], &["--print"], "");
    assert_eq!(session.status.code(), Some(0));

    let records = records(&record);
    assert_eq!(records.len(), 3);
    assert_eq!(records[0]["args"], json!(["--version"]));
    assert_eq!(records[1]["args"], json!(["--print"]));
    assert_eq!(without_pid(&records[2..]), [json!({"record": "stdin_eof"})]);
    assert_ne!(records[0]["pid"], records[1]["pid"]);
    assert_eq!(records[1]["pid"], records[2]["pid"]);
}

#[test]
fn a_record_file_that_cannot_be_created_exits_with_its_own_code() {
    let directory = tempfile::tempdir().unwrap();
    let record = directory.path().join("missing").join("record.jsonl");
    let output = run_recorded(None, &record, &[], &[], "");
    assert_eq!(output.status.code(), Some(67));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("cannot create record file"), "{stderr}");
    assert!(
        stderr.contains(record.to_string_lossy().as_ref()),
        "{stderr}"
    );
}
