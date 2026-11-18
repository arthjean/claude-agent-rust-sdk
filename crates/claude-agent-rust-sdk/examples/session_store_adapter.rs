//! A session store adapter written from scratch, backed by JSONL files.
//!
//! The CLI writes session transcripts under `~/.claude/`. A
//! [`SessionStore`] receives a copy of every entry, so sessions can be
//! listed, read, and resumed from storage you control: a database, an
//! object store, or here a directory. Copy [`JsonlDirectoryStore`] as a
//! starting point for your own backend.
//!
//! The store keeps one file per key under its directory:
//!
//! ```text
//! <project_key>/<session_id>.jsonl             main transcript
//! <project_key>/<session_id>.summary.json      summary, for fast listings
//! <project_key>/<session_id>/<subpath>.jsonl   subagent transcripts
//! ```
//!
//! The example then
//!
//! 1. runs a query whose transcript is mirrored to the store,
//! 2. reads the session back with the `*_from_store` functions,
//! 3. resumes the session from the store: the SDK loads the transcript
//!    from the store and hands it to the CLI, so the local copy is not
//!    needed.
//!
//! Run it with `cargo run --example session_store_adapter`. The store
//! lives in the temporary directory of the system by default; pass a
//! directory to use another one:
//! `cargo run --example session_store_adapter -- path/to/store`.

use std::fs::{self, DirEntry, File, OpenOptions};
use std::io::{self, BufRead, BufReader, ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::UNIX_EPOCH;

use claude_agent_rust_sdk::{
    BoxError, ClaudeAgentOptions, Message, SessionKey, SessionListSubkeysKey, SessionStore,
    SessionStoreCapabilities, SessionStoreEntry, SessionStoreError, SessionStoreListEntry,
    SessionSummaryEntry, SystemMessage, fold_session_summary, get_session_info_from_store,
    get_session_messages_from_store, list_sessions_from_store, query,
};
use futures_util::StreamExt;

/// Suffix of a transcript file.
const TRANSCRIPT_SUFFIX: &str = ".jsonl";

/// Suffix of the summary file of a session.
const SUMMARY_SUFFIX: &str = ".summary.json";

/// [`SessionStore`] that keeps each transcript in a JSONL file under one
/// directory.
///
/// It implements every optional method of the trait. Clones share the
/// same directory and the same write lock.
#[derive(Debug, Clone)]
struct JsonlDirectoryStore {
    root: PathBuf,
    writes: Arc<Mutex<()>>,
}

/// Checks that a part of a key is safe to use as one file name, so a key
/// can never name a file outside the directory of the store.
fn segment(name: &str) -> Result<&str, SessionStoreError> {
    let is_unsafe =
        name.is_empty() || matches!(name, "." | "..") || name.contains(['/', '\\', ':', '\0']);
    if is_unsafe {
        return Err(format!("{name:?} cannot be used as a file name").into());
    }
    Ok(name)
}

/// The subpath of a key, or `None` for the key of a main transcript.
fn subpath_of(key: &SessionKey) -> Option<&str> {
    key.subpath.as_deref().filter(|subpath| !subpath.is_empty())
}

/// Turns the error of a missing file or directory into `None`.
fn if_present<T>(result: io::Result<T>) -> io::Result<Option<T>> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

/// The entries of a directory, empty when the directory does not exist.
fn entries_of(directory: &Path) -> io::Result<Vec<DirEntry>> {
    match if_present(fs::read_dir(directory))? {
        Some(entries) => entries.collect(),
        None => Ok(Vec::new()),
    }
}

/// Last modification time of a file, in milliseconds since the Unix
/// epoch.
fn modified_millis(path: &Path) -> Result<i64, SessionStoreError> {
    let modified = fs::metadata(path)?.modified()?;
    Ok(i64::try_from(
        modified.duration_since(UNIX_EPOCH)?.as_millis(),
    )?)
}

/// Collects the subpaths of the transcripts under a session directory.
fn collect_subkeys(
    directory: &Path,
    prefix: &str,
    subkeys: &mut Vec<String>,
) -> Result<(), SessionStoreError> {
    for entry in entries_of(directory)? {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if entry.file_type()?.is_dir() {
            collect_subkeys(&entry.path(), &format!("{prefix}{name}/"), subkeys)?;
        } else if let Some(stem) = name.strip_suffix(TRANSCRIPT_SUFFIX) {
            subkeys.push(format!("{prefix}{stem}"));
        }
    }
    Ok(())
}

/// Runs blocking file work off the async runtime.
async fn blocking<T, F>(work: F) -> Result<T, SessionStoreError>
where
    F: FnOnce() -> Result<T, SessionStoreError> + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(work).await?
}

