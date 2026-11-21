//! Resuming a session held in a [`SessionStore`].
//!
//! The CLI resumes only from a local transcript. With a store and `resume`
//! or `continue_conversation`, the session is loaded from the store and
//! written to a temporary directory laid out like `~/.claude/`; the child
//! runs with `CLAUDE_CONFIG_DIR` pointing there. The authentication and
//! user settings files are copied so the child can log in, without the
//! OAuth refresh token: a child refreshing it would revoke the tokens of
//! the parent. The directory is removed after the child exits, and on any
//! failure or cancellation.
//!
//! As upstream, credentials that exist but cannot be written to the
//! temporary directory fail the resume before the child starts, since a
//! child without them would only fail later on authentication. The other
//! seed files (`.claude.json` and the settings) are best effort: one that
//! cannot be read or written is skipped with a warning.
//!
//! Every file operation and the Keychain read run on the blocking pool, so
//! a resume never stalls the runtime. The directory moves into each
//! blocking job and comes back with its result: a resume cancelled while a
//! job runs removes the directory once that job is done, never under it.
//! One removal is synchronous: a resume future dropped between two jobs
//! removes the directory in `Drop`, on the thread that drops it, because a
//! job queued at that point may never run if the runtime is shutting down,
//! and the directory holds a copy of the credentials. Upstream removes it
//! with a synchronous `shutil.rmtree` on the event loop in every case.

use crate::json_map::RemoveKey;
use std::ffi::OsString;
use std::fs::OpenOptions;
use std::future::Future;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use indexmap::IndexMap;
use serde_json::Value;

use crate::error::{Error, Result};
use crate::session_store::{
    SessionKey, SessionListSubkeysKey, SessionStoreEntry, SessionStoreError, SessionStoreListEntry,
    SharedSessionStore, is_unsupported,
};
use crate::sessions::{
    agent_metadata_sidecar_path, plain_line, project_key_for_directory, split_agent_metadata,
    validate_uuid,
};
use crate::types::ClaudeAgentOptions;

const CONFIG_DIR_VARIABLE: &str = "CLAUDE_CONFIG_DIR";
const TEMPORARY_PREFIX: &str = "claude-resume-";
const CLEANUP_ATTEMPTS: usize = 4;
const CLEANUP_DELAY: Duration = Duration::from_millis(100);
const SETTINGS_FILES: [&str; 2] = ["settings.json", "cowork_settings.json"];
const STRIPPED_SETTINGS: [&str; 2] = ["enabledPlugins", "extraKnownMarketplaces"];
#[cfg(target_os = "macos")]
const KEYCHAIN_SERVICE_NAME: &str = "Claude Code-credentials";

/// The temporary config directory of a resume. Dropping it removes the
/// directory, so a cancelled resume leaves nothing behind; [`remove`]
/// does the same with retries.
///
/// [`remove`]: Self::remove
#[derive(Debug)]
pub(crate) struct ResumeDir {
    path: Option<PathBuf>,
}

impl ResumeDir {
    /// Creates the directory, readable only by its owner on Unix as
    /// `tempfile.mkdtemp` makes it upstream: it holds the credentials.
    fn create() -> Result<Self> {
        let mut builder = tempfile::Builder::new();
        builder.prefix(TEMPORARY_PREFIX);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            builder.permissions(std::fs::Permissions::from_mode(0o700));
        }
        let directory = builder
            .tempdir()
            .map_err(|error| Error::io("Cannot create the resume directory", error))?;
        Ok(Self {
            path: Some(directory.keep()),
        })
    }

    pub(crate) fn path(&self) -> &Path {
        self.path.as_deref().unwrap_or(Path::new(""))
    }

    /// Removes the directory, retrying 4 times 100 ms apart while a handle
    /// is still held (antivirus and indexers on Windows), then once more
    /// ignoring errors. It never fails, and runs on the blocking pool, so
    /// it finishes even when this future is dropped.
    pub(crate) async fn remove(self) {
        let _ = crate::blocking::run(move || self.remove_blocking()).await;
    }

    fn remove_blocking(mut self) {
        let Some(path) = self.path.take() else {
            return;
        };
        for _ in 0..CLEANUP_ATTEMPTS {
            match std::fs::remove_dir_all(&path) {
                Ok(()) => return,
                Err(error) if error.kind() == ErrorKind::NotFound => return,
                Err(error) if is_transient(&error) => {}
                Err(_) => break,
            }
            std::thread::sleep(CLEANUP_DELAY);
        }
        let _ = std::fs::remove_dir_all(&path);
    }
}

/// Runs file work on the directory on the blocking pool, with the
/// directory moved in. The directory comes back with the result when the
/// work succeeds; when it fails, the directory is removed and the error
/// returned. A caller dropped meanwhile leaves the work to finish, and the
/// directory is removed after it.
async fn in_directory<T: Send + 'static>(
    directory: ResumeDir,
    work: impl FnOnce(&Path) -> Result<T> + Send + 'static,
) -> Result<(ResumeDir, T)> {
    crate::blocking::run(move || match work(directory.path()) {
        Ok(value) => Ok((directory, value)),
        Err(error) => {
            directory.remove_blocking();
            Err(error)
        }
    })
    .await?
}

