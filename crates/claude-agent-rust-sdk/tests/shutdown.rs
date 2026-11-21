#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use claude_agent_rust_sdk::{ClaudeAgentOptions, Error, SubprocessTransport};
use futures_util::FutureExt;
use serde_json::json;

use common::{Fake, fake_claude, next_message, process_exists, stand_in_options, wait_until_gone};

const HOST_VARIABLE: &str = "CLAUDE_AGENT_RUST_SDK_TEST_HOST_SCENARIO";
const WORST_CASE_CLOSE: Duration = Duration::from_secs(20);

async fn connected(options: ClaudeAgentOptions) -> (SubprocessTransport, u32) {
    let transport = SubprocessTransport::new(options);
    transport.connect().await.unwrap();
    let pid = transport.process_id().unwrap();
    assert!(process_exists(pid));
    (transport, pid)
}

fn stubborn_cli(fake: &Fake) -> PathBuf {
    if cfg!(windows) {
        return fake_claude();
    }
    let wrapper = fake.directory.path().join("claude-ignoring-sigterm");
    std::fs::write(
        &wrapper,
        format!(
            "#!/bin/sh\ntrap '' TERM\nexec \"{}\" \"$@\"\n",
            fake_claude().display()
        ),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    wrapper
}

#[tokio::test]
async fn close_ends_a_cooperative_process_without_escalating() {
    let (transport, pid) = connected(stand_in_options().build().unwrap()).await;
    let started = Instant::now();
    transport.close().await.unwrap();
    assert!(
        started.elapsed() < Duration::from_secs(4),
        "{:?}",
        started.elapsed()
    );
    assert!(!process_exists(pid));
    assert!(!transport.is_ready());

    let again = Instant::now();
    transport.close().await.unwrap();
    assert!(again.elapsed() < Duration::from_secs(1));
}

#[tokio::test]
async fn close_before_connect_is_a_no_op() {
    let transport = SubprocessTransport::new(stand_in_options().build().unwrap());
    transport.close().await.unwrap();
    assert!(!transport.is_ready());
    transport.connect().await.unwrap();
    assert!(transport.is_ready());
    transport.close().await.unwrap();
}

#[tokio::test]
async fn close_runs_to_completion_when_its_future_is_dropped() {
    let fake = Fake::new(json!({"read_stdin": false, "steps": [{"sleep_ms": 120_000}]}));
    let (transport, pid) = connected(fake.build()).await;
    assert!(transport.close().now_or_never().is_none());
    assert!(process_exists(pid));
    assert!(
        wait_until_gone(pid, WORST_CASE_CLOSE).await,
        "the detached shutdown did not end the process"
    );
    transport.close().await.unwrap();
}

#[tokio::test]
async fn dropping_the_transport_without_closing_still_ends_the_process() {
    let fake = Fake::new(json!({"read_stdin": false, "steps": [{"sleep_ms": 120_000}]}));
    let (transport, pid) = connected(fake.build()).await;
    drop(transport);
    assert!(
        wait_until_gone(pid, WORST_CASE_CLOSE).await,
        "the supervisor did not end the abandoned process"
    );
}

#[tokio::test]
async fn a_process_that_ignores_termination_is_dead_within_twenty_seconds() {
    let fake = Fake::new(json!({"read_stdin": false, "steps": [{"sleep_ms": 120_000}]}));
    let options = fake
        .options()
        .cli_path(stubborn_cli(&fake))
        .build()
        .unwrap();
    let (transport, pid) = connected(options).await;

    let started = Instant::now();
    transport.close().await.unwrap();
    let elapsed = started.elapsed();

    assert!(elapsed < WORST_CASE_CLOSE, "{elapsed:?}");
    let escalation_floor = if cfg!(windows) { 5 } else { 10 };
    assert!(
        elapsed >= Duration::from_secs(escalation_floor),
        "{elapsed:?}"
    );
    assert!(!process_exists(pid));
}

#[tokio::test]
async fn a_process_deaf_to_the_end_of_input_ends_at_the_first_escalation_step() {
    let fake = Fake::new(json!({"read_stdin": false, "steps": [{"sleep_ms": 120_000}]}));
    let (transport, pid) = connected(fake.build()).await;

    let started = Instant::now();
    transport.close().await.unwrap();
    let elapsed = started.elapsed();

    assert!(elapsed >= Duration::from_secs(5), "{elapsed:?}");
    assert!(elapsed < Duration::from_secs(10), "{elapsed:?}");
    assert!(!process_exists(pid));
}

#[tokio::test]
async fn close_returns_at_once_when_the_process_has_already_exited() {
    let fake = Fake::new(json!({"steps": [{"exit": 0}]}));
    let transport = SubprocessTransport::new(fake.build());
    transport.connect().await.unwrap();
    let pid = transport.process_id().unwrap();
    assert!(wait_until_gone(pid, Duration::from_secs(10)).await);

    let started = Instant::now();
    transport.close().await.unwrap();

    assert!(
        started.elapsed() < Duration::from_secs(1),
        "{:?}",
        started.elapsed()
    );
    assert!(!transport.is_ready());
}

#[tokio::test]
async fn descendants_end_with_the_session() {
    let fake = Fake::new(json!({
        "steps": [{"spawn_descendant": {"sleep_ms": 120_000}}, {"wait_for_eof": true}],
    }));
    let (transport, pid) = connected(fake.build()).await;
    let mut stream = transport.read_messages();
    assert_eq!(next_message(&mut stream).await["subtype"], "init");
    let descendant = next_message(&mut stream).await;
    assert_eq!(descendant["subtype"], "descendant");
    let descendant = u32::try_from(descendant["pid"].as_u64().unwrap()).unwrap();
    assert!(process_exists(descendant));

    transport.close().await.unwrap();
    assert!(!process_exists(pid));
    assert!(
        wait_until_gone(descendant, Duration::from_secs(5)).await,
        "the descendant outlived the session"
    );
}

/// A descendant left in the group when the CLI exits gets `SIGTERM` first,
/// so a server the agent started in the background can clean up.
#[cfg(unix)]
#[tokio::test]
async fn descendants_left_at_exit_get_sigterm_before_sigkill() {
    let fake = Fake::new(json!({"steps": [{"wait_for_eof": true}]}));
    let marker = fake.directory.path().join("cleaned");
    let ready = fake.directory.path().join("ready");
    let wrapper = fake
        .directory
        .path()
        .join("claude-with-a-background-server");
    std::fs::write(
        &wrapper,
        format!(
            "#!/bin/sh\n( trap 'echo cleaned > \"{}\"; exit 0' TERM; echo ready > \"{}\"; while :; do sleep 1 & wait $!; done ) &\nexec \"{}\" \"$@\"\n",
            marker.display(),
            ready.display(),
            fake_claude().display()
        ),
    )
    .unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let (transport, _pid) = connected(fake.options().cli_path(&wrapper).build().unwrap()).await;
    let deadline = Instant::now() + Duration::from_secs(10);
    while !std::fs::read_to_string(&ready).is_ok_and(|text| text.ends_with('\n')) {
        assert!(
            Instant::now() < deadline,
            "the background server never started"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    transport.close().await.unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while std::fs::read_to_string(&marker).ok().as_deref() != Some("cleaned\n") {
        assert!(
            Instant::now() < deadline,
            "the background server was killed without a chance to clean up"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn descendants_of_a_killed_process_end_too() {
    let fake = Fake::new(json!({
        "read_stdin": false,
        "steps": [{"spawn_descendant": {"sleep_ms": 120_000}}, {"sleep_ms": 120_000}],
    }));
    let (transport, pid) = connected(fake.build()).await;
    let mut stream = transport.read_messages();
    next_message(&mut stream).await;
    let descendant = next_message(&mut stream).await;
    let descendant = u32::try_from(descendant["pid"].as_u64().unwrap()).unwrap();

    let started = Instant::now();
    transport.close().await.unwrap();
    assert!(started.elapsed() < WORST_CASE_CLOSE);
    assert!(!process_exists(pid));
    assert!(wait_until_gone(descendant, Duration::from_secs(5)).await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_blocked_writer_cannot_hold_the_shutdown() {
    let fake = Fake::new(json!({"read_stdin": false, "steps": [{"sleep_ms": 120_000}]}));
    let (transport, pid) = connected(fake.build()).await;
    let transport = Arc::new(transport);

    let writer = {
        let transport = Arc::clone(&transport);
        tokio::spawn(async move {
            let payload = "x".repeat(64 * 1024 * 1024);
            transport.write(&payload).await
        })
    };
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(
        !writer.is_finished(),
        "the writer should be blocked on a full pipe"
    );

    let started = Instant::now();
    transport.close().await.unwrap();
    let elapsed = started.elapsed();
    assert!(elapsed >= Duration::from_secs(5), "{elapsed:?}");
    assert!(elapsed < WORST_CASE_CLOSE, "{elapsed:?}");
    assert!(!process_exists(pid));

    let written = tokio::time::timeout(Duration::from_secs(10), writer)
        .await
        .expect("the blocked writer was never released")
        .unwrap();
    assert!(
        written.as_ref().err().is_some_and(Error::is_cli_connection),
        "{written:?}"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn the_process_leads_its_own_process_group() {
    let (transport, pid) = connected(stand_in_options().build().unwrap()).await;
    let group_of = |pid: u32| {
        let output = std::process::Command::new("ps")
            .args(["-o", "pgid=", "-p", &pid.to_string()])
            .output()
            .unwrap();
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    };
    assert_eq!(group_of(pid), pid.to_string());
    assert_ne!(group_of(std::process::id()), pid.to_string());
    transport.close().await.unwrap();
}

#[test]
fn dropping_the_runtime_kills_the_process() {
    let fake = Fake::new(json!({"read_stdin": false, "steps": [{"sleep_ms": 120_000}]}));
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .unwrap();
    let (transport, pid) = runtime.block_on(connected(fake.build()));
    drop(runtime);

    let deadline = Instant::now() + Duration::from_secs(5);
    while process_exists(pid) {
        assert!(
            Instant::now() < deadline,
            "the process outlived its runtime"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    drop(transport);
}

#[test]
fn host_process_for_the_parent_death_test() {
    let Some(scenario) = std::env::var_os(HOST_VARIABLE) else {
        return;
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let options = common::options_for(std::path::Path::new(&scenario))
        .build()
        .unwrap();
    let (transport, pid) = runtime.block_on(connected(options));
    println!("child-pid={pid}");
    std::mem::forget(transport);
    std::process::abort();
}

#[cfg(any(windows, target_os = "linux"))]
#[test]
fn the_process_dies_when_its_parent_dies_abruptly() {
    let fake = Fake::new(json!({"read_stdin": false, "steps": [{"sleep_ms": 120_000}]}));
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "host_process_for_the_parent_death_test",
            "--exact",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(HOST_VARIABLE, &fake.scenario)
        .output()
        .unwrap();
    assert!(!output.status.success(), "the host process aborts");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let pid: u32 = stdout
        .lines()
        .find_map(|line| line.split_once("child-pid=").map(|(_, pid)| pid))
        .unwrap_or_else(|| panic!("the host did not report a pid: {stdout}"))
        .trim()
        .parse()
        .unwrap();

    let deadline = Instant::now() + Duration::from_secs(10);
    while process_exists(pid) {
        assert!(Instant::now() < deadline, "the process outlived its parent");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// The wrapper only ends through its `TERM` trap, or by being killed: it
/// does not wait on the CLI. It writes its pid once the trap is set, and
/// the runtime is dropped only then, because `connect` returns as soon as
/// the process is spawned: a `SIGTERM` that arrived before the trap would
/// end the shell by its default action, which the test would take for a
/// kill.
#[cfg(unix)]
#[test]
fn dropping_the_runtime_lets_the_process_handle_termination() {
    let fake = Fake::new(json!({"read_stdin": false, "steps": [{"sleep_ms": 120_000}]}));
    let marker = fake.directory.path().join("terminated");
    let shell_pid = fake.directory.path().join("shell-pid");
    let wrapper = fake.directory.path().join("claude-handling-sigterm");
    std::fs::write(
        &wrapper,
        format!(
            "#!/bin/sh\ntrap 'echo flushed > \"{}\"; exit 0' TERM\necho $$ > \"{}\"\n\"{}\" \"$@\" &\nwhile :; do sleep 1 & wait $!; done\n",
            marker.display(),
            shell_pid.display(),
            fake_claude().display()
        ),
    )
    .unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .unwrap();
    let options = fake.options().cli_path(&wrapper).build().unwrap();
    let (transport, pid) = runtime.block_on(connected(options));
    let deadline = Instant::now() + Duration::from_secs(10);
    while !std::fs::read_to_string(&shell_pid).is_ok_and(|pid| pid.ends_with('\n')) {
        assert!(Instant::now() < deadline, "the wrapper never set its trap");
        std::thread::sleep(Duration::from_millis(20));
    }
    drop(runtime);

    let deadline = Instant::now() + Duration::from_secs(5);
    while process_exists(pid) {
        assert!(
            Instant::now() < deadline,
            "the process outlived its runtime"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    while std::fs::read_to_string(&marker).ok().as_deref() != Some("flushed\n")
        && Instant::now() < deadline
    {
        std::thread::sleep(Duration::from_millis(20));
    }
    let shell = std::fs::read_to_string(&shell_pid)
        .ok()
        .and_then(|pid| pid.trim().parse::<u32>().ok());
    let shell_state = shell.map(|pid| {
        let output = std::process::Command::new("ps")
            .args(["-o", "stat=,command=", "-p", &pid.to_string()])
            .output()
            .unwrap();
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    });
    assert_eq!(
        std::fs::read_to_string(&marker).ok().as_deref(),
        Some("flushed\n"),
        "the process got SIGTERM and ran its handler, instead of being killed \
         (wrapper shell {shell:?}, its state now: {shell_state:?})"
    );
    drop(transport);
}

#[cfg(unix)]
#[tokio::test]
async fn a_cli_killed_by_a_signal_gives_a_retryable_process_error() {
    use std::os::unix::fs::PermissionsExt;

    let directory = tempfile::tempdir().unwrap();
    let script = directory.path().join("claude-killed");
    std::fs::write(&script, "#!/bin/sh\nkill -9 $$\n").unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    let options = ClaudeAgentOptions::builder()
        .cli_path(&script)
        .build()
        .unwrap();
    let transport = SubprocessTransport::new(options);
    transport.connect().await.unwrap();
    let mut stream = transport.read_messages();
    let error = loop {
        match common::next(&mut stream).await {
            Some(Err(error)) => break error,
            Some(Ok(_)) => {}
            None => panic!("the stream ended without the process error"),
        }
    };
    assert_eq!(error.kind(), claude_agent_rust_sdk::ErrorKind::Process);
    assert_eq!(error.exit_code(), Some(-9));
    assert!(error.is_retryable(), "{error}");
    transport.close().await.unwrap();
}
