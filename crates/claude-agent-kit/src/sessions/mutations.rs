//! Renaming, tagging, deleting, and forking sessions.
//!
//! Rename and tag append a metadata entry to the transcript, the way the
//! CLI does; readers take the last one. If the session is open in a CLI
//! process, the CLI reads the tail of the file before re-appending its own
//! metadata, so an entry written here is kept.

use std::fs::OpenOptions;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};
use unicode_normalization::UnicodeNormalization;
use unicode_properties::{GeneralCategory, UnicodeGeneralCategory};
use uuid::Uuid;

use super::chain::{Entry, truthy};
use super::disk::candidate_project_dirs;
use super::jsonl::{entries_to_jsonl, type_first_line};
use super::lite::{
    LITE_READ_BUF_SIZE, extract_first_prompt_from_head, extract_last_json_string_field,
};
use super::paths::project_key_for_directory;
use super::timestamp::now_iso;
use super::validate_uuid;
use crate::error::{Error, Result};
use crate::session_store::{SessionKey, SessionStore, SessionStoreEntry};

const SANITIZE_PASSES: usize = 10;
const FORK_TYPES: [&str; 5] = ["user", "assistant", "attachment", "system", "progress"];
const LEAKED_FIELDS: [&str; 4] = ["teamName", "agentName", "slug", "sourceToolAssistantUUID"];

/// Result of [`fork_session`] and
/// [`fork_session_via_store`](crate::fork_session_via_store).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ForkSessionResult {
    /// Id of the new session.
    pub session_id: String,
}

pub(crate) fn require_uuid<'a>(session_id: &'a str, name: &str) -> Result<&'a str> {
    validate_uuid(session_id).ok_or_else(|| Error::config(format!("Invalid {name}: {session_id}")))
}

pub(crate) fn clean_title(title: &str) -> Result<String> {
    let title = title.trim();
    if title.is_empty() {
        return Err(Error::config("title must be non-empty"));
    }
    Ok(title.to_owned())
}

/// The tag to write: sanitized and trimmed, or empty to clear it.
pub(crate) fn clean_tag(tag: Option<&str>) -> Result<String> {
    let Some(tag) = tag else {
        return Ok(String::new());
    };
    let sanitized = sanitize_unicode(tag);
    let sanitized = sanitized.trim();
    if sanitized.is_empty() {
        return Err(Error::config("tag must be non-empty (use None to clear)"));
    }
    Ok(sanitized.to_owned())
}

fn is_stripped(character: char) -> bool {
    matches!(
        character.general_category(),
        GeneralCategory::Format | GeneralCategory::PrivateUse | GeneralCategory::Unassigned
    ) || matches!(
        character,
        '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' | '\u{feff}' | '\u{e000}'..='\u{f8ff}'
    )
}

/// Removes zero-width, directional, private-use, and unassigned characters:
/// NFKC normalization, then removal of the characters of categories Cf,
/// Co, and Cn, repeated until nothing changes, at most 10 passes.
pub(crate) fn sanitize_unicode(value: &str) -> String {
    let mut current = value.to_owned();
    for _ in 0..SANITIZE_PASSES {
        let next: String = current
            .nfkc()
            .filter(|character| !is_stripped(*character))
            .collect();
        if next == current {
            break;
        }
        current = next;
    }
    current
}

fn object(value: Value) -> Entry {
    match value {
        Value::Object(entry) => entry,
        _ => Map::new(),
    }
}

/// Appends to an existing transcript. `Ok(false)` when the file does not
/// exist or is empty: an empty transcript means the session is not there.
fn try_append(path: &Path, data: &str) -> Result<bool> {
    let mut file = match OpenOptions::new().append(true).open(path) {
        Ok(file) => file,
        Err(error) if matches!(error.kind(), ErrorKind::NotFound | ErrorKind::NotADirectory) => {
            return Ok(false);
        }
        Err(error) => return Err(Error::io(format!("Cannot open {}", path.display()), error)),
    };
    let size = file
        .metadata()
        .map_err(|error| Error::io(format!("Cannot read {}", path.display()), error))?
        .len();
    if size == 0 {
        return Ok(false);
    }
    file.write_all(data.as_bytes())
        .map_err(|error| Error::io(format!("Cannot append to {}", path.display()), error))?;
    Ok(true)
}

