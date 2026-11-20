//! Comparison of what the CLI received from each SDK.
//!
//! A recording is the record file `fake-claude` writes (see its `record`
//! module). Records are grouped by process, in the order the processes
//! started. For each process the comparison looks at:
//!
//! - the arguments, read as flags: `--name value`, `--name=value` (kept
//!   apart from the two-token form, under the key `--name=`), or a bare
//!   `--name` (the value `true`). Flags are compared by name, not by
//!   position; a flag given several times is compared occurrence by
//!   occurrence. A value that parses as a JSON object or array is compared
//!   as JSON.
//! - the working directory and each recorded environment variable;
//! - the stdin frames. Each frame gets a label: `control_request:<subtype>`,
//!   `control_response:<request_id>`, the frame's `type` otherwise, `raw`
//!   for a line that is not JSON, and `eof` for the end of stdin; a label
//!   seen again gets `#2`, `#3`, and so on. Frames are paired by label and
//!   compared as JSON; the order of the labels both sides share is compared
//!   too, except inside a group a case declares `unordered`.
//!
//! JSON values are compared as the CLI reads them, numbers being equal when
//! their values are (`5` and `5.0`), and then on the order of the keys the
//! two sides share: key order reaches the model (the properties of an
//! output schema) and the CLI (the order of MCP servers or subagents), so
//! a different order is a difference at `<pointer> (key order)`. A stdin
//! frame is read from the line as received (the record's `text`), because
//! the parsed `frame` of `fake-claude` may have lost the order.
//! Values are normalized first; the rules are those of `Normalizer`.
//!
//! A difference has a location: `processes`, `cwd`, `env <NAME>`,
//! `arg <flag>` (with `[i]` for the occurrence `i` after the first, and a
//! JSON pointer for a JSON value), `frame <label>` (with a JSON pointer
//! into the frame), or `frames` for the order. The locations of a process
//! after the first start with `process[i] `.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value};

/// What one process received.
#[derive(Debug, Clone, PartialEq)]
pub struct Process {
    /// Arguments after the program name.
    pub args: Vec<String>,
    /// Working directory, or `null` when it could not be read.
    pub cwd: Value,
    /// Recorded environment variables.
    pub env: Map<String, Value>,
    /// The stdin records, in arrival order: a parsed frame, the text of a
    /// line that is not JSON, or the end of stdin.
    pub input: Vec<Input>,
}

/// One stdin record.
#[derive(Debug, Clone, PartialEq)]
pub enum Input {
    /// A line holding valid JSON.
    Frame(Value),
    /// A line that is not valid JSON.
    Raw(String),
    /// The end of stdin.
    Eof,
}

/// Every process that wrote to one record file.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Recording {
    /// Processes, in the order they started.
    pub processes: Vec<Process>,
}

impl Recording {
    /// Parses a record file.
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut processes: Vec<(Value, Process)> = Vec::new();
        for (index, line) in text.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let record: Value = serde_json::from_str(line)
                .map_err(|error| format!("record line {}: {error}", index + 1))?;
            let pid = record.get("pid").cloned().unwrap_or(Value::Null);
            let kind = record
                .get("record")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if kind == "spawn" {
                let args = record
                    .get("args")
                    .and_then(Value::as_array)
                    .map(|args| {
                        args.iter()
                            .map(|arg| arg.as_str().unwrap_or_default().to_owned())
                            .collect()
                    })
                    .unwrap_or_default();
                let env = record
                    .get("env")
                    .and_then(Value::as_object)
                    .cloned()
                    .unwrap_or_default();
                let cwd = record.get("cwd").cloned().unwrap_or(Value::Null);
                processes.push((
                    pid,
                    Process {
                        args,
                        cwd,
                        env,
                        input: Vec::new(),
                    },
                ));
                continue;
            }
            let input = match kind {
                "stdin" => Input::Frame(
                    record
                        .get("text")
                        .and_then(Value::as_str)
                        .and_then(|text| serde_json::from_str(text.trim()).ok())
                        .or_else(|| record.get("frame").cloned())
                        .unwrap_or(Value::Null),
                ),
                "stdin_raw" => Input::Raw(
                    record
                        .get("text")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                ),
                "stdin_eof" => Input::Eof,
                other => {
                    return Err(format!(
                        "record line {}: unknown record `{other}`",
                        index + 1
                    ));
                }
            };
            let Some((_, process)) = processes.iter_mut().rev().find(|(owner, _)| *owner == pid)
            else {
                return Err(format!(
                    "record line {}: no spawn record for process {pid}",
                    index + 1
                ));
            };
            process.input.push(input);
        }
        Ok(Self {
            processes: processes.into_iter().map(|(_, process)| process).collect(),
        })
    }

    /// The identifiers of the processes that wrote the file, read without
    /// a full parse, so that a file cut short by a kill still yields them.
    pub fn pids(text: &str) -> Vec<u32> {
        let mut pids = Vec::new();
        for line in text.lines() {
            let Ok(record) = serde_json::from_str::<Value>(line) else {
                continue;
            };
            if record.get("record").and_then(Value::as_str) == Some("spawn")
                && let Some(pid) = record.get("pid").and_then(Value::as_u64)
            {
                pids.extend(u32::try_from(pid).ok());
            }
        }
        pids
    }
}

