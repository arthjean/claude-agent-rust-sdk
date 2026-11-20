//! Reading transcripts from the local disk.

use std::path::{Path, PathBuf};

use indexmap::IndexMap;
use serde_json::{Map, Value};

use super::chain::{
    entries_to_session_messages, entries_to_subagent_messages, paginate,
    parent_ids_from_agent_metadata, parse_transcript_entries,
};
use super::lite::{parse_session_info_from_lite, read_session_lite};
use super::paths::{
    MAX_SANITIZED_LENGTH, canonicalize_path, find_project_dir, projects_dir, sanitize_path,
    subdirectories, worktree_paths,
};
use super::{SdkSessionInfo, SessionMessage, validate_uuid};

const TRANSCRIPT_SUFFIX: &str = ".jsonl";

fn read_sessions_from_dir(project_dir: &Path, project_path: Option<&str>) -> Vec<SdkSessionInfo> {
    let Ok(entries) = std::fs::read_dir(project_dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let session_id = validate_uuid(name.strip_suffix(TRANSCRIPT_SUFFIX)?)?;
            let lite = read_session_lite(&entry.path())?;
            parse_session_info_from_lite(session_id, &lite, project_path)
        })
        .collect()
}

/// Keeps one entry per session id, the most recently modified.
pub(crate) fn deduplicate_by_session_id(sessions: Vec<SdkSessionInfo>) -> Vec<SdkSessionInfo> {
    let mut by_id: IndexMap<String, SdkSessionInfo> = IndexMap::new();
    for session in sessions {
        match by_id.get(&session.session_id) {
            Some(existing) if existing.last_modified >= session.last_modified => {}
            _ => {
                by_id.insert(session.session_id.clone(), session);
            }
        }
    }
    by_id.into_values().collect()
}

/// Sorts by modification time, newest first, then paginates.
pub(crate) fn sort_and_paginate(
    mut sessions: Vec<SdkSessionInfo>,
    limit: Option<usize>,
    offset: usize,
) -> Vec<SdkSessionInfo> {
    sessions.sort_by_key(|session| std::cmp::Reverse(session.last_modified));
    paginate(sessions, limit, offset)
}

