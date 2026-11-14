#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use claude_agent_kit::{
    Error, MessageStream, SharedTransport, StderrCallback, SubprocessTransport, Transport,
};
use futures_util::StreamExt;
use serde_json::{Value, json};

use common::capture::Warnings;
use common::{Fake, collect, fake_claude, next, next_message, stand_in_options, wait_until_gone};

fn connection_error(error: Error) -> String {
    match error {
        Error::CliConnection { message } => message,
        other => panic!("expected a connection error, got {other:?}"),
    }
}

#[tokio::test]
async fn connect_read_and_close() {
    let transport = SubprocessTransport::new(stand_in_options().build().unwrap());
    assert!(!transport.is_ready());
    assert_eq!(transport.process_id(), None);

    transport.connect().await.unwrap();
    assert!(transport.is_ready());
    let pid = transport.process_id().unwrap();

    let mut stream = transport.read_messages();
    let init = next_message(&mut stream).await;
    assert_eq!(init["type"], "system");
    assert_eq!(init["subtype"], "init");

    transport.connect().await.unwrap();
    assert_eq!(transport.process_id(), Some(pid));

    transport.close().await.unwrap();
    assert!(!transport.is_ready());
    assert!(next(&mut stream).await.is_none());
    assert!(wait_until_gone(pid, Duration::from_secs(5)).await);

    let reconnect = transport.connect().await.unwrap_err();
    assert_eq!(connection_error(reconnect), "Transport is closed");
}

#[tokio::test]
async fn messages_are_exchanged_over_stdin_and_stdout() {
    let fake = Fake::new(json!({
        "auto_reply": true,
        "steps": [
            {"expect": {"type": "user"}},
            {"emit": {"type": "assistant", "text": "hello"}},
            {"emit": {"type": "result", "subtype": "success"}},
        ],
    }));
    let transport = SubprocessTransport::new(fake.build());
    transport.connect().await.unwrap();
    let mut stream = transport.read_messages();
    assert_eq!(next_message(&mut stream).await["subtype"], "init");

    let request = json!({"type": "control_request", "request_id": "r1",
                         "request": {"subtype": "initialize"}});
    transport.write(&format!("{request}\n")).await.unwrap();
    let response = next_message(&mut stream).await;
    assert_eq!(response["type"], "control_response");
    assert_eq!(response["response"]["request_id"], "r1");

    let prompt = json!({"type": "user", "message": {"role": "user", "content": "hi"}});
    transport.write(&format!("{prompt}\n")).await.unwrap();
    assert_eq!(next_message(&mut stream).await["text"], "hello");
    assert_eq!(next_message(&mut stream).await["type"], "result");

    transport.end_input().await.unwrap();
    assert!(next(&mut stream).await.is_none());
    let late = transport.write("{}\n").await.unwrap_err();
    assert_eq!(
        connection_error(late),
        "ProcessTransport is not ready for writing"
    );
    transport.close().await.unwrap();
}