/// The value of `result` with the directory, or its error once the
/// directory is removed.
async fn or_remove<T>(directory: ResumeDir, result: Result<T>) -> Result<(ResumeDir, T)> {
    match result {
        Ok(value) => Ok((directory, value)),
        Err(error) => {
            directory.remove().await;
            Err(error)
        }
    }
}

/// The removal of a directory that was not removed with
/// [`ResumeDir::remove`]: synchronous, so the copy of the credentials goes
/// away even when the runtime is shutting down (see the module docs).
impl Drop for ResumeDir {
    fn drop(&mut self) {
        if let Some(path) = &self.path {
            let _ = std::fs::remove_dir_all(path);
        }
    }
}

fn is_transient(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        ErrorKind::PermissionDenied | ErrorKind::ResourceBusy | ErrorKind::DirectoryNotEmpty
    ) || matches!(error.raw_os_error(), Some(code) if is_transient_code(code))
}

#[cfg(unix)]
fn is_transient_code(code: i32) -> bool {
    [
        libc::EBUSY,
        libc::EMFILE,
        libc::ENFILE,
        libc::ENOTEMPTY,
        libc::EPERM,
        libc::EACCES,
    ]
    .contains(&code)
}

#[cfg(windows)]
fn is_transient_code(code: i32) -> bool {
    const ERROR_ACCESS_DENIED: i32 = 5;
    const ERROR_SHARING_VIOLATION: i32 = 32;
    const ERROR_LOCK_VIOLATION: i32 = 33;
    const ERROR_DIR_NOT_EMPTY: i32 = 145;
    [
        ERROR_ACCESS_DENIED,
        ERROR_SHARING_VIOLATION,
        ERROR_LOCK_VIOLATION,
        ERROR_DIR_NOT_EMPTY,
    ]
    .contains(&code)
}

#[cfg(not(any(unix, windows)))]
fn is_transient_code(_code: i32) -> bool {
    false
}

/// A session written to a temporary config directory.
#[derive(Debug)]
pub(crate) struct MaterializedResume {
    pub(crate) directory: ResumeDir,
    pub(crate) session_id: String,
}

impl MaterializedResume {
    /// The options pointed at the temporary directory: `CLAUDE_CONFIG_DIR`
    /// set to it, `resume` set to the session, and `continue` off.
    pub(crate) fn apply(&self, options: &ClaudeAgentOptions) -> ClaudeAgentOptions {
        let mut applied = options.clone();
        applied.env.insert(
            CONFIG_DIR_VARIABLE.to_owned(),
            self.directory.path().to_string_lossy().into_owned(),
        );
        applied.resume = Some(self.session_id.clone());
        applied.continue_conversation = false;
        applied
    }
}

fn timeout_error(what: &str, limit: Duration) -> Error {
    Error::session_store_timeout(format!(
        "Session store {what} timed out after {} ms during resume materialization",
        limit.as_millis()
    ))
}

async fn bounded<T>(
    what: &str,
    limit: Duration,
    call: impl Future<Output = std::result::Result<T, SessionStoreError>>,
) -> Result<T> {
    match tokio::time::timeout(limit, call).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(error)) => Err(Error::session_store(
            format!("Session store {what} failed during resume materialization"),
            error,
        )),
        Err(_) => Err(timeout_error(what, limit)),
    }
}

async fn load_candidate(
    store: &SharedSessionStore,
    project_key: &str,
    session_id: &str,
    limit: Duration,
) -> Result<Option<Vec<SessionStoreEntry>>> {
    let entries = bounded(
        &format!("load for session {session_id}"),
        limit,
        store.load(&SessionKey::new(project_key, session_id)),
    )
    .await?;
    Ok(entries.filter(|entries| !entries.is_empty()))
}

/// The error of `continue_conversation` with a store that does not
/// implement `list_sessions`, upstream's text.
fn continue_needs_list_sessions() -> Error {
    Error::config(
        "continue_conversation with session_store requires the store to implement \
         list_sessions()",
    )
}

/// The sessions of a project for `continue_conversation`. A store that
/// answers [`UnsupportedStoreOperation`](crate::UnsupportedStoreOperation)
/// is refused with the `Config` error upstream raises when the store does
/// not override `list_sessions`: the call is the Rust way to ask, since a
/// trait cannot tell whether a method is overridden.
async fn sessions_to_continue(
    store: &SharedSessionStore,
    project_key: &str,
    limit: Duration,
) -> Result<Vec<SessionStoreListEntry>> {
    match tokio::time::timeout(limit, store.list_sessions(project_key)).await {
        Ok(Err(error)) if is_unsupported(&error) => Err(continue_needs_list_sessions()),
        Ok(Ok(sessions)) => Ok(sessions),
        Ok(Err(error)) => Err(Error::session_store(
            "Session store list_sessions failed during resume materialization",
            error,
        )),
        Err(_) => Err(timeout_error("list_sessions", limit)),
    }
}

