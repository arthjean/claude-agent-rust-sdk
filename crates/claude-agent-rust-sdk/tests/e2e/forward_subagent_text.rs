//! Text of a foreground subagent is forwarded only on request.

use std::collections::BTreeSet;

use claude_agent_rust_sdk::{AgentDefinition, ContentBlock, Message};

use crate::suite::{self, AGENT_TOOLS};

const PROMPT: &str = "Use the Agent tool exactly once with subagent_type 'greeter', prompt \
                      'say hi', and run_in_background set to false. Then reply with the single \
                      word DONE.";

struct Run {
    agent_tool_use_ids: BTreeSet<String>,
    attributed_text: Vec<String>,
}

async fn collect(forward_subagent_text: bool) -> Run {
    let options = suite::options()
        .forward_subagent_text(forward_subagent_text)
        .agent(
            "greeter",
            AgentDefinition::new(
                "Replies with a short greeting. Use for greeting tasks.",
                "Reply with one short friendly sentence. Do not use any tools.",
            )
            .model("haiku")
            .background(false),
        )
        .allowed_tools(AGENT_TOOLS)
        .max_turns(4)
        .build()
        .unwrap();

    let messages = suite::run(PROMPT, options).await;

    let result = suite::results(&messages);
    assert!(
        result.last().is_some_and(|result| !result.is_error),
        "{messages:#?}"
    );
    let mut run = Run {
        agent_tool_use_ids: BTreeSet::new(),
        attributed_text: Vec::new(),
    };
    for message in &messages {
        let Message::Assistant(assistant) = message else {
            continue;
        };
        match &assistant.parent_tool_use_id {
            None => {
                for block in &assistant.content {
                    if let ContentBlock::ToolUse(call) = block
                        && AGENT_TOOLS.contains(&call.name.as_str())
                    {
                        run.agent_tool_use_ids.insert(call.id.clone());
                    }
                }
            }
            Some(parent) => {
                if assistant
                    .content
                    .iter()
                    .any(|block| matches!(block, ContentBlock::Text(_)))
                {
                    run.attributed_text.push(parent.clone());
                }
            }
        }
    }
    assert!(
        !run.agent_tool_use_ids.is_empty(),
        "the model should have called the Agent tool: {messages:#?}"
    );
    run
}

#[tokio::test]
#[ignore = "runs the real CLI; run with `cargo xtask e2e`"]
async fn forwarded_subagent_text_is_attributed_to_the_agent_tool_use() {
    let run = collect(true).await;

    assert!(
        !run.attributed_text.is_empty(),
        "expected a subagent text message with parent_tool_use_id set"
    );
    for parent in &run.attributed_text {
        assert!(
            run.agent_tool_use_ids.contains(parent),
            "{parent} is not one of {:?}",
            run.agent_tool_use_ids
        );
    }
}

#[tokio::test]
#[ignore = "runs the real CLI; run with `cargo xtask e2e`"]
async fn subagent_text_is_not_forwarded_by_default() {
    let run = collect(false).await;

    assert_eq!(run.attributed_text, Vec::<String>::new());
}
