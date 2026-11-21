//! Reading sessions from a [`SessionStore`], and importing local sessions
//! into one.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use futures_util::StreamExt;
use serde_json::Value;

use super::chain::{
    entries_to_session_messages, entries_to_subagent_messages, filter_transcript_entries,
    parent_ids_from_agent_metadata, split_agent_metadata,
};
use super::disk::{
    agent_metadata_sidecar_path, read_agent_metadata_sidecar_strictly, resolve_session_file_path,
    sort_and_paginate,
};
use super::jsonl::entries_to_jsonl;
use super::lite::{LiteSessionFile, parse_session_info_from_lite};
use super::mutations::require_uuid;
use super::paths::{canonicalize_path, sanitize_path};
use super::summary::summary_entry_to_sdk_info;
use super::timestamp::{now_millis, parse_iso_millis};
use super::{
    ImportSessionOptions, ListSessionsOptions, SdkSessionInfo, SessionMessage,
    SessionMessagesOptions, validate_uuid,
};
use crate::error::{Error, Result};
use crate::session_store::{
    SessionKey, SessionListSubkeysKey, SessionStore, SessionStoreEntry, SessionStoreError,
    SessionStoreListEntry, UnsupportedStoreOperation, is_unsupported,
};

/// Most store loads a listing runs at once, so a large project does not
/// exhaust the connection pool of an adapter.
pub(crate) const STORE_LIST_LOAD_CONCURRENCY: usize = 16;

/// Entries per append of an import, and the entry threshold of the mirror.
pub(crate) const MAX_PENDING_ENTRIES: usize = 500;

/// Bytes per append of an import, and the byte threshold of the mirror.
pub(crate) const MAX_PENDING_BYTES: usize = 1_048_576;

fn project_path_of(directory: Option<&Path>) -> String {
    canonicalize_path(directory.unwrap_or(Path::new(".")))
}

fn store_error(context: &'static str) -> impl FnOnce(SessionStoreError) -> Error {
    move |error| match error.downcast::<UnsupportedStoreOperation>() {
        Ok(unsupported) => Error::from(*unsupported),
        Err(error) => Error::session_store(context, error),
    }
}

fn to_values(entries: Vec<SessionStoreEntry>) -> Vec<Value> {
    entries.into_iter().map(Value::Object).collect()
}

async fn load_as_jsonl<S: SessionStore + ?Sized>(
    store: &S,
    project_key: &str,
    session_id: &str,
) -> std::result::Result<Option<String>, SessionStoreError> {
    let entries = store
        .load(&SessionKey::new(project_key, session_id))
        .await?
        .unwrap_or_default();
    if entries.is_empty() {
        return Ok(None);
    }
    Ok(Some(entries_to_jsonl(&to_values(entries))))
}

/// The time of the last entry of a transcript, or now.
fn mtime_from_jsonl_tail(jsonl: &str) -> i64 {
    jsonl
        .trim_end()
        .rsplit('\n')
        .next()
        .and_then(|line| crate::transport::framing::parse_json(line).ok())
        .and_then(|entry| entry.get("timestamp")?.as_str().and_then(parse_iso_millis))
        .unwrap_or_else(now_millis)
}

/// Loads each listed session, at most 16 at once, and reads its metadata.
/// A load that fails gives a row with an empty summary; sidechain sessions
/// and sessions without a summary are dropped.
async fn derive_infos_via_load<S: SessionStore + ?Sized>(
    store: &S,
    listing: &[SessionStoreListEntry],
    project_key: &str,
    project_path: &str,
) -> Vec<SdkSessionInfo> {
    let loads = futures_util::stream::iter(listing.iter().map(|entry| async move {
        (
            entry,
            load_as_jsonl(store, project_key, &entry.session_id).await,
        )
    }))
    .buffered(STORE_LIST_LOAD_CONCURRENCY)
    .collect::<Vec<_>>()
    .await;
    loads
        .into_iter()
        .filter_map(|(entry, outcome)| match outcome {
            Err(error) => {
                tracing::debug!(
                    "Session store load failed for {}: {error}",
                    entry.session_id
                );
                Some(SdkSessionInfo::new(
                    entry.session_id.clone(),
                    "",
                    entry.mtime,
                ))
            }
            Ok(None) => None,
            Ok(Some(jsonl)) => {
                let lite = LiteSessionFile::from_bytes(jsonl.as_bytes(), entry.mtime);
                let mut info =
                    parse_session_info_from_lite(&entry.session_id, &lite, Some(project_path))?;
                info.last_modified = entry.mtime;
                Some(info)
            }
        })
        .collect()
}

