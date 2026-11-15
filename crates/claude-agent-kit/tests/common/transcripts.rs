use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::{Duration, SystemTime};

use claude_agent_kit::project_key_for_directory;
use serde_json::{Value, json};
use tempfile::TempDir;

/// A config directory shared by the tests of one binary, set as
/// `CLAUDE_CONFIG_DIR` before any test reads it. Tests stay independent by
/// working in their own project directory.
static CONFIG_DIR: LazyLock<TempDir> = LazyLock::new(|| {
    let directory = tempfile::tempdir().unwrap();
    unsafe { std::env::set_var("CLAUDE_CONFIG_DIR", directory.path()) };
    directory
});

pub fn config_dir() -> &'static Path {
    CONFIG_DIR.path()
}

pub fn projects_dir() -> PathBuf {
    config_dir().join("projects")
}

/// A working directory of its own and the project directory the CLI would
/// use for it.
pub struct Project {
    pub cwd: TempDir,
    pub dir: PathBuf,
}

impl Project {
    pub fn new() -> Self {
        let cwd = tempfile::tempdir().unwrap();
        Self::at(cwd)
    }

    pub fn at(cwd: TempDir) -> Self {
        let dir = projects_dir().join(project_key_for_directory(Some(cwd.path())));
        std::fs::create_dir_all(&dir).unwrap();
        Self { cwd, dir }
    }

    pub fn path(&self) -> &Path {
        self.cwd.path()
    }

    pub fn transcript(&self, session_id: &str) -> PathBuf {
        self.dir.join(format!("{session_id}.jsonl"))
    }

    /// Writes a transcript, one JSON value per line.
    pub fn write(&self, session_id: &str, lines: &[Value]) -> PathBuf {
        let path = self.transcript(session_id);
        write_lines(&path, lines);
        path
    }
}

pub fn write_lines(path: &Path, lines: &[Value]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut text = String::new();
    for line in lines {
        text.push_str(&type_first(line));
        text.push('\n');
    }
    std::fs::write(path, text).unwrap();
}

/// Serializes a value on one line with its `type` first, as the CLI writes
/// it.
pub fn type_first(value: &Value) -> String {
    let Value::Object(fields) = value else {
        return value.to_string();
    };
    let mut parts = Vec::new();
    if let Some(kind) = fields.get("type") {
        parts.push(format!("\"type\":{kind}"));
    }
    for (key, field) in fields.iter().filter(|(key, _)| *key != "type") {
        parts.push(format!("{}:{field}", Value::from(key.as_str())));
    }
    format!("{{{}}}", parts.join(","))
}

pub fn read_lines(path: &Path) -> Vec<Value> {
    std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

pub fn set_mtime(path: &Path, seconds_ago: u64) {
    let file = std::fs::File::options().append(true).open(path).unwrap();
    file.set_modified(SystemTime::now() - Duration::from_secs(seconds_ago))
        .unwrap();
}

pub fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

pub fn user(uuid: &str, parent: Option<&str>, session_id: &str, content: Value) -> Value {
    json!({
        "type": "user",
        "uuid": uuid,
        "parentUuid": parent,
        "sessionId": session_id,
        "timestamp": "2026-01-15T10:30:00.000Z",
        "message": {"role": "user", "content": content},
    })
}

pub fn assistant(uuid: &str, parent: Option<&str>, session_id: &str, text: &str) -> Value {
    json!({
        "type": "assistant",
        "uuid": uuid,
        "parentUuid": parent,
        "sessionId": session_id,
        "timestamp": "2026-01-15T10:30:01.000Z",
        "message": {"role": "assistant", "id": format!("msg-{uuid}"), "content": [{"type": "text", "text": text}]},
    })
}

/// A short conversation: a prompt, an answer, and optional tail fields.
pub fn conversation(session_id: &str, prompt: &str, tail: Value) -> Vec<Value> {
    let mut lines = vec![
        user("u1", None, session_id, Value::from(prompt)),
        assistant("a1", Some("u1"), session_id, "Hi there!"),
    ];
    if tail.as_object().is_some_and(|fields| !fields.is_empty()) {
        lines.push(tail);
    }
    lines
}
