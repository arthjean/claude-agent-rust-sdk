//! The shape of the results an MCP server sends, as the Python `mcp`
//! package 2.x writes them.
//!
//! `mcp` 2.x, which the Python SDK installs by default, passes the result
//! of every request MCP defines through a model of the MCP schema of the
//! negotiated version (`mcp_types.methods.serialize_server_result`) before
//! it sends it: the keys the schema defines come first, in the byte order
//! of their names (the order of the generated models), keys the schema
//! does not define are dropped, except in a JSON Schema object, which keeps
//! them after the defined ones, and `null` values of modeled fields are
//! left out. A map the schema leaves open (`_meta`, `properties`,
//! `structuredContent`, `experimental`) is kept as written.
//!
//! [`shape_result`] applies the same projection to the results of
//! `initialize`, `ping`, `tools/list`, and `tools/call`, the methods an
//! in-process server serves, so that the CLI receives the same bytes from
//! both SDKs. The schemas of the protocol versions the CLI negotiates
//! (2024-11-05 to 2025-11-25) define these objects alike.
//!
//! The one deliberate exception: the annotation keys a program sets with
//! [`ToolAnnotations::extra`](crate::ToolAnnotations::extra) are kept, after
//! the defined ones, where `mcp` 2.x drops them and `mcp` 1.x keeps them:
//! they are sent because the program asked for them.

use serde_json::{Map, Value};

/// What to do with the keys an object model does not define.
#[derive(Clone, Copy)]
enum Extra {
    /// Drop them, as a model with `extra="ignore"` does.
    Drop,
    /// Keep them after the defined keys, in their order, without `null`.
    Keep,
}

/// One object model: its fields, in the order they are written, each with
/// the shape of its value.
struct Model {
    fields: &'static [(&'static str, Shape)],
    extra: Extra,
}

