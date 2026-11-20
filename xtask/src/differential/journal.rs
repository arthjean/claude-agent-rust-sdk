//! Comparison of what the application observed through each SDK.
//!
//! Each driver writes a journal when `PARITY_JOURNAL` names a file: one
//! JSON object per line, an event whose `event` key names its kind. The
//! format is documented in `parity/README.md`:
//!
//! - `message`: a message the application received, in `message`;
//! - `call`: a client method returned, with `step` (its index, `null` for
//!   the one-shot query), `method`, and `result` or `error`;
//! - `can_use_tool`, `hook`, `tool`: a callback ran, with its arguments;
//! - `stderr`: the stderr callback received `line`;
//! - `store_append`: the SDK appended `entries` under `key` to the session
//!   store.
//!
//! Events are grouped by kind, and kinds are not ordered against each
//! other, because a callback runs concurrently with the reading of
//! messages. Messages are paired the way stdin frames are: each gets a
//! label, its `$type`, then `:<subtype>` when it has one, then `#2`, `#3`
//! for a label seen again; messages are paired by label and compared, and
//! the order of the labels both sides share is compared too. So a message
//! one SDK drops and the other keeps is one difference, not a shift of
//! every message after it. Events of the other kinds are compared in the
//! order they were written, the first of one side with the first of the
//! other.
//!
//! Before comparison, each event is normalized by [`normalize`], whose
//! rules `parity/README.md` lists with their reasons. A difference has the
//! location `journal message <label>` or `journal <kind>[<index>]`,
//! followed by a JSON pointer into the event, or `journal messages` for
//! the order of the messages.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value};

use super::compare::{Difference, Normalizer, diff_values, with_pointer};

/// Keys of the init frame that only `fake-claude` writes: the arguments
/// and variables it received, which the recording compares, and its
/// process id, which changes with every run.
const FAKE_INIT_KEYS: [&str; 4] = ["fake_args", "fake_env", "fake_pid", "fake_console_window"];

/// The events one driver wrote.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Journal {
    /// Events, in the order they were written.
    pub events: Vec<Value>,
}

impl Journal {
    /// Parses a journal file.
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut events = Vec::new();
        for (index, line) in text.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let event: Value = serde_json::from_str(line)
                .map_err(|error| format!("journal line {}: {error}", index + 1))?;
            if event.get("event").and_then(Value::as_str).is_none() {
                return Err(format!(
                    "journal line {}: an event needs an `event` string",
                    index + 1
                ));
            }
            events.push(event);
        }
        Ok(Self { events })
    }

    /// The events grouped by kind, each without its `event` key and
    /// normalized.
    fn grouped(&self, normalizer: &Normalizer) -> BTreeMap<String, Vec<Value>> {
        let mut kinds: BTreeMap<String, Vec<Value>> = BTreeMap::new();
        for event in &self.events {
            let mut event = event.clone();
            let kind = event
                .as_object_mut()
                .and_then(|fields| fields.shift_remove("event"))
                .and_then(|kind| kind.as_str().map(str::to_owned))
                .unwrap_or_default();
            kinds
                .entry(kind.clone())
                .or_default()
                .push(normalize(&kind, &event, normalizer));
        }
        kinds
    }
}

fn without_nulls_sorted(value: &Value) -> Value {
    match value {
        Value::Object(fields) => {
            let mut sorted: Vec<(&String, &Value)> = fields
                .iter()
                .filter(|(_, value)| !value.is_null())
                .collect();
            sorted.sort_by_key(|(key, _)| *key);
            sorted
                .into_iter()
                .map(|(key, value)| (key.clone(), without_nulls_sorted(value)))
                .collect::<Map<String, Value>>()
                .into()
        }
        Value::Array(items) => items.iter().map(without_nulls_sorted).collect(),
        other => other.clone(),
    }
}

/// Normalizes one event of kind `kind`:
///
/// 1. every string goes through the recording's [`Normalizer`];
/// 2. in a `call` event, the text of an error is removed: the kind and the
///    exit code are compared, the wording is not;
/// 3. in a `message` event holding the `init` system message, the keys of
///    [`FAKE_INIT_KEYS`] are removed from its `data`;
/// 4. a key whose value is `null` is removed, at every depth, since Python
///    gives every field of a dataclass a value and Rust leaves an absent
///    `Option` out (in an array, a `null` stays);
/// 5. keys are sorted, so key order is not compared: the Rust driver builds
///    each object from typed fields, in an order of its own.
pub fn normalize(kind: &str, event: &Value, normalizer: &Normalizer) -> Value {
    let mut event = normalizer.value(event);
    if kind == "call"
        && let Some(error) = event.get_mut("error").and_then(Value::as_object_mut)
    {
        error.shift_remove("message");
    }
    if kind == "message"
        && let Some(message) = event.get_mut("message")
        && message.get("$type").and_then(Value::as_str) == Some("SystemMessage")
        && message.get("subtype").and_then(Value::as_str) == Some("init")
        && let Some(data) = message.get_mut("data").and_then(Value::as_object_mut)
    {
        for key in FAKE_INIT_KEYS {
            data.shift_remove(key);
        }
    }
    without_nulls_sorted(&event)
}