/// The normalization applied to every string of a recording before it is
/// compared. Each rule is listed in `parity/README.md` with its reason.
#[derive(Debug, Clone, Default)]
pub struct Normalizer {
    prefixes: Vec<(String, &'static str)>,
}

/// Stands for the temporary working directory of a run.
pub const WORK_PLACEHOLDER: &str = "<work>";
/// Stands for the path of the CLI binary.
pub const CLI_PLACEHOLDER: &str = "<cli>";
const REQUEST_ID_RANDOM: &str = "<random>";
const REQUEST_ID_RANDOM_LENGTH: usize = 8;

impl Normalizer {
    /// A normalizer for one side: `work` is its temporary directory and
    /// `cli` the CLI binary, each given in every spelling the side may
    /// write (as created, canonical, with either separator).
    pub fn new(work: &[String], cli: &[String]) -> Self {
        let mut prefixes = Vec::new();
        for (spellings, placeholder) in [(cli, CLI_PLACEHOLDER), (work, WORK_PLACEHOLDER)] {
            for spelling in spellings {
                for variant in [spelling.clone(), spelling.replace('\\', "/")] {
                    if !variant.is_empty() && !prefixes.iter().any(|(known, _)| *known == variant) {
                        prefixes.push((variant, placeholder));
                    }
                }
            }
        }
        prefixes.sort_by_key(|(prefix, _)| std::cmp::Reverse(prefix.len()));
        Self { prefixes }
    }

    /// Normalizes one string: a request id loses its random suffix; the
    /// CLI path and the temporary directory become placeholders, and a
    /// string holding a placeholder gets forward slashes.
    pub fn string(&self, text: &str) -> String {
        if let Some(normalized) = request_id(text) {
            return normalized;
        }
        let mut result = text.to_owned();
        let mut replaced = false;
        for (prefix, placeholder) in &self.prefixes {
            if result.contains(prefix.as_str()) {
                result = result.replace(prefix.as_str(), placeholder);
                replaced = true;
            }
        }
        if replaced {
            result = result.replace('\\', "/");
        }
        result
    }

    /// Normalizes every string inside a JSON value; object keys are kept.
    pub fn value(&self, value: &Value) -> Value {
        match value {
            Value::String(text) => Value::String(self.string(text)),
            Value::Array(items) => items.iter().map(|item| self.value(item)).collect(),
            Value::Object(fields) => fields
                .iter()
                .map(|(key, value)| (key.clone(), self.value(value)))
                .collect::<Map<String, Value>>()
                .into(),
            other => other.clone(),
        }
    }
}

fn request_id(text: &str) -> Option<String> {
    let rest = text.strip_prefix("req_")?;
    let (counter, random) = rest.split_once('_')?;
    let counter_ok = !counter.is_empty() && counter.bytes().all(|byte| byte.is_ascii_digit());
    let random_ok = random.len() == REQUEST_ID_RANDOM_LENGTH
        && random
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
    (counter_ok && random_ok).then(|| format!("req_{counter}_{REQUEST_ID_RANDOM}"))
}

/// One difference between the two sides.
#[derive(Debug, Clone, PartialEq)]
pub struct Difference {
    /// Where the sides differ.
    pub location: String,
    /// The Python value, or `None` when Python sent nothing there.
    pub python: Option<Value>,
    /// The Rust value, or `None` when Rust sent nothing there.
    pub rust: Option<Value>,
}

impl Difference {
    fn new(location: String, python: Option<&Value>, rust: Option<&Value>) -> Self {
        Self {
            location,
            python: python.cloned(),
            rust: rust.cloned(),
        }
    }
}

/// Shows a value of a difference: its JSON text, or `<absent>`.
pub fn show(value: Option<&Value>) -> String {
    value.map_or_else(|| "<absent>".to_owned(), Value::to_string)
}

/// Whether two JSON values are equal the way the CLI reads them.
pub fn same(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::Number(left), Value::Number(right)) => {
            left == right
                || left
                    .as_f64()
                    .is_some_and(|value| Some(value) == right.as_f64())
        }
        (Value::Array(left), Value::Array(right)) => {
            left.len() == right.len()
                && left
                    .iter()
                    .zip(right)
                    .all(|(left, right)| same(left, right))
        }
        (Value::Object(left), Value::Object(right)) => {
            left.len() == right.len()
                && left
                    .iter()
                    .all(|(key, value)| right.get(key).is_some_and(|other| same(value, other)))
        }
        _ => left == right,
    }
}

