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
//! | `{"record": "stdin", "frame": value, "text": line}` | a stdin line holds valid JSON | `frame`, the parsed value; `text`, the line as received, which keeps the key order that `frame` may lose |
//! | `{"record": "stdin_raw", "text": line}` | a stdin line is not valid JSON | `text`, the line without its line feed |
//! | `{"record": "stdin_eof"}` | stdin closes | none |
//!
//! Every record also carries `pid`, the identifier of the process that
//! wrote it, so that the records of two processes sharing one file can be
//! told apart.
//!
//! The recording rule keeps every variable except those of
//! [`UNRECORDED_VARIABLES`] and those whose name starts with
//! `FAKE_CLAUDE_`, the variables that drive this program. The list names
//! what the operating system or the harness puts in every process (`PATH`,
//! the home and temporary directories, the locale, the Windows system
//! variables) and what the harness gives one driver only (`PYTHONPATH`,
//! `PYTHONDONTWRITEBYTECODE`, `PARITY_JOURNAL`): a variable an SDK adds,
//! removes, or passes on from its own process is recorded, whatever its
//! name. Names are matched as written, with no change of case, except on
//! Windows, where names are case-insensitive and are matched ignoring
//! case. A scenario that does not read stdin records only the spawn.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};

use serde_json::{Map, Value, json};

/// Environment variable naming the record file.
pub(crate) const RECORD_VARIABLE: &str = "FAKE_CLAUDE_RECORD";

/// Variables the recording leaves out: those of the operating system and
/// of the harness, which no SDK chooses.
pub(crate) const UNRECORDED_VARIABLES: [&str; 17] = [
    "PATH",
    "HOME",
    "USERPROFILE",
    "TMPDIR",
    "TEMP",
    "TMP",
    "LANG",
    "LC_ALL",
    "SYSTEMROOT",
    "SYSTEMDRIVE",
    "WINDIR",
    "COMSPEC",
    "PATHEXT",
    "PYTHONPATH",
    "PYTHONDONTWRITEBYTECODE",
    "PARITY_JOURNAL",
    "__CF_USER_TEXT_ENCODING",
];

/// Prefix of the variables that drive this program.
const OWN_PREFIX: &str = "FAKE_CLAUDE_";

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
            Some(frame) => self.write(&json!({"record": "stdin", "frame": frame, "text": line})),
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

/// Whether the recording rule keeps the variable.
pub(crate) fn is_recorded(name: &str) -> bool {
    let same = |known: &str| {
        if cfg!(windows) {
            known.eq_ignore_ascii_case(name)
        } else {
            known == name
        }
    };
    !name.starts_with(OWN_PREFIX) && !UNRECORDED_VARIABLES.iter().any(|known| same(known))
}

#[cfg(test)]
mod tests {
    use super::{UNRECORDED_VARIABLES, is_recorded};

    #[test]
    fn rule_keeps_every_variable_but_the_system_and_harness_ones() {
        for name in [
            "CLAUDE_CODE_ENTRYPOINT",
            "CLAUDE_AGENT_SDK_VERSION",
            "ANTHROPIC_API_KEY",
            "CLAUDECODE",
            "claude_code_entrypoint",
            "PWD",
            "OLDPWD",
            "MAX_THINKING_TOKENS",
            "TRACEPARENT",
            "TRACESTATE",
            "NODE_OPTIONS",
            "HTTPS_PROXY",
        ] {
            assert!(is_recorded(name), "{name}");
        }
        for name in UNRECORDED_VARIABLES
            .into_iter()
            .chain(["FAKE_CLAUDE_RECORD", "FAKE_CLAUDE_SCENARIO"])
        {
            assert!(!is_recorded(name), "{name}");
        }
    }
}
