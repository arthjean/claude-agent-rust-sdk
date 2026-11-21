//! Writing transcript lines.

use serde_json::{Map, Value};

/// Serializes an entry on one line with its `type` field first, the other
/// keys in their order.
///
/// Metadata detection matches line prefixes such as `{"type":"tag"`, the
/// shape the CLI writes. A session store may hand entries back with their
/// keys reordered (a JSONB column sorts them), so the store path hoists
/// `type` the way upstream `_entries_to_jsonl` does. A copy of entries read
/// from a transcript keeps their order instead: see [`plain_line`].
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

/// Serializes fields on one line in the order given, the order the CLI
/// writes them in.
pub(crate) fn ordered_line(fields: &[(&str, Value)]) -> String {
    let body: Vec<String> = fields
        .iter()
        .map(|(key, value)| format!("{}:{value}", Value::from(*key)))
        .collect();
    format!("{{{}}}", body.join(","))
}

/// Serializes an entry on one line with its keys in their order, as
/// upstream `json.dumps` writes a forked or resumed transcript.
pub(crate) fn plain_line(entry: &Map<String, Value>) -> String {
    let mut line = String::new();
    for (index, (key, value)) in entry.iter().enumerate() {
        line.push(if index == 0 { '{' } else { ',' });
        line.push_str(&Value::from(key.as_str()).to_string());
        line.push(':');
        line.push_str(&value.to_string());
    }
    if line.is_empty() {
        line.push('{');
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
    fn ordered_fields_keep_the_order_they_are_given_in() {
        let line = ordered_line(&[
            ("type", json!("tag")),
            ("tag", json!("a\"b")),
            ("sessionId", json!("s")),
        ]);
        assert_eq!(line, r#"{"type":"tag","tag":"a\"b","sessionId":"s"}"#);
    }

    #[cfg(feature = "preserve-order")]
    #[test]
    fn plain_lines_keep_the_key_order_of_the_entry() {
        let entry = json!({"parentUuid": null, "type": "user", "uuid": "u", "cwd": "/w"});
        let Value::Object(entry) = entry else {
            unreachable!()
        };
        assert_eq!(
            plain_line(&entry),
            r#"{"parentUuid":null,"type":"user","uuid":"u","cwd":"/w"}"#
        );
        assert_eq!(plain_line(&Map::new()), "{}");
    }

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