/// Refuses `continue_conversation` with a store that does not implement
/// `list_sessions`, for a session whose transport is custom: nothing is
/// materialized then, but upstream validates the options at connect all
/// the same.
pub(crate) async fn check_continue_store(options: &ClaudeAgentOptions) -> Result<()> {
    let Some(store) = &options.session_store else {
        return Ok(());
    };
    if !options.continue_conversation || options.resume.is_some() {
        return Ok(());
    }
    let limit = Duration::from_millis(options.load_timeout_ms);
    let project_key = project_key_for_directory(options.cwd.as_deref());
    sessions_to_continue(store, &project_key, limit)
        .await
        .map(drop)
}

/// The newest session whose first entry is not a sidechain: sidechains are
/// mirrored as top-level keys and are often the newest.
async fn continue_candidate(
    store: &SharedSessionStore,
    project_key: &str,
    limit: Duration,
) -> Result<Option<(String, Vec<SessionStoreEntry>)>> {
    let mut sessions = sessions_to_continue(store, project_key, limit).await?;
    sessions.sort_by_key(|session| std::cmp::Reverse(session.mtime));
    for candidate in sessions {
        if validate_uuid(&candidate.session_id).is_none() {
            continue;
        }
        let Some(entries) =
            load_candidate(store, project_key, &candidate.session_id, limit).await?
        else {
            continue;
        };
        if entries
            .first()
            .is_some_and(|first| first.get("isSidechain") == Some(&Value::Bool(true)))
        {
            continue;
        }
        return Ok(Some((candidate.session_id, entries)));
    }
    Ok(None)
}

/// Loads the session to resume from the store of the options and writes it
/// to a temporary config directory.
///
/// `None` when there is nothing to materialize: no store, neither `resume`
/// nor `continue_conversation`, a `resume` that is not a UUID (the CLI
/// receives it unchanged), or nothing stored. With `continue_conversation`
/// that means a fresh session.
pub(crate) async fn materialize_resume_session(
    options: &ClaudeAgentOptions,
) -> Result<Option<MaterializedResume>> {
    let Some(store) = &options.session_store else {
        return Ok(None);
    };
    if options.resume.is_none() && !options.continue_conversation {
        return Ok(None);
    }
    let limit = Duration::from_millis(options.load_timeout_ms);
    let project_key = project_key_for_directory(options.cwd.as_deref());
    let resolved = match &options.resume {
        Some(resume) => {
            if validate_uuid(resume).is_none() {
                return Ok(None);
            }
            load_candidate(store, &project_key, resume, limit)
                .await?
                .map(|entries| (resume.clone(), entries))
        }
        None => continue_candidate(store, &project_key, limit).await?,
    };
    let Some((session_id, entries)) = resolved else {
        return Ok(None);
    };

    let directory = crate::blocking::run(ResumeDir::create).await??;
    let project_dir = directory.path().join("projects").join(&project_key);
    let transcript = project_dir.join(format!("{session_id}.jsonl"));
    let env = options.env.clone();
    let (directory, skipped) = in_directory(directory, move |root| {
        write_jsonl(&transcript, &entries)?;
        copy_auth_files(root, &env)
    })
    .await?;
    for warning in skipped {
        tracing::warn!("{warning}");
    }
    let directory = materialize_subkeys(
        store,
        directory,
        &project_dir,
        &project_key,
        &session_id,
        limit,
    )
    .await?;
    Ok(Some(MaterializedResume {
        directory,
        session_id,
    }))
}

fn create_dir(path: &Path) -> Result<()> {
    std::fs::create_dir_all(path)
        .map_err(|error| Error::io(format!("Cannot create {}", path.display()), error))
}

/// Writes a new file readable only by its owner on Unix.
fn write_private(path: &Path, content: &[u8]) -> std::io::Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut file = options.open(path)?;
    file.write_all(content)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

fn write_jsonl(path: &Path, entries: &[SessionStoreEntry]) -> Result<()> {
    if let Some(parent) = path.parent() {
        create_dir(parent)?;
    }
    let mut text = String::new();
    for entry in entries {
        text.push_str(&plain_line(entry));
        text.push('\n');
    }
    write_private(path, text.as_bytes())
        .map_err(|error| Error::io(format!("Cannot write {}", path.display()), error))
}

/// Whether `name` is set to a value that is not empty, in the options or
/// else in this process, as upstream tests `opt_env.get(name) or
/// os.environ.get(name)`. A value that is not UTF-8 counts.
fn env_is_set(env: &IndexMap<String, String>, name: &str) -> bool {
    env.get(name).is_some_and(|value| !value.is_empty())
        || std::env::var_os(name).is_some_and(|value| !value.is_empty())
}

/// The config directory of the caller, as upstream reads it: the value
/// in the options when it is not empty, otherwise the variable of this
/// process, kept even when empty and byte for byte when it is not UTF-8.
fn caller_config_dir(env: &IndexMap<String, String>) -> Option<OsString> {
    caller_config_dir_from(env, std::env::var_os(CONFIG_DIR_VARIABLE))
}

/// [`caller_config_dir`] from the value of the variable in this process.
fn caller_config_dir_from(
    env: &IndexMap<String, String>,
    process: Option<OsString>,
) -> Option<OsString> {
    env.get(CONFIG_DIR_VARIABLE)
        .filter(|value| !value.is_empty())
        .map(OsString::from)
        .or(process)
}