fn not_found(session_id: &str, directory: Option<&Path>) -> Error {
    match directory {
        Some(directory) => Error::session_not_found(format!(
            "Session {session_id} not found in project directory for {}",
            directory.display()
        )),
        None => Error::session_not_found(format!("Session {session_id} not found")),
    }
}

fn append_to_session(session_id: &str, entry: &Entry, directory: Option<&Path>) -> Result<()> {
    let data = format!("{}\n", type_first_line(entry));
    let file_name = format!("{session_id}.jsonl");
    for (project_dir, _) in candidate_project_dirs(directory) {
        if try_append(&project_dir.join(&file_name), &data)? {
            return Ok(());
        }
    }
    Err(not_found(session_id, directory))
}

fn find_session_file(session_id: &str, directory: Option<&Path>) -> Option<(PathBuf, PathBuf)> {
    let file_name = format!("{session_id}.jsonl");
    candidate_project_dirs(directory).find_map(|(project_dir, _)| {
        let path = project_dir.join(&file_name);
        std::fs::metadata(&path)
            .is_ok_and(|metadata| metadata.len() > 0)
            .then_some((path, project_dir))
    })
}

/// Renames a session by appending a `custom-title` entry to its
/// transcript. The title is trimmed; the last title written wins.
///
/// `directory` has the meaning it has for
/// [`list_sessions`](crate::list_sessions); without it every project is
/// searched. The file is opened in append mode and never created.
///
/// This function blocks on filesystem calls.
///
/// # Errors
///
/// [`Error::Config`] for an id that is not a UUID or a title that is empty
/// after trimming, [`Error::SessionNotFound`] when no non-empty transcript
/// exists, and [`Error::Io`] when the write fails.
pub fn rename_session(session_id: &str, title: &str, directory: Option<&Path>) -> Result<()> {
    let session_id = require_uuid(session_id, "session_id")?;
    let title = clean_title(title)?;
    append_to_session(
        session_id,
        &object(json!({"type": "custom-title", "customTitle": title, "sessionId": session_id})),
        directory,
    )
}

/// Tags a session by appending a `tag` entry to its transcript; `None`
/// clears the tag by writing an empty one. The last tag written wins.
///
/// The tag is normalized with NFKC and stripped of format, private-use,
/// and unassigned characters (repeated until stable, at most 10 passes),
/// then trimmed, so it matches the filters of the CLI.
///
/// This function blocks on filesystem calls.
///
/// # Errors
///
/// [`Error::Config`] for an id that is not a UUID or a tag that is empty
/// after sanitizing, [`Error::SessionNotFound`] when no non-empty
/// transcript exists, and [`Error::Io`] when the write fails.
pub fn tag_session(session_id: &str, tag: Option<&str>, directory: Option<&Path>) -> Result<()> {
    let session_id = require_uuid(session_id, "session_id")?;
    let tag = clean_tag(tag)?;
    append_to_session(
        session_id,
        &object(json!({"type": "tag", "tag": tag, "sessionId": session_id})),
        directory,
    )
}

/// Deletes a session: its transcript and the `<session id>/` directory
/// beside it, which holds the transcripts of its subagents.
///
/// This is a hard delete. For a soft delete, tag the session and filter on
/// the tag when listing.
///
/// This function blocks on filesystem calls.
///
/// # Errors
///
/// [`Error::Config`] for an id that is not a UUID,
/// [`Error::SessionNotFound`] when no non-empty transcript exists, and
/// [`Error::Io`] when the transcript cannot be removed.
pub fn delete_session(session_id: &str, directory: Option<&Path>) -> Result<()> {
    let session_id = require_uuid(session_id, "session_id")?;
    let Some((path, project_dir)) = find_session_file(session_id, directory) else {
        return Err(not_found(session_id, directory));
    };
    std::fs::remove_file(&path).map_err(|error| match error.kind() {
        ErrorKind::NotFound => Error::session_not_found(format!("Session {session_id} not found")),
        _ => Error::io(format!("Cannot delete {}", path.display()), error),
    })?;
    let _ = std::fs::remove_dir_all(project_dir.join(session_id));
    Ok(())
}

