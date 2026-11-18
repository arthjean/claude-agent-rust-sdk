//! Rebuilds the visible conversation of a transcript from its `parentUuid`
//! links.

use std::collections::{HashMap, HashSet};

use indexmap::IndexMap;
use serde_json::{Map, Value};

use super::{SessionMessage, SessionMessageType};

/// One parsed transcript line.
pub(crate) type Entry = Map<String, Value>;

const TRANSCRIPT_ENTRY_TYPES: [&str; 5] = ["user", "assistant", "progress", "system", "attachment"];

fn str_field<'a>(entry: &'a Entry, name: &str) -> Option<&'a str> {
    entry.get(name).and_then(Value::as_str)
}

fn uuid(entry: &Entry) -> &str {
    str_field(entry, "uuid").unwrap_or_default()
}

fn entry_type(entry: &Entry) -> Option<&str> {
    str_field(entry, "type")
}

fn parent(entry: &Entry) -> Option<&str> {
    str_field(entry, "parentUuid").filter(|parent| !parent.is_empty())
}

/// Truth value of a JSON field, as Python's `bool()` computes it.
pub(crate) fn truthy(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => false,
        Some(Value::Bool(flag)) => *flag,
        Some(Value::Number(number)) => number.as_f64().is_some_and(|number| number != 0.0),
        Some(Value::String(text)) => !text.is_empty(),
        Some(Value::Array(items)) => !items.is_empty(),
        Some(Value::Object(fields)) => !fields.is_empty(),
    }
}

/// Whether an entry is a transcript message: one of the chained types,
/// with a string `uuid`.
pub(crate) fn is_transcript_entry(entry: &Entry) -> bool {
    entry_type(entry).is_some_and(|kind| TRANSCRIPT_ENTRY_TYPES.contains(&kind))
        && entry.get("uuid").is_some_and(Value::is_string)
}

/// Parses JSONL content into transcript entries, skipping corrupt lines and
/// entries that are not transcript messages.
pub(crate) fn parse_transcript_entries(content: &str) -> Vec<Entry> {
    content
        .split('\n')
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .filter_map(|line| match serde_json::from_str::<Value>(line) {
            Ok(Value::Object(entry)) if is_transcript_entry(&entry) => Some(entry),
            _ => None,
        })
        .collect()
}

/// Keeps the transcript messages of entries already parsed, as a store
/// returns them.
pub(crate) fn filter_transcript_entries(entries: Vec<Value>) -> Vec<Entry> {
    entries
        .into_iter()
        .filter_map(|entry| match entry {
            Value::Object(entry) if is_transcript_entry(&entry) => Some(entry),
            _ => None,
        })
        .collect()
}

fn walk_to_root<'a>(
    leaf: &'a Entry,
    by_uuid: &IndexMap<&str, &'a Entry>,
) -> (Vec<&'a Entry>, HashSet<&'a str>) {
    let mut chain = Vec::new();
    let mut seen = HashSet::new();
    let mut current = Some(leaf);
    while let Some(entry) = current {
        if !seen.insert(uuid(entry)) {
            break;
        }
        chain.push(entry);
        current = parent(entry).and_then(|parent| by_uuid.get(parent).copied());
    }
    chain.reverse();
    (chain, seen)
}

fn index_by_uuid(entries: &[Entry]) -> IndexMap<&str, &Entry> {
    let mut by_uuid = IndexMap::new();
    for entry in entries {
        by_uuid.insert(uuid(entry), entry);
    }
    by_uuid
}