#[tokio::test]
async fn the_child_environment_is_built_in_the_documented_order() {
    let names = [
        "CLAUDE_CODE_ENTRYPOINT",
        "CLAUDE_AGENT_SDK_VERSION",
        "CLAUDECODE",
        "CLAUDE_CODE_SDK_READS_SESSION_STATE",
        "CLAUDE_CODE_ENABLE_SDK_FILE_CHECKPOINTING",
        "CLAUDE_AGENT_SDK_CLIENT_APP",
        "CLAUDE_AGENT_SDK_DERIVED_CLIENT_APP",
        "PWD",
        "MY_TEST_VAR",
        "MAX_MCP_OUTPUT_TOKENS",
        "PATH",
    ];
    let fake = Fake::new(json!({"report_env": names, "steps": [{"exit": 0}]}));
    let cwd = tempfile::tempdir().unwrap();
    let options = fake
        .options()
        .env_var("MY_TEST_VAR", "test-value")
        .env_var("MAX_MCP_OUTPUT_TOKENS", "500000")
        .env_var("CLAUDE_AGENT_SDK_VERSION", "0.0.0")
        .enable_file_checkpointing(true)
        .client_app("my-app/1.0.0")
        .cwd(cwd.path())
        .build()
        .unwrap();
    let transport = SubprocessTransport::new(options);
    transport.connect().await.unwrap();
    let mut stream = transport.read_messages();
    let reported = next_message(&mut stream).await["fake_env"].clone();
    transport.close().await.unwrap();

    assert_eq!(reported["CLAUDE_CODE_ENTRYPOINT"], "sdk-rs");
    assert_eq!(
        reported["CLAUDE_AGENT_SDK_VERSION"],
        claude_agent_kit::VERSION
    );
    assert_eq!(reported["CLAUDECODE"], Value::Null);
    assert_eq!(
        reported["CLAUDE_CODE_ENABLE_SDK_FILE_CHECKPOINTING"],
        "true"
    );
    assert_eq!(reported["CLAUDE_AGENT_SDK_CLIENT_APP"], "my-app/1.0.0");
    assert_eq!(reported["CLAUDE_AGENT_SDK_DERIVED_CLIENT_APP"], Value::Null);
    assert_eq!(reported["PWD"], cwd.path().to_string_lossy().as_ref());
    assert_eq!(reported["MY_TEST_VAR"], "test-value");
    assert_eq!(reported["MAX_MCP_OUTPUT_TOKENS"], "500000");
    assert_eq!(
        reported["PATH"],
        std::env::var("PATH").unwrap().as_str(),
        "inherited variables reach the process"
    );
    if std::env::var_os("CLAUDE_CODE_SDK_READS_SESSION_STATE").is_none() {
        assert_eq!(reported["CLAUDE_CODE_SDK_READS_SESSION_STATE"], "1");
    }
}

#[tokio::test]
async fn the_caller_can_override_the_entrypoint_and_the_session_state_request() {
    let names = [
        "CLAUDE_CODE_ENTRYPOINT",
        "CLAUDE_AGENT_SDK_VERSION",
        "CLAUDECODE",
        "CLAUDE_CODE_SDK_READS_SESSION_STATE",
        "CLAUDE_CODE_ENABLE_SDK_FILE_CHECKPOINTING",
        "PWD",
    ];
    let fake = Fake::new(json!({"report_env": names, "steps": [{"exit": 0}]}));
    let options = fake
        .options()
        .env_var("CLAUDE_CODE_ENTRYPOINT", "custom-caller")
        .env_var("CLAUDECODE", "1")
        .env_var("CLAUDE_CODE_SDK_READS_SESSION_STATE", "0")
        .build()
        .unwrap();
    let transport = SubprocessTransport::new(options);
    transport.connect().await.unwrap();
    let mut stream = transport.read_messages();
    let reported = next_message(&mut stream).await["fake_env"].clone();
    transport.close().await.unwrap();

    assert_eq!(reported["CLAUDE_CODE_ENTRYPOINT"], "custom-caller");
    assert_eq!(reported["CLAUDECODE"], "1");
    assert_eq!(reported["CLAUDE_CODE_SDK_READS_SESSION_STATE"], "0");
    assert_eq!(
        reported["CLAUDE_AGENT_SDK_VERSION"],
        claude_agent_kit::VERSION
    );
    assert_eq!(
        reported["CLAUDE_CODE_ENABLE_SDK_FILE_CHECKPOINTING"],
        Value::Null
    );
}

#[tokio::test]
async fn a_missing_working_directory_is_a_connection_error() {
    let cwd = std::env::temp_dir().join("claude-agent-kit-missing-directory-9f3a");
    let options = stand_in_options().cwd(&cwd).build().unwrap();
    let transport = SubprocessTransport::new(options);
    let error = transport.connect().await.unwrap_err();
    assert_eq!(
        connection_error(error),
        format!("Working directory does not exist: {}", cwd.display())
    );
    assert_eq!(transport.process_id(), None);
    assert!(!transport.is_ready());
}