/// Splits parsed entries into the transcript lines a fork copies and the
/// content replacements recorded for the source session.
pub(crate) fn partition_fork_entries(
    entries: impl IntoIterator<Item = Value>,
    session_id: &str,
) -> (Vec<Entry>, Vec<Value>) {
    let mut transcript = Vec::new();
    let mut replacements = Vec::new();
    for entry in entries {
        let Value::Object(entry) = entry else {
            continue;
        };
        let kind = entry.get("type").and_then(Value::as_str);
        if kind.is_some_and(|kind| FORK_TYPES.contains(&kind))
            && entry.get("uuid").is_some_and(Value::is_string)
        {
            transcript.push(entry);
        } else if kind == Some("content-replacement")
            && entry.get("sessionId").and_then(Value::as_str) == Some(session_id)
            && let Some(Value::Array(items)) = entry.get("replacements")
        {
            replacements.extend(items.iter().cloned());
        }
    }
    (transcript, replacements)
}

fn parse_fork_transcript(content: &str, session_id: &str) -> (Vec<Entry>, Vec<Value>) {
    let entries = content
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .filter_map(|line| serde_json::from_str::<Value>(line).ok());
    partition_fork_entries(entries, session_id)
}

fn derive_title_from_bytes(content: &[u8]) -> Option<String> {
    let head = String::from_utf8_lossy(&content[..content.len().min(LITE_READ_BUF_SIZE)]);
    let tail =
        String::from_utf8_lossy(&content[content.len().saturating_sub(LITE_READ_BUF_SIZE)..]);
    let field = |text: &str, key: &str| {
        extract_last_json_string_field(text, key).filter(|value| !value.is_empty())
    };
    field(&tail, "customTitle")
        .or_else(|| field(&head, "customTitle"))
        .or_else(|| field(&tail, "aiTitle"))
        .or_else(|| field(&head, "aiTitle"))
        .or_else(|| Some(extract_first_prompt_from_head(&head)).filter(|prompt| !prompt.is_empty()))
}

/// The title a store-backed fork derives: the last custom title, else the
/// last generated title, else the first prompt.
pub(crate) fn derive_title_from_entries(entries: &[Value]) -> Option<String> {
    let last = |key: &str| {
        entries
            .iter()
            .filter_map(|entry| entry.get(key).and_then(Value::as_str))
            .rfind(|value| !value.is_empty())
            .map(str::to_owned)
    };
    last("customTitle").or_else(|| last("aiTitle")).or_else(|| {
        Some(extract_first_prompt_from_head(&entries_to_jsonl(entries)))
            .filter(|prompt| !prompt.is_empty())
    })
}

fn no_messages(session_id: &str) -> Error {
    Error::config(format!("Session {session_id} has no messages to fork"))
}

