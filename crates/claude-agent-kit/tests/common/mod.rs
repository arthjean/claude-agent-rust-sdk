#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use std::time::Duration;

use claude_agent_kit::{ClaudeAgentOptions, ClaudeAgentOptionsBuilder, Error, MessageStream};
use futures_util::StreamExt;
use serde_json::Value;

#[cfg(feature = "testing")]
pub mod mcp;
#[cfg(feature = "testing")]
pub mod mock;
pub mod transcripts;

pub const SCENARIO_VARIABLE: &str = "FAKE_CLAUDE_SCENARIO";
pub const STEP_LIMIT: Duration = Duration::from_secs(30);

pub const FAKE_CLAUDE_VARIABLE: &str = "CLAUDE_AGENT_KIT_TEST_FAKE_CLAUDE";

pub fn fake_claude() -> PathBuf {
    static BINARY: OnceLock<PathBuf> = OnceLock::new();
    BINARY
        .get_or_init(|| {
            let binary = match std::env::var_os(FAKE_CLAUDE_VARIABLE) {
                Some(path) => PathBuf::from(path),
                None => build_fake_claude(),
            };
            assert!(
                binary.is_file(),
                "fake-claude not found at {}",
                binary.display()
            );
            binary
        })
        .clone()
}

fn build_fake_claude() -> PathBuf {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let mut build = Command::new(cargo);
    build.args([
        "build",
        "--package",
        "fake-claude",
        "--bin",
        "fake-claude",
        "--message-format=json",
    ]);
    if !cfg!(debug_assertions) {
        build.arg("--release");
    }
    let output = build
        .stderr(std::process::Stdio::inherit())
        .output()
        .expect("cargo must be runnable");
    assert!(output.status.success(), "building fake-claude failed");
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|message| {
            message["reason"] == "compiler-artifact" && message["target"]["name"] == "fake-claude"
        })
        .find_map(|message| message["executable"].as_str().map(PathBuf::from))
        .expect("cargo did not report the fake-claude executable")
}

pub fn scenarios_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("fake-claude")
        .join("scenarios")
}

pub struct Fake {
    pub directory: tempfile::TempDir,
    pub scenario: PathBuf,
}

impl Fake {
    pub fn new(scenario: Value) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("scenario.json");
        std::fs::write(&path, scenario.to_string()).unwrap();
        Self {
            directory,
            scenario: path,
        }
    }

    pub fn options(&self) -> ClaudeAgentOptionsBuilder {
        options_for(&self.scenario)
    }

    pub fn build(&self) -> ClaudeAgentOptions {
        self.options().build().unwrap()
    }
}

pub fn options_for(scenario: &Path) -> ClaudeAgentOptionsBuilder {
    ClaudeAgentOptions::builder()
        .cli_path(fake_claude())
        .env_var(SCENARIO_VARIABLE, scenario.to_string_lossy())
}

pub fn stand_in_options() -> ClaudeAgentOptionsBuilder {
    ClaudeAgentOptions::builder().cli_path(fake_claude())
}

pub async fn next(stream: &mut MessageStream) -> Option<Result<Value, Error>> {
    tokio::time::timeout(STEP_LIMIT, stream.next())
        .await
        .expect("the message stream stalled")
}

pub async fn next_message(stream: &mut MessageStream) -> Value {
    next(stream)
        .await
        .expect("the message stream ended early")
        .expect("the message stream yielded an error")
}

pub async fn collect(mut stream: MessageStream) -> Vec<Result<Value, Error>> {
    let mut items = Vec::new();
    while let Some(item) = next(&mut stream).await {
        items.push(item);
    }
    items
}

pub fn strings(arguments: &[std::ffi::OsString]) -> Vec<String> {
    arguments
        .iter()
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect()
}

pub fn process_exists(pid: u32) -> bool {
    if cfg!(windows) {
        let output = Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
            .output()
            .unwrap();
        String::from_utf8_lossy(&output.stdout).contains(&format!("\"{pid}\""))
    } else {
        let output = Command::new("ps")
            .args(["-o", "stat=", "-p", &pid.to_string()])
            .output()
            .unwrap();
        let state = String::from_utf8_lossy(&output.stdout);
        let state = state.trim();
        !state.is_empty() && !state.starts_with('Z')
    }
}

pub async fn wait_until_gone(pid: u32, limit: Duration) -> bool {
    let deadline = tokio::time::Instant::now() + limit;
    while process_exists(pid) {
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    true
}

pub fn processes_matching(token: &str) -> Vec<String> {
    let output = if cfg!(windows) {
        Command::new("tasklist")
            .args(["/FO", "CSV", "/NH"])
            .output()
    } else {
        Command::new("ps").args(["-A", "-o", "pid=,args="]).output()
    }
    .unwrap();
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| line.contains(token))
        .map(str::to_owned)
        .collect()
}

pub mod capture {
    use std::fmt::Debug;
    use std::sync::{Arc, Mutex};

    use tracing::field::{Field, Visit};
    use tracing::span::{Attributes, Id, Record};
    use tracing::subscriber::{DefaultGuard, Interest};
    use tracing::{Event, Level, Metadata, Subscriber};

    #[derive(Clone, Default)]
    pub struct Warnings(Arc<Mutex<Vec<String>>>);

    impl Warnings {
        pub fn install() -> (Self, DefaultGuard) {
            let warnings = Self::default();
            let guard = tracing::subscriber::set_default(Collector(warnings.clone()));
            (warnings, guard)
        }

        pub fn all(&self) -> Vec<String> {
            self.0.lock().unwrap().clone()
        }

        pub fn about_the_cli_version(&self) -> Vec<String> {
            self.0
                .lock()
                .unwrap()
                .iter()
                .filter(|message| {
                    message.contains("too old to report its version")
                        || message.contains("verbatim_prompts is enabled")
                })
                .cloned()
                .collect()
        }
    }

    struct Collector(Warnings);

    struct Message(String);

    impl Visit for Message {
        fn record_debug(&mut self, field: &Field, value: &dyn Debug) {
            if field.name() == "message" {
                self.0 = format!("{value:?}");
            }
        }
    }

    impl Subscriber for Collector {
        fn register_callsite(&self, _metadata: &'static Metadata<'static>) -> Interest {
            Interest::sometimes()
        }

        fn enabled(&self, _metadata: &Metadata<'_>) -> bool {
            true
        }

        fn new_span(&self, _attributes: &Attributes<'_>) -> Id {
            Id::from_u64(1)
        }

        fn record(&self, _span: &Id, _values: &Record<'_>) {}

        fn record_follows_from(&self, _span: &Id, _follows: &Id) {}

        fn event(&self, event: &Event<'_>) {
            if *event.metadata().level() != Level::WARN {
                return;
            }
            let mut message = Message(String::new());
            event.record(&mut message);
            self.0.0.lock().unwrap().push(message.0);
        }

        fn enter(&self, _span: &Id) {}

        fn exit(&self, _span: &Id) {}
    }
}
