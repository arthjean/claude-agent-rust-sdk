//! Session summaries a store maintains as entries are appended, so a
//! listing needs no per-session load.

use serde_json::{Map, Value};

use super::SdkSessionInfo;
use super::chain::truthy;
use super::lite::{PromptText, classify_prompt, entry_texts};
use super::timestamp::parse_iso_millis;
use crate::session_store::{SessionKey, SessionStoreEntry, SessionSummaryEntry};

const LAST_WINS_FIELDS: [(&str, &str); 5] = [
    ("customTitle", "custom_title"),
    ("aiTitle", "ai_title"),
    ("lastPrompt", "last_prompt"),
    ("summary", "summary_hint"),
    ("gitBranch", "git_branch"),
];

fn fold_first_prompt(data: &mut Map<String, Value>, entry: &SessionStoreEntry) {
    if truthy(data.get("first_prompt_locked"))
        || entry.get("type").and_then(Value::as_str) != Some("user")
    {
        return;
    }
    let is_true = |name: &str| entry.get(name) == Some(&Value::Bool(true));
    if is_true("isMeta") || is_true("isCompactSummary") {
        return;
    }
    let carries_tool_result = entry
        .get("message")
        .and_then(|message| message.get("content"))
        .and_then(Value::as_array)
        .is_some_and(|blocks| {
            blocks
                .iter()
                .any(|block| block.get("type").and_then(Value::as_str) == Some("tool_result"))
        });
    if carries_tool_result {
        return;
    }
    for raw in entry_texts(entry) {
        match classify_prompt(raw) {
            PromptText::Prompt(prompt) => {
                data.insert("first_prompt".to_owned(), Value::from(prompt));
                data.insert("first_prompt_locked".to_owned(), Value::Bool(true));
                return;
            }
            PromptText::Command(name) => {
                if !truthy(data.get("command_fallback")) {
                    data.insert("command_fallback".to_owned(), Value::from(name));
                }
            }
            PromptText::Skipped => {}
        }
    }
}

/// Folds a batch of appended entries into the running summary of a
/// session.
///
/// A store calls it inside [`SessionStore::append`](crate::SessionStore::append)
/// with the previous summary of the key (`None` on the first append) and
/// persists the result, so
/// [`list_sessions_from_store`](crate::list_sessions_from_store) reads every
/// summary in one call. Every field is set once or last-wins, so earlier
/// entries are never read again. Do not call it for a key with a subpath:
/// subagent transcripts do not contribute to the summary of their session.
///
/// The derived state lives in [`SessionSummaryEntry::data`], which the
/// store persists verbatim; its keys are those of the upstream Python SDK,
/// so summaries written by either SDK are read by both. The fold keeps the
/// `mtime` of `previous` (0 for a new session): the store stamps its own
/// storage write time after persisting, from the clock of
/// [`SessionStore::list_sessions`](crate::SessionStore::list_sessions).
#[must_use]
pub fn fold_session_summary(
    previous: Option<&SessionSummaryEntry>,
    key: &SessionKey,
    entries: &[SessionStoreEntry],
) -> SessionSummaryEntry {
    let mut summary = previous
        .cloned()
        .unwrap_or_else(|| SessionSummaryEntry::new(key.session_id.clone(), 0, Map::new()));
    let data = &mut summary.data;
    for entry in entries {
        if !data.contains_key("is_sidechain") {
            data.insert(
                "is_sidechain".to_owned(),
                Value::Bool(entry.get("isSidechain") == Some(&Value::Bool(true))),
            );
        }
        if !data.contains_key("created_at")
            && let Some(millis) = entry
                .get("timestamp")
                .and_then(Value::as_str)
                .and_then(parse_iso_millis)
        {
            data.insert("created_at".to_owned(), Value::from(millis));
        }
        if !data.contains_key("cwd")
            && let Some(cwd) = entry
                .get("cwd")
                .and_then(Value::as_str)
                .filter(|cwd| !cwd.is_empty())
        {
            data.insert("cwd".to_owned(), Value::from(cwd));
        }
        fold_first_prompt(data, entry);
        for (source, target) in LAST_WINS_FIELDS {
            if let Some(Value::String(value)) = entry.get(source) {
                data.insert(target.to_owned(), Value::from(value.as_str()));
            }
        }
        if entry.get("type").and_then(Value::as_str) == Some("tag") {
            match entry
                .get("tag")
                .and_then(Value::as_str)
                .filter(|tag| !tag.is_empty())
            {
                Some(tag) => {
                    data.insert("tag".to_owned(), Value::from(tag));
                }
                None => {
                    data.shift_remove("tag");
                }
            }
        }
    }
    summary
}

