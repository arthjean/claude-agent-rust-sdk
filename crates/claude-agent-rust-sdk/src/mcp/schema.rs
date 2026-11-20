//! Input schemas of typed tools: the schema derived from a Rust type, put
//! in the shape the CLI and the Claude API accept, and the adjustment of the
//! arguments of a call to what `serde` reads.
//!
//! The shape differs from the schemas of the Python SDK in deliberate ways:
//!
//! - The schema is the one `schemars` derives, so an `Option` field is typed
//!   `["integer", "null"]`, and `format`, bounds, and the `title` of the
//!   type are kept, where the Python SDK writes `{"type": "integer"}`. They
//!   tell the model the range a Rust integer holds.
//! - The alternatives of an enum, or of a field flattened from one, are
//!   merged into the root object: the Claude API refuses a tool whose
//!   schema has `oneOf`, `anyOf`, or `allOf` at the top level, and that
//!   refusal fails every request of the session. The merged schema is
//!   looser than the enum, so `serde` checks the exact shape when the
//!   arguments are read.

use schemars::JsonSchema;
use schemars::generate::SchemaSettings;
use serde_json::{Map, Number, Value, json};

const COMBINATORS: [&str; 3] = ["allOf", "anyOf", "oneOf"];

/// How deep the arguments of a call are walked to turn integral floats into
/// integers. Deeper values are read as they came.
const MAX_DEPTH: usize = 64;

/// The input schema of a tool whose arguments are read into `Args`.
///
/// The root has the type `object`, with the alternatives of an enum merged
/// into it. A type that describes no object (`serde_json::Value`, a scalar,
/// a sequence, an enum of unit variants) is listed with the schema of an
/// object that states no property.
pub(super) fn input_schema_of<Args: JsonSchema>() -> Value {
    let settings = SchemaSettings::draft2020_12()
        .for_deserialize()
        .with(|settings| {
            settings.meta_schema = None;
            settings.inline_subschemas = true;
        });
    let schema = settings
        .into_generator()
        .into_root_schema_for::<Args>()
        .to_value();
    let Value::Object(mut root) = schema else {
        return empty_object();
    };
    let merged = merge_alternatives(&mut root);
    if !merged && !describes_an_object(&root) {
        return empty_object();
    }
    let mut typed = Map::with_capacity(root.len() + 1);
    typed.insert("type".to_owned(), json!("object"));
    typed.extend(root.into_iter().filter(|(key, _)| key != "type"));
    Value::Object(typed)
}

fn empty_object() -> Value {
    json!({"type": "object", "properties": {}})
}

/// Whether a schema describes a JSON object: its type is `object`, alone or
/// among others, or it states properties.
fn describes_an_object(schema: &Map<String, Value>) -> bool {
    let typed_as_an_object = match schema.get("type") {
        Some(Value::String(kind)) => kind == "object",
        Some(Value::Array(kinds)) => kinds.iter().any(|kind| kind == "object"),
        _ => false,
    };
    typed_as_an_object || schema.get("properties").is_some_and(Value::is_object)
}

