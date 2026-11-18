//! Session reads return a turn of parallel tool calls whole.
//!
//! The CLI writes each tool result under the entry of its own call. When
//! every call is written before the first result, the transcript forks
//! and only one branch leads to the next turn.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::Duration;

use claude_agent_kit::{
    ContentBlock, InMemorySessionStore, SdkMcpTool, SessionKey, SessionMessage, SessionMessageType,
    ToolResult, create_sdk_mcp_server, get_session_messages, get_session_messages_from_store,
    project_key_for_directory,
};
use schemars::JsonSchema;
use serde::Deserialize;

use crate::suite;

const KEYS: [&str; 3] = ["alpha", "beta", "gamma"];
const TOOL: &str = "mcp__slow__lookup";
const ATTEMPTS: usize = 3;

#[derive(Deserialize, JsonSchema)]
struct Lookup {
    key: String,
}

struct Turn {
    session_id: String,
    calls: BTreeSet<String>,
    forked: bool,
    store: InMemorySessionStore,
}

async fn parallel_turn(cwd: &Path) -> Turn {
    let lookup = SdkMcpTool::typed::<Lookup>(
        "lookup",
        "Look up the value stored for a key",
        |args| async move {
            tokio::time::sleep(Duration::from_secs(1)).await;
            Ok(ToolResult::text(format!("value of {}", args.key)))
        },
    );
    let store = InMemorySessionStore::new();
    let options = suite::options()
        .cwd(cwd)
        .session_store(store.clone())
        .mcp_server(
            "slow",
            create_sdk_mcp_server("slow", None, [lookup]).unwrap(),
        )
        .allowed_tools([TOOL])
        .max_turns(4)
        .build()
        .unwrap();
    let prompt = format!(
        "Look up {} with the {TOOL} tool, one call per key. Make all {} tool calls in a single \
         response, so they run in parallel: do not wait for one result before making the next \
         call. Then reply with the single word DONE.",
        KEYS.join(", "),
        KEYS.len()
    );

    let client = suite::connect(options).await;
    let messages = suite::turn(&client, prompt).await;
    client.disconnect().await.unwrap();

    let result = suite::result(&messages);
    assert!(!result.is_error, "the run failed: {result:#?}");
    let session_id = result.session_id.clone();
    let mut calls = BTreeSet::new();
    let mut results = BTreeSet::new();
    for (parent, block) in suite::blocks(&messages) {
        match block {
            ContentBlock::ToolUse(call) if parent.is_none() => {
                calls.insert(call.id.clone());
            }
            ContentBlock::ToolResult(result) if parent.is_none() => {
                results.insert(result.tool_use_id.clone());
            }
            _ => {}
        }
    }
    assert!(
        !calls.is_empty(),
        "the model should have called the lookup tool"
    );
    assert_eq!(calls, results, "every live tool call should have a result");

    let key = SessionKey::new(project_key_for_directory(Some(cwd)), &session_id);
    let parents: Vec<String> = store
        .get_entries(&key)
        .iter()
        .filter(|entry| {
            matches!(
                entry.get("type").and_then(|kind| kind.as_str()),
                Some("user" | "assistant")
            ) && entry.contains_key("uuid")
        })
        .filter_map(|entry| entry.get("parentUuid")?.as_str().map(str::to_owned))
        .collect();
    let distinct: BTreeSet<&String> = parents.iter().collect();
    Turn {
        session_id,
        calls,
        forked: distinct.len() < parents.len(),
        store,
    }
}

fn block_ids(messages: &[SessionMessage], block_type: &str, field: &str) -> Vec<(usize, String)> {
    let mut found = Vec::new();
    for (index, message) in messages.iter().enumerate() {
        let Some(content) = message.message["content"].as_array() else {
            continue;
        };
        for block in content {
            if block["type"] == block_type
                && let Some(id) = block[field].as_str()
            {
                found.push((index, id.to_owned()));
            }
        }
    }
    found
}

fn sorted_ids(blocks: &[(usize, String)]) -> Vec<String> {
    let mut ids: Vec<String> = blocks.iter().map(|(_, id)| id.clone()).collect();
    ids.sort();
    ids
}

#[tokio::test]
#[ignore = "runs the real CLI; run with `cargo xtask e2e`"]
async fn parallel_tool_calls_are_read_back_whole() {
    let (_directory, cwd) = suite::workspace();
    let mut turn = parallel_turn(&cwd).await;
    for _ in 1..ATTEMPTS {
        if turn.forked {
            break;
        }
        turn = parallel_turn(&cwd).await;
    }
    assert!(
        turn.forked,
        "the model made no parallel tool calls in {ATTEMPTS} runs, so the transcript never \
         forked and the readers were not exercised"
    );
    let expected: Vec<String> = turn.calls.iter().cloned().collect();

    let local = get_session_messages(&turn.session_id, Some(&cwd), None, 0);
    let mirrored =
        get_session_messages_from_store(&turn.store, &turn.session_id, Some(&cwd), None, 0)
            .await
            .unwrap();
    suite::forget(&cwd);

    for (reader, messages) in [("local", &local), ("store", &mirrored)] {
        let shape: Vec<_> = messages
            .iter()
            .map(|message| (message.r#type, message.uuid.as_str()))
            .collect();
        let calls = block_ids(messages, "tool_use", "id");
        let results = block_ids(messages, "tool_result", "tool_use_id");

        assert_eq!(sorted_ids(&calls), expected, "{reader}: {shape:?}");
        assert_eq!(sorted_ids(&results), expected, "{reader}: {shape:?}");
        let uuids: BTreeSet<&str> = messages
            .iter()
            .map(|message| message.uuid.as_str())
            .collect();
        assert_eq!(uuids.len(), messages.len(), "{reader}: {shape:?}");

        let call_index: BTreeMap<&str, usize> = calls
            .iter()
            .map(|(index, id)| (id.as_str(), *index))
            .collect();
        let response_at = |index: usize| {
            let message = &messages[index];
            (message.r#type == SessionMessageType::Assistant).then(|| message.message["id"].clone())
        };
        for (index, call_id) in &results {
            let start = call_index[call_id.as_str()];
            assert!(start < *index, "{reader}: {shape:?}");
            let responses: BTreeSet<String> = (start..*index)
                .filter_map(response_at)
                .map(|id| id.to_string())
                .collect();
            let own: BTreeSet<String> = response_at(start)
                .into_iter()
                .map(|id| id.to_string())
                .collect();
            assert_eq!(responses, own, "{reader}: {shape:?}");
        }
        assert_eq!(
            messages.first().map(|message| message.r#type),
            Some(SessionMessageType::User),
            "{reader}: {shape:?}"
        );
        let last = messages.last().unwrap();
        assert_eq!(
            last.r#type,
            SessionMessageType::Assistant,
            "{reader}: {shape:?}"
        );
        assert!(
            block_ids(std::slice::from_ref(last), "tool_use", "id").is_empty(),
            "{reader}: {shape:?}"
        );
    }

    let uuids = |messages: &[SessionMessage]| -> Vec<String> {
        messages
            .iter()
            .map(|message| message.uuid.clone())
            .collect()
    };
    assert_eq!(uuids(&local), uuids(&mirrored));
}