/// The label of a normalized `message` event, before numbering.
fn message_label(event: &Value) -> String {
    let message = &event["message"];
    let kind = message
        .get("$type")
        .and_then(Value::as_str)
        .unwrap_or("message");
    match message.get("subtype").and_then(Value::as_str) {
        Some(subtype) => format!("{kind}:{subtype}"),
        None => kind.to_owned(),
    }
}

fn labeled(events: &[Value]) -> Vec<(String, &Value)> {
    let mut seen: BTreeMap<String, usize> = BTreeMap::new();
    events
        .iter()
        .map(|event| {
            let base = message_label(event);
            let count = seen.entry(base.clone()).or_insert(0);
            *count += 1;
            let label = if *count == 1 {
                base
            } else {
                format!("{base}#{count}")
            };
            (label, event)
        })
        .collect()
}

fn compare_messages(python: &[Value], rust: &[Value], found: &mut Vec<Difference>) {
    let python = labeled(python);
    let rust = labeled(rust);
    let python_index: BTreeMap<&str, &Value> = python
        .iter()
        .map(|(label, event)| (label.as_str(), *event))
        .collect();
    let rust_index: BTreeMap<&str, &Value> = rust
        .iter()
        .map(|(label, event)| (label.as_str(), *event))
        .collect();
    let labels: BTreeSet<&str> = python_index
        .keys()
        .chain(rust_index.keys())
        .copied()
        .collect();
    for label in labels {
        let base = format!("journal message {label}");
        match (python_index.get(label), rust_index.get(label)) {
            (Some(left), Some(right)) => {
                let location = |pointer: &str| with_pointer(&base, pointer);
                diff_values(&location, "", left, right, found);
            }
            (left, right) => found.push(Difference {
                location: base,
                python: left.map(|value| (*value).clone()),
                rust: right.map(|value| (*value).clone()),
            }),
        }
    }
    let order = |own: &[(String, &Value)], other: &BTreeMap<&str, &Value>| -> Value {
        own.iter()
            .filter(|(label, _)| other.contains_key(label.as_str()))
            .map(|(label, _)| Value::String(label.clone()))
            .collect()
    };
    let python_order = order(&python, &rust_index);
    let rust_order = order(&rust, &python_index);
    if python_order != rust_order {
        found.push(Difference {
            location: "journal messages".to_owned(),
            python: Some(python_order),
            rust: Some(rust_order),
        });
    }
}

