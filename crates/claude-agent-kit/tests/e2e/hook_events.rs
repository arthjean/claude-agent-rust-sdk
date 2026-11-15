//! Hook event types and the inputs they deliver.

use claude_agent_kit::{
    HookInput, HookMatcher, HookPermissionDecision, NotificationHookSpecificOutput,
    PostToolUseHookSpecificOutput, PreToolUseHookSpecificOutput, SyncHookOutput,
};

use crate::suite::{self, SHELL_MATCHER, SHELL_TOOLS};

#[tokio::test]
#[ignore = "runs the real CLI; run with `cargo xtask e2e`"]
async fn a_pre_tool_use_hook_receives_the_tool_use_id_and_adds_context() {
    let invocations = suite::log::<(String, String)>();
    let seen = invocations.clone();
    let hook = suite::hook(move |input| {
        if let Some(call) = suite::tool_call(input) {
            seen.lock().unwrap().push(call);
        }
        SyncHookOutput::default().hook_specific_output(
            PreToolUseHookSpecificOutput::default()
                .permission_decision(HookPermissionDecision::Allow)
                .permission_decision_reason("Approved with context")
                .additional_context("This command is running in a test environment"),
        )
    });
    let options = suite::options()
        .allowed_tools(SHELL_TOOLS)
        .hook(
            "PreToolUse",
            HookMatcher::new().matcher(SHELL_MATCHER).hook(hook),
        )
        .build()
        .unwrap();

    let client = suite::connect(options).await;
    suite::turn(&client, "Run: echo 'test additional context'").await;
    client.disconnect().await.unwrap();

    let invocations = suite::entries(&invocations);
    assert!(
        !invocations.is_empty(),
        "the PreToolUse hook should have been invoked"
    );
    assert!(!invocations[0].1.is_empty(), "{invocations:?}");
}

#[tokio::test]
#[ignore = "runs the real CLI; run with `cargo xtask e2e`"]
async fn a_post_tool_use_hook_receives_the_tool_use_id() {
    let invocations = suite::log::<(String, String)>();
    let seen = invocations.clone();
    let hook = suite::hook(move |input| {
        if let Some(call) = suite::tool_call(input) {
            seen.lock().unwrap().push(call);
        }
        SyncHookOutput::default().hook_specific_output(
            PostToolUseHookSpecificOutput::default()
                .additional_context("Post-tool monitoring active"),
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
    suite::turn(&client, "Run: echo 'test tool_use_id'").await;
    client.disconnect().await.unwrap();

    let invocations = suite::entries(&invocations);
    assert!(
        !invocations.is_empty(),
        "the PostToolUse hook should have been invoked"
    );
    assert!(!invocations[0].1.is_empty(), "{invocations:?}");
}

#[tokio::test]
#[ignore = "runs the real CLI; run with `cargo xtask e2e`"]
async fn a_notification_hook_receives_typed_notifications() {
    let invocations = suite::log::<(String, Option<String>)>();
    let seen = invocations.clone();
    let hook = suite::hook(move |input| {
        let notification_type = match input {
            HookInput::Notification(notification) => Some(notification.notification_type.clone()),
            _ => None,
        };
        seen.lock()
            .unwrap()
            .push((input.hook_event_name().to_owned(), notification_type));
        SyncHookOutput::default().hook_specific_output(
            NotificationHookSpecificOutput::default().additional_context("Notification received"),
        )
    });
    let options = suite::options()
        .hook("Notification", HookMatcher::new().hook(hook))
        .build()
        .unwrap();

    let client = suite::connect(options).await;
    let messages = suite::turn(&client, "Say hello in one word.").await;
    client.disconnect().await.unwrap();

    assert!(!suite::result(&messages).is_error);
    for (event, notification_type) in suite::entries(&invocations) {
        assert_eq!(event, "Notification");
        assert!(notification_type.is_some_and(|kind| !kind.is_empty()));
    }
}

#[tokio::test]
#[ignore = "runs the real CLI; run with `cargo xtask e2e`"]
async fn several_hook_events_are_registered_together() {
    let invocations = suite::log::<String>();
    let track = |log: &suite::Log<String>| {
        let seen = log.clone();
        suite::hook(move |input| {
            seen.lock()
                .unwrap()
                .push(input.hook_event_name().to_owned());
            SyncHookOutput::default()
        })
    };
    let options = suite::options()
        .allowed_tools(SHELL_TOOLS)
        .hook("Notification", HookMatcher::new().hook(track(&invocations)))
        .hook(
            "PreToolUse",
            HookMatcher::new()
                .matcher(SHELL_MATCHER)
                .hook(track(&invocations)),
        )
        .hook(
            "PostToolUse",
            HookMatcher::new()
                .matcher(SHELL_MATCHER)
                .hook(track(&invocations)),
        )
        .build()
        .unwrap();

    let client = suite::connect(options).await;
    suite::turn(&client, "Run: echo 'multi-hook test'").await;
    client.disconnect().await.unwrap();

    let events = suite::entries(&invocations);
    assert!(events.contains(&"PreToolUse".to_owned()), "{events:?}");
    assert!(events.contains(&"PostToolUse".to_owned()), "{events:?}");
}