impl JsonlDirectoryStore {
    fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            writes: Arc::default(),
        }
    }

    /// Path shared by the files of a session, without a suffix. It is
    /// also the directory of its subagent transcripts.
    fn session_path(
        &self,
        project_key: &str,
        session_id: &str,
    ) -> Result<PathBuf, SessionStoreError> {
        Ok(self
            .root
            .join(segment(project_key)?)
            .join(segment(session_id)?))
    }

    fn transcript_path(&self, key: &SessionKey) -> Result<PathBuf, SessionStoreError> {
        let mut path = self.session_path(&key.project_key, &key.session_id)?;
        if let Some(subpath) = subpath_of(key) {
            for part in subpath.split('/') {
                path.push(segment(part)?);
            }
        }
        path.as_mut_os_string().push(TRANSCRIPT_SUFFIX);
        Ok(path)
    }

    fn summary_path(&self, key: &SessionKey) -> Result<PathBuf, SessionStoreError> {
        let mut path = self.session_path(&key.project_key, &key.session_id)?;
        path.as_mut_os_string().push(SUMMARY_SUFFIX);
        Ok(path)
    }

    /// Appends the entries to the transcript of the key, one JSON object
    /// per line, then updates the summary of a main transcript. The lock
    /// keeps the transcript and its summary in step when appends overlap.
    fn append_entries(
        &self,
        key: &SessionKey,
        entries: &[SessionStoreEntry],
    ) -> Result<(), SessionStoreError> {
        if entries.is_empty() {
            return Ok(());
        }
        let mut lines = String::new();
        for entry in entries {
            lines.push_str(&serde_json::to_string(entry)?);
            lines.push('\n');
        }

        let path = self.transcript_path(key)?;
        let _writing = self.writes.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)?
            .write_all(lines.as_bytes())?;

        if subpath_of(key).is_none() {
            let summary_path = self.summary_path(key)?;
            let previous: Option<SessionSummaryEntry> = match if_present(fs::read(&summary_path))? {
                Some(bytes) => Some(serde_json::from_slice(&bytes)?),
                None => None,
            };
            let mut summary = fold_session_summary(previous.as_ref(), key, entries);
            summary.mtime = modified_millis(&path)?;
            fs::write(&summary_path, serde_json::to_vec(&summary)?)?;
        }
        Ok(())
    }

    fn load_entries(
        &self,
        key: &SessionKey,
    ) -> Result<Option<Vec<SessionStoreEntry>>, SessionStoreError> {
        let Some(file) = if_present(File::open(self.transcript_path(key)?))? else {
            return Ok(None);
        };
        let mut entries = Vec::new();
        for line in BufReader::new(file).lines() {
            let line = line?;
            if !line.trim().is_empty() {
                entries.push(serde_json::from_str(&line)?);
            }
        }
        Ok(Some(entries))
    }

    /// The main transcripts of a project, with the modification time of
    /// their files.
    fn sessions_of(
        &self,
        project_key: &str,
    ) -> Result<Vec<SessionStoreListEntry>, SessionStoreError> {
        let mut sessions = Vec::new();
        for entry in entries_of(&self.root.join(segment(project_key)?))? {
            let name = entry.file_name();
            let session_id = name
                .to_str()
                .and_then(|name| name.strip_suffix(TRANSCRIPT_SUFFIX));
            if let Some(session_id) = session_id
                && entry.file_type()?.is_file()
            {
                sessions.push(SessionStoreListEntry::new(
                    session_id,
                    modified_millis(&entry.path())?,
                ));
            }
        }
        Ok(sessions)
    }

    fn summaries_of(
        &self,
        project_key: &str,
    ) -> Result<Vec<SessionSummaryEntry>, SessionStoreError> {
        let mut summaries = Vec::new();
        for entry in entries_of(&self.root.join(segment(project_key)?))? {
            let is_summary = entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.ends_with(SUMMARY_SUFFIX));
            if is_summary {
                summaries.push(serde_json::from_slice(&fs::read(entry.path())?)?);
            }
        }
        Ok(summaries)
    }

    /// Removes the transcript of the key. For a main transcript, also
    /// removes its summary and every subagent transcript of the session.
    fn delete_key(&self, key: &SessionKey) -> Result<(), SessionStoreError> {
        let _writing = self.writes.lock().unwrap_or_else(PoisonError::into_inner);
        if_present(fs::remove_file(self.transcript_path(key)?))?;
        if subpath_of(key).is_none() {
            if_present(fs::remove_file(self.summary_path(key)?))?;
            if_present(fs::remove_dir_all(
                self.session_path(&key.project_key, &key.session_id)?,
            ))?;
        }
        Ok(())
    }

    fn subkeys_of(&self, key: &SessionListSubkeysKey) -> Result<Vec<String>, SessionStoreError> {
        let mut subkeys = Vec::new();
        collect_subkeys(
            &self.session_path(&key.project_key, &key.session_id)?,
            "",
            &mut subkeys,
        )?;
        Ok(subkeys)
    }
}