/// Builds the conversation chain, root first.
///
/// From each terminal entry (one no entry names as its parent) the walk
/// goes back to the nearest user or assistant entry, a leaf. The leaf kept
/// is the latest in the file among leaves that are not sidechain, team, or
/// meta entries, or among all leaves when none qualifies. The chain is the
/// walk from that leaf to the root along `parentUuid`, protected against
/// cycles. `logicalParentUuid` is not followed: after a compaction the
/// compact summary replaces the earlier messages.
pub(crate) fn build_conversation_chain(entries: &[Entry]) -> Vec<&Entry> {
    if entries.is_empty() {
        return Vec::new();
    }
    let by_uuid = index_by_uuid(entries);
    let position: HashMap<&str, usize> = entries
        .iter()
        .enumerate()
        .map(|(index, entry)| (uuid(entry), index))
        .collect();
    let parents: HashSet<&str> = entries.iter().filter_map(parent).collect();

    let mut leaves: Vec<&Entry> = Vec::new();
    for terminal in entries
        .iter()
        .filter(|entry| !parents.contains(uuid(entry)))
    {
        let mut seen = HashSet::new();
        let mut current = Some(terminal);
        while let Some(entry) = current {
            if !seen.insert(uuid(entry)) {
                break;
            }
            if matches!(entry_type(entry), Some("user" | "assistant")) {
                leaves.push(entry);
                break;
            }
            current = parent(entry).and_then(|parent| by_uuid.get(parent).copied());
        }
    }
    if leaves.is_empty() {
        return Vec::new();
    }
    let main_leaves: Vec<&Entry> = leaves
        .iter()
        .copied()
        .filter(|leaf| {
            !truthy(leaf.get("isSidechain"))
                && !truthy(leaf.get("teamName"))
                && !truthy(leaf.get("isMeta"))
        })
        .collect();
    let candidates = if main_leaves.is_empty() {
        &leaves
    } else {
        &main_leaves
    };
    let rank = |entry: &Entry| {
        position
            .get(uuid(entry))
            .map_or(-1, |&index| i64::try_from(index).unwrap_or(i64::MAX))
    };
    let mut leaf = candidates[0];
    for &candidate in &candidates[1..] {
        if rank(candidate) > rank(leaf) {
            leaf = candidate;
        }
    }
    let (chain, mut seen) = walk_to_root(leaf, &by_uuid);
    recover_orphaned_parallel_tool_results(&by_uuid, chain, &mut seen)
}

fn assistant_message_id(entry: &Entry) -> Option<&str> {
    if entry_type(entry) != Some("assistant") {
        return None;
    }
    entry
        .get("message")?
        .get("id")?
        .as_str()
        .filter(|id| !id.is_empty())
}

fn content_block_ids<'a>(entry: &'a Entry, block_type: &str, id_field: &str) -> Vec<&'a str> {
    let Some(Value::Array(blocks)) = entry
        .get("message")
        .and_then(|message| message.get("content"))
    else {
        return Vec::new();
    };
    blocks
        .iter()
        .filter(|block| block.get("type").and_then(Value::as_str) == Some(block_type))
        .filter_map(|block| block.get(id_field).and_then(Value::as_str))
        .collect()
}

fn is_tool_result_entry(entry: &Entry) -> bool {
    if entry_type(entry) != Some("user") || parent(entry).is_none() {
        return false;
    }
    let Some(Value::Array(blocks)) = entry
        .get("message")
        .and_then(|message| message.get("content"))
    else {
        return false;
    };
    blocks
        .iter()
        .any(|block| block.get("type").and_then(Value::as_str) == Some("tool_result"))
}

fn same_writer(first: &Entry, second: &Entry) -> bool {
    truthy(first.get("isSidechain")) == truthy(second.get("isSidechain"))
        && first.get("agentId") == second.get("agentId")
}

fn timestamp_key(entry: &Entry) -> &str {
    str_field(entry, "timestamp").unwrap_or_default()
}

