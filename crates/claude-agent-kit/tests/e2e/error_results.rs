//! A terminal error result surfaces as a typed error.

use claude_agent_kit::Error;

use crate::suite;

const BAD_MODEL: &str = "claude-nonexistent-model-for-sdk-e2e";

#[tokio::test]
#[ignore = "runs the real CLI; run with `cargo xtask e2e`"]
async fn an_api_failure_ends_the_query_with_a_result_error_carrying_the_payload() {
    let options = suite::options()
        .model(BAD_MODEL)
        .max_turns(1)
        .build()
        .unwrap();

    let (messages, error) = suite::run_to_end("Reply with: pong", options).await;

    let result = suite::result(&messages);
    assert!(result.is_error, "{result:#?}");
    let error = error.expect("the error result should end the stream with an error");
    let text = error.to_string();
    let Error::ResultError {
        data, exit_code, ..
    } = &error
    else {
        panic!("expected a result error, got {error:?}");
    };
    assert!(exit_code.is_some());
    assert!(!text.contains("error result: success"), "{text}");
    let prose = result.result.as_deref().unwrap_or_default().trim();
    assert!(
        !prose.is_empty() && text.contains(prose),
        "{text} / {prose}"
    );

    assert_eq!(data.subtype(), Some(result.subtype.as_str()));
    assert_eq!(data.result(), result.result.as_deref());
    assert_eq!(data.api_error_status(), result.api_error_status);
    assert_eq!(data.subtype(), Some("success"));
    assert_eq!(result.terminal_reason.as_deref(), Some("api_error"));
}