#[tokio::test]
async fn a_missing_executable_is_a_not_found_error() {
    let directory = tempfile::tempdir().unwrap();
    let cli_path = directory.path().join("claude-is-not-here");
    let options = claude_agent_kit::ClaudeAgentOptions::builder()
        .cli_path(&cli_path)
        .build()
        .unwrap();
    let transport = SubprocessTransport::new(options);
    match transport.connect().await.unwrap_err() {
        Error::CliNotFound { message } => assert_eq!(
            message,
            format!("Claude CLI not found at: {}", cli_path.display())
        ),
        other => panic!("expected CliNotFound, got {other:?}"),
    }
}

#[tokio::test]
async fn reading_before_connecting_yields_one_connection_error() {
    let transport = SubprocessTransport::new(stand_in_options().build().unwrap());
    let items = collect(transport.read_messages()).await;
    assert_eq!(items.len(), 1);
    assert!(matches!(items[0], Err(Error::CliConnection { .. })));

    let before = transport.write("{}\n").await.unwrap_err();
    assert_eq!(
        connection_error(before),
        "ProcessTransport is not ready for writing"
    );

    transport.connect().await.unwrap();
    let first = transport.read_messages();
    let second = collect(transport.read_messages()).await;
    assert!(matches!(second[0], Err(Error::CliConnection { .. })));
    drop(first);
    transport.close().await.unwrap();
}

#[tokio::test]
async fn blank_and_non_json_lines_are_skipped_and_split_messages_reassembled() {
    let fake = Fake::new(json!({
        "init": false,
        "steps": [
            {"emit_raw": "\n\n[SandboxDebug] Seccomp filtering not available\n"},
            {"emit_raw": "{\"type\": \"assistant\", \"text\": \"hello "},
            {"sleep_ms": 100},
            {"emit_raw": " world\"}\r\nplain text\n"},
            {"emit_raw": "{\"type\": \"result\"}"},
            {"exit": 0},
        ],
    }));
    let transport = SubprocessTransport::new(fake.build());
    transport.connect().await.unwrap();
    let items = collect(transport.read_messages()).await;
    let messages: Vec<Value> = items.into_iter().map(Result::unwrap).collect();
    assert_eq!(
        messages,
        [
            json!({"type": "assistant", "text": "hello  world"}),
            json!({"type": "result"}),
        ]
    );
    transport.close().await.unwrap();
}

#[tokio::test]
async fn a_corrupt_line_yields_a_decode_error_and_ends_the_stream() {
    let fake = Fake::new(json!({
        "init": false,
        "steps": [
            {"emit": {"type": "first"}},
            {"emit_raw": "{\"type\": \"assistant\", broken\n"},
            {"emit": {"type": "never seen"}},
            {"exit": 0},
        ],
    }));
    let transport = SubprocessTransport::new(fake.build());
    transport.connect().await.unwrap();
    let items = collect(transport.read_messages()).await;
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].as_ref().unwrap()["type"], "first");
    match &items[1] {
        Err(Error::JsonDecode { line, .. }) => {
            assert_eq!(line, "{\"type\": \"assistant\", broken");
        }
        other => panic!("expected a decode error, got {other:?}"),
    }
    transport.close().await.unwrap();
}

#[tokio::test]
async fn an_oversize_message_yields_a_decode_error_and_ends_the_stream() {
    let fake = Fake::new(json!({
        "init": false,
        "steps": [
            {"emit": {"type": "small"}},
            {"emit": {"type": "large", "data": "x".repeat(2000)}},
            {"emit": {"type": "never seen"}},
            {"exit": 0},
        ],
    }));
    let transport = SubprocessTransport::new(fake.options().max_buffer_size(512).build().unwrap());
    transport.connect().await.unwrap();
    let items = collect(transport.read_messages()).await;
    assert_eq!(items.len(), 2);
    let error = items[1].as_ref().unwrap_err();
    assert!(matches!(error, Error::JsonDecode { .. }));
    assert_eq!(
        error.to_string(),
        "Failed to decode JSON: JSON message exceeded maximum buffer size of 512 bytes..."
    );
    transport.close().await.unwrap();
}