/// The fork transform, shared by the disk and store paths.
///
/// Sidechain entries are dropped, the transcript is cut after
/// `up_to_message_id` (inclusive), every entry gets a new id, parents are
/// re-linked across the skipped `progress` entries, and `forkedFrom`
/// records the source. A `content-replacement` entry follows when the source
/// had replacements, and a `custom-title` entry ends the fork. Returns the
/// new session id and its entries.
pub(crate) fn build_fork_entries(
    transcript: Vec<Entry>,
    content_replacements: Vec<Value>,
    session_id: &str,
    up_to_message_id: Option<&str>,
    title: Option<&str>,
    derive_title: impl FnOnce() -> Option<String>,
) -> Result<(String, Vec<Entry>)> {
    let mut transcript: Vec<Entry> = transcript
        .into_iter()
        .filter(|entry| !truthy(entry.get("isSidechain")))
        .collect();
    if transcript.is_empty() {
        return Err(no_messages(session_id));
    }
    if let Some(cutoff_id) = up_to_message_id.filter(|id| !id.is_empty()) {
        let Some(cutoff) = transcript
            .iter()
            .position(|entry| entry.get("uuid").and_then(Value::as_str) == Some(cutoff_id))
        else {
            return Err(Error::config(format!(
                "Message {cutoff_id} not found in session {session_id}"
            )));
        };
        transcript.truncate(cutoff + 1);
    }
    let uuid_of = |entry: &Entry| {
        entry
            .get("uuid")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned()
    };
    let mut mapping: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    let mut by_uuid: std::collections::HashMap<String, &Entry> = std::collections::HashMap::new();
    for entry in &transcript {
        mapping.insert(uuid_of(entry), Uuid::new_v4().to_string());
        by_uuid.insert(uuid_of(entry), entry);
    }
    let writable: Vec<&Entry> = transcript
        .iter()
        .filter(|entry| entry.get("type").and_then(Value::as_str) != Some("progress"))
        .collect();
    if writable.is_empty() {
        return Err(no_messages(session_id));
    }

    let forked_session_id = Uuid::new_v4().to_string();
    let now = now_iso();
    let mut entries = Vec::with_capacity(writable.len() + 2);
    for (index, original) in writable.iter().enumerate() {
        let original_uuid = uuid_of(original);
        let mut new_parent: Option<String> = None;
        let mut parent_id = original
            .get("parentUuid")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty());
        while let Some(id) = parent_id {
            let Some(parent) = by_uuid.get(id) else {
                break;
            };
            if parent.get("type").and_then(Value::as_str) != Some("progress") {
                new_parent = mapping.get(id).cloned();
                break;
            }
            parent_id = parent
                .get("parentUuid")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty());
        }
        let timestamp = if index == writable.len() - 1 {
            Value::from(now.as_str())
        } else {
            original
                .get("timestamp")
                .cloned()
                .unwrap_or_else(|| Value::from(now.as_str()))
        };
        let logical_parent = match original.get("logicalParentUuid") {
            Some(value) if truthy(Some(value)) => value
                .as_str()
                .and_then(|id| mapping.get(id))
                .map_or(Value::Null, |id| Value::from(id.as_str())),
            Some(value) => value.clone(),
            None => Value::Null,
        };
        let mut forked = (*original).clone();
        forked.insert(
            "uuid".to_owned(),
            Value::from(mapping.get(&original_uuid).cloned().unwrap_or_default()),
        );
        forked.insert(
            "parentUuid".to_owned(),
            new_parent.map_or(Value::Null, Value::from),
        );
        forked.insert("logicalParentUuid".to_owned(), logical_parent);
        forked.insert(
            "sessionId".to_owned(),
            Value::from(forked_session_id.as_str()),
        );
        forked.insert("timestamp".to_owned(), timestamp);
        forked.insert("isSidechain".to_owned(), Value::Bool(false));
        forked.insert(
            "forkedFrom".to_owned(),
            json!({"sessionId": session_id, "messageUuid": original_uuid}),
        );
        for field in LEAKED_FIELDS {
            forked.remove(field);
        }
        entries.push(forked);
    }
    if !content_replacements.is_empty() {
        entries.push(object(json!({
            "type": "content-replacement",
            "sessionId": forked_session_id,
            "replacements": content_replacements,
            "uuid": Uuid::new_v4().to_string(),
            "timestamp": now,
        })));
    }
    let fork_title = match title.map(str::trim).filter(|title| !title.is_empty()) {
        Some(title) => title.to_owned(),
        None => format!(
            "{} (fork)",
            derive_title().unwrap_or_else(|| "Forked session".to_owned())
        ),
    };
    entries.push(object(json!({
        "type": "custom-title",
        "sessionId": forked_session_id,
        "customTitle": fork_title,
        "uuid": Uuid::new_v4().to_string(),
        "timestamp": now,
    })));
    Ok((forked_session_id, entries))
}