fn escape(key: &str) -> String {
    key.replace('~', "~0").replace('/', "~1")
}

fn diff_values(
    location: &dyn Fn(&str) -> String,
    pointer: &str,
    python: &Value,
    rust: &Value,
    found: &mut Vec<Difference>,
) {
    match (python, rust) {
        (Value::Object(left), Value::Object(right)) => {
            let keys: BTreeSet<&String> = left.keys().chain(right.keys()).collect();
            for key in keys {
                let path = format!("{pointer}/{}", escape(key));
                match (left.get(key), right.get(key)) {
                    (Some(left), Some(right)) => diff_values(location, &path, left, right, found),
                    (left, right) => found.push(Difference::new(location(&path), left, right)),
                }
            }
            let order = |own: &Map<String, Value>, other: &Map<String, Value>| -> Vec<Value> {
                own.keys()
                    .filter(|key| other.contains_key(*key))
                    .map(|key| Value::String(key.clone()))
                    .collect()
            };
            let (python_order, rust_order) = (order(left, right), order(right, left));
            if python_order != rust_order {
                found.push(Difference::new(
                    location(&format!("{pointer} (key order)")),
                    Some(&Value::Array(python_order)),
                    Some(&Value::Array(rust_order)),
                ));
            }
        }
        (Value::Array(left), Value::Array(right)) if left.len() == right.len() => {
            for (index, (left, right)) in left.iter().zip(right).enumerate() {
                diff_values(location, &format!("{pointer}/{index}"), left, right, found);
            }
        }
        _ if same(python, rust) => {}
        _ => found.push(Difference::new(location(pointer), Some(python), Some(rust))),
    }
}

fn with_pointer(base: &str, pointer: &str) -> String {
    if pointer.is_empty() {
        base.to_owned()
    } else {
        format!("{base} {pointer}")
    }
}

fn arguments(args: &[String], normalizer: &Normalizer) -> BTreeMap<String, Vec<Value>> {
    let mut flags: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    let mut index = 0;
    while let Some(token) = args.get(index) {
        index += 1;
        let (key, value) = if let Some(flag) = token.strip_prefix("--") {
            match flag.split_once('=') {
                Some((name, value)) => (format!("--{name}="), Some(value.to_owned())),
                None => match args.get(index) {
                    Some(next) if !next.starts_with("--") => {
                        index += 1;
                        (token.clone(), Some(next.clone()))
                    }
                    _ => (token.clone(), None),
                },
            }
        } else {
            ("<positional>".to_owned(), Some(token.clone()))
        };
        let value = match value {
            None => Value::Bool(true),
            Some(text) => match serde_json::from_str::<Value>(&text) {
                Ok(parsed @ (Value::Object(_) | Value::Array(_))) => normalizer.value(&parsed),
                _ => Value::String(normalizer.string(&text)),
            },
        };
        flags.entry(key).or_default().push(value);
    }
    flags
}

