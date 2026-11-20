#![cfg(all(feature = "testing", feature = "macros"))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use claude_agent_rust_sdk::{SdkMcpTool, ToolAnnotations, ToolResult, create_sdk_mcp_server, tool};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

use common::mcp::{SdkMcpClient, texts};

#[derive(Deserialize, JsonSchema)]
struct Add {
    a: f64,
    b: f64,
}

#[derive(Deserialize, JsonSchema)]
struct Divide {
    dividend: f64,
    divisor: f64,
}

#[derive(Debug, thiserror::Error)]
#[error("Division by zero")]
struct DivisionByZero;

#[tool(name = "add", description = "Add two numbers")]
async fn add_numbers(args: Add) -> Result<ToolResult, std::convert::Infallible> {
    Ok(ToolResult::text((args.a + args.b).to_string()))
}

/// Divide one number by another.
///
/// The divisor must not be zero.
#[tool]
pub(crate) async fn divide(args: Divide) -> Result<ToolResult, DivisionByZero> {
    if args.divisor == 0.0 {
        return Err(DivisionByZero);
    }
    Ok(ToolResult::text((args.dividend / args.divisor).to_string()))
}

/// The doc comment loses to an explicit description.
#[tool(
    name = "wipe",
    description = "Delete every note",
    title = "Wipe notes",
    read_only_hint = false,
    destructive_hint = true,
    idempotent_hint = true,
    open_world_hint = false,
    max_result_size_chars = 250_000
)]
async fn wipe_notes(_args: Add) -> Result<ToolResult, String> {
    Err("nothing to delete".to_owned())
}

/// Add two numbers, under a name the generated code must not collide with.
#[tool]
async fn arguments(args: Add) -> Result<ToolResult, String> {
    Ok(ToolResult::text((args.a + args.b).to_string()))
}

#[tokio::test(start_paused = true)]
async fn function_named_arguments_is_still_a_tool() {
    let server = create_sdk_mcp_server("calculator", None, [arguments()]).unwrap();
    let client = SdkMcpClient::connected(server).await;
    let sum = client
        .call_tool("srv", "arguments", json!({"a": 1, "b": 2}))
        .await;
    assert_eq!(texts(&sum), ["3"]);
}

#[test]
fn attribute_generates_a_function_returning_the_tool() {
    let tool: SdkMcpTool = add_numbers();
    assert_eq!(tool.name(), "add");
    assert_eq!(tool.description(), "Add two numbers");
    assert_eq!(tool.input_schema()["required"], json!(["a", "b"]));
    assert_eq!(tool.tool_annotations(), None);
}

#[test]
fn doc_comment_is_the_description_and_the_function_name_is_the_tool_name() {
    let tool = divide();
    assert_eq!(tool.name(), "divide");
    assert_eq!(
        tool.description(),
        "Divide one number by another.\n\nThe divisor must not be zero."
    );
    assert_eq!(
        tool.input_schema()["required"],
        json!(["dividend", "divisor"])
    );
    assert_eq!(wipe_notes().description(), "Delete every note");
}

#[test]
fn annotation_hints_are_attribute_arguments() {
    let tool = wipe_notes();
    assert_eq!(
        tool.tool_annotations(),
        Some(
            &ToolAnnotations::new()
                .title("Wipe notes")
                .read_only_hint(false)
                .destructive_hint(true)
                .idempotent_hint(true)
                .open_world_hint(false)
                .max_result_size_chars(250_000)
        )
    );
    assert_eq!(
        tool.to_wire()["_meta"],
        json!({"anthropic/maxResultSizeChars": 250_000})
    );
}

