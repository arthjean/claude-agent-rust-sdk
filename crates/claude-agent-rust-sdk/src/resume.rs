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

use std::fs::OpenOptions;
use std::future::Future;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::Value;

use crate::error::{Error, Result};
use crate::session_store::{
    SessionKey, SessionListSubkeysKey, SessionStoreEntry, SessionStoreError, SharedSessionStore,
};
use crate::sessions::{
    agent_metadata_sidecar_path, entries_to_jsonl, project_key_for_directory, split_agent_metadata,
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
    fn create() -> Result<Self> {
        let directory = tempfile::Builder::new()
            .prefix(TEMPORARY_PREFIX)
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
    /// ignoring errors. It never fails.
    pub(crate) async fn remove(mut self) {
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
            let mut sweep = Sweep(Some(path.clone()));
            tokio::time::sleep(CLEANUP_DELAY).await;
            sweep.0 = None;
        }
        let _ = std::fs::remove_dir_all(&path);
    }
}

/// Removes the directory if a retry wait is cancelled.
struct Sweep(Option<PathBuf>);

impl Drop for Sweep {
    fn drop(&mut self) {
        if let Some(path) = &self.0 {
            let _ = std::fs::remove_dir_all(path);
        }
    }
}

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
    Error::SessionStore {
        message: format!(
            "Session store {what} timed out after {} ms during resume materialization",
            limit.as_millis()
        ),
        source: None,
    }
}

async fn bounded<T>(
    what: &str,
    limit: Duration,
    call: impl Future<Output = std::result::Result<T, SessionStoreError>>,
) -> Result<T> {
    match tokio::time::timeout(limit, call).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(error)) => Err(Error::SessionStore {
            message: format!("Session store {what} failed during resume materialization: {error}"),
            source: Some(error),
        }),
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

/// The newest session whose first entry is not a sidechain: sidechains are
/// mirrored as top-level keys and are often the newest.
async fn continue_candidate(
    store: &SharedSessionStore,
    project_key: &str,
    limit: Duration,
) -> Result<Option<(String, Vec<SessionStoreEntry>)>> {
    let mut sessions = bounded("list_sessions", limit, store.list_sessions(project_key)).await?;
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

    let directory = ResumeDir::create()?;
    let project_dir = directory.path().join("projects").join(&project_key);
    let filled = async {
        write_jsonl(&project_dir.join(format!("{session_id}.jsonl")), &entries)?;
        copy_auth_files(directory.path(), options)?;
        if store.capabilities().list_subkeys {
            materialize_subkeys(store, &project_dir, &project_key, &session_id, limit).await?;
        }
        Ok(())
    }
    .await;
    if let Err(error) = filled {
        directory.remove().await;
        return Err(error);
    }
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
    let values: Vec<Value> = entries.iter().cloned().map(Value::Object).collect();
    write_private(path, entries_to_jsonl(&values).as_bytes())
        .map_err(|error| Error::io(format!("Cannot write {}", path.display()), error))
}

fn env_value(options: &ClaudeAgentOptions, name: &str) -> Option<String> {
    options
        .env
        .get(name)
        .cloned()
        .filter(|value| !value.is_empty())
        .or_else(|| std::env::var(name).ok().filter(|value| !value.is_empty()))
}

/// Reads a regular file. A missing file is absent; any other reason it
/// cannot be read is logged and treated as absent, since these files only
/// enrich the temporary directory.
fn read_if_present(path: &Path) -> Option<Vec<u8>> {
    match std::fs::metadata(path) {
        Err(error) if error.kind() == ErrorKind::NotFound => return None,
        Err(error) => {
            tracing::warn!(
                "[SessionStore] resume: skipping {} ({error})",
                path.display()
            );
            return None;
        }
        Ok(metadata) if !metadata.is_file() => {
            tracing::warn!(
                "[SessionStore] resume: skipping {} (not a regular file)",
                path.display()
            );
            return None;
        }
        Ok(_) => {}
    }
    match std::fs::read(path) {
        Ok(content) => Some(content),
        Err(error) => {
            tracing::warn!(
                "[SessionStore] resume: skipping {} ({error})",
                path.display()
            );
            None
        }
    }
}