/// Moves the `allOf`, `anyOf`, and `oneOf` of the root into its own
/// `properties` and `required`, and returns whether one of them held an
/// object.
///
/// Every property an object alternative states becomes a property of the
/// root; a property stated differently by several alternatives takes them
/// all (`anyOf`, or one `enum` for the tags of a tagged enum). A property
/// is required when the root requires it, when an `allOf` member does, or
/// when every object alternative of an `anyOf` or `oneOf` does.
/// Alternatives that are not objects are left out: the arguments of a tool
/// are always an object.
fn merge_alternatives(root: &mut Map<String, Value>) -> bool {
    let definitions = root.get("$defs").cloned().unwrap_or(Value::Null);
    let mut properties: Vec<(String, Vec<Value>)> = Vec::new();
    let mut required: Vec<String> = strings(root.get("required"));
    let mut found_an_object = false;
    if let Some(Value::Object(own)) = root.get("properties") {
        for (name, schema) in own {
            add_property(&mut properties, name, schema);
        }
    }
    for combinator in COMBINATORS {
        let Some(Value::Array(alternatives)) = root.shift_remove(combinator) else {
            continue;
        };
        let objects: Vec<Map<String, Value>> = alternatives
            .iter()
            .filter_map(|alternative| resolve(alternative, &definitions))
            .filter(describes_an_object)
            .collect();
        if objects.is_empty() {
            continue;
        }
        found_an_object = true;
        for object in &objects {
            if let Some(Value::Object(stated)) = object.get("properties") {
                for (name, schema) in stated {
                    add_property(&mut properties, name, schema);
                }
            }
        }
        let mut lists = objects.iter().map(|object| strings(object.get("required")));
        let shared = if combinator == "allOf" {
            lists.flatten().collect()
        } else {
            let first = lists.next().unwrap_or_default();
            let rest: Vec<Vec<String>> = lists.collect();
            first
                .into_iter()
                .filter(|name| rest.iter().all(|list| list.contains(name)))
                .collect::<Vec<_>>()
        };
        for name in shared {
            if !required.contains(&name) {
                required.push(name);
            }
        }
    }
    if !found_an_object {
        return false;
    }
    let merged: Map<String, Value> = properties
        .into_iter()
        .map(|(name, schemas)| (name, union_of(schemas)))
        .collect();
    root.insert("properties".to_owned(), Value::Object(merged));
    if required.is_empty() {
        root.shift_remove("required");
    } else {
        root.insert("required".to_owned(), json!(required));
    }
    true
}

/// An alternative as an object schema, following a local `$ref` into the
/// `$defs` of the root.
fn resolve(alternative: &Value, definitions: &Value) -> Option<Map<String, Value>> {
    let target = match alternative.get("$ref").and_then(Value::as_str) {
        Some(reference) => reference
            .strip_prefix("#/$defs/")
            .and_then(|name| definitions.get(name))?,
        None => alternative,
    };
    target.as_object().cloned()
}