enum Slot {
    Ready(i64, SdkSessionInfo),
    Pending(SessionStoreListEntry),
}

impl Slot {
    fn mtime(&self) -> i64 {
        match self {
            Self::Ready(mtime, _) => *mtime,
            Self::Pending(entry) => entry.mtime,
        }
    }
}

/// Lists the sessions of a project from a store, newest first, with the
/// metadata [`list_sessions`](crate::list_sessions) gives for the same
/// content.
///
/// The project is that of `directory`, by default the current directory.
/// With `list_session_summaries`, the summaries are read in one call; a
/// session listed by `list_sessions` whose summary is missing or older than
/// its listing time is loaded and read again, after pagination, so at most
/// one page of loads runs. Without summaries, every session is loaded, at
/// most 16 at once. A session whose load fails is listed with an empty
/// summary. Worktrees are a filesystem notion and are not searched:
/// `include_worktrees` has no effect here, since store keys hold no
/// worktree layout (upstream's store function has no such parameter).
///
/// # Errors
///
/// [`ErrorKind::StoreUnsupported`](crate::ErrorKind::StoreUnsupported) when the store implements neither
/// `list_session_summaries` nor `list_sessions`, and
/// [`ErrorKind::SessionStore`](crate::ErrorKind::SessionStore) when a listing call fails.
pub async fn list_sessions_from_store<S: SessionStore + ?Sized>(
    store: &S,
    options: ListSessionsOptions,
) -> Result<Vec<SdkSessionInfo>> {
    let ListSessionsOptions {
        directory,
        limit,
        offset,
        include_worktrees: _,
    } = options;
    let project_path = project_path_of(directory.as_deref());
    let project_key = sanitize_path(&project_path);
    match store.list_session_summaries(&project_key).await {
        Err(error) if is_unsupported(&error) => {}
        Err(error) => {
            return Err(Error::session_store(
                "Session store list_session_summaries failed",
                error,
            ));
        }
        Ok(summaries) => {
            let listing = match store.list_sessions(&project_key).await {
                Ok(listing) => Some(listing),
                Err(error) if is_unsupported(&error) => {
                    tracing::debug!(
                        "list_session_summaries without list_sessions: sessions without a summary are omitted"
                    );
                    None
                }
                Err(error) => {
                    return Err(Error::session_store(
                        "Session store list_sessions failed",
                        error,
                    ));
                }
            };
            let known: Option<HashMap<&str, i64>> = listing.as_ref().map(|listing| {
                listing
                    .iter()
                    .map(|entry| (entry.session_id.as_str(), entry.mtime))
                    .collect()
            });
            let mut slots = Vec::new();
            let mut fresh: HashSet<String> = HashSet::new();
            for summary in &summaries {
                if let Some(known) = &known {
                    match known.get(summary.session_id.as_str()) {
                        None => continue,
                        Some(&mtime) if summary.mtime < mtime => continue,
                        Some(_) => {}
                    }
                }
                fresh.insert(summary.session_id.clone());
                if let Some(info) = summary_entry_to_sdk_info(summary, Some(&project_path)) {
                    slots.push(Slot::Ready(summary.mtime, info));
                }
            }
            slots.extend(
                listing
                    .iter()
                    .flatten()
                    .filter(|entry| !fresh.contains(&entry.session_id))
                    .cloned()
                    .map(Slot::Pending),
            );
            slots.sort_by_key(|slot| std::cmp::Reverse(slot.mtime()));
            let page = super::chain::paginate(slots, limit, offset);
            let pending: Vec<SessionStoreListEntry> = page
                .iter()
                .filter_map(|slot| match slot {
                    Slot::Pending(entry) => Some(entry.clone()),
                    Slot::Ready(..) => None,
                })
                .collect();
            let filled: HashMap<String, SdkSessionInfo> =
                derive_infos_via_load(store, &pending, &project_key, &project_path)
                    .await
                    .into_iter()
                    .map(|info| (info.session_id.clone(), info))
                    .collect();
            return Ok(page
                .into_iter()
                .filter_map(|slot| match slot {
                    Slot::Ready(_, info) => Some(info),
                    Slot::Pending(entry) => filled.get(&entry.session_id).cloned(),
                })
                .collect());
        }
    }

    let listing = store
        .list_sessions(&project_key)
        .await
        .map_err(store_error("Session store list_sessions failed"))?;
    let infos = derive_infos_via_load(store, &listing, &project_key, &project_path).await;
    Ok(sort_and_paginate(infos, limit, offset))
}