/// The shape of a value.
#[derive(Clone, Copy)]
enum Shape {
    /// Kept as written: a scalar or an open map.
    Verbatim,
    /// An object of this model.
    Object(&'static Model),
    /// A list of values of this shape.
    List(&'static Shape),
    /// A content block, whose model depends on its `type`.
    Content,
    /// A number the model reads as a float.
    Float,
}

const IMPLEMENTATION: Model = Model {
    fields: &[
        ("description", Shape::Verbatim),
        ("icons", Shape::List(&Shape::Object(&ICON))),
        ("name", Shape::Verbatim),
        ("title", Shape::Verbatim),
        ("version", Shape::Verbatim),
        ("websiteUrl", Shape::Verbatim),
    ],
    extra: Extra::Drop,
};

const ICON: Model = Model {
    fields: &[
        ("mimeType", Shape::Verbatim),
        ("sizes", Shape::Verbatim),
        ("src", Shape::Verbatim),
        ("theme", Shape::Verbatim),
    ],
    extra: Extra::Drop,
};

const LIST_CHANGED: Model = Model {
    fields: &[("listChanged", Shape::Verbatim)],
    extra: Extra::Drop,
};

const RESOURCES_CAPABILITY: Model = Model {
    fields: &[
        ("listChanged", Shape::Verbatim),
        ("subscribe", Shape::Verbatim),
    ],
    extra: Extra::Drop,
};

const SERVER_CAPABILITIES: Model = Model {
    fields: &[
        ("completions", Shape::Verbatim),
        ("experimental", Shape::Verbatim),
        ("logging", Shape::Verbatim),
        ("prompts", Shape::Object(&LIST_CHANGED)),
        ("resources", Shape::Object(&RESOURCES_CAPABILITY)),
        ("tasks", Shape::Verbatim),
        ("tools", Shape::Object(&LIST_CHANGED)),
    ],
    extra: Extra::Drop,
};

const INITIALIZE_RESULT: Model = Model {
    fields: &[
        ("_meta", Shape::Verbatim),
        ("capabilities", Shape::Object(&SERVER_CAPABILITIES)),
        ("instructions", Shape::Verbatim),
        ("protocolVersion", Shape::Verbatim),
        ("serverInfo", Shape::Object(&IMPLEMENTATION)),
    ],
    extra: Extra::Drop,
};

const EMPTY_RESULT: Model = Model {
    fields: &[("_meta", Shape::Verbatim)],
    extra: Extra::Keep,
};

const JSON_SCHEMA: Model = Model {
    fields: &[
        ("$schema", Shape::Verbatim),
        ("properties", Shape::Verbatim),
        ("required", Shape::Verbatim),
        ("type", Shape::Verbatim),
    ],
    extra: Extra::Keep,
};

const TOOL_ANNOTATIONS: Model = Model {
    fields: &[
        ("destructiveHint", Shape::Verbatim),
        ("idempotentHint", Shape::Verbatim),
        ("openWorldHint", Shape::Verbatim),
        ("readOnlyHint", Shape::Verbatim),
        ("title", Shape::Verbatim),
    ],
    extra: Extra::Keep,
};

const TOOL_EXECUTION: Model = Model {
    fields: &[("taskSupport", Shape::Verbatim)],
    extra: Extra::Drop,
};

const TOOL: Model = Model {
    fields: &[
        ("_meta", Shape::Verbatim),
        ("annotations", Shape::Object(&TOOL_ANNOTATIONS)),
        ("description", Shape::Verbatim),
        ("execution", Shape::Object(&TOOL_EXECUTION)),
        ("icons", Shape::List(&Shape::Object(&ICON))),
        ("inputSchema", Shape::Object(&JSON_SCHEMA)),
        ("name", Shape::Verbatim),
        ("outputSchema", Shape::Object(&JSON_SCHEMA)),
        ("title", Shape::Verbatim),
    ],
    extra: Extra::Drop,
};

const LIST_TOOLS_RESULT: Model = Model {
    fields: &[
        ("_meta", Shape::Verbatim),
        ("nextCursor", Shape::Verbatim),
        ("tools", Shape::List(&Shape::Object(&TOOL))),
    ],
    extra: Extra::Drop,
};

const CALL_TOOL_RESULT: Model = Model {
    fields: &[
        ("_meta", Shape::Verbatim),
        ("content", Shape::List(&Shape::Content)),
        ("isError", Shape::Verbatim),
        ("structuredContent", Shape::Verbatim),
    ],
    extra: Extra::Drop,
};

const ANNOTATIONS: Model = Model {
    fields: &[
        ("audience", Shape::Verbatim),
        ("lastModified", Shape::Verbatim),
        ("priority", Shape::Float),
    ],
    extra: Extra::Drop,
};

const TEXT_CONTENT: Model = Model {
    fields: &[
        ("_meta", Shape::Verbatim),
        ("annotations", Shape::Object(&ANNOTATIONS)),
        ("text", Shape::Verbatim),
        ("type", Shape::Verbatim),
    ],
    extra: Extra::Drop,
};

const BINARY_CONTENT: Model = Model {
    fields: &[
        ("_meta", Shape::Verbatim),
        ("annotations", Shape::Object(&ANNOTATIONS)),
        ("data", Shape::Verbatim),
        ("mimeType", Shape::Verbatim),
        ("type", Shape::Verbatim),
    ],
    extra: Extra::Drop,
};

const RESOURCE_LINK: Model = Model {
    fields: &[
        ("_meta", Shape::Verbatim),
        ("annotations", Shape::Object(&ANNOTATIONS)),
        ("description", Shape::Verbatim),
        ("icons", Shape::List(&Shape::Object(&ICON))),
        ("mimeType", Shape::Verbatim),
        ("name", Shape::Verbatim),
        ("size", Shape::Verbatim),
        ("title", Shape::Verbatim),
        ("type", Shape::Verbatim),
        ("uri", Shape::Verbatim),
    ],
    extra: Extra::Drop,
};

const RESOURCE_CONTENTS: Model = Model {
    fields: &[
        ("_meta", Shape::Verbatim),
        ("blob", Shape::Verbatim),
        ("mimeType", Shape::Verbatim),
        ("text", Shape::Verbatim),
        ("uri", Shape::Verbatim),
    ],
    extra: Extra::Drop,
};

const EMBEDDED_RESOURCE: Model = Model {
    fields: &[
        ("_meta", Shape::Verbatim),
        ("annotations", Shape::Object(&ANNOTATIONS)),
        ("resource", Shape::Object(&RESOURCE_CONTENTS)),
        ("type", Shape::Verbatim),
    ],
    extra: Extra::Drop,
};

/// The result of `method` as `mcp` 2.x sends it. A method outside the four
/// an in-process server serves is returned unchanged.
pub(crate) fn shape_result(method: &str, result: Value) -> Value {
    let model = match method {
        "initialize" => &INITIALIZE_RESULT,
        "ping" => &EMPTY_RESULT,
        "tools/list" => &LIST_TOOLS_RESULT,
        "tools/call" => &CALL_TOOL_RESULT,
        _ => return result,
    };
    shape(Shape::Object(model), result)
}

fn shape(shape_of: Shape, value: Value) -> Value {
    match (shape_of, value) {
        (Shape::Object(model), Value::Object(object)) => Value::Object(object_of(model, object)),
        (Shape::List(item), Value::Array(items)) => {
            Value::Array(items.into_iter().map(|value| shape(*item, value)).collect())
        }
        (Shape::Content, Value::Object(object)) => {
            let model = match object.get("type").and_then(Value::as_str) {
                Some("text") => &TEXT_CONTENT,
                Some("image" | "audio") => &BINARY_CONTENT,
                Some("resource_link") => &RESOURCE_LINK,
                Some("resource") => &EMBEDDED_RESOURCE,
                _ => return Value::Object(object),
            };
            Value::Object(object_of(model, object))
        }
        (Shape::Float, Value::Number(number)) if !number.is_f64() => number
            .as_f64()
            .and_then(serde_json::Number::from_f64)
            .map_or(Value::Number(number), Value::Number),
        (_, value) => value,
    }
}

fn object_of(model: &Model, mut object: Map<String, Value>) -> Map<String, Value> {
    let mut shaped = Map::new();
    for (key, shape_of) in model.fields {
        if let Some(value) = object.shift_remove(*key)
            && !value.is_null()
        {
            shaped.insert((*key).to_owned(), shape(*shape_of, value));
        }
    }
    if let Extra::Keep = model.extra {
        for (key, value) in object {
            if !value.is_null() {
                shaped.insert(key, value);
            }
        }
    }
    shaped
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::shape_result;

    fn keys(value: &serde_json::Value) -> Vec<&str> {
        value
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect()
    }

    #[test]
    fn a_tool_list_is_written_in_the_order_and_with_the_keys_of_the_schema() {
        let listed = shape_result(
            "tools/list",
            json!({"tools": [{
                "name": "a",
                "description": "d",
                "title": null,
                "unmodeled": 1,
                "inputSchema": {"type": "object", "x": null, "aaa": 1, "properties": {"b": {}, "a": {}}, "$defs": {}},
                "annotations": {"title": "T", "readOnlyHint": true, "custom": 2},
                "_meta": {"anthropic/maxResultSizeChars": 5, "n": null},
            }]}),
        );
        let tool = &listed["tools"][0];
        assert_eq!(
            keys(tool),
            ["_meta", "annotations", "description", "inputSchema", "name"]
        );
        assert_eq!(
            keys(&tool["inputSchema"]),
            ["properties", "type", "aaa", "$defs"]
        );
        assert_eq!(keys(&tool["inputSchema"]["properties"]), ["b", "a"]);
        assert_eq!(
            keys(&tool["annotations"]),
            ["readOnlyHint", "title", "custom"]
        );
        assert_eq!(
            tool["_meta"],
            json!({"anthropic/maxResultSizeChars": 5, "n": null})
        );
    }

    #[test]
    fn a_call_result_and_the_handshake_are_written_as_mcp_2_writes_them() {
        let called = shape_result(
            "tools/call",
            json!({"isError": false, "content": [
                {"type": "text", "text": "t", "annotations": {"priority": 1, "x": 0}},
                {"type": "image", "mimeType": "image/png", "data": "aGk="},
                {"type": "other", "z": 1, "a": 2},
            ]}),
        );
        assert_eq!(keys(&called), ["content", "isError"]);
        assert_eq!(keys(&called["content"][0]), ["annotations", "text", "type"]);
        assert_eq!(
            called["content"][0]["annotations"],
            json!({"priority": 1.0})
        );
        assert_eq!(keys(&called["content"][1]), ["data", "mimeType", "type"]);
        assert_eq!(keys(&called["content"][2]), ["type", "z", "a"]);

        let handshake = shape_result(
            "initialize",
            json!({
                "protocolVersion": "2025-11-25",
                "capabilities": {"tools": {"listChanged": false}},
                "serverInfo": {"name": "n", "version": "1"},
            }),
        );
        assert_eq!(
            keys(&handshake),
            ["capabilities", "protocolVersion", "serverInfo"]
        );
        assert_eq!(shape_result("ping", json!({})), json!({}));
        assert_eq!(
            shape_result("resources/list", json!({"b": 1, "a": 2})),
            json!({"b": 1, "a": 2})
        );
    }
}
