//! Recording of what `fake-claude` received.
//!
//! When the environment variable `FAKE_CLAUDE_RECORD` names a file, the
//! program appends one JSON object per line to it, so that two SDKs can be
//! compared on what they actually sent. The file is opened in append mode:
//! every process started with the variable adds its own records, in the
//! order it receives them. Each record is written with one call and flushed
//! at once, so a process killed mid-run leaves complete lines only.
//!
//! | Record | Written when | Fields |
//! |---|---|---|
//! | `{"record": "spawn", ...}` | the process starts | `args`, the arguments after the program name; `cwd`, the working directory; `env`, the selected variables |
//! | `{"record": "stdin", "frame": value}` | a stdin line holds valid JSON | `frame`, the parsed value |
//! | `{"record": "stdin_raw", "text": line}` | a stdin line is not valid JSON | `text`, the line without its line feed |
//! | `{"record": "stdin_eof"}` | stdin closes | none |
//!
//! Every record also carries `pid`, the identifier of the process that
//! wrote it, so that the records of two processes sharing one file can be
//! told apart.
//!
//! The recording rule selects every variable whose name starts with
//! `CLAUDE_` or `ANTHROPIC_`, plus `PWD`, `MAX_THINKING_TOKENS`,
//! `TRACEPARENT`, and `TRACESTATE`. Names are matched as written, with no
//! change of case. A scenario that does not read stdin records only the
//! spawn.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};

use serde_json::{Map, Value, json};

/// Environment variable naming the record file.
pub(crate) const RECORD_VARIABLE: &str = "FAKE_CLAUDE_RECORD";

const RECORDED_PREFIXES: [&str; 2] = ["CLAUDE_", "ANTHROPIC_"];
const RECORDED_NAMES: [&str; 4] = ["PWD", "MAX_THINKING_TOKENS", "TRACEPARENT", "TRACESTATE"];

/// Writer of the record file; a recorder with no file writes nothing.
#[derive(Clone, Default)]
pub(crate) struct Recorder(Option<Arc<Mutex<File>>>);

impl Recorder {
    /// Opens the record file in append mode, creating it when missing.
    pub(crate) fn open(path: &Path) -> std::io::Result<Self> {
        let file = OpenOptions::new().create(true).append(true).open(path)?;
        Ok(Self(Some(Arc::new(Mutex::new(file)))))
    }

    /// Writes the `spawn` record of this process.
    pub(crate) fn spawn(&self, arguments: &[String]) {
        if self.0.is_none() {
            return;
        }
        let cwd = std::env::current_dir().map_or(Value::Null, |cwd| {
            Value::String(cwd.to_string_lossy().into_owned())
        });
        let env: Map<String, Value> = std::env::vars_os()
            .filter_map(|(name, value)| {
                let name = name.into_string().ok()?;
                is_recorded(&name)
                    .then(|| (name, Value::String(value.to_string_lossy().into_owned())))
            })
            .collect();
        self.write(&json!({
            "record": "spawn",
            "args": arguments,
            "cwd": cwd,
            "env": env,
        }));
    }

    /// Writes the record of one stdin line.
    pub(crate) fn stdin_line(&self, line: &str, frame: Option<&Value>) {
        match frame {
            Some(frame) => self.write(&json!({"record": "stdin", "frame": frame})),
            None => self.write(&json!({"record": "stdin_raw", "text": line})),
        }
    }

    /// Writes the record of the end of stdin.
    pub(crate) fn stdin_eof(&self) {
        self.write(&json!({"record": "stdin_eof"}));
    }

    fn write(&self, record: &Value) {
        let Some(file) = &self.0 else {
            return;
        };
        let mut record = record.clone();
        record["pid"] = Value::from(std::process::id());
        let mut line = record.to_string();
        line.push('\n');
        let mut file = file.lock().unwrap_or_else(PoisonError::into_inner);
        let _ = file.write_all(line.as_bytes());
        let _ = file.flush();
    }
}

/// Whether the recording rule selects the variable.
pub(crate) fn is_recorded(name: &str) -> bool {
    RECORDED_PREFIXES
        .iter()
        .any(|prefix| name.starts_with(prefix))
        || RECORDED_NAMES.contains(&name)
}

#[cfg(test)]
mod tests {
    use super::is_recorded;

    #[test]
    fn rule_selects_the_documented_variables_only() {
        for name in [
            "CLAUDE_CODE_ENTRYPOINT",
            "CLAUDE_AGENT_SDK_VERSION",
            "ANTHROPIC_API_KEY",
            "PWD",
            "MAX_THINKING_TOKENS",
            "TRACEPARENT",
            "TRACESTATE",
        ] {
            assert!(is_recorded(name), "{name}");
        }
        for name in [
            "CLAUDECODE",
            "claude_code_entrypoint",
            "PATH",
            "HOME",
            "FAKE_CLAUDE_RECORD",
            "FAKE_CLAUDE_SCENARIO",
            "OLDPWD",
        ] {
            assert!(!is_recorded(name), "{name}");
        }
    }
}