/// Whether the credentials are read from the macOS Keychain: only when
/// no config directory is set anywhere, even an empty one, and no
/// variable authenticates the child, as upstream tests
/// `caller_config_dir is None`.
fn consults_keychain(caller: Option<&OsString>, env_auth: bool) -> bool {
    caller.is_none() && !env_auth
}

/// Reads a regular file. A missing file is absent; any other reason it
/// cannot be read is added to `skipped` and treated as absent, since these
/// files only enrich the temporary directory.
fn read_if_present(path: &Path, skipped: &mut Vec<String>) -> Option<Vec<u8>> {
    let mut skip = |reason: String| {
        skipped.push(format!(
            "[SessionStore] resume: skipping {} ({reason})",
            path.display()
        ));
    };
    match std::fs::metadata(path) {
        Err(error) if error.kind() == ErrorKind::NotFound => return None,
        Err(error) => {
            skip(error.to_string());
            return None;
        }
        Ok(metadata) if !metadata.is_file() => {
            skip("not a regular file".to_owned());
            return None;
        }
        Ok(_) => {}
    }
    match std::fs::read(path) {
        Ok(content) => Some(content),
        Err(error) => {
            skip(error.to_string());
            None
        }
    }
}

fn copy_if_present(
    source: &Path,
    target: &Path,
    transform: impl FnOnce(Vec<u8>) -> Vec<u8>,
    skipped: &mut Vec<String>,
) {
    let Some(content) = read_if_present(source, skipped) else {
        return;
    };
    if let Err(error) = write_private(target, &transform(content)) {
        let _ = std::fs::remove_file(target);
        skipped.push(format!(
            "[SessionStore] resume: skipping {} ({error})",
            source.display()
        ));
    }
}

/// Removes the settings that misbehave under a redirected config
/// directory: plugin declarations, which would reinstall every marketplace
/// into the empty plugin cache, and `env.CLAUDE_CONFIG_DIR`. Content that is
/// not a JSON object is kept as is.
pub(crate) fn strip_settings_for_resume(content: Vec<u8>) -> Vec<u8> {
    let text = content.strip_prefix(b"\xef\xbb\xbf").unwrap_or(&content);
    let Ok(Value::Object(mut settings)) = serde_json::from_slice::<Value>(text) else {
        return content;
    };
    let mut stripped = false;
    for key in STRIPPED_SETTINGS {
        stripped |= settings.remove_key(key).is_some();
    }
    if let Some(Value::Object(env)) = settings.get_mut("env") {
        stripped |= env.remove_key(CONFIG_DIR_VARIABLE).is_some();
    }
    if !stripped {
        return content;
    }
    serde_json::to_vec(&Value::Object(settings)).unwrap_or(content)
}

/// The credentials with `claudeAiOauth.refreshToken` removed. Content that
/// does not parse is kept as is; the child would not parse it either.
pub(crate) fn redact_credentials(credentials: &str) -> String {
    let Ok(mut data) = serde_json::from_str::<Value>(credentials) else {
        return credentials.to_owned();
    };
    let removed = data
        .get_mut("claudeAiOauth")
        .and_then(Value::as_object_mut)
        .and_then(|oauth| oauth.remove_key("refreshToken"))
        .is_some();
    if !removed {
        return credentials.to_owned();
    }
    data.to_string()
}

/// Copies what the child needs to authenticate and the user settings,
/// from where the CLI reads them: `.credentials.json`, `settings.json`,
/// and `cowork_settings.json` in the config directory, `.claude.json` in
/// the config directory when one is set and in the home directory
/// otherwise.
///
/// # Errors
///
/// Returns [`ErrorKind::Io`](crate::ErrorKind::Io) naming the credentials file when credentials were
/// found but cannot be written. A seed file that cannot be copied is only
/// reported in the returned warnings, which the caller logs: this runs on
/// the blocking pool, away from the task, and the span, of the caller.
fn copy_auth_files(target: &Path, env: &IndexMap<String, String>) -> Result<Vec<String>> {
    let mut skipped = Vec::new();
    let caller = caller_config_dir(env);
    let caller_config_dir = caller
        .clone()
        .filter(|directory| !directory.is_empty())
        .map(PathBuf::from);
    let home = || {
        std::env::home_dir().ok_or_else(|| {
            Error::spawn(
                "Cannot resume from the session store: CLAUDE_CONFIG_DIR is not set and the \
                 home directory cannot be found",
            )
        })
    };
    let source = match &caller_config_dir {
        Some(directory) => directory.clone(),
        None => home()?.join(".claude"),
    };

    let mut credentials = read_if_present(&source.join(".credentials.json"), &mut skipped)
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned());
    let env_auth =
        env_is_set(env, "ANTHROPIC_API_KEY") || env_is_set(env, "CLAUDE_CODE_OAUTH_TOKEN");
    if consults_keychain(caller.as_ref(), env_auth)
        && let Some(keychain) = read_keychain_credentials()
    {
        credentials = Some(keychain);
    }
    if let Some(credentials) = credentials {
        let target = target.join(".credentials.json");
        write_credentials(
            &source,
            &target,
            redact_credentials(&credentials).as_bytes(),
        )
        .map_err(|error| {
            Error::io(
                format!(
                    "Cannot write the credentials file {} for the resumed session",
                    target.display()
                ),
                error,
            )
        })?;
    }

    let claude_json = match &caller_config_dir {
        Some(directory) => directory.join(".claude.json"),
        None => home()?.join(".claude.json"),
    };
    copy_if_present(
        &claude_json,
        &target.join(".claude.json"),
        |content| content,
        &mut skipped,
    );
    for name in SETTINGS_FILES {
        copy_if_present(
            &source.join(name),
            &target.join(name),
            strip_settings_for_resume,
            &mut skipped,
        );
    }
    Ok(skipped)
}