/// Reads the metadata of one session from a store, like
/// [`get_session_info`](crate::get_session_info). The modification time is
/// that of the last entry, or now.
///
/// `None` when the id is not a UUID, the store holds nothing for it, or it
/// is a sidechain or has no summary.
///
/// # Errors
///
/// [`ErrorKind::SessionStore`](crate::ErrorKind::SessionStore) when the load fails.
pub async fn get_session_info_from_store<S: SessionStore + ?Sized>(
    store: &S,
    session_id: &str,
    directory: Option<&Path>,
) -> Result<Option<SdkSessionInfo>> {
    let Some(session_id) = validate_uuid(session_id) else {
        return Ok(None);
    };
    let project_path = project_path_of(directory);
    let Some(jsonl) = load_as_jsonl(store, &sanitize_path(&project_path), session_id)
        .await
        .map_err(store_error("Session store load failed"))?
    else {
        return Ok(None);
    };
    let lite = LiteSessionFile::from_bytes(jsonl.as_bytes(), mtime_from_jsonl_tail(&jsonl));
    Ok(parse_session_info_from_lite(
        session_id,
        &lite,
        Some(&project_path),
    ))
}

/// Reads the conversation of a session from a store, like
/// [`get_session_messages`](crate::get_session_messages).
///
/// Empty when the id is not a UUID or the store holds nothing for it.
///
/// # Errors
///
/// [`ErrorKind::SessionStore`](crate::ErrorKind::SessionStore) when the load fails.
pub async fn get_session_messages_from_store<S: SessionStore + ?Sized>(
    store: &S,
    session_id: &str,
    options: SessionMessagesOptions,
) -> Result<Vec<SessionMessage>> {
    let SessionMessagesOptions {
        directory,
        limit,
        offset,
    } = options;
    let Some(session_id) = validate_uuid(session_id) else {
        return Ok(Vec::new());
    };
    let key = SessionKey::new(
        sanitize_path(&project_path_of(directory.as_deref())),
        session_id,
    );
    let entries = store
        .load(&key)
        .await
        .map_err(store_error("Session store load failed"))?
        .unwrap_or_default();
    if entries.is_empty() {
        return Ok(Vec::new());
    }
    Ok(entries_to_session_messages(
        &filter_transcript_entries(to_values(entries)),
        limit,
        offset,
    ))
}

/// Whether an agent id stays one path segment once prefixed with
/// `agent-`. Without `list_subkeys` the id becomes part of the subpath a
/// store loads, so an id carrying a separator or NUL could name a key
/// outside the subagents of the session in an adapter that maps subpaths
/// to files. `.` and `..` are safe there: `agent-..` is one segment.
fn is_single_segment(agent_id: &str) -> bool {
    !agent_id.contains(['/', '\\', '\0'])
}

fn agent_id_of(subpath: &str) -> Option<&str> {
    if !subpath.starts_with("subagents/") {
        return None;
    }
    subpath.rsplit('/').next()?.strip_prefix("agent-")
}

/// Lists the ids of the subagents of a session from a store, like
/// [`list_subagents`](crate::list_subagents), from the subkeys under
/// `subagents/`.
///
/// # Errors
///
/// [`ErrorKind::StoreUnsupported`](crate::ErrorKind::StoreUnsupported) when the store does not implement
/// `list_subkeys`, and [`ErrorKind::SessionStore`](crate::ErrorKind::SessionStore) when it fails.
pub async fn list_subagents_from_store<S: SessionStore + ?Sized>(
    store: &S,
    session_id: &str,
    directory: Option<&Path>,
) -> Result<Vec<String>> {
    let Some(session_id) = validate_uuid(session_id) else {
        return Ok(Vec::new());
    };
    let key = SessionListSubkeysKey::new(sanitize_path(&project_path_of(directory)), session_id);
    let subkeys = store
        .list_subkeys(&key)
        .await
        .map_err(store_error("Session store list_subkeys failed"))?;
    let mut seen = HashSet::new();
    Ok(subkeys
        .iter()
        .filter_map(|subpath| agent_id_of(subpath))
        .filter(|agent_id| seen.insert(agent_id.to_owned()))
        .map(str::to_owned)
        .collect())
}