impl SessionStore for JsonlDirectoryStore {
    async fn append(
        &self,
        key: &SessionKey,
        entries: &[SessionStoreEntry],
    ) -> Result<(), SessionStoreError> {
        let (store, key, entries) = (self.clone(), key.clone(), entries.to_vec());
        blocking(move || store.append_entries(&key, &entries)).await
    }

    async fn load(
        &self,
        key: &SessionKey,
    ) -> Result<Option<Vec<SessionStoreEntry>>, SessionStoreError> {
        let (store, key) = (self.clone(), key.clone());
        blocking(move || store.load_entries(&key)).await
    }

    fn capabilities(&self) -> SessionStoreCapabilities {
        SessionStoreCapabilities::all()
    }

    async fn list_sessions(
        &self,
        project_key: &str,
    ) -> Result<Vec<SessionStoreListEntry>, SessionStoreError> {
        let (store, project_key) = (self.clone(), project_key.to_owned());
        blocking(move || store.sessions_of(&project_key)).await
    }

    async fn list_session_summaries(
        &self,
        project_key: &str,
    ) -> Result<Vec<SessionSummaryEntry>, SessionStoreError> {
        let (store, project_key) = (self.clone(), project_key.to_owned());
        blocking(move || store.summaries_of(&project_key)).await
    }

    async fn delete(&self, key: &SessionKey) -> Result<(), SessionStoreError> {
        let (store, key) = (self.clone(), key.clone());
        blocking(move || store.delete_key(&key)).await
    }

    async fn list_subkeys(
        &self,
        key: &SessionListSubkeysKey,
    ) -> Result<Vec<String>, SessionStoreError> {
        let (store, key) = (self.clone(), key.clone());
        blocking(move || store.subkeys_of(&key)).await
    }
}

/// Runs one query and returns the id of its session.
async fn run(prompt: &str, options: ClaudeAgentOptions) -> Result<String, BoxError> {
    println!("User: {prompt}");
    let mut session_id = None;
    let mut messages = query(prompt, options);
    while let Some(message) = messages.next().await {
        match message? {
            Message::Assistant(assistant) => {
                for block in &assistant.content {
                    if let Some(text) = block.as_text() {
                        println!("Claude: {text}");
                    }
                }
            }
            Message::System(SystemMessage::MirrorError(failure)) => {
                eprintln!("The store missed a batch of entries: {}", failure.error);
            }
            Message::Result(result) => session_id = Some(result.session_id),
            _ => {}
        }
    }
    session_id.ok_or_else(|| "the query ended without a result".into())
}

#[tokio::main]
async fn main() -> Result<(), BoxError> {
    let root = std::env::args_os().nth(1).map_or_else(
        || std::env::temp_dir().join("claude-agent-rust-sdk-session-store"),
        PathBuf::from,
    );
    let store = JsonlDirectoryStore::new(root);
    println!("Session store directory: {}\n", store.root.display());

    println!("=== A query mirrored to the store ===");
    let options = ClaudeAgentOptions::builder()
        .session_store(store.clone())
        .max_turns(1)
        .build()?;
    let session_id = run(
        "Remember this word for later: marmalade. Reply with just OK.",
        options,
    )
    .await?;
    println!("Session: {session_id}\n");

    println!("=== The session, read back from the store ===");
    let sessions = list_sessions_from_store(&store, None, Some(5), 0).await?;
    println!("Latest sessions of this project in the store:");
    for session in &sessions {
        println!("  {}  {}", session.session_id, session.summary);
    }
    if let Some(info) = get_session_info_from_store(&store, &session_id, None).await? {
        println!("First prompt: {:?}", info.first_prompt);
        println!("Working directory: {:?}", info.cwd);
    }
    let stored = get_session_messages_from_store(&store, &session_id, None, None, 0).await?;
    println!("Messages in the store: {}", stored.len());
    for message in &stored {
        println!("  {:?}: {}", message.r#type, message.message["content"]);
    }
    println!();

    println!("=== The session, resumed from the store ===");
    let options = ClaudeAgentOptions::builder()
        .session_store(store.clone())
        .resume(&session_id)
        .max_turns(1)
        .build()?;
    let resumed_session_id = run("Which word did I ask you to remember?", options).await?;
    println!("Session: {resumed_session_id}");

    let stored = get_session_messages_from_store(&store, &session_id, None, None, 0).await?;
    println!("Messages in the store after the resume: {}", stored.len());
    Ok(())
}