fn non_empty_string(data: &Map<String, Value>, name: &str) -> Option<String> {
    data.get(name)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

/// The metadata a summary gives, or `None` for a sidechain session or one
/// with no title, summary, or prompt.
pub(crate) fn summary_entry_to_sdk_info(
    entry: &SessionSummaryEntry,
    project_path: Option<&str>,
) -> Option<SdkSessionInfo> {
    let data = &entry.data;
    if truthy(data.get("is_sidechain")) {
        return None;
    }
    let first_prompt = if truthy(data.get("first_prompt_locked")) {
        non_empty_string(data, "first_prompt")
    } else {
        non_empty_string(data, "command_fallback")
    };
    let custom_title =
        non_empty_string(data, "custom_title").or_else(|| non_empty_string(data, "ai_title"));
    let summary = custom_title
        .clone()
        .or_else(|| non_empty_string(data, "last_prompt"))
        .or_else(|| non_empty_string(data, "summary_hint"))
        .or_else(|| first_prompt.clone())?;
    let mut info = SdkSessionInfo::new(entry.session_id.clone(), summary, entry.mtime);
    info.custom_title = custom_title;
    info.first_prompt = first_prompt;
    info.git_branch = non_empty_string(data, "git_branch");
    info.cwd = non_empty_string(data, "cwd").or_else(|| {
        project_path
            .filter(|path| !path.is_empty())
            .map(str::to_owned)
    });
    info.tag = non_empty_string(data, "tag");
    info.created_at = data.get("created_at").and_then(Value::as_i64);
    Some(info)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn entries(values: &[Value]) -> Vec<SessionStoreEntry> {
        values
            .iter()
            .filter_map(|value| value.as_object().cloned())
            .collect()
    }

    #[test]
    fn folds_set_once_and_last_wins_fields() {
        let key = SessionKey::new("p", "s");
        let first = fold_session_summary(
            None,
            &key,
            &entries(&[
                json!({"type": "user", "timestamp": "2024-01-01T00:00:00.000Z", "cwd": "/w", "message": {"content": "<command-name>/init</command-name>"}}),
                json!({"type": "user", "message": {"content": [{"type": "tool_result"}]}}),
                json!({"type": "user", "message": {"content": "real prompt"}, "gitBranch": "a"}),
                json!({"type": "tag", "tag": "t1"}),
            ]),
        );
        assert_eq!(first.mtime, 0);
        assert_eq!(first.session_id, "s");
        assert_eq!(
            Value::Object(first.data.clone()),
            json!({
                "is_sidechain": false, "created_at": 1_704_067_200_000_i64, "cwd": "/w",
                "command_fallback": "/init", "first_prompt": "real prompt", "first_prompt_locked": true,
                "git_branch": "a", "tag": "t1",
            })
        );
        let mut stamped = first;
        stamped.mtime = 42;
        let second = fold_session_summary(
            Some(&stamped),
            &key,
            &entries(&[
                json!({"type": "user", "message": {"content": "later"}, "cwd": "/other", "customTitle": "T"}),
                json!({"type": "tag", "tag": ""}),
            ]),
        );
        assert_eq!(second.mtime, 42);
        assert_eq!(second.data["first_prompt"], "real prompt");
        assert_eq!(second.data["cwd"], "/w");
        assert_eq!(second.data["custom_title"], "T");
        assert!(second.data.get("tag").is_none());
        let info = summary_entry_to_sdk_info(&second, Some("/project")).unwrap();
        assert_eq!(info.summary, "T");
        assert_eq!(info.first_prompt.as_deref(), Some("real prompt"));
        assert_eq!(info.cwd.as_deref(), Some("/w"));
        assert_eq!(info.created_at, Some(1_704_067_200_000));
        assert_eq!(info.last_modified, 42);
        assert_eq!(info.file_size, None);
    }

    #[test]
    fn sidechain_and_empty_summaries_give_no_info() {
        let key = SessionKey::new("p", "s");
        let sidechain = fold_session_summary(
            None,
            &key,
            &entries(&[json!({"type": "user", "isSidechain": true, "message": {"content": "x"}})]),
        );
        assert!(summary_entry_to_sdk_info(&sidechain, None).is_none());
        let empty = fold_session_summary(None, &key, &entries(&[json!({"type": "x"})]));
        assert!(summary_entry_to_sdk_info(&empty, None).is_none());
        let command_only = fold_session_summary(
            None,
            &key,
            &entries(&[
                json!({"type": "user", "message": {"content": "<command-name>/c</command-name>"}}),
            ]),
        );
        assert_eq!(
            summary_entry_to_sdk_info(&command_only, None)
                .unwrap()
                .summary,
            "/c"
        );
    }
}
