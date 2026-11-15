//! Writing transcript lines.

use serde_json::{Map, Value};

/// Serializes an entry on one line with its `type` field first.
///
/// Metadata detection matches line prefixes such as `{"type":"tag"`, the
/// shape the CLI writes. The maps of this crate do not keep insertion
/// order, so `type` is hoisted explicitly.
pub(crate) fn type_first_line(entry: &Map<String, Value>) -> String {
    let mut line = String::from("{");
    let mut push = |key: &str, value: &Value| {
        if line.len() > 1 {
            line.push(',');
        }
        line.push_str(&Value::from(key).to_string());
        line.push(':');
        line.push_str(&value.to_string());
    };
    if let Some(kind) = entry.get("type") {
        push("type", kind);
    }
    for (key, value) in entry {
        if key != "type" {
            push(key, value);
        }
    }
    line.push('}');
    line
}

/// Serializes any value on one line, objects with their `type` first.
pub(crate) fn value_line(value: &Value) -> String {
    match value {
        Value::Object(entry) => type_first_line(entry),
        other => other.to_string(),
    }
}

/// Serializes entries to JSONL, one line each and a final newline.
pub(crate) fn entries_to_jsonl<'a>(entries: impl IntoIterator<Item = &'a Value>) -> String {
    let mut text = String::new();
    for entry in entries {
        text.push_str(&value_line(entry));
        text.push('\n');
    }
    text
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn type_comes_first_and_the_rest_is_unchanged() {
        let entry = json!({"tag": "x", "sessionId": "s", "type": "tag"});
        let line = value_line(&entry);
        assert!(line.starts_with(r#"{"type":"tag","#), "{line}");
        assert_eq!(serde_json::from_str::<Value>(&line).unwrap(), entry);
        assert_eq!(value_line(&json!({"a": 1})), r#"{"a":1}"#);
        assert_eq!(
            entries_to_jsonl(&[json!(1), json!({"type": "t"})]),
            "1\n{\"type\":\"t\"}\n"
        );
    }
}