/// Writes the redacted credentials read from the config directory
/// `source`. Unit tests can make the write fail, as a full disk or a
/// denied permission would.
fn write_credentials(source: &Path, path: &Path, content: &[u8]) -> std::io::Result<()> {
    #[cfg(test)]
    if let Some(error) = tests::credentials_write_fault(source, path) {
        return Err(error);
    }
    #[cfg(not(test))]
    let _ = source;
    write_private(path, content)
}

/// The OAuth credentials the CLI keeps in the macOS Keychain by default.
#[cfg(target_os = "macos")]
fn read_keychain_credentials() -> Option<String> {
    let user = keychain_account(
        |name| std::env::var(name).ok(),
        crate::process::sys::current_user_name,
    );
    let mut command = std::process::Command::new("security");
    command.args([
        "find-generic-password",
        "-a",
        &user,
        "-w",
        "-s",
        KEYCHAIN_SERVICE_NAME,
    ]);
    crate::sessions::run_with_limit(command, Duration::from_secs(5))
        .map(|output| output.trim().to_owned())
        .filter(|output| !output.is_empty())
}

#[cfg(not(target_os = "macos"))]
fn read_keychain_credentials() -> Option<String> {
    None
}

/// The Keychain account the CLI stores its credentials under: the user
/// this process runs as, found as upstream finds it (`USER`, then what
/// Python's `getpass.getuser` reads: `LOGNAME`, `USER`, `LNAME`,
/// `USERNAME`, and the user database), so a daemon started by `launchd`
/// without those variables still finds its credentials. Empty variables
/// are skipped.
#[cfg(any(target_os = "macos", test))]
fn keychain_account(
    variable: impl Fn(&str) -> Option<String>,
    user_database: impl FnOnce() -> Option<String>,
) -> String {
    ["USER", "LOGNAME", "LNAME", "USERNAME"]
        .into_iter()
        .find_map(|name| variable(name).filter(|value| !value.is_empty()))
        .or_else(user_database)
        .filter(|user| !user.is_empty())
        .unwrap_or_else(|| "claude-code-user".to_owned())
}

/// Whether a subpath from a store may become a path under the session
/// directory: not empty, not absolute or drive-prefixed, without `.` or
/// `..` segments or NUL. A drive prefix is any character followed by `:`,
/// as `ntpath.splitdrive` reads it on every host, so `1:foo` cannot name
/// an alternate data stream on Windows.
pub(crate) fn is_safe_subpath(subpath: &str) -> bool {
    if subpath.is_empty() || subpath.contains('\0') || subpath.starts_with(['/', '\\']) {
        return false;
    }
    if subpath.chars().nth(1) == Some(':') {
        return false;
    }
    if Path::new(subpath).is_absolute() {
        return false;
    }
    !subpath
        .split(['/', '\\'])
        .any(|segment| matches!(segment, "." | ".."))
}

/// The transcript file of a subpath: its last segment with `.jsonl`
/// appended, as upstream names it with `Path.with_name`, so a trailing
/// separator (`subagents/agent-x/`) names the same file as the subpath
/// without it. `None` for a path that would leave the session directory.
fn subkey_transcript_path(session_dir: &Path, subpath: &str) -> Option<PathBuf> {
    let relative = Path::new(subpath);
    let name = relative.file_name()?;
    let mut file_name = name.to_os_string();
    file_name.push(".jsonl");
    let file = session_dir.join(relative).with_file_name(file_name);
    file.starts_with(session_dir).then_some(file)
}