/// Reads the conversation of a subagent from a store, like
/// [`get_subagent_messages`](crate::get_subagent_messages).
///
/// The subagent is found among the subkeys when the store lists them, by
/// the last segment of each, as upstream; otherwise at
/// `subagents/agent-<id>`, where an id holding a separator or NUL loads
/// nothing. An empty id finds nothing. The parent ids come from its
/// `agent_metadata` entry, the store's copy of the `.meta.json` sidecar.
///
/// # Errors
///
/// [`ErrorKind::SessionStore`](crate::ErrorKind::SessionStore) when the store fails.
pub async fn get_subagent_messages_from_store<S: SessionStore + ?Sized>(
    store: &S,
    session_id: &str,
    agent_id: &str,
    options: SessionMessagesOptions,
) -> Result<Vec<SessionMessage>> {
    let SessionMessagesOptions {
        directory,
        limit,
        offset,
    } = options;
    let Some(session_id) = validate_uuid(session_id) else {
        return Ok(Vec::new());
    };
    if agent_id.is_empty() {
        return Ok(Vec::new());
    }
    let project_key = sanitize_path(&project_path_of(directory.as_deref()));
    let mut subpath = format!("subagents/agent-{agent_id}");
    match store
        .list_subkeys(&SessionListSubkeysKey::new(project_key.clone(), session_id))
        .await
    {
        Err(error) if is_unsupported(&error) => {
            if !is_single_segment(agent_id) {
                return Ok(Vec::new());
            }
        }
        Err(error) => {
            return Err(Error::session_store(
                "Session store list_subkeys failed",
                error,
            ));
        }
        Ok(subkeys) => {
            let target = format!("agent-{agent_id}");
            let Some(found) = subkeys.into_iter().find(|subkey| {
                subkey.starts_with("subagents/")
                    && subkey.rsplit('/').next() == Some(target.as_str())
            }) else {
                return Ok(Vec::new());
            };
            subpath = found;
        }
    }
    let key = SessionKey::new(project_key, session_id).with_subpath(subpath);
    let entries = store
        .load(&key)
        .await
        .map_err(store_error("Session store load failed"))?
        .unwrap_or_default();
    if entries.is_empty() {
        return Ok(Vec::new());
    }
    let (metadata, transcript) = split_agent_metadata(to_values(entries));
    if transcript.is_empty() {
        return Ok(Vec::new());
    }
    let (tool_use_id, parent_agent_id) = parent_ids_from_agent_metadata(metadata.as_ref());
    Ok(entries_to_subagent_messages(
        &filter_transcript_entries(transcript),
        limit,
        offset,
        tool_use_id.as_deref(),
        parent_agent_id.as_deref(),
    ))
}

fn collect_jsonl_files(base: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(base) else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).collect();
    paths.sort_by_key(|path| {
        path.file_name()
            .map(|name| name.to_string_lossy().into_owned())
    });
    let mut found = Vec::new();
    for path in paths {
        if path.is_dir() {
            found.extend(collect_jsonl_files(&path));
        } else if path.is_file()
            && path
                .extension()
                .is_some_and(|extension| extension == "jsonl")
        {
            found.push(path);
        }
    }
    found
}

async fn append_file_in_batches<S: SessionStore + ?Sized>(
    store: &S,
    path: &Path,
    key: &SessionKey,
    batch_size: usize,
) -> Result<()> {
    let cannot_read = |error| Error::io(format!("Cannot read {}", path.display()), error);
    let owned = path.to_path_buf();
    let file = crate::blocking::run(move || std::fs::File::open(owned))
        .await?
        .map_err(cannot_read)?;
    let mut reader = std::io::BufReader::new(file);
    let mut batch: Vec<SessionStoreEntry> = Vec::new();
    let mut bytes = 0;
    loop {
        let (returned, lines, done) = crate::blocking::run(move || read_line_chunk(reader)).await?;
        reader = returned;
        for line in lines.map_err(cannot_read)? {
            let line = String::from_utf8_lossy(&line);
            match crate::transport::framing::parse_json(&line) {
                Ok(Value::Object(entry)) => batch.push(entry),
                Ok(_) => {
                    tracing::warn!(
                        "Skipping a transcript line that is not a JSON object in {}",
                        path.display()
                    );
                    continue;
                }
                Err(error) => return Err(Error::json_decode(line, error)),
            }
            bytes += line.len();
            if batch.len() >= batch_size || bytes >= MAX_PENDING_BYTES {
                store
                    .append(key, &batch)
                    .await
                    .map_err(store_error("Session store append failed"))?;
                batch.clear();
                bytes = 0;
            }
        }
        if done {
            break;
        }
    }
    if !batch.is_empty() {
        store
            .append(key, &batch)
            .await
            .map_err(store_error("Session store append failed"))?;
    }
    Ok(())
}

/// Bytes of transcript read from disk at a time by an import, so a large
/// transcript is never held whole in memory.
const IMPORT_READ_CHUNK: usize = 1024 * 1024;