fn fold_case(name: &str) -> String {
    if cfg!(windows) {
        name.to_lowercase()
    } else {
        name.to_owned()
    }
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn single_project(canonical: &str, limit: Option<usize>, offset: usize) -> Vec<SdkSessionInfo> {
    let Some(project_dir) = find_project_dir(canonical) else {
        return Vec::new();
    };
    sort_and_paginate(
        read_sessions_from_dir(&project_dir, Some(canonical)),
        limit,
        offset,
    )
}

fn list_sessions_for_project(
    directory: &Path,
    limit: Option<usize>,
    offset: usize,
    include_worktrees: bool,
) -> Vec<SdkSessionInfo> {
    let canonical = canonicalize_path(directory);
    let worktrees = if include_worktrees {
        worktree_paths(&canonical)
    } else {
        Vec::new()
    };
    if worktrees.len() <= 1 {
        return single_project(&canonical, limit, offset);
    }
    let Some(directories) = subdirectories(&projects_dir(None)) else {
        return single_project(&canonical, limit, offset);
    };

    let mut indexed: Vec<(String, String)> = worktrees
        .into_iter()
        .map(|worktree| {
            let prefix = fold_case(&sanitize_path(&worktree));
            (worktree, prefix)
        })
        .collect();
    indexed.sort_by_key(|item| std::cmp::Reverse(item.1.len()));

    let mut sessions = Vec::new();
    let mut seen = std::collections::HashSet::new();
    if let Some(own) = find_project_dir(&canonical) {
        seen.insert(fold_case(&file_name(&own)));
        sessions.extend(read_sessions_from_dir(&own, Some(&canonical)));
    }
    for directory in directories {
        let name = fold_case(&file_name(&directory));
        if seen.contains(&name) {
            continue;
        }
        let matched = indexed.iter().find(|(_, prefix)| {
            name == *prefix
                || (prefix.len() >= MAX_SANITIZED_LENGTH && name.starts_with(&format!("{prefix}-")))
        });
        if let Some((worktree, _)) = matched {
            sessions.extend(read_sessions_from_dir(&directory, Some(worktree)));
            seen.insert(name);
        }
    }
    sort_and_paginate(deduplicate_by_session_id(sessions), limit, offset)
}

fn list_all_sessions(limit: Option<usize>, offset: usize) -> Vec<SdkSessionInfo> {
    let Some(directories) = subdirectories(&projects_dir(None)) else {
        return Vec::new();
    };
    let sessions = directories
        .iter()
        .flat_map(|directory| read_sessions_from_dir(directory, None))
        .collect();
    sort_and_paginate(deduplicate_by_session_id(sessions), limit, offset)
}

/// Lists sessions with the metadata read from the first and last 64 KiB of
/// each transcript.
///
/// With a `directory`, lists the sessions of that project, and with
/// `include_worktrees` of every git worktree of its repository, found
/// through `git worktree list` (an unavailable `git` lists the project
/// alone). Without one, lists the sessions of every project. Results are
/// sorted by modification time, newest first; `offset` entries are skipped,
/// then at most `limit` are kept when it is positive. Sidechain transcripts
/// and transcripts without a title, summary, or prompt are left out.
///
/// The config directory is `CLAUDE_CONFIG_DIR` when set, otherwise
/// `~/.claude`.
///
/// This function blocks on filesystem reads and on `git`: from async code,
/// call it inside `tokio::task::spawn_blocking`.
///
/// ```no_run
/// let recent = claude_agent_rust_sdk::list_sessions(Some("/path/to/project".as_ref()), Some(20), 0, true);
/// for session in recent {
///     println!("{} {}", session.session_id, session.summary);
/// }
/// ```
#[must_use]
pub fn list_sessions(
    directory: Option<&Path>,
    limit: Option<usize>,
    offset: usize,
    include_worktrees: bool,
) -> Vec<SdkSessionInfo> {
    match directory {
        Some(directory) if !directory.as_os_str().is_empty() => {
            list_sessions_for_project(directory, limit, offset, include_worktrees)
        }
        _ => list_all_sessions(limit, offset),
    }
}

/// The project directories a session of `directory` may live in: its own,
/// then those of the other git worktrees. Without a directory, every
/// project directory.
///
/// The worktrees are listed only when the caller looks past its own
/// directory, so a session found there costs no `git` call.
pub(crate) fn candidate_project_dirs(
    directory: Option<&Path>,
) -> Box<dyn Iterator<Item = (PathBuf, Option<String>)>> {
    match directory {
        Some(directory) if !directory.as_os_str().is_empty() => {
            let canonical = canonicalize_path(directory);
            let own = find_project_dir(&canonical).map(|own| (own, Some(canonical.clone())));
            let worktrees = std::iter::once(canonical).flat_map(|canonical| {
                worktree_paths(&canonical)
                    .into_iter()
                    .filter(move |worktree| *worktree != canonical)
                    .filter_map(|worktree| {
                        find_project_dir(&worktree).map(|project_dir| (project_dir, Some(worktree)))
                    })
            });
            Box::new(own.into_iter().chain(worktrees))
        }
        _ => Box::new(
            subdirectories(&projects_dir(None))
                .unwrap_or_default()
                .into_iter()
                .map(|directory| (directory, None)),
        ),
    }
}

/// Reads the metadata of one session, or `None` when the id is not a UUID,
/// the transcript is not found, or it is a sidechain or has no summary.
///
/// `directory` has the meaning it has for [`list_sessions`], worktrees
/// included; without it every project is searched.
///
/// This function blocks on filesystem reads and on `git`.
#[must_use]
pub fn get_session_info(session_id: &str, directory: Option<&Path>) -> Option<SdkSessionInfo> {
    let session_id = validate_uuid(session_id)?;
    let file_name = format!("{session_id}{TRANSCRIPT_SUFFIX}");
    candidate_project_dirs(directory)
        .into_iter()
        .find_map(|(project_dir, project_path)| {
            let lite = read_session_lite(&project_dir.join(&file_name))?;
            Some(parse_session_info_from_lite(
                session_id,
                &lite,
                project_path.as_deref(),
            ))
        })
        .flatten()
}

fn read_session_file(session_id: &str, directory: Option<&Path>) -> Option<String> {
    let file_name = format!("{session_id}{TRANSCRIPT_SUFFIX}");
    candidate_project_dirs(directory)
        .into_iter()
        .find_map(|(project_dir, _)| {
            let bytes = std::fs::read(project_dir.join(&file_name)).ok()?;
            (!bytes.is_empty()).then(|| String::from_utf8_lossy(&bytes).into_owned())
        })
}

/// The path of a non-empty session transcript.
pub(crate) fn resolve_session_file_path(
    session_id: &str,
    directory: Option<&Path>,
) -> Option<PathBuf> {
    let file_name = format!("{session_id}{TRANSCRIPT_SUFFIX}");
    candidate_project_dirs(directory)
        .map(|(project_dir, _)| project_dir.join(&file_name))
        .find(|candidate| std::fs::metadata(candidate).is_ok_and(|metadata| metadata.len() > 0))
}

/// Reads the conversation of a session from its transcript.
///
/// The chain is rebuilt from the `parentUuid` links, and the user and
/// assistant messages on it are returned oldest first, with every tool call
/// and result of a turn that ran tools in parallel. Meta, sidechain, and
/// team entries are left out; compact summaries are kept. Corrupt lines are
/// skipped. `offset` messages are skipped, then at most `limit` are kept
/// when it is positive.
///
/// Empty when the id is not a UUID or the transcript is missing. This
/// function blocks on filesystem reads.
#[must_use]
pub fn get_session_messages(
    session_id: &str,
    directory: Option<&Path>,
    limit: Option<usize>,
    offset: usize,
) -> Vec<SessionMessage> {
    let Some(session_id) = validate_uuid(session_id) else {
        return Vec::new();
    };
    let Some(content) = read_session_file(session_id, directory) else {
        return Vec::new();
    };
    entries_to_session_messages(&parse_transcript_entries(&content), limit, offset)
}

fn subagents_dir(session_id: &str, directory: Option<&Path>) -> Option<PathBuf> {
    let transcript = resolve_session_file_path(session_id, directory)?;
    Some(transcript.with_extension("").join("subagents"))
}

/// The `agent-<id>.jsonl` files under a directory, recursively, sorted by
/// name at each level.
pub(crate) fn collect_agent_files(base: &Path) -> Vec<(String, PathBuf)> {
    fn walk(directory: &Path, found: &mut Vec<(String, PathBuf)>) {
        let Ok(entries) = std::fs::read_dir(directory) else {
            return;
        };
        let mut paths: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).collect();
        paths.sort_by_key(|path| file_name(path));
        for path in paths {
            let name = file_name(&path);
            let agent_id = name
                .strip_prefix("agent-")
                .and_then(|rest| rest.strip_suffix(TRANSCRIPT_SUFFIX));
            match agent_id {
                Some(agent_id) if path.is_file() => found.push((agent_id.to_owned(), path)),
                _ if path.is_dir() => walk(&path, found),
                _ => {}
            }
        }
    }
    let mut found = Vec::new();
    walk(base, &mut found);
    found
}

