//! Tool permission callbacks.

use std::path::PathBuf;

use claude_agent_rust_sdk::{
    BoxError, CanUseTool, PermissionBehavior, PermissionResult, PermissionRuleValue,
    PermissionUpdate, PermissionUpdateDestination, PermissionUpdateKind, Prompt, SettingSource,
    ToolPermissionContext,
};
use futures_util::stream;
use serde_json::{Value, json};

use crate::suite::{self, Log, SHELL_TOOLS};

fn allow_and_record(invocations: &Log<String>) -> CanUseTool {
    let invocations = invocations.clone();
    CanUseTool::new(
        move |tool_name: String, _input: Value, _context: ToolPermissionContext| {
            let invocations = invocations.clone();
            async move {
                invocations.lock().unwrap().push(tool_name);
                Ok::<PermissionResult, BoxError>(PermissionResult::allow())
            }
        },
    )
}

struct Scratch(PathBuf);

impl Scratch {
    fn in_directory(directory: PathBuf, prefix: &str) -> Self {
        let id = uuid::Uuid::new_v4().simple().to_string();
        Self(directory.join(format!("{prefix}_{}.txt", &id[..8])))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[tokio::test]
#[ignore = "runs the real CLI; run with `cargo xtask e2e`"]
async fn the_permission_callback_is_called_for_a_command_that_is_not_read_only() {
    let invocations = suite::log::<String>();
    let file = Scratch::in_directory(std::env::temp_dir(), "sdk_permission_test");
    let options = suite::options()
        .can_use_tool(allow_and_record(&invocations))
        .setting_sources(Vec::<SettingSource>::new())
        .build()
        .unwrap();

    let client = suite::connect(options).await;
    suite::turn(
        &client,
        format!(
            "Create an empty file at {} with one shell command and nothing else: `touch` in \
             Bash, `New-Item` in PowerShell.",
            file.0.display()
        ),
    )
    .await;
    client.disconnect().await.unwrap();

    let invocations = suite::entries(&invocations);
    assert!(
        invocations
            .iter()
            .any(|tool| SHELL_TOOLS.contains(&tool.as_str())),
        "the callback should have been invoked for a shell tool, got {invocations:?}"
    );
}

async fn run_query_with_only_a_permission_callback(prompt: impl FnOnce(String) -> Prompt) {
    let invocations = suite::log::<String>();
    let file = Scratch::in_directory(std::env::home_dir().unwrap(), ".agent_sdk_permission_e2e");
    let options = suite::options()
        .can_use_tool(allow_and_record(&invocations))
        .setting_sources(Vec::<SettingSource>::new())
        .disallowed_tools(SHELL_TOOLS)
        .max_turns(3)
        .build()
        .unwrap();
    let text = format!(
        "Use the Write tool to create the file {} containing exactly the text hello. Do not use \
         any other tool.",
        file.0.display()
    );

    let messages = suite::run(prompt(text), options).await;

    let results = suite::results(&messages);
    let result = results.last().expect("the query should yield a result");
    assert!(!result.is_error, "{result:#?}");
    let invocations = suite::entries(&invocations);
    assert!(
        invocations.contains(&"Write".to_owned()),
        "the callback should have been invoked for Write, got {invocations:?}"
    );
    assert!(file.0.exists(), "the permitted Write should have run");
}

#[tokio::test]
#[ignore = "runs the real CLI; run with `cargo xtask e2e`"]
async fn a_streamed_prompt_with_only_a_permission_callback_keeps_stdin_open_for_the_verdict() {
    run_query_with_only_a_permission_callback(|text| {
        Prompt::stream(stream::iter([json!({
            "type": "user",
            "message": {"role": "user", "content": text},
            "parent_tool_use_id": null,
            "session_id": "",
        })]))
    })
    .await;
}

#[tokio::test]
#[ignore = "runs the real CLI; run with `cargo xtask e2e`"]
async fn a_text_prompt_works_with_a_permission_callback() {
    run_query_with_only_a_permission_callback(Prompt::from).await;
}

#[tokio::test]
#[ignore = "runs the real CLI; run with `cargo xtask e2e`"]
async fn a_whole_tool_rule_returned_by_the_callback_is_applied_by_the_cli() {
    let invocations = suite::log::<String>();
    let recorded = invocations.clone();
    let callback = CanUseTool::new(
        move |tool_name: String, _input: Value, _context: ToolPermissionContext| {
            let recorded = recorded.clone();
            async move {
                recorded.lock().unwrap().push(tool_name.clone());
                let rule = PermissionUpdate::new(PermissionUpdateKind::AddRules)
                    .rules([PermissionRuleValue::new(tool_name, None)])
                    .behavior(PermissionBehavior::Allow)
                    .destination(PermissionUpdateDestination::Session);
                Ok::<PermissionResult, BoxError>(
                    PermissionResult::allow().updated_permissions([rule]),
                )
            }
        },
    );
    let (_directory, cwd) = suite::workspace();
    let options = suite::options()
        .cwd(&cwd)
        .can_use_tool(callback)
        .setting_sources(Vec::<SettingSource>::new())
        .disallowed_tools(SHELL_TOOLS)
        .max_turns(4)
        .build()
        .unwrap();

    let client = suite::connect(options).await;
    let messages = suite::turn(
        &client,
        "Use the Write tool to create first.txt containing exactly the text one. Once that has \
         succeeded, use the Write tool again, in a separate step, to create second.txt containing \
         exactly the text two. Use no other tool.",
    )
    .await;
    client.disconnect().await.unwrap();

    let result = suite::result(&messages);
    eprintln!("cost: {:?} USD", result.total_cost_usd);
    let invocations = suite::entries(&invocations);
    let seen = format!(
        "callback calls {invocations:?}, result {result:#?}, text {:?}",
        suite::assistant_text(&messages)
    );
    assert!(cwd.join("first.txt").exists(), "{seen}");
    assert!(cwd.join("second.txt").exists(), "{seen}");
    assert_eq!(
        invocations,
        ["Write"],
        "the rule `Write` sent with the first allow covers the second call"
    );
}
