//! The `verbatim_prompts` option.
//!
//! The CLI expands an `@/absolute/path` token of a prompt into the
//! content of that file, outside the working directory and without a tool
//! call. With `verbatim_prompts` the prompt is delivered as written. Each
//! test plants a random marker in a file and checks whether the CLI
//! attached the file to the session, as recorded in its transcript: the
//! transcript decides, not the reply of the model.

use std::path::{Path, PathBuf};

use claude_agent_kit::{ClaudeAgentOptions, ContentBlock, Message, SettingSource};

use crate::suite;

struct Planted {
    _directory: tempfile::TempDir,
    cwd: PathBuf,
    prompt: String,
    marker: String,
}

fn plant() -> Planted {
    let (directory, root) = suite::workspace();
    let id = uuid::Uuid::new_v4().simple().to_string();
    let marker = format!("MARKER-{}", &id[..12]);
    let secret = root.join("outside").join("secret.txt");
    std::fs::create_dir(root.join("outside")).unwrap();
    std::fs::write(&secret, format!("The marker is {marker}.\n")).unwrap();
    let cwd = root.join("cwd");
    std::fs::create_dir(&cwd).unwrap();
    Planted {
        _directory: directory,
        cwd,
        prompt: format!(
            "What marker does this file contain? Reply with only the marker, or UNKNOWN if you \
             cannot see it. @{}",
            secret.display()
        ),
        marker,
    }
}

fn options(cwd: &Path, verbatim_prompts: bool) -> ClaudeAgentOptions {
    suite::options()
        .verbatim_prompts(verbatim_prompts)
        .cwd(cwd)
        .model("haiku")
        .tools(Vec::<String>::new())
        .setting_sources(Vec::<SettingSource>::new())
        .extra_arg("strict-mcp-config", None)
        .max_turns(1)
        .build()
        .unwrap()
}

fn reply_and_attachments(messages: &[Message], cwd: &Path) -> (String, String) {
    for (_, block) in suite::blocks(messages) {
        assert!(
            !matches!(block, ContentBlock::ToolUse(_)),
            "the model has no tools: {messages:#?}"
        );
    }
    let reply = suite::assistant_text(messages);
    let results = suite::results(messages);
    let session_id = &results
        .last()
        .expect("the run reports no result")
        .session_id;
    let attached = suite::transcript(session_id, cwd)
        .iter()
        .filter(|entry| entry["type"] == "attachment" && entry["attachment"]["type"] == "file")
        .map(|entry| entry["attachment"].to_string())
        .collect();
    suite::forget(cwd);
    (reply, attached)
}

#[tokio::test]
#[ignore = "runs the real CLI; run with `cargo xtask e2e`"]
async fn a_mentioned_path_is_expanded_by_default() {
    let planted = plant();

    let messages = suite::run(planted.prompt.as_str(), options(&planted.cwd, false)).await;

    let (_, attached) = reply_and_attachments(&messages, &planted.cwd);
    assert!(attached.contains(&planted.marker), "{attached}");
}

#[tokio::test]
#[ignore = "runs the real CLI; run with `cargo xtask e2e`"]
async fn a_text_prompt_of_a_query_is_not_expanded() {
    let planted = plant();

    let messages = suite::run(planted.prompt.as_str(), options(&planted.cwd, true)).await;

    let (reply, attached) = reply_and_attachments(&messages, &planted.cwd);
    assert!(!attached.contains(&planted.marker), "{attached}");
    assert!(!reply.contains(&planted.marker), "{reply}");
}

#[tokio::test]
#[ignore = "runs the real CLI; run with `cargo xtask e2e`"]
async fn a_prompt_sent_through_a_client_is_not_expanded() {
    let planted = plant();

    let client = suite::connect(options(&planted.cwd, true)).await;
    let messages = suite::turn(&client, planted.prompt.as_str()).await;
    client.disconnect().await.unwrap();

    assert!(!suite::result(&messages).is_error, "{messages:#?}");
    let (reply, attached) = reply_and_attachments(&messages, &planted.cwd);
    assert!(!attached.contains(&planted.marker), "{attached}");
    assert!(!reply.contains(&planted.marker), "{reply}");
}