/// Every difference between the journals of the two sides, by kind.
pub fn compare(python: (&Journal, &Normalizer), rust: (&Journal, &Normalizer)) -> Vec<Difference> {
    let python_kinds = python.0.grouped(python.1);
    let rust_kinds = rust.0.grouped(rust.1);
    let kinds: BTreeSet<&String> = python_kinds.keys().chain(rust_kinds.keys()).collect();
    let empty = Vec::new();
    let mut found = Vec::new();
    for kind in kinds {
        let left = python_kinds.get(kind).unwrap_or(&empty);
        let right = rust_kinds.get(kind).unwrap_or(&empty);
        if kind == "message" {
            compare_messages(left, right, &mut found);
            continue;
        }
        for index in 0..left.len().max(right.len()) {
            let base = format!("journal {kind}[{index}]");
            match (left.get(index), right.get(index)) {
                (Some(left), Some(right)) => {
                    let location = |pointer: &str| with_pointer(&base, pointer);
                    diff_values(&location, "", left, right, &mut found);
                }
                (left, right) => found.push(Difference {
                    location: base,
                    python: left.cloned(),
                    rust: right.cloned(),
                }),
            }
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::{Journal, compare};
    use crate::differential::compare::Normalizer;

    fn journal(events: &[Value]) -> Journal {
        let text: Vec<String> = events.iter().map(Value::to_string).collect();
        Journal::parse(&text.join("\n")).unwrap()
    }

    fn locations(python: &Journal, rust: &Journal) -> Vec<(String, String, String)> {
        let python_normalizer = Normalizer::new(&["/w/python".to_owned()], &[]);
        let rust_normalizer = Normalizer::new(&["/w/rust".to_owned()], &[]);
        compare((python, &python_normalizer), (rust, &rust_normalizer))
            .into_iter()
            .map(|difference| {
                (
                    difference.location,
                    crate::differential::compare::show(difference.python.as_ref()),
                    crate::differential::compare::show(difference.rust.as_ref()),
                )
            })
            .collect()
    }

    #[test]
    fn messages_are_paired_by_label_other_events_by_index_and_kinds_are_not_ordered() {
        let python = journal(&[
            json!({"event": "message", "message": {"$type": "AssistantMessage", "model": "m"}}),
            json!({"event": "can_use_tool", "tool_name": "Bash", "input": {}}),
            json!({"event": "message", "message": {"$type": "SystemMessage", "subtype": "init"}}),
            json!({"event": "message", "message": {"$type": "ResultMessage", "subtype": "success", "result": "a"}}),
            json!({"event": "tool", "tool": "add"}),
        ]);
        let rust = journal(&[
            json!({"event": "can_use_tool", "tool_name": "Bash", "input": {}}),
            json!({"event": "message", "message": {"$type": "SystemMessage", "subtype": "init"}}),
            json!({"event": "message", "message": {"$type": "AssistantMessage", "model": "m"}}),
            json!({"event": "message", "message": {"$type": "Unknown"}}),
            json!({"event": "message", "message": {"$type": "ResultMessage", "subtype": "success", "result": "b"}}),
            json!({"event": "tool", "tool": "add"}),
            json!({"event": "tool", "tool": "sub"}),
        ]);
        assert_eq!(
            locations(&python, &rust),
            [
                (
                    "journal message ResultMessage:success /message/result".to_owned(),
                    "\"a\"".to_owned(),
                    "\"b\"".to_owned()
                ),
                (
                    "journal message Unknown".to_owned(),
                    "<absent>".to_owned(),
                    r#"{"message":{"$type":"Unknown"}}"#.to_owned()
                ),
                (
                    "journal messages".to_owned(),
                    r#"["AssistantMessage","SystemMessage:init","ResultMessage:success"]"#
                        .to_owned(),
                    r#"["SystemMessage:init","AssistantMessage","ResultMessage:success"]"#
                        .to_owned()
                ),
                (
                    "journal tool[1]".to_owned(),
                    "<absent>".to_owned(),
                    r#"{"tool":"sub"}"#.to_owned()
                ),
            ]
        );
    }

    #[test]
    fn normalization_drops_nulls_error_text_and_fake_init_keys_and_ignores_key_order() {
        let python = journal(&[
            json!({"event": "message", "message": {
                "$type": "SystemMessage", "subtype": "init",
                "data": {"cwd": "/w/python/a", "fake_pid": 1, "fake_args": ["--x"], "model": "m"},
            }}),
            json!({"event": "call", "step": 0, "method": "interrupt",
                   "error": {"kind": "control", "exit_code": null, "message": "refused"}}),
            json!({"event": "can_use_tool", "input": {"b": 1, "a": [null]},
                   "context": {"signal": null, "title": null}}),
        ]);
        let rust = journal(&[
            json!({"event": "message", "message": {
                "$type": "SystemMessage", "subtype": "init",
                "data": {"model": "m", "cwd": "/w/rust/a", "fake_pid": 2},
            }}),
            json!({"event": "call", "step": 0, "method": "interrupt",
                   "error": {"kind": "control", "message": "Control request failed: refused"}}),
            json!({"event": "can_use_tool", "input": {"a": [null], "b": 1}, "context": {}}),
        ]);
        assert_eq!(locations(&python, &rust), []);

        let rust = journal(&[
            json!({"event": "message", "message": {
                "$type": "SystemMessage", "subtype": "init", "data": {"model": "m", "cwd": "/w/rust/a"},
            }}),
            json!({"event": "call", "step": 0, "method": "interrupt", "error": {"kind": "process"}}),
            json!({"event": "can_use_tool", "input": {"a": [], "b": 1}, "context": {}}),
        ]);
        assert_eq!(
            locations(&python, &rust),
            [
                (
                    "journal call[0] /error/kind".to_owned(),
                    "\"control\"".to_owned(),
                    "\"process\"".to_owned()
                ),
                (
                    "journal can_use_tool[0] /input/a".to_owned(),
                    "[null]".to_owned(),
                    "[]".to_owned()
                ),
            ]
        );
    }

    #[test]
    fn a_journal_line_without_an_event_is_rejected() {
        let error = Journal::parse("{\"message\": {}}").unwrap_err();
        assert!(error.contains("line 1"), "{error}");
        assert!(Journal::parse("not json").is_err());
    }
}