fn create_private(path: &Path) -> std::io::Result<std::fs::File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    options.open(path)
}

/// Forks a session into a new session beside it, with new ids for every
/// entry.
///
/// The fork copies the transcript up to and including `up_to_message_id`,
/// or all of it. Sidechain and `progress` entries are dropped, parents are
/// re-linked across the skipped entries, each entry records the message it
/// was copied from in `forkedFrom`, and undo history is not copied. The fork
/// is titled `title`, or the title of the source followed by ` (fork)`. The
/// new transcript is created exclusively, with mode `0o600` on Unix.
///
/// This function blocks on filesystem calls.
///
/// # Errors
///
/// [`Error::Config`] for an id that is not a UUID, a source with no
/// message, or an `up_to_message_id` absent from the transcript (no file is
/// created then), [`Error::SessionNotFound`] when the source is missing,
/// and [`Error::Io`] when a read or write fails.
pub fn fork_session(
    session_id: &str,
    directory: Option<&Path>,
    up_to_message_id: Option<&str>,
    title: Option<&str>,
) -> Result<ForkSessionResult> {
    let session_id = require_uuid(session_id, "session_id")?;
    if let Some(cutoff) = up_to_message_id.filter(|id| !id.is_empty()) {
        require_uuid(cutoff, "up_to_message_id")?;
    }
    let Some((path, project_dir)) = find_session_file(session_id, directory) else {
        return Err(not_found(session_id, directory));
    };
    let content = std::fs::read(&path)
        .map_err(|error| Error::io(format!("Cannot read {}", path.display()), error))?;
    if content.is_empty() {
        return Err(no_messages(session_id));
    }
    let (transcript, replacements) =
        parse_fork_transcript(&String::from_utf8_lossy(&content), session_id);
    let (forked_session_id, entries) = build_fork_entries(
        transcript,
        replacements,
        session_id,
        up_to_message_id,
        title,
        || derive_title_from_bytes(&content),
    )?;
    let fork_path = project_dir.join(format!("{forked_session_id}.jsonl"));
    let lines: Vec<String> = entries.iter().map(type_first_line).collect();
    let mut file = create_private(&fork_path)
        .map_err(|error| Error::io(format!("Cannot create {}", fork_path.display()), error))?;
    file.write_all(format!("{}\n", lines.join("\n")).as_bytes())
        .map_err(|error| Error::io(format!("Cannot write {}", fork_path.display()), error))?;
    Ok(ForkSessionResult {
        session_id: forked_session_id,
    })
}

fn store_key(session_id: &str, directory: Option<&Path>) -> SessionKey {
    SessionKey::new(project_key_for_directory(directory), session_id)
}

async fn append_via_store<S: SessionStore + ?Sized>(
    store: &S,
    key: &SessionKey,
    entries: Vec<Entry>,
) -> Result<()> {
    store
        .append(key, &entries)
        .await
        .map_err(|error| Error::session_store("Session store append failed", error))
}

/// Store-backed counterpart of [`rename_session`]: appends a
/// `custom-title` entry, with a `uuid` and a `timestamp`, under the key of
/// `directory` (by default the current directory).
///
/// # Errors
///
/// [`Error::Config`] for an id that is not a UUID or an empty title, and
/// [`Error::SessionStore`] when the store fails.
pub async fn rename_session_via_store<S: SessionStore + ?Sized>(
    store: &S,
    session_id: &str,
    title: &str,
    directory: Option<&Path>,
) -> Result<()> {
    let session_id = require_uuid(session_id, "session_id")?;
    let title = clean_title(title)?;
    let entry = object(json!({
        "type": "custom-title",
        "customTitle": title,
        "sessionId": session_id,
        "uuid": Uuid::new_v4().to_string(),
        "timestamp": now_iso(),
    }));
    append_via_store(store, &store_key(session_id, directory), vec![entry]).await
}