fn strings(list: Option<&Value>) -> Vec<String> {
    list.and_then(Value::as_array)
        .map(|names| {
            names
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

fn add_property(properties: &mut Vec<(String, Vec<Value>)>, name: &str, schema: &Value) {
    match properties.iter_mut().find(|(known, _)| known == name) {
        Some((_, schemas)) => {
            if !schemas.contains(schema) {
                schemas.push(schema.clone());
            }
        }
        None => properties.push((name.to_owned(), vec![schema.clone()])),
    }
}

/// One schema for a property the alternatives state in several ways: the
/// tags of a tagged enum (`{"type": "string", "const": "A"}`, ...) become
/// one `enum`, anything else an `anyOf`.
fn union_of(mut schemas: Vec<Value>) -> Value {
    if schemas.len() == 1 {
        return schemas.swap_remove(0);
    }
    let tag_keys = ["type", "const", "description"];
    let all_tags = schemas.iter().all(|schema| {
        schema.as_object().is_some_and(|fields| {
            fields.contains_key("const") && fields.keys().all(|key| tag_keys.contains(&&**key))
        })
    });
    let first_type = schemas[0].get("type").cloned();
    let one_type = schemas
        .iter()
        .all(|schema| schema.get("type").cloned() == first_type);
    if all_tags && one_type {
        let values: Vec<Value> = schemas
            .iter()
            .filter_map(|schema| schema.get("const").cloned())
            .collect();
        let mut tag = Map::new();
        if let Some(kind) = first_type {
            tag.insert("type".to_owned(), kind);
        }
        tag.insert("enum".to_owned(), Value::Array(values));
        return Value::Object(tag);
    }
    json!({"anyOf": schemas})
}

/// Turns each float without a fractional part into an integer where the
/// schema types the value as an integer and not as a number.
///
/// JSON Schema counts `3.0` as an integer and the Python SDK hands it to
/// the tool, while `serde` refuses a float for an integer field. The schema
/// is the generated one, so its `$ref`s point into its own `$defs`.
pub(super) fn integral_floats_to_integers(schema: &Value, value: &mut Value) {
    coerce(schema, schema, value, 0);
}

fn coerce(schema: &Value, root: &Value, value: &mut Value, depth: usize) {
    if depth > MAX_DEPTH {
        return;
    }
    let Value::Object(fields) = schema else {
        return;
    };
    if let Some(target) = fields
        .get("$ref")
        .and_then(Value::as_str)
        .and_then(|reference| reference.strip_prefix('#'))
        .and_then(|pointer| root.pointer(pointer))
    {
        coerce(target, root, value, depth + 1);
    }
    for combinator in COMBINATORS {
        if let Some(Value::Array(alternatives)) = fields.get(combinator) {
            for alternative in alternatives {
                coerce(alternative, root, value, depth + 1);
            }
        }
    }
    match value {
        Value::Number(number) => {
            if integer_only(fields)
                && let Some(integer) = integral(number)
            {
                *value = Value::Number(integer);
            }
        }
        Value::Object(members) => {
            let properties = fields.get("properties");
            let others = fields.get("additionalProperties");
            for (name, member) in members.iter_mut() {
                if let Some(subschema) = properties.and_then(|stated| stated.get(name)).or(others) {
                    coerce(subschema, root, member, depth + 1);
                }
            }
        }
        Value::Array(items) => {
            let prefix = fields.get("prefixItems").and_then(Value::as_array);
            let rest = fields.get("items");
            for (index, item) in items.iter_mut().enumerate() {
                if let Some(subschema) = prefix.and_then(|prefix| prefix.get(index)).or(rest) {
                    coerce(subschema, root, item, depth + 1);
                }
            }
        }
        _ => {}
    }
}

fn integer_only(schema: &Map<String, Value>) -> bool {
    match schema.get("type") {
        Some(Value::String(kind)) => kind == "integer",
        Some(Value::Array(kinds)) => {
            kinds.iter().any(|kind| kind == "integer") && !kinds.iter().any(|kind| kind == "number")
        }
        _ => false,
    }
}

/// The integer a float without a fractional part stands for, when an `i64`
/// or a `u64` holds it.
fn integral(number: &Number) -> Option<Number> {
    if !number.is_f64() {
        return None;
    }
    let float = number.as_f64()?;
    if float.fract() != 0.0 {
        return None;
    }
    const TWO_TO_THE_63: f64 = 9_223_372_036_854_775_808.0;
    if (-TWO_TO_THE_63..TWO_TO_THE_63).contains(&float) {
        return Some(Number::from(float as i64));
    }
    if (0.0..2.0 * TWO_TO_THE_63).contains(&float) {
        return Some(Number::from(float as u64));
    }
    None
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::integral_floats_to_integers;

    #[test]
    fn integral_floats_become_integers_only_where_the_schema_wants_an_integer() {
        let schema = json!({
            "type": "object",
            "properties": {
                "count": {"type": "integer"},
                "maybe": {"type": ["integer", "null"]},
                "ratio": {"type": "number"},
                "either": {"type": ["integer", "number"]},
                "list": {"type": "array", "items": {"type": "integer"}},
                "pair": {"type": "array", "prefixItems": [{"type": "integer"}, {"type": "number"}]},
                "nested": {"anyOf": [{"$ref": "#/$defs/Inner"}, {"type": "null"}]},
                "map": {"type": "object", "additionalProperties": {"type": "integer"}},
            },
            "$defs": {"Inner": {"type": "object", "properties": {"n": {"type": "integer"}}}},
        });
        let mut arguments = json!({
            "count": 3.0,
            "maybe": 4.0,
            "ratio": 5.0,
            "either": 6.0,
            "list": [1.0, 2.5],
            "pair": [7.0, 8.0],
            "nested": {"n": 9.0},
            "map": {"a": 10.0},
            "unknown": 11.0,
        });
        integral_floats_to_integers(&schema, &mut arguments);
        assert_eq!(
            arguments.to_string(),
            r#"{"count":3,"maybe":4,"ratio":5.0,"either":6.0,"list":[1,2.5],"pair":[7,8.0],"nested":{"n":9},"map":{"a":10},"unknown":11.0}"#
        );
    }

    #[test]
    fn a_float_too_large_for_any_integer_is_left_as_it_came() {
        let schema = json!({"type": "integer"});
        let mut large = json!(1e300);
        integral_floats_to_integers(&schema, &mut large);
        assert_eq!(large, json!(1e300));
        let mut unsigned = json!(1.5e19);
        integral_floats_to_integers(&schema, &mut unsigned);
        assert_eq!(unsigned, json!(15_000_000_000_000_000_000_u64));
        let mut negative = json!(-2.0);
        integral_floats_to_integers(&schema, &mut negative);
        assert_eq!(negative, json!(-2));
    }
}