fn label_frames(input: &[Input], normalizer: &Normalizer) -> Vec<(String, Value)> {
    let mut seen: BTreeMap<String, usize> = BTreeMap::new();
    input
        .iter()
        .map(|record| {
            let (base, value) = match record {
                Input::Frame(frame) => {
                    let frame = normalizer.value(frame);
                    let kind = frame.get("type").and_then(Value::as_str).unwrap_or("frame");
                    let base = match kind {
                        "control_request" => format!(
                            "control_request:{}",
                            frame["request"]["subtype"].as_str().unwrap_or("?")
                        ),
                        "control_response" => format!(
                            "control_response:{}",
                            frame["response"]["request_id"].as_str().unwrap_or("?")
                        ),
                        other => other.to_owned(),
                    };
                    (base, frame)
                }
                Input::Raw(text) => ("raw".to_owned(), Value::String(normalizer.string(text))),
                Input::Eof => ("eof".to_owned(), Value::Bool(true)),
            };
            let count = seen.entry(base.clone()).or_insert(0);
            *count += 1;
            let label = if *count == 1 {
                base
            } else {
                format!("{base}#{count}")
            };
            (label, value)
        })
        .collect()
}

fn grouped(labels: &[&str], unordered: &[Vec<String>]) -> Vec<String> {
    let mut sequence = Vec::new();
    let mut placed: BTreeSet<usize> = BTreeSet::new();
    for label in labels {
        match unordered
            .iter()
            .position(|group| group.iter().any(|member| member == label))
        {
            None => sequence.push((*label).to_owned()),
            Some(group) => {
                if placed.insert(group) {
                    let mut members: Vec<&str> = labels
                        .iter()
                        .copied()
                        .filter(|label| unordered[group].iter().any(|member| member == label))
                        .collect();
                    members.sort_unstable();
                    sequence.push(format!("{{{}}}", members.join(", ")));
                }
            }
        }
    }
    sequence
}

fn compare_process(
    prefix: &str,
    python: (&Process, &Normalizer),
    rust: (&Process, &Normalizer),
    unordered: &[Vec<String>],
    found: &mut Vec<Difference>,
) {
    let (python, python_normalizer) = python;
    let (rust, rust_normalizer) = rust;

    let python_args = arguments(&python.args, python_normalizer);
    let rust_args = arguments(&rust.args, rust_normalizer);
    let flags: BTreeSet<&String> = python_args.keys().chain(rust_args.keys()).collect();
    for flag in flags {
        let empty = Vec::new();
        let left = python_args.get(flag).unwrap_or(&empty);
        let right = rust_args.get(flag).unwrap_or(&empty);
        for occurrence in 0..left.len().max(right.len()) {
            let base = if occurrence == 0 {
                format!("{prefix}arg {flag}")
            } else {
                format!("{prefix}arg {flag}[{occurrence}]")
            };
            let location = |pointer: &str| with_pointer(&base, pointer);
            match (left.get(occurrence), right.get(occurrence)) {
                (Some(left), Some(right)) => diff_values(&location, "", left, right, found),
                (left, right) => found.push(Difference::new(base.clone(), left, right)),
            }
        }
    }

    let python_cwd = python_normalizer.value(&python.cwd);
    let rust_cwd = rust_normalizer.value(&rust.cwd);
    if !same(&python_cwd, &rust_cwd) {
        found.push(Difference::new(
            format!("{prefix}cwd"),
            Some(&python_cwd),
            Some(&rust_cwd),
        ));
    }

    let names: BTreeSet<&String> = python.env.keys().chain(rust.env.keys()).collect();
    for name in names {
        let left = python
            .env
            .get(name)
            .map(|value| python_normalizer.value(value));
        let right = rust.env.get(name).map(|value| rust_normalizer.value(value));
        let equal = match (&left, &right) {
            (Some(left), Some(right)) => same(left, right),
            _ => false,
        };
        if !equal {
            found.push(Difference::new(
                format!("{prefix}env {name}"),
                left.as_ref(),
                right.as_ref(),
            ));
        }
    }

    let python_frames = label_frames(&python.input, python_normalizer);
    let rust_frames = label_frames(&rust.input, rust_normalizer);
    let python_index: BTreeMap<&str, &Value> = python_frames
        .iter()
        .map(|(label, value)| (label.as_str(), value))
        .collect();
    let rust_index: BTreeMap<&str, &Value> = rust_frames
        .iter()
        .map(|(label, value)| (label.as_str(), value))
        .collect();
    let labels: BTreeSet<&str> = python_index
        .keys()
        .chain(rust_index.keys())
        .copied()
        .collect();
    for label in labels {
        let base = format!("{prefix}frame {label}");
        let location = |pointer: &str| with_pointer(&base, pointer);
        match (python_index.get(label), rust_index.get(label)) {
            (Some(left), Some(right)) => diff_values(&location, "", left, right, found),
            (left, right) => {
                found.push(Difference::new(base.clone(), left.copied(), right.copied()))
            }
        }
    }
    let shared = |frames: &[(String, Value)], other: &BTreeMap<&str, &Value>| -> Vec<String> {
        let labels: Vec<&str> = frames
            .iter()
            .map(|(label, _)| label.as_str())
            .filter(|label| other.contains_key(label))
            .collect();
        grouped(&labels, unordered)
    };
    let python_order = shared(&python_frames, &rust_index);
    let rust_order = shared(&rust_frames, &python_index);
    if python_order != rust_order {
        found.push(Difference::new(
            format!("{prefix}frames"),
            Some(&Value::from(python_order)),
            Some(&Value::from(rust_order)),
        ));
    }
}