/// Store-backed counterpart of [`tag_session`]: appends a `tag` entry, with
/// a `uuid` and a `timestamp`. `None` clears the tag.
///
/// # Errors
///
/// [`Error::Config`] for an id that is not a UUID or a tag empty after
/// sanitizing, and [`Error::SessionStore`] when the store fails.
pub async fn tag_session_via_store<S: SessionStore + ?Sized>(
    store: &S,
    session_id: &str,
    tag: Option<&str>,
    directory: Option<&Path>,
) -> Result<()> {
    let session_id = require_uuid(session_id, "session_id")?;
    let tag = clean_tag(tag)?;
    let entry = object(json!({
        "type": "tag",
        "tag": tag,
        "sessionId": session_id,
        "uuid": Uuid::new_v4().to_string(),
        "timestamp": now_iso(),
    }));
    append_via_store(store, &store_key(session_id, directory), vec![entry]).await
}

/// Store-backed counterpart of [`delete_session`]. A store that does not
/// declare `delete` is left unchanged, as fits an append-only backend.
/// Whether subagent transcripts go too depends on the store; the contract
/// asks it to cascade.
///
/// # Errors
///
/// [`Error::Config`] for an id that is not a UUID, and
/// [`Error::SessionStore`] when the store fails.
pub async fn delete_session_via_store<S: SessionStore + ?Sized>(
    store: &S,
    session_id: &str,
    directory: Option<&Path>,
) -> Result<()> {
    let session_id = require_uuid(session_id, "session_id")?;
    if !store.capabilities().delete {
        return Ok(());
    }
    store
        .delete(&store_key(session_id, directory))
        .await
        .map_err(|error| Error::session_store("Session store delete failed", error))
}