/// Reads the next non-empty lines of a transcript, about
/// [`IMPORT_READ_CHUNK`] bytes of them, without their `\n` or `\r\n`
/// ending. Returns the reader, the lines, and whether the end was reached.
fn read_line_chunk(
    mut reader: std::io::BufReader<std::fs::File>,
) -> (
    std::io::BufReader<std::fs::File>,
    std::io::Result<Vec<Vec<u8>>>,
    bool,
) {
    use std::io::BufRead;

    let mut lines = Vec::new();
    let mut read = 0;
    loop {
        let mut line = Vec::new();
        match reader.read_until(b'\n', &mut line) {
            Ok(0) => return (reader, Ok(lines), true),
            Ok(count) => read += count,
            Err(error) => return (reader, Err(error), true),
        }
        if line.last() == Some(&b'\n') {
            line.pop();
        }
        if line.last() == Some(&b'\r') {
            line.pop();
        }
        if !line.is_empty() {
            lines.push(line);
        }
        if read >= IMPORT_READ_CHUNK {
            return (reader, Ok(lines), false);
        }
    }
}

/// Copies a local session into a store, so it can be read or resumed from
/// the store.
///
/// The transcript is appended in batches of `batch_size` entries (500 by
/// default, and when 0) or 1 MiB of lines, whichever comes first. The key is named after the
/// project directory the transcript was found in, as the live mirror names
/// it. With `include_subagents`, every transcript under
/// `<session>/subagents/` is copied under its subpath, followed by its
/// `.meta.json` sidecar as an `agent_metadata` entry; a sidecar that is
/// missing or not a JSON object is left out, and one that cannot be read
/// fails the import. Stores should treat
/// `uuid` as an idempotency key, so importing again is safe.
///
/// The local files are found and read on the blocking pool of tokio, one
/// at a time, so the import never stalls the runtime.
///
/// # Errors
///
/// [`ErrorKind::Config`](crate::ErrorKind::Config) for an id that is not a UUID,
/// [`ErrorKind::SessionNotFound`](crate::ErrorKind::SessionNotFound) when the local transcript is missing,
/// [`ErrorKind::JsonDecode`](crate::ErrorKind::JsonDecode) for a corrupt line, [`ErrorKind::Io`](crate::ErrorKind::Io) when a read
/// fails, and [`ErrorKind::SessionStore`](crate::ErrorKind::SessionStore) when an append fails.
pub async fn import_session_to_store<S: SessionStore + ?Sized>(
    session_id: &str,
    store: &S,
    options: ImportSessionOptions,
) -> Result<()> {
    let ImportSessionOptions {
        directory,
        include_subagents,
        batch_size,
    } = options;
    let session_id = require_uuid(session_id, "session_id")?;
    let owned_id = session_id.to_owned();
    let resolved =
        crate::blocking::run(move || resolve_session_file_path(&owned_id, directory.as_deref()))
            .await?;
    let Some(transcript) = resolved else {
        return Err(Error::session_not_found(format!(
            "Session {session_id} not found"
        )));
    };
    let project_key = transcript
        .parent()
        .and_then(Path::file_name)
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let batch_size = if batch_size == 0 {
        MAX_PENDING_ENTRIES
    } else {
        batch_size
    };
    let main = SessionKey::new(project_key.clone(), session_id);
    append_file_in_batches(store, &transcript, &main, batch_size).await?;
    if !include_subagents {
        return Ok(());
    }
    let session_dir = transcript.with_extension("");
    let subagents = session_dir.join("subagents");
    let files = crate::blocking::run(move || collect_jsonl_files(&subagents)).await?;
    for file in files {
        let Ok(relative) = file.strip_prefix(&session_dir) else {
            continue;
        };
        let mut parts: Vec<String> = relative
            .components()
            .map(|component| component.as_os_str().to_string_lossy().into_owned())
            .collect();
        if let Some(last) = parts.last_mut()
            && let Some(stem) = last.strip_suffix(".jsonl")
        {
            *last = stem.to_owned();
        }
        let key = SessionKey::new(project_key.clone(), session_id).with_subpath(parts.join("/"));
        append_file_in_batches(store, &file, &key, batch_size).await?;
        let sidecar = crate::blocking::run(move || {
            read_agent_metadata_sidecar_strictly(&file).map_err(|error| {
                Error::io(
                    format!(
                        "Cannot read {}",
                        agent_metadata_sidecar_path(&file).display()
                    ),
                    error,
                )
            })
        })
        .await??;
        if let Some(mut metadata) = sidecar {
            metadata.insert("type".to_owned(), Value::from("agent_metadata"));
            store
                .append(&key, &[metadata])
                .await
                .map_err(store_error("Session store append failed"))?;
        }
    }
    Ok(())
}