/// Puts back the tool calls and results a parallel turn left off the
/// chain.
///
/// The CLI writes one assistant entry per content block, so a response
/// with several parallel tool calls is several assistant entries sharing
/// `message.id`, and each result names its own call as parent: the
/// transcript forks and the walk keeps one branch. For each response on the
/// chain, its entries and results the walk missed are inserted after its
/// last entry on the chain, siblings first, each group ordered by
/// timestamp.
fn recover_orphaned_parallel_tool_results<'a>(
    by_uuid: &IndexMap<&'a str, &'a Entry>,
    chain: Vec<&'a Entry>,
    seen: &mut HashSet<&'a str>,
) -> Vec<&'a Entry> {
    let chain_assistants: Vec<&Entry> = chain
        .iter()
        .copied()
        .filter(|entry| entry_type(entry) == Some("assistant"))
        .collect();
    let mut anchor_by_message_id: HashMap<&str, &Entry> = HashMap::new();
    for &entry in &chain_assistants {
        if let Some(message_id) = assistant_message_id(entry) {
            anchor_by_message_id.insert(message_id, entry);
        }
    }
    if anchor_by_message_id.is_empty() {
        return chain;
    }

    let mut siblings_by_message_id: HashMap<&str, Vec<&Entry>> = HashMap::new();
    let mut call_owner: HashMap<&str, Option<&Entry>> = HashMap::new();
    let mut tool_result_entries: Vec<&Entry> = Vec::new();
    for &entry in by_uuid.values() {
        if let Some(message_id) = assistant_message_id(entry) {
            siblings_by_message_id
                .entry(message_id)
                .or_default()
                .push(entry);
            for call_id in content_block_ids(entry, "tool_use", "id") {
                match call_owner.get(call_id) {
                    None => {
                        call_owner.insert(call_id, Some(entry));
                    }
                    Some(owner) => {
                        let same_response = owner
                            .is_some_and(|owner| assistant_message_id(owner) == Some(message_id));
                        call_owner.insert(call_id, same_response.then_some(entry));
                    }
                }
            }
        } else if is_tool_result_entry(entry) {
            tool_result_entries.push(entry);
        }
    }

    let mut results_by_assistant: HashMap<&str, Vec<&Entry>> = HashMap::new();
    for &result in &tool_result_entries {
        if let Some(parent) = parent(result) {
            results_by_assistant.entry(parent).or_default().push(result);
        }
        if let Some(source) = str_field(result, "sourceToolAssistantUUID")
            && let Some(&source_entry) = by_uuid.get(source)
            && same_writer(result, source_entry)
        {
            results_by_assistant.entry(source).or_default().push(result);
        }
        for call_id in content_block_ids(result, "tool_result", "tool_use_id") {
            if let Some(Some(owner)) = call_owner.get(call_id)
                && same_writer(result, owner)
            {
                results_by_assistant
                    .entry(uuid(owner))
                    .or_default()
                    .push(result);
            }
        }
    }

    let mut answered_on_chain: Option<HashSet<&str>> = None;
    let file_position: HashMap<&str, usize> = by_uuid
        .keys()
        .enumerate()
        .map(|(index, &key)| (key, index))
        .collect();
    let mut processed: HashSet<&str> = HashSet::new();
    let mut inserts: HashMap<&str, Vec<&Entry>> = HashMap::new();
    for &assistant in &chain_assistants {
        let Some(message_id) = assistant_message_id(assistant) else {
            continue;
        };
        if !processed.insert(message_id) {
            continue;
        }
        let group: Vec<&Entry> = siblings_by_message_id
            .get(message_id)
            .cloned()
            .unwrap_or_else(|| vec![assistant]);
        let member_uuids: HashSet<&str> = group.iter().map(|member| uuid(member)).collect();
        let mut orphaned_siblings: Vec<&Entry> = group
            .iter()
            .copied()
            .filter(|member| !seen.contains(uuid(member)))
            .collect();
        let mut orphaned_results: Vec<&Entry> = Vec::new();
        let mut strays: Vec<&Entry> = Vec::new();
        let mut collected: HashSet<&str> = HashSet::new();
        for member in &group {
            for &result in results_by_assistant.get(uuid(member)).into_iter().flatten() {
                let id = uuid(result);
                if seen.contains(id) || !collected.insert(id) {
                    continue;
                }
                if parent(result).is_some_and(|parent| member_uuids.contains(parent)) {
                    orphaned_results.push(result);
                } else {
                    strays.push(result);
                }
            }
        }

        if !strays.is_empty() {
            let on_chain = answered_on_chain.get_or_insert_with(|| {
                chain
                    .iter()
                    .flat_map(|entry| content_block_ids(entry, "tool_result", "tool_use_id"))
                    .collect()
            });
            let mut answered = on_chain.clone();
            for result in &orphaned_results {
                answered.extend(content_block_ids(result, "tool_result", "tool_use_id"));
            }
            strays.sort_by_key(|entry| {
                file_position
                    .get(uuid(entry))
                    .copied()
                    .unwrap_or(usize::MAX)
            });
            for result in strays {
                let call_ids = content_block_ids(result, "tool_result", "tool_use_id");
                let answers_open_call = call_ids.iter().any(|call_id| {
                    !answered.contains(call_id)
                        && matches!(call_owner.get(call_id), Some(Some(owner)) if member_uuids.contains(uuid(owner)))
                });
                if !answers_open_call {
                    continue;
                }
                answered.extend(call_ids);
                orphaned_results.push(result);
            }
        }

        if orphaned_siblings.is_empty() && orphaned_results.is_empty() {
            continue;
        }
        orphaned_siblings.sort_by_key(|entry| timestamp_key(entry));
        orphaned_results.sort_by_key(|entry| timestamp_key(entry));
        let recovered: Vec<&Entry> = orphaned_siblings
            .into_iter()
            .chain(orphaned_results)
            .collect();
        for entry in &recovered {
            seen.insert(uuid(entry));
        }
        if let Some(anchor) = anchor_by_message_id.get(message_id) {
            inserts.insert(uuid(anchor), recovered);
        }
    }

    if inserts.is_empty() {
        return chain;
    }
    let mut rebuilt = Vec::with_capacity(chain.len());
    for entry in chain {
        rebuilt.push(entry);
        if let Some(recovered) = inserts.remove(uuid(entry)) {
            rebuilt.extend(recovered);
        }
    }
    rebuilt
}