#[tokio::test]
async fn a_truncated_final_line_is_dropped_without_error() {
    let fake = Fake::new(json!({
        "init": false,
        "steps": [
            {"emit": {"type": "complete"}},
            {"emit_raw": "{\"type\": \"assis"},
            {"exit": 0},
        ],
    }));
    let transport = SubprocessTransport::new(fake.build());
    transport.connect().await.unwrap();
    let items = collect(transport.read_messages()).await;
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].as_ref().unwrap()["type"], "complete");
    transport.close().await.unwrap();
}

#[tokio::test]
async fn a_non_zero_exit_code_ends_the_stream_with_a_process_error() {
    let fake = Fake::new(json!({"steps": [{"exit": 3}]}));
    let transport = SubprocessTransport::new(fake.build());
    transport.connect().await.unwrap();
    let mut items = collect(transport.read_messages()).await;
    assert_eq!(items.len(), 2);
    match items.pop().unwrap() {
        Err(Error::Process {
            message,
            exit_code,
            stderr,
        }) => {
            assert_eq!(message, "Command failed with exit code 3");
            assert_eq!(exit_code, Some(3));
            assert_eq!(stderr.as_deref(), Some("Check stderr output for details"));
        }
        other => panic!("expected a process error, got {other:?}"),
    }

    let error = transport.write("{}\n").await.unwrap_err();
    assert_eq!(
        connection_error(error),
        "Cannot write to terminated process (exit code: 3)"
    );
    transport.close().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_writes_are_serialized() {
    const WRITERS: usize = 24;
    let mut steps: Vec<Value> = (0..WRITERS)
        .map(|_| json!({"expect": {"type": "user"}}))
        .collect();
    steps.push(json!({"emit": {"type": "result", "subtype": "success"}}));
    steps.push(json!({"exit": 0}));
    let fake = Fake::new(json!({"init": false, "steps": steps}));
    let transport = Arc::new(SubprocessTransport::new(fake.build()));
    transport.connect().await.unwrap();

    let writers: Vec<_> = (0..WRITERS)
        .map(|index| {
            let transport = Arc::clone(&transport);
            tokio::spawn(async move {
                let frame = json!({"type": "user", "id": index, "padding": "x".repeat(64 * 1024)});
                transport.write(&format!("{frame}\n")).await
            })
        })
        .collect();
    for writer in writers {
        writer.await.unwrap().unwrap();
    }

    let items = collect(transport.read_messages()).await;
    assert_eq!(items.len(), 1, "{items:?}");
    assert_eq!(items[0].as_ref().unwrap()["type"], "result");
    transport.close().await.unwrap();
}

#[tokio::test]
async fn the_stderr_callback_receives_each_non_empty_line_trimmed() {
    let received = Arc::new(Mutex::new(Vec::<String>::new()));
    let sink = Arc::clone(&received);
    let callback = StderrCallback::from_sync(move |line| {
        let first = {
            let mut lines = sink.lock().unwrap();
            lines.push(line.to_owned());
            lines.len() == 1
        };
        assert!(!first, "the first call fails");
    });
    let fake = Fake::new(json!({
        "steps": [
            {"stderr": "first line  \n\n   \nsecond "},
            {"stderr": "half\nlast line without newline"},
            {"wait_for_eof": true},
        ],
    }));
    let transport = SubprocessTransport::new(fake.options().stderr(callback).build().unwrap());
    transport.connect().await.unwrap();

    let deadline = tokio::time::Instant::now() + common::STEP_LIMIT;
    while received.lock().unwrap().len() < 2 {
        assert!(
            tokio::time::Instant::now() < deadline,
            "stderr lines never arrived"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    transport.close().await.unwrap();
    assert_eq!(
        *received.lock().unwrap(),
        ["first line", "second half", "last line without newline"]
    );
}

#[tokio::test]
async fn a_cli_too_old_to_report_its_version_warns_once() {
    let (warnings, _guard) = Warnings::install();
    let init = json!({"type": "system", "subtype": "init"});
    let fake = Fake::new(json!({
        "claude_code_version": null,
        "steps": [{"emit": init}, {"exit": 0}],
    }));
    let transport = SubprocessTransport::new(fake.build());
    transport.connect().await.unwrap();
    let items = collect(transport.read_messages()).await;
    assert_eq!(items.len(), 2);
    transport.close().await.unwrap();

    let messages = warnings.about_the_cli_version();
    assert_eq!(messages.len(), 1, "{messages:?}");
    assert!(messages[0].contains("too old to report its version"));
    assert!(messages[0].contains("Minimum required version is 2.0.0"));
    assert!(messages[0].contains(&fake_claude().display().to_string()));
}

#[tokio::test]
async fn a_supported_cli_version_does_not_warn() {
    let (warnings, _guard) = Warnings::install();
    for version in ["2.0.20", "99.99.99"] {
        let fake = Fake::new(json!({"claude_code_version": version, "steps": [{"exit": 0}]}));
        let transport = SubprocessTransport::new(fake.build());
        transport.connect().await.unwrap();
        collect(transport.read_messages()).await;
        transport.close().await.unwrap();
    }
    assert_eq!(warnings.about_the_cli_version(), Vec::<String>::new());
}

#[tokio::test]
async fn verbatim_prompts_warns_on_a_cli_that_ignores_it() {
    let (warnings, _guard) = Warnings::install();
    for version in ["2.1.247", "2.1.248"] {
        let fake = Fake::new(json!({"claude_code_version": version, "steps": [{"exit": 0}]}));
        let options = fake.options().verbatim_prompts(true).build().unwrap();
        let transport = SubprocessTransport::new(options);
        transport.connect().await.unwrap();
        collect(transport.read_messages()).await;
        transport.close().await.unwrap();
    }
    let messages = warnings.about_the_cli_version();
    assert_eq!(messages.len(), 1, "{messages:?}");
    assert!(messages[0].contains("verbatim_prompts is enabled"));
    assert!(messages[0].contains("version 2.1.247"));
    assert!(messages[0].contains("2.1.248 or later is required"));
}

#[derive(Default)]
struct LoopbackTransport {
    connected: Mutex<bool>,
    written: Mutex<Vec<Value>>,
}

impl Transport for LoopbackTransport {
    async fn connect(&self) -> claude_agent_kit::Result<()> {
        *self.connected.lock().unwrap() = true;
        Ok(())
    }

    async fn write(&self, data: &str) -> claude_agent_kit::Result<()> {
        let frame =
            serde_json::from_str(data.trim()).map_err(|source| Error::json_decode(data, source))?;
        self.written.lock().unwrap().push(frame);
        Ok(())
    }

    fn read_messages(&self) -> MessageStream {
        let echoed: Vec<claude_agent_kit::Result<Value>> = self
            .written
            .lock()
            .unwrap()
            .iter()
            .map(|frame| Ok(json!({"type": "echo", "frame": frame})))
            .collect();
        Box::pin(futures_util::stream::iter(echoed))
    }

    async fn close(&self) -> claude_agent_kit::Result<()> {
        *self.connected.lock().unwrap() = false;
        Ok(())
    }

    fn is_ready(&self) -> bool {
        *self.connected.lock().unwrap()
    }

    async fn end_input(&self) -> claude_agent_kit::Result<()> {
        Ok(())
    }
}

async fn drive(transport: SharedTransport, frame: Value) -> Vec<Value> {
    transport.connect().await.unwrap();
    assert!(transport.is_ready());
    transport.write(&format!("{frame}\n")).await.unwrap();
    transport.end_input().await.unwrap();
    let messages: Vec<Value> = transport
        .read_messages()
        .map(Result::unwrap)
        .collect()
        .await;
    transport.close().await.unwrap();
    assert!(!transport.is_ready());
    messages
}

#[tokio::test]
async fn custom_and_subprocess_transports_are_interchangeable_behind_the_shared_handle() {
    let frame = json!({"type": "user", "message": "hi"});

    let custom = drive(LoopbackTransport::default().into(), frame.clone()).await;
    assert_eq!(custom, [json!({"type": "echo", "frame": frame})]);

    let fake = Fake::new(json!({
        "init": false,
        "steps": [{"expect": {"type": "user"}}, {"emit": {"type": "echo"}}],
    }));
    let subprocess = drive(SubprocessTransport::new(fake.build()).into(), frame).await;
    assert_eq!(subprocess, [json!({"type": "echo"})]);
}

#[cfg(windows)]
#[tokio::test]
async fn a_batch_script_is_refused_on_windows_and_nothing_is_spawned() {
    let directory = tempfile::tempdir().unwrap();
    let marker = directory.path().join("executed.txt");
    for name in ["claude.cmd", "claude.bat", "claude.CMD"] {
        let script = directory.path().join(name);
        std::fs::write(
            &script,
            format!("@echo off\r\necho ran> \"{}\"\r\n", marker.display()),
        )
        .unwrap();
        let options = claude_agent_kit::ClaudeAgentOptions::builder()
            .cli_path(&script)
            .build()
            .unwrap();
        let transport = SubprocessTransport::new(options);
        let message = connection_error(transport.connect().await.unwrap_err());
        assert!(message.contains("batch script"), "{message}");
        assert_eq!(transport.process_id(), None);
    }
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(!marker.exists());
}

#[cfg(windows)]
#[tokio::test]
async fn a_resume_value_with_cmd_metacharacters_fails_connect_on_windows() {
    for value in [
        "x&calc", "x|whoami", "x<in", "x>out", "x^y", "x%PATH%y", "x!V!y", "x\"y", "x\ny",
    ] {
        let options = stand_in_options().resume(value).build().unwrap();
        let transport = SubprocessTransport::new(options);
        let error = transport.connect().await.unwrap_err();
        assert!(
            matches!(error, Error::Config { .. }),
            "{value:?}: {error:?}"
        );
        assert_eq!(transport.process_id(), None);
    }
}

#[cfg(unix)]
fn command_output(program: &str, arguments: &[&str]) -> String {
    let output = std::process::Command::new(program)
        .args(arguments)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{program} {arguments:?}: {output:?}"
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

#[cfg(unix)]
#[tokio::test]
async fn the_process_runs_as_the_requested_user() {
    use std::os::unix::fs::PermissionsExt;

    let started_as_root = command_output("id", &["-u"]) == "0";
    let user = if started_as_root {
        "nobody".to_owned()
    } else {
        command_output("id", &["-un"])
    };
    let expected_uid = command_output("id", &["-u", &user]);
    let expected_gid = command_output("id", &["-g", &user]);

    let directory = tempfile::tempdir().unwrap();
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
    let cli_path = directory.path().join("claude");
    std::fs::copy(fake_claude(), &cli_path).unwrap();

    let options = claude_agent_kit::ClaudeAgentOptions::builder()
        .cli_path(&cli_path)
        .user(user)
        .build()
        .unwrap();
    let transport = SubprocessTransport::new(options);
    transport.connect().await.unwrap();
    let pid = transport.process_id().unwrap().to_string();
    assert_eq!(
        command_output("ps", &["-o", "uid=", "-p", &pid]),
        expected_uid
    );
    assert_eq!(
        command_output("ps", &["-o", "gid=", "-p", &pid]),
        expected_gid
    );
    if cfg!(target_os = "linux") && started_as_root {
        let status = std::fs::read_to_string(format!("/proc/{pid}/status")).unwrap();
        let groups = status
            .lines()
            .find_map(|line| line.strip_prefix("Groups:"))
            .unwrap();
        assert_eq!(groups.trim(), "", "supplementary groups were kept");
    }
    transport.close().await.unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn an_unknown_user_fails_connect_and_nothing_is_spawned() {
    let options = stand_in_options()
        .user("claude-agent-kit-no-such-user")
        .build()
        .unwrap();
    let transport = SubprocessTransport::new(options);
    let message = connection_error(transport.connect().await.unwrap_err());
    assert_eq!(
        message,
        "Failed to start Claude CLI: unknown user: claude-agent-kit-no-such-user"
    );
    assert_eq!(transport.process_id(), None);
    assert!(!transport.is_ready());
}

#[cfg(windows)]
#[tokio::test]
async fn the_user_option_is_refused_on_windows_and_nothing_is_spawned() {
    let options = stand_in_options().user("claude").build().unwrap();
    let transport = SubprocessTransport::new(options);
    let message = connection_error(transport.connect().await.unwrap_err());
    assert_eq!(
        message,
        "Failed to start Claude CLI: the user option is not supported on this platform"
    );
    assert_eq!(transport.process_id(), None);
    assert!(!transport.is_ready());
}

const CONSOLE_HOST_VARIABLE: &str = "CLAUDE_AGENT_KIT_TEST_CONSOLE_HOST";

#[test]
fn host_process_for_the_console_window_test() {
    if std::env::var_os(CONSOLE_HOST_VARIABLE).is_none() {
        return;
    }
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let transport = SubprocessTransport::new(stand_in_options().build().unwrap());
        transport.connect().await.unwrap();
        let mut stream = transport.read_messages();
        let init = next_message(&mut stream).await;
        println!("console-window={}", init["fake_console_window"]);
        transport.close().await.unwrap();
    });
}

#[cfg(windows)]
#[test]
fn the_process_gets_no_console_window_even_when_its_parent_has_one() {
    use std::os::windows::process::CommandExt;

    const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;

    let mut host = std::process::Command::new(std::env::current_exe().unwrap());
    host.args([
        "host_process_for_the_console_window_test",
        "--exact",
        "--nocapture",
        "--test-threads=1",
    ]);
    host.env(CONSOLE_HOST_VARIABLE, "1");
    host.creation_flags(CREATE_NEW_CONSOLE);
    let output = host.output().unwrap();
    assert!(output.status.success(), "{output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("console-window=false"), "{stdout}");
}

const STDERR_HOST_VARIABLE: &str = "CLAUDE_AGENT_KIT_TEST_STDERR_HOST_SCENARIO";
const STDERR_MARKER: &str = "inherited-stderr-marker-5c1e";

#[test]
fn host_process_for_the_inherited_stderr_test() {
    let Some(scenario) = std::env::var_os(STDERR_HOST_VARIABLE) else {
        return;
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let options = common::options_for(std::path::Path::new(&scenario))
            .build()
            .unwrap();
        let transport = SubprocessTransport::new(options);
        transport.connect().await.unwrap();
        collect(transport.read_messages()).await;
        transport.close().await.unwrap();
    });
}

#[test]
fn without_a_callback_the_error_output_is_inherited_not_piped() {
    let fake = Fake::new(json!({
        "steps": [{"stderr": format!("{STDERR_MARKER}\n")}, {"exit": 0}],
    }));
    let mut host = std::process::Command::new(std::env::current_exe().unwrap());
    host.args([
        "host_process_for_the_inherited_stderr_test",
        "--exact",
        "--nocapture",
        "--test-threads=1",
    ]);
    host.env(STDERR_HOST_VARIABLE, &fake.scenario);
    let output = host.output().unwrap();
    assert!(output.status.success(), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains(STDERR_MARKER),
        "the marker did not reach the inherited error output: {output:?}"
    );
}