/// Every difference between the recordings of the two sides, in a stable
/// order.
pub fn compare(
    python: (&Recording, &Normalizer),
    rust: (&Recording, &Normalizer),
    unordered: &[Vec<String>],
) -> Vec<Difference> {
    let mut found = Vec::new();
    let (python_recording, python_normalizer) = python;
    let (rust_recording, rust_normalizer) = rust;
    let python_count = python_recording.processes.len();
    let rust_count = rust_recording.processes.len();
    if python_count != rust_count {
        found.push(Difference::new(
            "processes".to_owned(),
            Some(&Value::from(python_count)),
            Some(&Value::from(rust_count)),
        ));
    }
    for (index, (left, right)) in python_recording
        .processes
        .iter()
        .zip(&rust_recording.processes)
        .enumerate()
    {
        let prefix = if index == 0 {
            String::new()
        } else {
            format!("process[{index}] ")
        };
        compare_process(
            &prefix,
            (left, python_normalizer),
            (right, rust_normalizer),
            unordered,
            &mut found,
        );
    }
    found
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::{Difference, Input, Normalizer, Process, Recording, compare, same};

    fn process(args: &[&str], env: Value, input: Vec<Input>) -> Process {
        Process {
            args: args.iter().map(|arg| (*arg).to_owned()).collect(),
            cwd: Value::Null,
            env: env.as_object().unwrap().clone(),
            input,
        }
    }

    fn recording(process: Process) -> Recording {
        Recording {
            processes: vec![process],
        }
    }

    fn sides() -> (Normalizer, Normalizer) {
        (
            Normalizer::new(&["/w/python".to_owned()], &["/bin/fake".to_owned()]),
            Normalizer::new(&["C:\\w\\rust".to_owned()], &["/bin/fake".to_owned()]),
        )
    }

    fn differences(
        python: &Recording,
        rust: &Recording,
        unordered: &[Vec<String>],
    ) -> Vec<Difference> {
        let (python_normalizer, rust_normalizer) = sides();
        compare(
            (python, &python_normalizer),
            (rust, &rust_normalizer),
            unordered,
        )
    }

    #[test]
    fn records_are_grouped_by_process_and_stdin_kinds_are_kept() {
        let text = [
            json!({"record": "spawn", "pid": 1, "args": ["--version"], "cwd": "/a", "env": {}}),
            json!({"record": "spawn", "pid": 2, "args": ["-p"], "cwd": "/a", "env": {"CLAUDE_X": "1"}}),
            json!({"record": "stdin", "pid": 2, "frame": {"type": "user"}}),
            json!({"record": "stdin_raw", "pid": 2, "text": "oops"}),
            json!({"record": "stdin_eof", "pid": 2}),
        ]
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n");
        let parsed = Recording::parse(&text).unwrap();
        assert_eq!(parsed.processes.len(), 2);
        assert!(parsed.processes[0].input.is_empty());
        assert_eq!(
            parsed.processes[1].input,
            [
                Input::Frame(json!({"type": "user"})),
                Input::Raw("oops".to_owned()),
                Input::Eof
            ]
        );
        assert_eq!(Recording::pids(&text), [1, 2]);
        assert!(
            Recording::parse("{\"record\": \"stdin\", \"pid\": 9}")
                .unwrap_err()
                .contains("no spawn")
        );
    }

    #[test]
    fn normalization_replaces_only_the_documented_values() {
        let (python, rust) = sides();
        assert_eq!(python.string("req_3_0a1b2c3d"), "req_3_<random>");
        assert_eq!(python.string("req_3_0a1b2c3"), "req_3_0a1b2c3");
        assert_eq!(python.string("req_x_0a1b2c3d"), "req_x_0a1b2c3d");
        assert_eq!(python.string("req_3_0A1B2C3D"), "req_3_0A1B2C3D");
        assert_eq!(python.string("/w/python/a/b"), "<work>/a/b");
        assert_eq!(rust.string("C:\\w\\rust\\a\\b"), "<work>/a/b");
        assert_eq!(rust.string("C:/w/rust/a"), "<work>/a");
        assert_eq!(python.string("/bin/fake"), "<cli>");
        assert_eq!(python.string("a\\b"), "a\\b");
        assert_eq!(python.string("hook_0"), "hook_0");
    }

    #[test]
    fn arguments_are_compared_by_flag_form_and_json_value() {
        let python = recording(process(
            &[
                "--verbose",
                "--resume=abc",
                "--mcp-config",
                r#"{"mcpServers": {"a": {"type": "stdio", "command": "x"}}}"#,
                "--add-dir",
                "/w/python/a",
                "--add-dir",
                "b",
                "--max-budget-usd",
                "2.0",
            ],
            json!({}),
            vec![],
        ));
        let rust = recording(process(
            &[
                "--resume",
                "abc",
                "--add-dir",
                "C:\\w\\rust\\a",
                "--max-budget-usd",
                "2",
                "--mcp-config",
                r#"{"mcpServers":{"a":{"command":"x"}}}"#,
                "--verbose",
            ],
            json!({}),
            vec![],
        ));
        let found = differences(&python, &rust, &[]);
        let shown: Vec<(String, String, String)> = found
            .iter()
            .map(|difference| {
                (
                    difference.location.clone(),
                    super::show(difference.python.as_ref()),
                    super::show(difference.rust.as_ref()),
                )
            })
            .collect();
        assert_eq!(
            shown,
            [
                (
                    "arg --add-dir[1]".to_owned(),
                    "\"b\"".to_owned(),
                    "<absent>".to_owned()
                ),
                (
                    "arg --max-budget-usd".to_owned(),
                    "\"2.0\"".to_owned(),
                    "\"2\"".to_owned()
                ),
                (
                    "arg --mcp-config /mcpServers/a/type".to_owned(),
                    "\"stdio\"".to_owned(),
                    "<absent>".to_owned()
                ),
                (
                    "arg --resume".to_owned(),
                    "<absent>".to_owned(),
                    "\"abc\"".to_owned()
                ),
                (
                    "arg --resume=".to_owned(),
                    "\"abc\"".to_owned(),
                    "<absent>".to_owned()
                ),
            ]
        );
    }

    #[test]
    fn key_order_of_shared_keys_is_compared_at_every_depth() {
        let python = recording(process(
            &[
                "--json-schema",
                r#"{"properties": {"reasoning": 1, "answer": 2}}"#,
            ],
            json!({}),
            vec![Input::Frame(
                json!({"type": "user", "a": 1, "b": {"x": 1, "y": 2}}),
            )],
        ));
        let rust = recording(process(
            &[
                "--json-schema",
                r#"{"properties":{"answer":2,"reasoning":1}}"#,
            ],
            json!({}),
            vec![Input::Frame(
                json!({"type": "user", "b": {"x": 1, "y": 2}, "a": 1, "extra": true}),
            )],
        ));
        let found: Vec<(String, String, String)> = differences(&python, &rust, &[])
            .iter()
            .map(|difference| {
                (
                    difference.location.clone(),
                    super::show(difference.python.as_ref()),
                    super::show(difference.rust.as_ref()),
                )
            })
            .collect();
        assert_eq!(
            found,
            [
                (
                    "arg --json-schema /properties (key order)".to_owned(),
                    r#"["reasoning","answer"]"#.to_owned(),
                    r#"["answer","reasoning"]"#.to_owned()
                ),
                (
                    "frame user /extra".to_owned(),
                    "<absent>".to_owned(),
                    "true".to_owned()
                ),
                (
                    "frame user  (key order)".to_owned(),
                    r#"["type","a","b"]"#.to_owned(),
                    r#"["type","b","a"]"#.to_owned()
                ),
            ]
        );
    }

    #[test]
    fn a_stdin_record_is_read_from_its_text_to_keep_the_key_order() {
        let text = [
            json!({"record": "spawn", "pid": 1, "args": [], "cwd": "/a", "env": {}}),
            json!({"record": "stdin", "pid": 1, "frame": {"a": 1, "b": 2}, "text": "{\"b\":2,\"a\":1}"}),
        ]
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n");
        let recording = Recording::parse(&text).unwrap();
        let Input::Frame(frame) = &recording.processes[0].input[0] else {
            panic!("expected a frame");
        };
        let keys: Vec<&String> = frame.as_object().unwrap().keys().collect();
        assert_eq!(keys, ["b", "a"]);
    }

    #[test]
    fn environment_and_working_directory_are_compared_after_normalization() {
        let mut python_process = process(
            &[],
            json!({"A": "1", "PWD": "/w/python", "ONLY_PY": "x"}),
            vec![],
        );
        python_process.cwd = json!("/w/python");
        let python = recording(python_process);
        let mut rust_process = process(&[], json!({"A": "2", "PWD": "C:\\w\\rust"}), vec![]);
        rust_process.cwd = json!("C:\\w\\rust");
        let found = differences(&python, &recording(rust_process), &[]);
        let locations: Vec<&str> = found
            .iter()
            .map(|difference| difference.location.as_str())
            .collect();
        assert_eq!(locations, ["env A", "env ONLY_PY"]);
    }

    #[test]
    fn frames_are_paired_by_label_and_their_order_is_compared() {
        let initialize = |id: &str, extra: Value| {
            let mut request = json!({"subtype": "initialize", "hooks": null});
            request
                .as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            Input::Frame(json!({"type": "control_request", "request_id": id, "request": request}))
        };
        let user =
            Input::Frame(json!({"type": "user", "message": {"role": "user", "content": "hi"}}));
        let python = recording(process(
            &[],
            json!({}),
            vec![
                initialize("req_1_aaaaaaaa", json!({})),
                user.clone(),
                Input::Raw("x".to_owned()),
                Input::Eof,
            ],
        ));
        let rust = recording(process(
            &[],
            json!({}),
            vec![
                user,
                initialize("req_1_bbbbbbbb", json!({"agents": {}})),
                Input::Eof,
            ],
        ));
        let found = differences(&python, &rust, &[]);
        let locations: Vec<&str> = found
            .iter()
            .map(|difference| difference.location.as_str())
            .collect();
        assert_eq!(
            locations,
            [
                "frame control_request:initialize /request/agents",
                "frame raw",
                "frames"
            ]
        );
        assert_eq!(
            found[2].python,
            Some(json!(["control_request:initialize", "user", "eof"]))
        );
        assert_eq!(
            found[2].rust,
            Some(json!(["user", "control_request:initialize", "eof"]))
        );

        let unordered = vec![vec![
            "user".to_owned(),
            "control_request:initialize".to_owned(),
        ]];
        let found = differences(&python, &rust, &unordered);
        assert!(
            found
                .iter()
                .all(|difference| difference.location != "frames")
        );
    }

    #[test]
    fn repeated_labels_are_numbered_and_extra_processes_are_reported() {
        let user = |text: &str| Input::Frame(json!({"type": "user", "message": text}));
        let python = Recording {
            processes: vec![
                process(&[], json!({}), vec![user("a"), user("b")]),
                process(&["--x"], json!({}), vec![]),
            ],
        };
        let rust = recording(process(&[], json!({}), vec![user("a"), user("c")]));
        let found = differences(&python, &rust, &[]);
        let locations: Vec<&str> = found
            .iter()
            .map(|difference| difference.location.as_str())
            .collect();
        assert_eq!(locations, ["processes", "frame user#2 /message"]);
    }

    #[test]
    fn numbers_compare_by_value_and_key_order_never_matters() {
        assert!(same(
            &json!({"a": 5, "b": [1.0]}),
            &json!({"b": [1], "a": 5.0})
        ));
        assert!(!same(&json!({"a": 5}), &json!({"a": 6})));
        assert!(!same(&json!({"a": 5}), &json!({"a": 5, "b": null})));
        assert!(!same(&json!("5"), &json!(5)));
    }
}