fn copy_if_present(source: &Path, target: &Path, transform: impl FnOnce(Vec<u8>) -> Vec<u8>) {
    let Some(content) = read_if_present(source) else {
        return;
    };
    if let Err(error) = write_private(target, &transform(content)) {
        let _ = std::fs::remove_file(target);
        tracing::warn!(
            "[SessionStore] resume: skipping {} ({error})",
            source.display()
        );
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
        stripped |= settings.remove(key).is_some();
    }
    if let Some(Value::Object(env)) = settings.get_mut("env") {
        stripped |= env.remove(CONFIG_DIR_VARIABLE).is_some();
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
        .and_then(|oauth| oauth.remove("refreshToken"))
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
/// Returns [`Error::Io`] naming the credentials file when credentials were
/// found but cannot be written. A seed file that cannot be copied is only
/// logged.
fn copy_auth_files(target: &Path, options: &ClaudeAgentOptions) -> Result<()> {
    let caller_config_dir = env_value(options, CONFIG_DIR_VARIABLE).map(PathBuf::from);
    let home = std::env::home_dir().unwrap_or_default();
    let source = caller_config_dir
        .clone()
        .unwrap_or_else(|| home.join(".claude"));

    let mut credentials = read_if_present(&source.join(".credentials.json"))
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned());
    let env_auth = env_value(options, "ANTHROPIC_API_KEY").is_some()
        || env_value(options, "CLAUDE_CODE_OAUTH_TOKEN").is_some();
    if caller_config_dir.is_none()
        && !env_auth
        && let Some(keychain) = read_keychain_credentials()
    {
        credentials = Some(keychain);
    }
    if let Some(credentials) = credentials {
        let target = target.join(".credentials.json");
        write_credentials(&target, redact_credentials(&credentials).as_bytes()).map_err(
            |error| {
                Error::io(
                    format!(
                        "Cannot write the credentials file {} for the resumed session",
                        target.display()
                    ),
                    error,
                )
            },
        )?;
    }

    let claude_json = match &caller_config_dir {
        Some(directory) => directory.join(".claude.json"),
        None => home.join(".claude.json"),
    };
    copy_if_present(&claude_json, &target.join(".claude.json"), |content| {
        content
    });
    for name in SETTINGS_FILES {
        copy_if_present(
            &source.join(name),
            &target.join(name),
            strip_settings_for_resume,
        );
    }
    Ok(())
}

/// Writes the redacted credentials. Unit tests can make the write fail, as
/// a full disk or a denied permission would.
fn write_credentials(path: &Path, content: &[u8]) -> std::io::Result<()> {
    #[cfg(test)]
    if let Some(error) = tests::credentials_write_fault(path) {
        return Err(error);
    }
    write_private(path, content)
}

/// The OAuth credentials the CLI keeps in the macOS Keychain by default.
#[cfg(target_os = "macos")]
fn read_keychain_credentials() -> Option<String> {
    let user = std::env::var("USER")
        .or_else(|_| std::env::var("LOGNAME"))
        .unwrap_or_else(|_| "claude-code-user".to_owned());
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

/// Whether a subpath from a store may become a path under the session
/// directory: not empty, not absolute or drive-prefixed, without `.` or
/// `..` segments or NUL.
pub(crate) fn is_safe_subpath(subpath: &str) -> bool {
    if subpath.is_empty() || subpath.contains('\0') || subpath.starts_with(['/', '\\']) {
        return false;
    }
    let bytes = subpath.as_bytes();
    if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        return false;
    }
    if Path::new(subpath).is_absolute() {
        return false;
    }
    !subpath
        .split(['/', '\\'])
        .any(|segment| matches!(segment, "." | ".."))
}

async fn materialize_subkeys(
    store: &SharedSessionStore,
    project_dir: &Path,
    project_key: &str,
    session_id: &str,
    limit: Duration,
) -> Result<()> {
    let session_dir = project_dir.join(session_id);
    let subkeys = bounded(
        &format!("list_subkeys for session {session_id}"),
        limit,
        store.list_subkeys(&SessionListSubkeysKey::new(project_key, session_id)),
    )
    .await?;
    for subpath in subkeys {
        if !is_safe_subpath(&subpath) {
            tracing::warn!("[SessionStore] skipping unsafe subpath from list_subkeys: {subpath:?}");
            continue;
        }
        let key = SessionKey::new(project_key, session_id).with_subpath(subpath.clone());
        let entries = bounded(
            &format!("load for session {session_id} subpath {subpath}"),
            limit,
            store.load(&key),
        )
        .await?
        .unwrap_or_default();
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
        let file = session_dir.join(format!("{subpath}.jsonl"));
        if !file.starts_with(&session_dir) {
            continue;
        }
        if !transcript.is_empty() {
            write_jsonl(&file, &transcript)?;
        }
        if let Some(mut metadata) = metadata {
            metadata.remove("type");
            let sidecar = agent_metadata_sidecar_path(&file);
            if let Some(parent) = sidecar.parent() {
                create_dir(parent)?;
            }
            write_private(&sidecar, Value::Object(metadata).to_string().as_bytes())
                .map_err(|error| Error::io(format!("Cannot write {}", sidecar.display()), error))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use futures_util::StreamExt;
    use serde_json::json;

    use super::*;
    use crate::session_store::{InMemorySessionStore, SessionStore};
    use crate::types::ClaudeAgentOptionsBuilder;

    thread_local! {
        static CREDENTIALS_FAULT: RefCell<Option<Vec<PathBuf>>> = const { RefCell::new(None) };
    }

    /// The error a credentials write on this thread fails with while a test
    /// holds an [`Arm`]; the path of each attempt is recorded for the test.
    pub(super) fn credentials_write_fault(path: &Path) -> Option<std::io::Error> {
        CREDENTIALS_FAULT.with(|fault| {
            fault.borrow_mut().as_mut().map(|attempts| {
                attempts.push(path.to_path_buf());
                std::io::Error::new(ErrorKind::PermissionDenied, "injected: disk refused")
            })
        })
    }

    /// Makes every credentials write on this thread fail until dropped.
    struct Arm;

    impl Arm {
        fn new() -> Self {
            CREDENTIALS_FAULT.with(|fault| *fault.borrow_mut() = Some(Vec::new()));
            Self
        }

        fn attempts(&self) -> Vec<PathBuf> {
            CREDENTIALS_FAULT.with(|fault| fault.borrow().clone().unwrap_or_default())
        }
    }

    impl Drop for Arm {
        fn drop(&mut self) {
            CREDENTIALS_FAULT.with(|fault| *fault.borrow_mut() = None);
        }
    }

    /// Options resuming a session the store holds, with the caller config
    /// directory `config` and a CLI path that does not exist: a run that
    /// gets past the resume fails with [`Error::CliNotFound`].
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
        let arm = Arm::new();
        let mut messages = crate::query("hi", options);
        let first = messages.next().await;
        let Some(Err(Error::Io { message, source })) = &first else {
            panic!("expected the credentials error, got {first:?}");
        };
        assert!(message.contains(".credentials.json"), "{message}");
        assert_eq!(source.kind(), ErrorKind::PermissionDenied);
        assert!(source.to_string().contains("injected: disk refused"));
        assert!(
            first
                .as_ref()
                .unwrap()
                .as_ref()
                .unwrap_err()
                .to_string()
                .contains("injected: disk refused")
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
        let arm = Arm::new();
        let mut messages = crate::query("hi", options);
        let first = messages.next().await;
        assert!(
            matches!(first, Some(Err(Error::CliNotFound { .. }))),
            "the resume was materialized and the CLI looked up: {first:?}"
        );
        assert!(arm.attempts().is_empty());
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