/// Whether an entry is shown: a user or assistant entry that is not meta,
/// sidechain, or team. Compact summaries are kept, since after a
/// compaction they are the only copy of what came before.
fn is_visible_message(entry: &Entry) -> bool {
    matches!(entry_type(entry), Some("user" | "assistant"))
        && !truthy(entry.get("isMeta"))
        && !truthy(entry.get("isSidechain"))
        && !truthy(entry.get("teamName"))
}

fn to_session_message(
    entry: &Entry,
    parent_tool_use_id: Option<&str>,
    parent_agent_id: Option<&str>,
) -> SessionMessage {
    SessionMessage {
        r#type: if entry_type(entry) == Some("user") {
            SessionMessageType::User
        } else {
            SessionMessageType::Assistant
        },
        uuid: uuid(entry).to_owned(),
        session_id: str_field(entry, "sessionId").unwrap_or_default().to_owned(),
        message: entry.get("message").cloned().unwrap_or(Value::Null),
        parent_tool_use_id: parent_tool_use_id.map(str::to_owned),
        parent_agent_id: parent_agent_id.map(str::to_owned),
    }
}

/// Skips `offset` items, then keeps at most `limit` when it is positive.
pub(crate) fn paginate<T>(items: Vec<T>, limit: Option<usize>, offset: usize) -> Vec<T> {
    let items = items.into_iter().skip(offset);
    match limit {
        Some(limit) if limit > 0 => items.take(limit).collect(),
        _ => items.collect(),
    }
}

/// The visible messages of a main transcript, paginated.
pub(crate) fn entries_to_session_messages(
    entries: &[Entry],
    limit: Option<usize>,
    offset: usize,
) -> Vec<SessionMessage> {
    let messages = build_conversation_chain(entries)
        .into_iter()
        .filter(|entry| is_visible_message(entry))
        .map(|entry| to_session_message(entry, None, None))
        .collect();
    paginate(messages, limit, offset)
}

/// The chain of a subagent transcript, which is linear: from the last user
/// or assistant entry back to the root.
fn build_subagent_chain(entries: &[Entry]) -> Vec<&Entry> {
    let by_uuid = index_by_uuid(entries);
    let Some(leaf) = entries
        .iter()
        .rev()
        .find(|entry| matches!(entry_type(entry), Some("user" | "assistant")))
    else {
        return Vec::new();
    };
    walk_to_root(leaf, &by_uuid).0
}

/// The user and assistant messages of a subagent transcript, paginated,
/// each carrying the ids of the parent that spawned the subagent.
pub(crate) fn entries_to_subagent_messages(
    entries: &[Entry],
    limit: Option<usize>,
    offset: usize,
    parent_tool_use_id: Option<&str>,
    parent_agent_id: Option<&str>,
) -> Vec<SessionMessage> {
    let messages = build_subagent_chain(entries)
        .into_iter()
        .filter(|entry| matches!(entry_type(entry), Some("user" | "assistant")))
        .map(|entry| to_session_message(entry, parent_tool_use_id, parent_agent_id))
        .collect();
    paginate(messages, limit, offset)
}