/// Writes the subagent transcripts the store lists under the session into
/// the directory, which comes back on success and is removed on failure.
/// A store without `list_subkeys` gets the main transcript only.
async fn materialize_subkeys(
    store: &SharedSessionStore,
    directory: ResumeDir,
    project_dir: &Path,
    project_key: &str,
    session_id: &str,
    limit: Duration,
) -> Result<ResumeDir> {
    let session_dir = project_dir.join(session_id);
    let listed = bounded(
        &format!("list_subkeys for session {session_id}"),
        limit,
        async {
            match store
                .list_subkeys(&SessionListSubkeysKey::new(project_key, session_id))
                .await
            {
                Err(error) if is_unsupported(&error) => Ok(None),
                result => result.map(Some),
            }
        },
    )
    .await;
    let (mut directory, listed) = or_remove(directory, listed).await?;
    let Some(subkeys) = listed else {
        return Ok(directory);
    };
    for subpath in subkeys {
        if !is_safe_subpath(&subpath) {
            tracing::warn!("[SessionStore] skipping unsafe subpath from list_subkeys: {subpath:?}");
            continue;
        }
        let key = SessionKey::new(project_key, session_id).with_subpath(subpath.clone());
        let loaded = bounded(
            &format!("load for session {session_id} subpath {subpath}"),
            limit,
            store.load(&key),
        )
        .await;
        let (kept, entries) = or_remove(directory, loaded).await?;
        directory = kept;
        let entries = entries.unwrap_or_default();
        if entries.is_empty() {
            continue;
        }
        let (metadata, transcript) =
            split_agent_metadata(entries.into_iter().map(Value::Object).collect());
        let transcript: Vec<SessionStoreEntry> = transcript
            .into_iter()
            .filter_map(|entry| match entry {
                Value::Object(entry) => Some(entry),
                _ => None,
            })
            .collect();
        let Some(file) = subkey_transcript_path(&session_dir, &subpath) else {
            continue;
        };
        (directory, ()) = in_directory(directory, move |_| {
            if !transcript.is_empty() {
                write_jsonl(&file, &transcript)?;
            }
            if let Some(mut metadata) = metadata {
                metadata.remove_key("type");
                let sidecar = agent_metadata_sidecar_path(&file);
                if let Some(parent) = sidecar.parent() {
                    create_dir(parent)?;
                }
                write_private(&sidecar, Value::Object(metadata).to_string().as_bytes()).map_err(
                    |error| Error::io(format!("Cannot write {}", sidecar.display()), error),
                )?;
            }
            Ok(())
        })
        .await?;
    }
    Ok(directory)
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use futures_util::StreamExt;
    use serde_json::json;

    use super::*;
    use crate::session_store::{InMemorySessionStore, SessionStore};
    use crate::types::ClaudeAgentOptionsBuilder;

    /// Upstream `test_write_jsonl_round_trip`: the streamed file is the
    /// compact JSON of each entry in its key order, one per line, and reads
    /// back to the entries. Text outside ASCII stays raw UTF-8, as the CLI
    /// writes it, where `json.dumps` escapes it.
    #[cfg(feature = "preserve-order")]
    #[test]
    fn a_materialized_transcript_is_the_compact_json_of_each_entry_in_its_key_order() {
        let entries: Vec<SessionStoreEntry> = (0..100)
            .map(|i| {
                let Value::Object(entry) = json!({
                    "type": if i % 2 == 0 { "user" } else { "assistant" },
                    "uuid": format!("uuid-{i}"),
                    "message": {"role": "user", "content": format!("line {i} \"q\" \n nl caf\u{e9}")},
                    "nested": {"a": [i, i + 1], "b": null},
                }) else {
                    unreachable!()
                };
                entry
            })
            .collect();
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("stream.jsonl");
        write_jsonl(&path, &entries).unwrap();

        let written = std::fs::read_to_string(&path).unwrap();
        let reference: String = entries
            .iter()
            .map(|entry| format!("{}\n", serde_json::to_string(entry).unwrap()))
            .collect();
        assert_eq!(written, reference);
        assert!(written.contains("caf\u{e9}"));
        let read: Vec<SessionStoreEntry> = written
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(read, entries);
    }

    /// The config directories whose credentials writes fail, each with the
    /// path of every attempt. Keyed by directory, not by thread, since the
    /// writes run on the blocking pool.
    static CREDENTIALS_FAULTS: Mutex<Vec<(PathBuf, Vec<PathBuf>)>> = Mutex::new(Vec::new());

    fn faults() -> std::sync::MutexGuard<'static, Vec<(PathBuf, Vec<PathBuf>)>> {
        CREDENTIALS_FAULTS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The error a write of the credentials read from `source` fails with
    /// while a test holds an [`Arm`] on it; the path of each attempt is
    /// recorded for the test.
    pub(super) fn credentials_write_fault(source: &Path, path: &Path) -> Option<std::io::Error> {
        faults()
            .iter_mut()
            .find(|(armed, _)| armed == source)
            .map(|(_, attempts)| {
                attempts.push(path.to_path_buf());
                std::io::Error::new(ErrorKind::PermissionDenied, "injected: disk refused")
            })
    }

    /// Makes every write of the credentials of one config directory fail
    /// until dropped.
    struct Arm(PathBuf);

    impl Arm {
        fn new(config: &Path) -> Self {
            faults().push((config.to_path_buf(), Vec::new()));
            Self(config.to_path_buf())
        }

        fn attempts(&self) -> Vec<PathBuf> {
            faults()
                .iter()
                .find(|(armed, _)| *armed == self.0)
                .map(|(_, attempts)| attempts.clone())
                .unwrap_or_default()
        }
    }

    impl Drop for Arm {
        fn drop(&mut self) {
            faults().retain(|(armed, _)| *armed != self.0);
        }
    }

    /// Options resuming a session the store holds, with the caller config
    /// directory `config` and a CLI path that does not exist: a run that
    /// gets past the resume fails with [`ErrorKind::CliNotFound`](crate::ErrorKind::CliNotFound).
    async fn resumable(config: &Path, cwd: &Path) -> ClaudeAgentOptionsBuilder {
        let id = uuid::Uuid::new_v4().to_string();
        let store = InMemorySessionStore::new();
        let key = SessionKey::new(project_key_for_directory(Some(cwd)), id.as_str());
        let entry = json!({"type": "user", "uuid": uuid::Uuid::new_v4().to_string(), "sessionId": id, "message": {"role": "user", "content": "hi"}});
        let Value::Object(entry) = entry else {
            unreachable!()
        };
        store.append(&key, &[entry]).await.unwrap();
        ClaudeAgentOptions::builder()
            .cwd(cwd)
            .session_store(store)
            .resume(id.as_str())
            .env_var(CONFIG_DIR_VARIABLE, config.to_string_lossy())
            .cli_path(cwd.join("no-such-claude"))
    }

    #[tokio::test]
    async fn a_credentials_write_failure_fails_the_query_before_any_spawn_and_removes_the_directory()
     {
        let config = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        std::fs::write(
            config.path().join(".credentials.json"),
            br#"{"claudeAiOauth":{"accessToken":"a","refreshToken":"r"}}"#,
        )
        .unwrap();
        let options = resumable(config.path(), cwd.path()).await.build().unwrap();
        let arm = Arm::new(config.path());
        let mut messages = crate::query("hi", options);
        let first = messages.next().await;
        let Some(Err(error)) = &first else {
            panic!("expected the credentials error, got {first:?}");
        };
        assert_eq!(error.kind(), crate::ErrorKind::Io);
        assert!(error.to_string().contains(".credentials.json"), "{error}");
        assert_eq!(error.io_kind(), Some(ErrorKind::PermissionDenied));
        let source = std::error::Error::source(error).unwrap();
        assert!(source.to_string().contains("injected: disk refused"));
        assert!(
            !error.to_string().contains("injected: disk refused"),
            "the text does not repeat the source"
        );
        assert!(messages.next().await.is_none());
        let attempts = arm.attempts();
        assert_eq!(attempts.len(), 1, "{attempts:?}");
        let directory = attempts[0].parent().unwrap();
        assert!(
            directory
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(TEMPORARY_PREFIX)
        );
        assert!(
            !directory.exists(),
            "{} is left behind",
            directory.display()
        );
    }

    #[tokio::test]
    async fn without_credentials_to_copy_the_resume_reaches_the_cli_lookup() {
        let config = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let options = resumable(config.path(), cwd.path())
            .await
            .env_var("ANTHROPIC_API_KEY", "sk-test")
            .build()
            .unwrap();
        let arm = Arm::new(config.path());
        let mut messages = crate::query("hi", options);
        let first = messages.next().await;
        assert!(
            matches!(&first, Some(Err(error)) if error.kind() == crate::ErrorKind::CliNotFound),
            "the resume was materialized and the CLI looked up: {first:?}"
        );
        assert!(arm.attempts().is_empty());
    }

    fn variables(set: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let set: Vec<(String, String)> = set
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect();
        move |name| {
            set.iter()
                .find(|(set_name, _)| set_name == name)
                .map(|(_, value)| value.clone())
        }
    }

    #[test]
    fn without_user_variables_the_keychain_account_comes_from_the_user_database() {
        assert_eq!(
            keychain_account(variables(&[]), || Some("daemon-user".to_owned())),
            "daemon-user"
        );
        assert_eq!(
            keychain_account(variables(&[("USER", ""), ("LOGNAME", "")]), || Some(
                "daemon-user".to_owned()
            )),
            "daemon-user",
            "empty variables are skipped"
        );
        assert_eq!(
            keychain_account(variables(&[]), || None),
            "claude-code-user"
        );
        #[cfg(unix)]
        {
            let name = crate::process::sys::current_user_name().unwrap();
            assert_eq!(
                keychain_account(variables(&[]), crate::process::sys::current_user_name),
                name
            );
        }
    }

    #[test]
    fn an_empty_config_dir_variable_of_the_process_keeps_the_keychain_out_as_upstream() {
        let options = IndexMap::new();
        let empty = caller_config_dir_from(&options, Some(OsString::new()));
        assert_eq!(empty, Some(OsString::new()));
        assert!(!consults_keychain(empty.as_ref(), false));
        assert!(consults_keychain(
            caller_config_dir_from(&options, None).as_ref(),
            false
        ));

        let mut options = IndexMap::new();
        options.insert(CONFIG_DIR_VARIABLE.to_owned(), String::new());
        assert_eq!(caller_config_dir_from(&options, None), None);
        options.insert(CONFIG_DIR_VARIABLE.to_owned(), "/cfg".to_owned());
        assert_eq!(
            caller_config_dir_from(&options, Some(OsString::from("/process"))),
            Some(OsString::from("/cfg"))
        );
    }

    #[test]
    fn user_variables_name_the_keychain_account_in_the_order_upstream_reads_them() {
        let database = || Some("database".to_owned());
        assert_eq!(
            keychain_account(variables(&[("USER", "u"), ("LOGNAME", "l")]), database),
            "u"
        );
        assert_eq!(
            keychain_account(variables(&[("LOGNAME", "l"), ("USERNAME", "n")]), database),
            "l"
        );
        assert_eq!(
            keychain_account(variables(&[("LNAME", "ln"), ("USERNAME", "n")]), database),
            "ln"
        );
        assert_eq!(
            keychain_account(variables(&[("USERNAME", "n")]), database),
            "n"
        );
    }

    #[test]
    fn unsafe_subpaths_are_rejected() {
        for subpath in [
            "", "/abs", "\\abs", "C:foo", "c:\\x", "a/../b", "..", "./x", "a\\..\\b", "a\0b",
        ] {
            assert!(!is_safe_subpath(subpath), "{subpath:?}");
        }
        for subpath in [
            "subagents/agent-1",
            "subagents/workflows/run/agent-2",
            "a..b/c",
        ] {
            assert!(is_safe_subpath(subpath), "{subpath:?}");
        }
    }

    /// Upstream decodes the credentials strictly and fails on bytes that
    /// are not UTF-8; they are read with replacement characters here, as
    /// transcripts are, and the refresh token is still removed.
    #[test]
    fn credentials_that_are_not_utf8_are_copied_with_replacement_characters_and_redacted() {
        let source = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        let file = ".credentials.json";
        std::fs::write(
            source.path().join(file),
            b"{\"claudeAiOauth\":{\"accessToken\":\"a\xff\",\"refreshToken\":\"r\"}}",
        )
        .unwrap();
        let mut env = IndexMap::new();
        env.insert(
            "CLAUDE_CONFIG_DIR".to_owned(),
            source.path().to_string_lossy().into_owned(),
        );
        env.insert("ANTHROPIC_API_KEY".to_owned(), "placeholder".to_owned());
        copy_auth_files(target.path(), &env).unwrap();
        let copied: Value =
            serde_json::from_slice(&std::fs::read(target.path().join(file)).unwrap()).unwrap();
        assert_eq!(
            copied,
            json!({"claudeAiOauth": {"accessToken": "a\u{fffd}"}})
        );
    }

    #[test]
    fn refresh_tokens_are_removed_and_the_rest_is_kept() {
        let credentials = json!({"claudeAiOauth": {"accessToken": "a", "refreshToken": "r", "expiresAt": 1}, "other": true});
        let redacted: Value =
            serde_json::from_str(&redact_credentials(&credentials.to_string())).unwrap();
        assert_eq!(
            redacted,
            json!({"claudeAiOauth": {"accessToken": "a", "expiresAt": 1}, "other": true})
        );
        assert_eq!(redact_credentials("not json"), "not json");
        assert_eq!(redact_credentials(r#"{"x":1}"#), r#"{"x":1}"#);
    }

    #[test]
    fn plugin_settings_and_the_config_dir_are_stripped() {
        let settings = json!({"enabledPlugins": {"p": true}, "extraKnownMarketplaces": {}, "apiKeyHelper": "h", "env": {"CLAUDE_CONFIG_DIR": "/x", "KEEP": "1"}});
        let mut bom = b"\xef\xbb\xbf".to_vec();
        bom.extend(settings.to_string().as_bytes());
        let stripped: Value = serde_json::from_slice(&strip_settings_for_resume(bom)).unwrap();
        assert_eq!(stripped, json!({"apiKeyHelper": "h", "env": {"KEEP": "1"}}));
        let untouched = br#"{"apiKeyHelper": "h"}"#.to_vec();
        assert_eq!(strip_settings_for_resume(untouched.clone()), untouched);
        assert_eq!(strip_settings_for_resume(b"[1]".to_vec()), b"[1]");
        assert_eq!(strip_settings_for_resume(b"{oops".to_vec()), b"{oops");
    }

    #[tokio::test]
    async fn the_directory_is_removed_by_remove_and_by_drop() {
        let directory = ResumeDir::create().unwrap();
        let path = directory.path().to_path_buf();
        assert!(
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(TEMPORARY_PREFIX)
        );
        std::fs::write(path.join("file"), "x").unwrap();
        directory.remove().await;
        assert!(!path.exists());
        let dropped = ResumeDir::create().unwrap();
        let path = dropped.path().to_path_buf();
        drop(dropped);
        assert!(!path.exists());
    }

    #[test]
    fn a_subpath_with_any_drive_prefix_is_rejected_as_ntpath_splitdrive_does() {
        for subpath in ["1:foo", "\u{e9}:foo", "::x", "a:"] {
            assert!(!is_safe_subpath(subpath), "{subpath:?}");
        }
        for subpath in [":foo", "ab:c", "subagents/a:b"] {
            assert!(is_safe_subpath(subpath), "{subpath:?}");
        }
    }

    #[test]
    fn subpath_metadata_is_split_and_kept_last() {
        let (metadata, transcript) = split_agent_metadata(vec![
            json!({"type": "agent_metadata", "agentType": "a"}),
            json!({"type": "user", "uuid": "u"}),
            json!({"type": "agent_metadata", "agentType": "b"}),
        ]);
        assert_eq!(
            metadata.and_then(|metadata| metadata.get("agentType").cloned()),
            Some(Value::from("b"))
        );
        assert_eq!(transcript.len(), 1);
    }
}
