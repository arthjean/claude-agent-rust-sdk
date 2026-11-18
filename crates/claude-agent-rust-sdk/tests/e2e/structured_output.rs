//! Structured output validated against a JSON Schema.

use std::path::Path;

use claude_agent_rust_sdk::{ClaudeAgentOptionsBuilder, PermissionMode};
use serde_json::{Value, json};

use crate::suite;

fn options(schema: Value) -> ClaudeAgentOptionsBuilder {
    suite::options()
        .output_format(json!({"type": "json_schema", "schema": schema}))
        .permission_mode(PermissionMode::AcceptEdits)
}

fn plant(root: &Path, files: &[(&str, &str)]) {
    for (path, content) in files {
        let file = root.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, content).unwrap();
    }
}

fn python_project(root: &Path) {
    plant(
        root,
        &[
            ("src/package/__init__.py", ""),
            ("src/package/client.py", "def connect():\n    return True\n"),
            ("src/package/query.py", "def query():\n    return 42\n"),
            (
                "tests/test_client.py",
                "import pytest\n\n\ndef test_connect():\n    assert True\n",
            ),
            (
                "tests/test_query.py",
                "import pytest\n\n\n@pytest.mark.parametrize('value', [1, 2])\ndef \
                 test_query(value):\n    assert value\n",
            ),
        ],
    );
}

async fn structured_output(prompt: &str, options: ClaudeAgentOptionsBuilder, cwd: &Path) -> Value {
    let messages = suite::run(prompt, options.cwd(cwd).build().unwrap()).await;
    suite::forget(cwd);
    let results = suite::results(&messages);
    let result = results.last().expect("no result message received");
    assert!(!result.is_error, "the query failed: {result:#?}");
    assert_eq!(result.subtype, "success");
    result
        .structured_output
        .clone()
        .unwrap_or_else(|| panic!("no structured output in the result: {result:#?}"))
}

#[tokio::test]
#[ignore = "runs the real CLI; run with `cargo xtask e2e`"]
async fn a_flat_schema_is_filled_after_exploring_with_tools() {
    let (_directory, cwd) = suite::workspace();
    python_project(&cwd);
    let schema = json!({
        "type": "object",
        "properties": {
            "file_count": {"type": "number"},
            "has_tests": {"type": "boolean"},
            "test_file_count": {"type": "number"},
        },
        "required": ["file_count", "has_tests"],
    });

    let output = structured_output(
        "Count how many Python files are in src/package/ and check if there are any test files. \
         Use tools to explore the filesystem.",
        options(schema),
        &cwd,
    )
    .await;

    assert!(output["has_tests"].is_boolean(), "{output}");
    assert!(
        output["file_count"]
            .as_f64()
            .is_some_and(|count| count > 0.0),
        "{output}"
    );
}

#[tokio::test]
#[ignore = "runs the real CLI; run with `cargo xtask e2e`"]
async fn a_schema_with_nested_objects_and_arrays_is_filled() {
    let (_directory, cwd) = suite::workspace();
    let schema = json!({
        "type": "object",
        "properties": {
            "analysis": {
                "type": "object",
                "properties": {
                    "word_count": {"type": "number"},
                    "character_count": {"type": "number"},
                },
                "required": ["word_count", "character_count"],
            },
            "words": {"type": "array", "items": {"type": "string"}},
        },
        "required": ["analysis", "words"],
    });

    let output = structured_output(
        "Analyze this text: 'Hello world'. Provide word count, character count, and list of \
         words.",
        options(schema),
        &cwd,
    )
    .await;

    assert_eq!(
        output["analysis"]["word_count"].as_f64(),
        Some(2.0),
        "{output}"
    );
    assert_eq!(
        output["analysis"]["character_count"].as_f64(),
        Some(11.0),
        "{output}"
    );
    assert_eq!(
        output["words"].as_array().map(Vec::len),
        Some(2),
        "{output}"
    );
}

#[tokio::test]
#[ignore = "runs the real CLI; run with `cargo xtask e2e`"]
async fn an_enum_constraint_is_respected() {
    let (_directory, cwd) = suite::workspace();
    python_project(&cwd);
    let frameworks = ["pytest", "unittest", "nose", "unknown"];
    let schema = json!({
        "type": "object",
        "properties": {
            "has_tests": {"type": "boolean"},
            "test_framework": {"type": "string", "enum": frameworks},
            "test_count": {"type": "number"},
        },
        "required": ["has_tests", "test_framework"],
    });

    let output = structured_output(
        "Search for test files in the tests/ directory. Determine which test framework is being \
         used (pytest/unittest/nose) and count how many test files exist. Use Grep to search for \
         framework imports.",
        options(schema),
        &cwd,
    )
    .await;

    assert_eq!(output["has_tests"], true, "{output}");
    assert_eq!(output["test_framework"], "pytest", "{output}");
}

#[tokio::test]
#[ignore = "runs the real CLI; run with `cargo xtask e2e`"]
async fn a_schema_is_filled_when_the_agent_uses_tools() {
    let (_directory, cwd) = suite::workspace();
    plant(
        &cwd,
        &[("README.md", "# Sample\n"), ("notes.txt", "notes\n")],
    );
    let schema = json!({
        "type": "object",
        "properties": {
            "file_count": {"type": "number"},
            "has_readme": {"type": "boolean"},
        },
        "required": ["file_count", "has_readme"],
    });

    let output = structured_output(
        "Count how many files are in the current directory and check if there's a README file. \
         Use tools as needed.",
        options(schema),
        &cwd,
    )
    .await;

    assert!(output["has_readme"].is_boolean(), "{output}");
    assert!(
        output["file_count"]
            .as_f64()
            .is_some_and(|count| count >= 0.0),
        "{output}"
    );
}