/// The parent ids recorded by subagent metadata: the `.meta.json` sidecar
/// on disk or the `agent_metadata` entry in a store.
pub(crate) fn parent_ids_from_agent_metadata(
    metadata: Option<&Map<String, Value>>,
) -> (Option<String>, Option<String>) {
    let field = |name: &str| {
        metadata
            .and_then(|metadata| metadata.get(name))
            .and_then(Value::as_str)
            .map(str::to_owned)
    };
    (field("toolUseId"), field("parentAgentId"))
}

/// Separates the `agent_metadata` entries of a subagent stream from its
/// transcript lines. The last metadata entry wins, since a resume rewrites
/// it.
pub(crate) fn split_agent_metadata(
    entries: Vec<Value>,
) -> (Option<Map<String, Value>>, Vec<Value>) {
    let mut metadata = None;
    let mut transcript = Vec::new();
    for entry in entries {
        match entry {
            Value::Object(fields)
                if fields.get("type").and_then(Value::as_str) == Some("agent_metadata") =>
            {
                metadata = Some(fields);
            }
            other => transcript.push(other),
        }
    }
    (metadata, transcript)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn entry(value: Value) -> Entry {
        match value {
            Value::Object(fields) => fields,
            _ => Map::new(),
        }
    }

    fn uuids(chain: &[&Entry]) -> Vec<String> {
        chain.iter().map(|entry| uuid(entry).to_owned()).collect()
    }

    fn user(id: &str, parent: Option<&str>) -> Entry {
        entry(json!({"type": "user", "uuid": id, "parentUuid": parent, "message": {"content": id}}))
    }

    #[test]
    fn linear_chain_is_returned_root_first() {
        let entries = vec![user("a", None), user("b", Some("a")), user("c", Some("b"))];
        assert_eq!(uuids(&build_conversation_chain(&entries)), ["a", "b", "c"]);
        assert!(build_conversation_chain(&[]).is_empty());
    }

    #[test]
    fn main_chain_wins_over_a_later_sidechain_leaf() {
        let mut side = user("s", Some("a"));
        side.insert("isSidechain".to_owned(), Value::Bool(true));
        let entries = vec![user("a", None), user("m", Some("a")), side];
        assert_eq!(uuids(&build_conversation_chain(&entries)), ["a", "m"]);
    }

    #[test]
    fn cycles_end_the_walk() {
        let entries = vec![
            user("a", Some("b")),
            user("b", Some("a")),
            user("c", Some("a")),
        ];
        assert_eq!(uuids(&build_conversation_chain(&entries)), ["b", "a", "c"]);
        let subagent = vec![user("x", Some("y")), user("y", Some("x"))];
        assert_eq!(build_subagent_chain(&subagent).len(), 2);
    }

    #[test]
    fn logical_parent_is_not_followed() {
        let mut summary = user("s", None);
        summary.insert("logicalParentUuid".to_owned(), Value::from("a"));
        summary.insert("isCompactSummary".to_owned(), Value::Bool(true));
        let entries = vec![user("a", None), summary, user("n", Some("s"))];
        assert_eq!(uuids(&build_conversation_chain(&entries)), ["s", "n"]);
    }

    #[test]
    fn only_progress_entries_give_an_empty_chain() {
        let entries = vec![
            entry(json!({"type": "progress", "uuid": "p1", "parentUuid": null})),
            entry(json!({"type": "progress", "uuid": "p2", "parentUuid": "p1"})),
        ];
        assert!(build_conversation_chain(&entries).is_empty());
    }

    fn parallel_tool_turn(
        parent: &str,
        tools: usize,
        result_order: &[usize],
    ) -> (Vec<Entry>, Vec<Entry>) {
        let mut calls: Vec<Entry> = Vec::new();
        for index in 0..tools {
            let previous = calls
                .last()
                .map_or(parent.to_owned(), |call| uuid(call).to_owned());
            calls.push(entry(json!({
                "type": "assistant", "uuid": format!("call-{index}"), "parentUuid": previous,
                "sessionId": "s", "timestamp": format!("2026-01-01T00:00:01.{index:03}Z"),
                "message": {"id": "msg_parallel", "role": "assistant", "content": [
                    {"type": "tool_use", "id": format!("toolu_{index}"), "name": "Read", "input": {}}
                ]},
            })));
        }
        let results = result_order
            .iter()
            .enumerate()
            .map(|(position, index)| {
                entry(json!({
                    "type": "user", "uuid": format!("result-{index}"), "parentUuid": format!("call-{index}"),
                    "sessionId": "s", "sourceToolAssistantUUID": format!("call-{index}"),
                    "timestamp": format!("2026-01-01T00:00:02.{position:03}Z"),
                    "message": {"role": "user", "content": [
                        {"type": "tool_result", "tool_use_id": format!("toolu_{index}"), "content": "ok"}
                    ]},
                }))
            })
            .collect();
        (calls, results)
    }

    fn turn(
        prompt_first: bool,
        calls: Vec<Entry>,
        results: Vec<Entry>,
        answer_parent: Option<&str>,
    ) -> Vec<Entry> {
        let mut entries = Vec::new();
        if prompt_first {
            entries.push(entry(
                json!({"type": "user", "uuid": "prompt", "parentUuid": null}),
            ));
        }
        entries.extend(calls);
        entries.extend(results);
        if let Some(parent) = answer_parent {
            entries.push(entry(
                json!({"type": "assistant", "uuid": "answer", "parentUuid": parent}),
            ));
        }
        entries
    }

    #[test]
    fn recovers_sibling_calls_dropped_by_the_walk() {
        let (calls, results) = parallel_tool_turn("prompt", 3, &[1, 2, 0]);
        let entries = turn(true, calls, results, Some("result-0"));
        assert_eq!(
            uuids(&build_conversation_chain(&entries)),
            [
                "prompt", "call-0", "call-1", "call-2", "result-1", "result-2", "result-0",
                "answer"
            ]
        );
    }

    #[test]
    fn recovered_results_are_in_completion_order() {
        let (calls, results) = parallel_tool_turn("prompt", 3, &[1, 0, 2]);
        let entries = turn(true, calls, results, Some("result-2"));
        assert_eq!(
            uuids(&build_conversation_chain(&entries)),
            [
                "prompt", "call-0", "call-1", "call-2", "result-1", "result-0", "result-2",
                "answer"
            ]
        );
    }

    #[test]
    fn recovered_calls_are_in_timestamp_order() {
        let (mut calls, results) = parallel_tool_turn("prompt", 3, &[1, 2, 0]);
        calls.swap(1, 2);
        let entries = turn(true, calls, results, Some("result-0"));
        assert_eq!(
            uuids(&build_conversation_chain(&entries))[1..4],
            ["call-0", "call-1", "call-2"]
        );
    }

    #[test]
    fn sequential_tool_calls_are_unchanged() {
        let entries = vec![
            entry(json!({"type": "user", "uuid": "prompt", "parentUuid": null})),
            entry(
                json!({"type": "assistant", "uuid": "call-0", "parentUuid": "prompt",
                "message": {"id": "msg_1", "content": [{"type": "tool_use", "id": "toolu_0"}]}}),
            ),
            entry(
                json!({"type": "user", "uuid": "result-0", "parentUuid": "call-0",
                "message": {"content": [{"type": "tool_result", "tool_use_id": "toolu_0"}]}}),
            ),
            entry(
                json!({"type": "assistant", "uuid": "call-1", "parentUuid": "result-0",
                "message": {"id": "msg_1", "content": [{"type": "tool_use", "id": "toolu_1"}]}}),
            ),
            entry(
                json!({"type": "user", "uuid": "result-1", "parentUuid": "call-1",
                "message": {"content": [{"type": "tool_result", "tool_use_id": "toolu_1"}]}}),
            ),
        ];
        let chain: Vec<Entry> = build_conversation_chain(&entries)
            .into_iter()
            .cloned()
            .collect();
        assert_eq!(chain, entries);
    }

    #[test]
    fn abandoned_branch_is_not_recovered() {
        let (calls, results) = parallel_tool_turn("prompt", 2, &[0, 1]);
        let mut entries = vec![
            entry(json!({"type": "user", "uuid": "prompt", "parentUuid": null})),
            entry(
                json!({"type": "assistant", "uuid": "old-call", "parentUuid": "prompt",
                "message": {"id": "msg_abandoned", "content": [{"type": "tool_use", "id": "toolu_old"}]}}),
            ),
            entry(
                json!({"type": "user", "uuid": "old-result", "parentUuid": "old-call",
                "message": {"content": [{"type": "tool_result", "tool_use_id": "toolu_old"}]}}),
            ),
        ];
        entries.extend(turn(false, calls, results, None));
        assert_eq!(
            uuids(&build_conversation_chain(&entries)),
            ["prompt", "call-0", "call-1", "result-0", "result-1"]
        );
    }

    #[test]
    fn recovers_a_result_whose_parent_is_outside_the_turn() {
        let (calls, mut results) = parallel_tool_turn("prompt", 2, &[0, 1]);
        results[0].insert("parentUuid".to_owned(), Value::from("missing"));
        results[0].remove("sourceToolAssistantUUID");
        let entries = turn(true, calls, results, Some("result-1"));
        assert_eq!(
            uuids(&build_conversation_chain(&entries)),
            [
                "prompt", "call-0", "call-1", "result-0", "result-1", "answer"
            ]
        );
    }

    #[test]
    fn does_not_duplicate_an_answered_call() {
        let (calls, mut results) = parallel_tool_turn("prompt", 2, &[0, 1]);
        let mut duplicate = results[1].clone();
        duplicate.insert("uuid".to_owned(), Value::from("result-1-copy"));
        duplicate.insert("parentUuid".to_owned(), Value::from("elsewhere"));
        duplicate.remove("sourceToolAssistantUUID");
        results.push(duplicate);
        let entries = turn(true, calls, results, Some("result-1"));
        assert_eq!(
            uuids(&build_conversation_chain(&entries)),
            [
                "prompt", "call-0", "call-1", "result-0", "result-1", "answer"
            ]
        );
    }

    #[test]
    fn does_not_pull_in_another_agents_result() {
        let (calls, mut results) = parallel_tool_turn("prompt", 2, &[0, 1]);
        results[0].insert("parentUuid".to_owned(), Value::from("sidechain-call"));
        results[0].insert("isSidechain".to_owned(), Value::Bool(true));
        results[0].insert("agentId".to_owned(), Value::from("agent-1"));
        results[0].remove("sourceToolAssistantUUID");
        let entries = turn(true, calls, results, Some("result-1"));
        assert_eq!(
            uuids(&build_conversation_chain(&entries)),
            ["prompt", "call-0", "call-1", "result-1", "answer"]
        );
    }

    #[test]
    fn recovery_tolerates_malformed_entries() {
        let (calls, mut results) = parallel_tool_turn("prompt", 2, &[0, 1]);
        results[0].insert("timestamp".to_owned(), Value::from(12_345));
        let mut entries = turn(true, calls, results, None);
        entries.extend([
            entry(json!({"type": "assistant", "uuid": "no-message", "parentUuid": "result-1"})),
            entry(
                json!({"type": "assistant", "uuid": "odd", "parentUuid": "no-message",
                "message": {"id": 7, "content": ["text", null, {"type": "tool_use"}]}}),
            ),
            entry(
                json!({"type": "user", "uuid": "odd-result", "parentUuid": "odd",
                "message": {"content": [{"type": "tool_result", "tool_use_id": 3}]}}),
            ),
        ]);
        assert_eq!(
            uuids(&build_conversation_chain(&entries)),
            [
                "prompt",
                "call-0",
                "call-1",
                "result-0",
                "result-1",
                "no-message",
                "odd",
                "odd-result"
            ]
        );
    }

    #[test]
    fn pagination_skips_then_limits() {
        assert_eq!(paginate(vec![1, 2, 3, 4], Some(2), 1), [2, 3]);
        assert_eq!(paginate(vec![1, 2, 3], Some(0), 1), [2, 3]);
        assert_eq!(paginate(vec![1, 2], None, 5), Vec::<i32>::new());
    }
}