/// The `agent-<id>.meta.json` sidecar beside a subagent transcript.
pub(crate) fn agent_metadata_sidecar_path(transcript: &Path) -> PathBuf {
    let name = file_name(transcript);
    let stem = name.strip_suffix(TRANSCRIPT_SUFFIX).unwrap_or(&name);
    transcript.with_file_name(format!("{stem}.meta.json"))
}

/// Reads the sidecar of a subagent transcript. A missing, unreadable, or
/// invalid sidecar, or one that is not a JSON object, is absent.
pub(crate) fn read_agent_metadata_sidecar(transcript: &Path) -> Option<Map<String, Value>> {
    read_agent_metadata_sidecar_strictly(transcript)
        .ok()
        .flatten()
}

/// Reads the sidecar of a subagent transcript as upstream does on import:
/// a missing sidecar, or one that is not valid UTF-8, not valid JSON, or not
/// a JSON object, is absent; any other read failure, such as a denied
/// permission, is returned.
pub(crate) fn read_agent_metadata_sidecar_strictly(
    transcript: &Path,
) -> std::io::Result<Option<Map<String, Value>>> {
    let text = match std::fs::read(agent_metadata_sidecar_path(transcript)) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    Ok(match serde_json::from_slice(&text) {
        Ok(Value::Object(metadata)) => Some(metadata),
        _ => None,
    })
}

/// Lists the ids of the subagents of a session, from the
/// `<session>/subagents/` directory beside its transcript and its
/// subdirectories.
///
/// Empty when the id is not a UUID, the session is not found, or it has no
/// subagents. This function blocks on filesystem reads.
#[must_use]
pub fn list_subagents(session_id: &str, directory: Option<&Path>) -> Vec<String> {
    let Some(session_id) = validate_uuid(session_id) else {
        return Vec::new();
    };
    let Some(subagents) = subagents_dir(session_id, directory) else {
        return Vec::new();
    };
    collect_agent_files(&subagents)
        .into_iter()
        .map(|(agent_id, _)| agent_id)
        .collect()
}

/// Reads the conversation of a subagent, oldest first.
///
/// Each message carries the id of the `Agent` tool call that spawned the
/// subagent, and for a nested subagent the id of its parent subagent, read
/// from the `.meta.json` sidecar of the transcript; both are `None` when
/// the sidecar is missing or unusable. Pagination is that of
/// [`get_session_messages`].
///
/// Empty when the session or the subagent is not found. This function
/// blocks on filesystem reads.
#[must_use]
pub fn get_subagent_messages(
    session_id: &str,
    agent_id: &str,
    directory: Option<&Path>,
    limit: Option<usize>,
    offset: usize,
) -> Vec<SessionMessage> {
    let Some(session_id) = validate_uuid(session_id) else {
        return Vec::new();
    };
    if agent_id.is_empty() {
        return Vec::new();
    }
    let Some(subagents) = subagents_dir(session_id, directory) else {
        return Vec::new();
    };
    let Some((_, transcript)) = collect_agent_files(&subagents)
        .into_iter()
        .find(|(found, _)| found == agent_id)
    else {
        return Vec::new();
    };
    let Ok(bytes) = std::fs::read(&transcript) else {
        return Vec::new();
    };
    if bytes.is_empty() {
        return Vec::new();
    }
    let metadata = read_agent_metadata_sidecar(&transcript);
    let (tool_use_id, parent_agent_id) = parent_ids_from_agent_metadata(metadata.as_ref());
    entries_to_subagent_messages(
        &parse_transcript_entries(&String::from_utf8_lossy(&bytes)),
        limit,
        offset,
        tool_use_id.as_deref(),
        parent_agent_id.as_deref(),
    )
}