#[tokio::test(start_paused = true)]
async fn tools_made_by_the_attribute_are_served_and_called() {
    let server =
        create_sdk_mcp_server("calculator", None, [add_numbers(), divide(), wipe_notes()]).unwrap();
    let client = SdkMcpClient::connected(server).await;

    let names: Vec<String> = client
        .list_tools("srv")
        .await
        .iter()
        .map(|tool| tool["name"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(names, ["add", "divide", "wipe"]);

    let sum = client
        .call_tool("srv", "add", json!({"a": 2, "b": 3}))
        .await;
    assert_eq!(texts(&sum), ["5"]);
    assert_eq!(sum["isError"], false);

    let quotient = client
        .call_tool("srv", "divide", json!({"dividend": 9, "divisor": 3}))
        .await;
    assert_eq!(texts(&quotient), ["3"]);

    let failure = client
        .call_tool("srv", "divide", json!({"dividend": 1, "divisor": 0}))
        .await;
    assert_eq!(failure["isError"], true);
    assert_eq!(texts(&failure), ["Division by zero"]);

    let refused = client
        .call_tool("srv", "wipe", json!({"a": 1, "b": 2}))
        .await;
    assert_eq!(refused["isError"], true);
    assert_eq!(texts(&refused), ["nothing to delete"]);

    let invalid = client.call_tool("srv", "add", json!({"a": "one"})).await;
    assert_eq!(invalid["isError"], true);
    assert!(texts(&invalid)[0].starts_with("Input validation error: "));
}

#[derive(Default)]
struct Ledger {
    entries: std::sync::Mutex<Vec<String>>,
}

#[derive(Deserialize, JsonSchema)]
struct Entry {
    text: String,
}

/// Record one entry in the shared ledger.
#[tool]
async fn record(ledger: &Ledger, args: Entry) -> Result<ToolResult, String> {
    let mut entries = ledger.entries.lock().unwrap();
    entries.push(args.text);
    Ok(ToolResult::text(entries.len().to_string()))
}

/// Greet with the prefix the tool was built with.
#[tool]
async fn greet(prefix: String, args: Entry) -> Result<ToolResult, String> {
    Ok(ToolResult::text(format!("{prefix} {}", args.text)))
}

#[tokio::test(start_paused = true)]
async fn a_tool_with_state_shares_it_across_calls_and_with_its_owner() {
    let ledger = std::sync::Arc::new(Ledger::default());
    let tool = record(std::sync::Arc::clone(&ledger));
    assert_eq!(tool.name(), "record");
    assert_eq!(tool.input_schema()["required"], json!(["text"]));
    let server = create_sdk_mcp_server("journal", None, [tool, greet("Hello".to_owned())]).unwrap();
    let client = SdkMcpClient::connected(server).await;

    let first = client
        .call_tool("srv", "record", json!({"text": "a"}))
        .await;
    let second = client
        .call_tool("srv", "record", json!({"text": "b"}))
        .await;
    assert_eq!(texts(&first), ["1"]);
    assert_eq!(texts(&second), ["2"]);
    assert_eq!(*ledger.entries.lock().unwrap(), ["a", "b"]);

    let greeting = client
        .call_tool("srv", "greet", json!({"text": "Ada"}))
        .await;
    assert_eq!(texts(&greeting), ["Hello Ada"]);
}

#[tokio::test]
async fn a_tool_made_by_the_attribute_is_called_directly_with_its_arguments() {
    assert_eq!(
        divide().call(json!({"dividend": 9, "divisor": 3})).await,
        ToolResult::text("3")
    );
    assert_eq!(
        divide().call(json!({"dividend": 1, "divisor": 0})).await,
        ToolResult::error("Division by zero")
    );
    let invalid = divide().call(json!({"dividend": 1})).await;
    assert!(invalid.is_error);
    assert_eq!(
        greet("Hi".to_owned()).call(json!({"text": "Bo"})).await,
        ToolResult::text("Hi Bo")
    );
}

/// Look a word up.
///
/// Example:
///
///     lookup("tool")
///
/// - A nested list:
///   - keeps its indent.
#[tool]
async fn lookup(args: Entry) -> Result<ToolResult, String> {
    Ok(ToolResult::text(args.text))
}

mod sdk {
    pub use claude_agent_rust_sdk::*;
}

/// Echo through a re-exported path.
#[tool(crate = "crate::sdk")]
async fn echo_through_reexport(args: Entry) -> Result<sdk::ToolResult, String> {
    Ok(sdk::ToolResult::text(args.text))
}

#[test]
fn doc_comment_loses_only_the_indentation_its_lines_share() {
    assert_eq!(
        lookup().description(),
        "Look a word up.\n\nExample:\n\n    lookup(\"tool\")\n\n- A nested list:\n  - keeps its indent."
    );
}

#[tokio::test]
async fn the_crate_argument_names_the_path_of_the_sdk_in_the_generated_code() {
    let tool = echo_through_reexport();
    assert_eq!(tool.name(), "echo_through_reexport");
    assert_eq!(
        tool.call(json!({"text": "hi"})).await,
        ToolResult::text("hi")
    );
}

#[test]
fn misuse_of_the_attribute_is_a_compile_error() {
    let cases = trybuild::TestCases::new();
    cases.compile_fail("tests/ui/tool_*.rs");
}
