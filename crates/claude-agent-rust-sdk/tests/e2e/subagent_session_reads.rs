//! Subagent transcript reads recover the tool call that spawned the
//! subagent.

use std::collections::{BTreeMap, BTreeSet};

use claude_agent_rust_sdk::{
    AgentDefinition, ContentBlock, InMemorySessionStore, SessionMessage, SessionMessagesOptions,
    get_subagent_messages, get_subagent_messages_from_store, list_subagents,
    list_subagents_from_store,
};

use crate::suite::{self, AGENT_TOOLS};

const PROMPT: &str = "Use the Agent tool exactly once with subagent_type 'greeter', prompt \
                      'say hi', and run_in_background set to false. Then reply with the single \
                      word DONE.";

type Parents = BTreeSet<Option<String>>;

fn parents(messages: &[SessionMessage]) -> Parents {
    messages
        .iter()
        .map(|message| message.parent_tool_use_id.clone())
        .collect()
}

fn attributed(
    messages_by_agent: &BTreeMap<String, Vec<SessionMessage>>,
    agent_tool_use_ids: &BTreeSet<String>,
) -> BTreeMap<String, Parents> {
    messages_by_agent
        .iter()
        .filter(|(_, messages)| !messages.is_empty())
        .map(|(agent_id, messages)| (agent_id.clone(), parents(messages)))
        .filter(|(_, parents)| {
            parents.iter().all(|parent| {
                parent
                    .as_ref()
                    .is_some_and(|id| agent_tool_use_ids.contains(id))
            })
        })
        .collect()
}

#[tokio::test]
#[ignore = "runs the real CLI; run with `cargo xtask e2e`"]
async fn subagent_messages_carry_the_id_of_the_tool_call_that_spawned_them() {
    let (_directory, cwd) = suite::workspace();
    let store = InMemorySessionStore::new();
    let options = suite::options()
        .cwd(&cwd)
        .session_store(store.clone())
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

    let results = suite::results(&messages);
    for result in &results {
        assert!(!result.is_error, "the run failed: {result:#?}");
    }
    let result = results.last().expect("the run reports no result");
    let session_id = result.session_id.as_str();
    let agent_tool_use_ids: BTreeSet<String> = suite::blocks(&messages)
        .into_iter()
        .filter_map(|(parent, block)| match block {
            ContentBlock::ToolUse(call)
                if parent.is_none() && AGENT_TOOLS.contains(&call.name.as_str()) =>
            {
                Some(call.id.clone())
            }
            _ => None,
        })
        .collect();
    assert!(
        !agent_tool_use_ids.is_empty(),
        "the model should have called the Agent tool: {messages:#?}"
    );

    let local: BTreeMap<String, Vec<SessionMessage>> = list_subagents(session_id, Some(&cwd))
        .into_iter()
        .map(|agent_id| {
            let messages = get_subagent_messages(
                session_id,
                &agent_id,
                SessionMessagesOptions::new().directory(&cwd),
            );
            (agent_id, messages)
        })
        .collect();
    let mut mirrored = BTreeMap::new();
    for agent_id in list_subagents_from_store(&store, session_id, Some(&cwd))
        .await
        .unwrap()
    {
        let messages = get_subagent_messages_from_store(
            &store,
            session_id,
            &agent_id,
            SessionMessagesOptions::new().directory(&cwd),
        )
        .await
        .unwrap();
        mirrored.insert(agent_id, messages);
    }
    suite::forget(&cwd);

    let local_attributed = attributed(&local, &agent_tool_use_ids);
    assert!(
        !local_attributed.is_empty(),
        "expected a subagent transcript attributed to one of {agent_tool_use_ids:?}, got {:?}",
        local
            .iter()
            .map(|(agent_id, messages)| (agent_id, parents(messages)))
            .collect::<Vec<_>>()
    );
    for agent_id in local_attributed.keys() {
        assert!(
            local[agent_id]
                .iter()
                .all(|message| message.parent_agent_id.is_none())
        );
    }

    let mirrored_attributed = attributed(&mirrored, &agent_tool_use_ids);
    assert!(
        !mirrored_attributed.is_empty(),
        "expected a mirrored subagent transcript attributed to one of {agent_tool_use_ids:?}, \
         got {:?}",
        mirrored
            .iter()
            .map(|(agent_id, messages)| (agent_id, parents(messages)))
            .collect::<Vec<_>>()
    );
    let shared: Vec<&String> = local_attributed
        .keys()
        .filter(|agent_id| mirrored_attributed.contains_key(*agent_id))
        .collect();
    assert!(
        !shared.is_empty(),
        "no subagent is attributed on both paths"
    );
    for agent_id in shared {
        assert_eq!(local_attributed[agent_id], mirrored_attributed[agent_id]);
    }
    for agent_id in mirrored_attributed.keys() {
        assert!(
            mirrored[agent_id]
                .iter()
                .all(|message| message.parent_agent_id.is_none())
        );
    }
}
