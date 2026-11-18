//! Hook callbacks and the fields of their answers.

use claude_agent_kit::{
    HookMatcher, HookPermissionDecision, PostToolUseHookSpecificOutput,
    PreToolUseHookSpecificOutput, SyncHookOutput,
};

use crate::suite::{self, SHELL_MATCHER, SHELL_TOOLS};

fn called_a_shell_tool(invocations: &[String]) -> bool {
    invocations
        .iter()
        .any(|tool| SHELL_TOOLS.contains(&tool.as_str()))
}

#[tokio::test]
#[ignore = "runs the real CLI; run with `cargo xtask e2e`"]
async fn a_hook_answers_with_a_permission_decision_and_a_reason() {
    let invocations = suite::log::<String>();
    let seen = invocations.clone();
    let hook = suite::hook(move |input| {
        let tool = suite::tool_call(input)
            .map(|call| call.0)
            .unwrap_or_default();
        seen.lock().unwrap().push(tool.clone());
        if SHELL_TOOLS.contains(&tool.as_str()) {
            return SyncHookOutput::default()
                .reason("Shell commands are blocked in this test for safety")
                .system_message("Command blocked by hook")
                .hook_specific_output(
                    PreToolUseHookSpecificOutput::default()
                        .permission_decision(HookPermissionDecision::Deny)
                        .permission_decision_reason("Security policy: shell commands blocked"),
                );
        }
        SyncHookOutput::default()
            .reason("Tool approved by security review")
            .hook_specific_output(
                PreToolUseHookSpecificOutput::default()
                    .permission_decision(HookPermissionDecision::Allow)
                    .permission_decision_reason("Tool passed security checks"),
            )
    });
    let options = suite::options()
        .allowed_tools(["Bash", "PowerShell", "Write"])
        .hook(
            "PreToolUse",
            HookMatcher::new().matcher(SHELL_MATCHER).hook(hook),
        )
        .build()
        .unwrap();

    let client = suite::connect(options).await;
    suite::turn(&client, "Run this bash command: echo 'hello'").await;
    client.disconnect().await.unwrap();

    let invocations = suite::entries(&invocations);
    assert!(called_a_shell_tool(&invocations), "{invocations:?}");
}

#[tokio::test]
#[ignore = "runs the real CLI; run with `cargo xtask e2e`"]
async fn a_hook_stops_the_run_with_continue_false_and_a_stop_reason() {
    let invocations = suite::log::<String>();
    let seen = invocations.clone();
    let hook = suite::hook(move |input| {
        let tool = suite::tool_call(input)
            .map(|call| call.0)
            .unwrap_or_default();
        seen.lock().unwrap().push(tool);
        SyncHookOutput::default()
            .continue_(false)
            .stop_reason("Execution halted by test hook for validation")
            .reason("Testing continue and stopReason fields")
            .system_message("Test hook stopped execution")
    });
    let options = suite::options()
        .allowed_tools(SHELL_TOOLS)
        .hook(
            "PostToolUse",
            HookMatcher::new().matcher(SHELL_MATCHER).hook(hook),
        )
        .build()
        .unwrap();

    let client = suite::connect(options).await;
    suite::turn(&client, "Run: echo 'test message'").await;
    client.disconnect().await.unwrap();

    let invocations = suite::entries(&invocations);
    assert!(called_a_shell_tool(&invocations), "{invocations:?}");
}

#[tokio::test]
#[ignore = "runs the real CLI; run with `cargo xtask e2e`"]
async fn a_hook_adds_context_through_its_specific_output() {
    let invocations = suite::log::<String>();
    let seen = invocations.clone();
    let hook = suite::hook(move |_input| {
        seen.lock().unwrap().push("context_added".to_owned());
        SyncHookOutput::default()
            .system_message("Additional context provided by hook")
            .reason("Hook providing monitoring feedback")
            .suppress_output(false)
            .hook_specific_output(
                PostToolUseHookSpecificOutput::default()
                    .additional_context("The command executed successfully with hook monitoring"),
            )
    });
    let options = suite::options()
        .allowed_tools(SHELL_TOOLS)
        .hook(
            "PostToolUse",
            HookMatcher::new().matcher(SHELL_MATCHER).hook(hook),
        )
        .build()
        .unwrap();

    let client = suite::connect(options).await;
    suite::turn(&client, "Run: echo 'testing hooks'").await;
    client.disconnect().await.unwrap();

    assert!(
        suite::entries(&invocations).contains(&"context_added".to_owned()),
        "the hook with a specific output should have been invoked"
    );
}