/// Store-backed counterpart of [`fork_session`]: loads the source from the
/// store, applies the same transform, and appends the fork under a new key
/// of the same project.
///
/// # Errors
///
/// [`Error::Config`] for an invalid id, a source with no message, or an
/// absent `up_to_message_id`, [`Error::SessionNotFound`] when the store
/// holds nothing for the source, and [`Error::SessionStore`] when the store
/// fails.
pub async fn fork_session_via_store<S: SessionStore + ?Sized>(
    store: &S,
    session_id: &str,
    directory: Option<&Path>,
    up_to_message_id: Option<&str>,
    title: Option<&str>,
) -> Result<ForkSessionResult> {
    let session_id = require_uuid(session_id, "session_id")?;
    if let Some(cutoff) = up_to_message_id.filter(|id| !id.is_empty()) {
        require_uuid(cutoff, "up_to_message_id")?;
    }
    let project_key = project_key_for_directory(directory);
    let source = SessionKey::new(project_key.clone(), session_id);
    let loaded: Vec<SessionStoreEntry> = store
        .load(&source)
        .await
        .map_err(|error| Error::session_store("Session store load failed", error))?
        .unwrap_or_default();
    if loaded.is_empty() {
        return Err(Error::session_not_found(format!(
            "Session {session_id} not found"
        )));
    }
    let raw: Vec<Value> = loaded.into_iter().map(Value::Object).collect();
    let (transcript, replacements) = partition_fork_entries(raw.iter().cloned(), session_id);
    let (forked_session_id, entries) = build_fork_entries(
        transcript,
        replacements,
        session_id,
        up_to_message_id,
        title,
        || derive_title_from_entries(&raw),
    )?;
    append_via_store(
        store,
        &SessionKey::new(project_key, forked_session_id.clone()),
        entries,
    )
    .await?;
    Ok(ForkSessionResult {
        session_id: forked_session_id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizing_removes_invisible_and_private_characters() {
        assert_eq!(
            sanitize_unicode("a\u{200b}b\u{202e}c\u{feff}d\u{e000}e"),
            "abcde"
        );
        assert_eq!(sanitize_unicode("\u{ff21}\u{fb01}"), "Afi");
        assert_eq!(sanitize_unicode("plain tag"), "plain tag");
        assert_eq!(sanitize_unicode("\u{10ffff}x"), "x");
    }

    #[test]
    fn tags_and_titles_are_validated() {
        assert_eq!(clean_tag(None).unwrap(), "");
        assert_eq!(clean_tag(Some("  hi\u{200b} ")).unwrap(), "hi");
        assert!(matches!(
            clean_tag(Some("\u{200b} ")),
            Err(Error::Config { .. })
        ));
        assert!(matches!(clean_title("   "), Err(Error::Config { .. })));
        assert_eq!(clean_title(" t ").unwrap(), "t");
    }

    fn entry(value: Value) -> Entry {
        object(value)
    }

    #[test]
    fn fork_relinks_across_progress_and_drops_sidechains() {
        let source = vec![
            entry(
                json!({"type": "user", "uuid": "u1", "parentUuid": null, "timestamp": "t1", "slug": "s"}),
            ),
            entry(json!({"type": "progress", "uuid": "p1", "parentUuid": "u1"})),
            entry(
                json!({"type": "assistant", "uuid": "a1", "parentUuid": "p1", "timestamp": "t2"}),
            ),
            entry(json!({"type": "user", "uuid": "side", "parentUuid": "a1", "isSidechain": true})),
            entry(
                json!({"type": "user", "uuid": "u2", "parentUuid": "a1", "logicalParentUuid": "u1"}),
            ),
        ];
        let (id, entries) = build_fork_entries(
            source,
            vec![json!({"r": 1})],
            "src",
            Some("u2"),
            None,
            || Some("Title".to_owned()),
        )
        .unwrap();
        assert_eq!(entries.len(), 5);
        let user = &entries[0];
        let assistant = &entries[1];
        let last = &entries[2];
        assert_eq!(assistant["parentUuid"], user["uuid"]);
        assert_eq!(last["parentUuid"], assistant["uuid"]);
        assert_eq!(last["logicalParentUuid"], user["uuid"]);
        assert_eq!(user["parentUuid"], Value::Null);
        assert_eq!(user["timestamp"], "t1");
        assert_ne!(last["timestamp"], Value::Null);
        assert!(user.get("slug").is_none());
        assert_eq!(
            user["forkedFrom"],
            json!({"sessionId": "src", "messageUuid": "u1"})
        );
        assert!(
            entries[..3]
                .iter()
                .all(|entry| entry["sessionId"] == id.as_str() && entry["isSidechain"] == false)
        );
        assert_eq!(entries[3]["type"], "content-replacement");
        assert_eq!(entries[3]["replacements"], json!([{"r": 1}]));
        assert_eq!(entries[4]["customTitle"], "Title (fork)");
    }

    #[test]
    fn fork_cut_and_title_rules() {
        let source = || {
            vec![
                entry(json!({"type": "user", "uuid": "u1"})),
                entry(json!({"type": "assistant", "uuid": "a1", "parentUuid": "u1"})),
            ]
        };
        let missing = build_fork_entries(source(), vec![], "src", Some("nope"), None, || None);
        assert!(matches!(missing, Err(Error::Config { .. })));
        let (_, entries) = build_fork_entries(
            source(),
            vec![],
            "src",
            Some("u1"),
            Some("  Named "),
            || None,
        )
        .unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[1]["customTitle"], "Named");
        let (_, entries) =
            build_fork_entries(source(), vec![], "src", None, None, || None).unwrap();
        assert_eq!(entries[2]["customTitle"], "Forked session (fork)");
        let progress_only = vec![entry(json!({"type": "progress", "uuid": "p"}))];
        assert!(matches!(
            build_fork_entries(progress_only, vec![], "src", None, None, || None),
            Err(Error::Config { .. })
        ));
    }
}
