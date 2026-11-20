//! Storage contract for mirroring session transcripts outside the local
//! disk.
//!
//! The CLI still writes transcripts to local disk; a [`SessionStore`]
//! receives a secondary copy. Only [`SessionStore::append`] and
//! [`SessionStore::load`] are required. An optional method a store leaves
//! out keeps its default implementation, which returns
//! [`UnsupportedStoreOperation`]: the crate calls an optional method and
//! takes that error as absence, as upstream takes an inherited method that
//! raises `NotImplementedError`. [`SessionStore::capabilities`] is a hint
//! read only where no call can be made, when options are built.
//!
//! The store contract differs from upstream in one deliberate way:
//!
//! - Python checks at option validation whether the store overrides
//!   `list_sessions`, so `continue_conversation` with a store that does not
//!   fails before the child starts. Rust cannot see an override, so that
//!   check reads [`SessionStoreCapabilities::list_sessions`] from
//!   [`SessionStore::capabilities`]: a store meant for
//!   `continue_conversation` declares it.

use std::fmt;
use std::future::Future;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::types::BoxFuture;
use crate::types::macros::wire_enum;

mod memory;

pub use memory::InMemorySessionStore;

/// The [`SessionKey`] of a transcript file, from its path relative to the
/// projects directory.
///
/// `<projects>/<project>/<session>.jsonl` is a main transcript, and
/// `<projects>/<project>/<session>/subagents/.../agent-<id>.jsonl` a
/// subagent transcript whose subpath joins the segments after the session
/// with `/` on every platform, without the `.jsonl` suffix. `None` for a
/// path outside the projects directory or of another shape.
///
/// Both paths are made absolute and their `.` and `..` segments resolved
/// without reading the disk, and on Windows they are compared without
/// regard to case, as Python's `os.path.relpath` does.
#[must_use]
pub fn file_path_to_session_key(
    file_path: &std::path::Path,
    projects_dir: &std::path::Path,
) -> Option<SessionKey> {
    let file = lexical_components(file_path);
    let projects = lexical_components(projects_dir);
    if file.len() < projects.len()
        || !projects
            .iter()
            .zip(&file)
            .all(|(base, part)| same_component(base, part))
    {
        return None;
    }
    let parts: Vec<String> = file[projects.len()..]
        .iter()
        .map(|part| part.to_string_lossy().into_owned())
        .collect();
    match parts.as_slice() {
        [project_key, file] => {
            let session_id = file.strip_suffix(".jsonl")?;
            Some(SessionKey::new(project_key.as_str(), session_id))
        }
        [project_key, session_id, rest @ ..] if rest.len() >= 2 => {
            let mut subpath = rest.to_vec();
            if let Some(last) = subpath.last_mut()
                && let Some(stem) = last.strip_suffix(".jsonl")
            {
                *last = stem.to_owned();
            }
            Some(
                SessionKey::new(project_key.as_str(), session_id.as_str())
                    .with_subpath(subpath.join("/")),
            )
        }
        _ => None,
    }
}

/// The components of a path made absolute against the working directory,
/// with `.` dropped and each `..` removing the segment before it, as
/// `os.path.normpath(os.path.abspath(path))` does.
fn lexical_components(path: &std::path::Path) -> Vec<std::ffi::OsString> {
    use std::path::Component;

    let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let mut parts: Vec<std::ffi::OsString> = Vec::new();
    let mut anchored = 0;
    for component in absolute.components() {
        match component {
            Component::Prefix(_) | Component::RootDir => {
                parts.push(component.as_os_str().to_owned());
                anchored = parts.len();
            }
            Component::CurDir => {}
            Component::ParentDir => {
                if parts.len() > anchored {
                    parts.pop();
                }
            }
            Component::Normal(part) => parts.push(part.to_owned()),
        }
    }
    parts
}

/// Whether two path components name the same entry: equal, or on Windows
/// equal once lowercased, as `ntpath.normcase` compares them.
fn same_component(left: &std::ffi::OsStr, right: &std::ffi::OsStr) -> bool {
    if cfg!(windows) {
        left.to_string_lossy().to_lowercase() == right.to_string_lossy().to_lowercase()
    } else {
        left == right
    }
}

/// Error returned by a session store.
pub type SessionStoreError = Box<dyn std::error::Error + Send + Sync + 'static>;

/// One transcript line, as an opaque JSON object.
///
/// Stores treat entries as pass-through values: an entry read back must be
/// equal to the entry that was appended.
pub type SessionStoreEntry = Map<String, Value>;

/// Identifies a session transcript or a subagent transcript in a store.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub struct SessionKey {
    /// Scope defined by the caller, by default the sanitized working
    /// directory.
    pub project_key: String,
    /// Id of the session.
    pub session_id: String,
    /// Absent for the main transcript; set for a subagent transcript, as
    /// path segments joined with `/`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subpath: Option<String>,
}

impl SessionKey {
    /// Creates the key of a main transcript.
    #[must_use]
    pub fn new(project_key: impl Into<String>, session_id: impl Into<String>) -> Self {
        Self {
            project_key: project_key.into(),
            session_id: session_id.into(),
            subpath: None,
        }
    }

    /// Sets the subpath, turning the key into the key of a subagent
    /// transcript.
    #[must_use]
    pub fn with_subpath(mut self, subpath: impl Into<String>) -> Self {
        self.subpath = Some(subpath.into());
        self
    }
}

/// Key argument of [`SessionStore::list_subkeys`]: a session, without a
/// subpath.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub struct SessionListSubkeysKey {
    /// Scope defined by the caller.
    pub project_key: String,
    /// Id of the session.
    pub session_id: String,
}

impl SessionListSubkeysKey {
    /// Creates the key.
    #[must_use]
    pub fn new(project_key: impl Into<String>, session_id: impl Into<String>) -> Self {
        Self {
            project_key: project_key.into(),
            session_id: session_id.into(),
        }
    }
}

/// Entry returned by [`SessionStore::list_sessions`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct SessionStoreListEntry {
    /// Id of the session.
    pub session_id: String,
    /// Last modification time, in milliseconds since the Unix epoch.
    pub mtime: i64,
}

impl SessionStoreListEntry {
    /// Creates the entry.
    #[must_use]
    pub fn new(session_id: impl Into<String>, mtime: i64) -> Self {
        Self {
            session_id: session_id.into(),
            mtime,
        }
    }
}

/// Session summary maintained incrementally by a store.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct SessionSummaryEntry {
    /// Id of the session.
    pub session_id: String,
    /// Storage write time of the summary, in milliseconds since the Unix
    /// epoch, from the same clock as [`SessionStoreListEntry::mtime`].
    pub mtime: i64,
    /// Opaque state owned by this crate. Persist it verbatim.
    pub data: Map<String, Value>,
}

impl SessionSummaryEntry {
    /// Creates the entry.
    #[must_use]
    pub fn new(session_id: impl Into<String>, mtime: i64, data: Map<String, Value>) -> Self {
        Self {
            session_id: session_id.into(),
            mtime,
            data,
        }
    }
}

wire_enum! {
    /// When mirrored transcript entries are flushed to a store.
    #[derive(Default)]
    pub enum SessionStoreFlushMode {
        /// Buffer entries and flush once per turn, or when the buffer
        /// exceeds 500 entries or 1 MiB.
        #[default]
        Batched => "batched",
        /// Trigger a background flush after every mirrored frame.
        Eager => "eager",
    }
}

/// Optional methods a [`SessionStore`] declares it implements.
///
/// The declaration is a hint: an optional method is always called, and its
/// [`UnsupportedStoreOperation`] error is what marks it absent. Only the
/// validation of `continue_conversation`, which runs before any call can
/// be made, reads [`list_sessions`](Self::list_sessions).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub struct SessionStoreCapabilities {
    /// [`SessionStore::list_sessions`] is implemented.
    pub list_sessions: bool,
    /// [`SessionStore::list_session_summaries`] is implemented.
    pub list_session_summaries: bool,
    /// [`SessionStore::delete`] is implemented.
    pub delete: bool,
    /// [`SessionStore::list_subkeys`] is implemented.
    pub list_subkeys: bool,
}

impl SessionStoreCapabilities {
    /// No optional method is implemented.
    #[must_use]
    pub const fn none() -> Self {
        Self {
            list_sessions: false,
            list_session_summaries: false,
            delete: false,
            list_subkeys: false,
        }
    }

    /// Every optional method is implemented.
    #[must_use]
    pub const fn all() -> Self {
        Self {
            list_sessions: true,
            list_session_summaries: true,
            delete: true,
            list_subkeys: true,
        }
    }

    /// Declares [`SessionStore::list_sessions`].
    #[must_use]
    pub const fn with_list_sessions(mut self) -> Self {
        self.list_sessions = true;
        self
    }

    /// Declares [`SessionStore::list_session_summaries`].
    #[must_use]
    pub const fn with_list_session_summaries(mut self) -> Self {
        self.list_session_summaries = true;
        self
    }

    /// Declares [`SessionStore::delete`].
    #[must_use]
    pub const fn with_delete(mut self) -> Self {
        self.delete = true;
        self
    }

    /// Declares [`SessionStore::list_subkeys`].
    #[must_use]
    pub const fn with_list_subkeys(mut self) -> Self {
        self.list_subkeys = true;
        self
    }
}

/// Error returned by the default implementation of an optional
/// [`SessionStore`] method.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("session store does not implement {operation}()")]
pub struct UnsupportedStoreOperation {
    /// Name of the method that is not implemented.
    pub operation: &'static str,
}

fn unsupported<T>(operation: &'static str) -> Result<T, SessionStoreError> {
    Err(Box::new(UnsupportedStoreOperation { operation }))
}

/// Whether a store error says the optional method is not implemented.
pub(crate) fn is_unsupported(error: &SessionStoreError) -> bool {
    error.downcast_ref::<UnsupportedStoreOperation>().is_some()
}

/// Adapter that mirrors session transcripts to external storage.
///
/// Retention is the adapter's responsibility: this crate deletes from a
/// store only through an explicit delete call.
pub trait SessionStore: Send + Sync + 'static {
    /// Mirrors a batch of transcript entries.
    ///
    /// Called after the local write succeeded. Persist entries in call
    /// order. Most entries carry a stable `uuid` to treat as an idempotency
    /// key; entries without one are appended as is.
    fn append(
        &self,
        key: &SessionKey,
        entries: &[SessionStoreEntry],
    ) -> impl Future<Output = Result<(), SessionStoreError>> + Send;

    /// Loads a whole transcript, or `None` for a key that was never
    /// written.
    fn load(
        &self,
        key: &SessionKey,
    ) -> impl Future<Output = Result<Option<Vec<SessionStoreEntry>>, SessionStoreError>> + Send;

    /// Optional methods this store declares. The default declares none.
    ///
    /// A hint only: the crate calls an optional method whatever the
    /// declaration says, and takes [`UnsupportedStoreOperation`] as its
    /// absence. Declare [`list_sessions`](SessionStoreCapabilities::list_sessions)
    /// for a store used with `continue_conversation`, which is validated
    /// before the store can be called.
    fn capabilities(&self) -> SessionStoreCapabilities {
        SessionStoreCapabilities::none()
    }

    /// Lists the sessions of a project with their modification times, in
    /// any order. Optional.
    fn list_sessions(
        &self,
        project_key: &str,
    ) -> impl Future<Output = Result<Vec<SessionStoreListEntry>, SessionStoreError>> + Send {
        let _ = project_key;
        async { unsupported("list_sessions") }
    }

    /// Returns the summaries of every session of a project in one call.
    /// Optional.
    fn list_session_summaries(
        &self,
        project_key: &str,
    ) -> impl Future<Output = Result<Vec<SessionSummaryEntry>, SessionStoreError>> + Send {
        let _ = project_key;
        async { unsupported("list_session_summaries") }
    }

    /// Deletes a transcript. Deleting a main key also deletes every subkey
    /// of that session. Optional.
    fn delete(
        &self,
        key: &SessionKey,
    ) -> impl Future<Output = Result<(), SessionStoreError>> + Send {
        let _ = key;
        async { unsupported("delete") }
    }

    /// Lists the subpaths stored under a session. Optional.
    fn list_subkeys(
        &self,
        key: &SessionListSubkeysKey,
    ) -> impl Future<Output = Result<Vec<String>, SessionStoreError>> + Send {
        let _ = key;
        async { unsupported("list_subkeys") }
    }
}

trait ErasedSessionStore: Send + Sync {
    fn append<'a>(
        &'a self,
        key: &'a SessionKey,
        entries: &'a [SessionStoreEntry],
    ) -> BoxFuture<'a, Result<(), SessionStoreError>>;

    fn load<'a>(
        &'a self,
        key: &'a SessionKey,
    ) -> BoxFuture<'a, Result<Option<Vec<SessionStoreEntry>>, SessionStoreError>>;

    fn capabilities(&self) -> SessionStoreCapabilities;

    fn list_sessions<'a>(
        &'a self,
        project_key: &'a str,
    ) -> BoxFuture<'a, Result<Vec<SessionStoreListEntry>, SessionStoreError>>;

    fn list_session_summaries<'a>(
        &'a self,
        project_key: &'a str,
    ) -> BoxFuture<'a, Result<Vec<SessionSummaryEntry>, SessionStoreError>>;

    fn delete<'a>(&'a self, key: &'a SessionKey) -> BoxFuture<'a, Result<(), SessionStoreError>>;

    fn list_subkeys<'a>(
        &'a self,
        key: &'a SessionListSubkeysKey,
    ) -> BoxFuture<'a, Result<Vec<String>, SessionStoreError>>;
}

impl<S: SessionStore> ErasedSessionStore for S {
    fn append<'a>(
        &'a self,
        key: &'a SessionKey,
        entries: &'a [SessionStoreEntry],
    ) -> BoxFuture<'a, Result<(), SessionStoreError>> {
        Box::pin(SessionStore::append(self, key, entries))
    }

    fn load<'a>(
        &'a self,
        key: &'a SessionKey,
    ) -> BoxFuture<'a, Result<Option<Vec<SessionStoreEntry>>, SessionStoreError>> {
        Box::pin(SessionStore::load(self, key))
    }

    fn capabilities(&self) -> SessionStoreCapabilities {
        SessionStore::capabilities(self)
    }

    fn list_sessions<'a>(
        &'a self,
        project_key: &'a str,
    ) -> BoxFuture<'a, Result<Vec<SessionStoreListEntry>, SessionStoreError>> {
        Box::pin(SessionStore::list_sessions(self, project_key))
    }

    fn list_session_summaries<'a>(
        &'a self,
        project_key: &'a str,
    ) -> BoxFuture<'a, Result<Vec<SessionSummaryEntry>, SessionStoreError>> {
        Box::pin(SessionStore::list_session_summaries(self, project_key))
    }

    fn delete<'a>(&'a self, key: &'a SessionKey) -> BoxFuture<'a, Result<(), SessionStoreError>> {
        Box::pin(SessionStore::delete(self, key))
    }

    fn list_subkeys<'a>(
        &'a self,
        key: &'a SessionListSubkeysKey,
    ) -> BoxFuture<'a, Result<Vec<String>, SessionStoreError>> {
        Box::pin(SessionStore::list_subkeys(self, key))
    }
}

/// Shared handle to a [`SessionStore`], as held by the options.
///
/// Cloning the handle shares the same store.
#[derive(Clone)]
pub struct SharedSessionStore(Arc<dyn ErasedSessionStore>);

impl SharedSessionStore {
    /// Wraps a store. Wrapping a handle returns a handle to the same store.
    pub fn new(store: impl SessionStore) -> Self {
        let any: &dyn std::any::Any = &store;
        match any.downcast_ref::<Self>() {
            Some(shared) => shared.clone(),
            None => Self(Arc::new(store)),
        }
    }

    /// See [`SessionStore::capabilities`].
    #[must_use]
    pub fn capabilities(&self) -> SessionStoreCapabilities {
        self.0.capabilities()
    }

    /// See [`SessionStore::append`].
    ///
    /// # Errors
    ///
    /// Returns the error of the store.
    pub async fn append(
        &self,
        key: &SessionKey,
        entries: &[SessionStoreEntry],
    ) -> Result<(), SessionStoreError> {
        self.0.append(key, entries).await
    }

    /// See [`SessionStore::load`].
    ///
    /// # Errors
    ///
    /// Returns the error of the store.
    pub async fn load(
        &self,
        key: &SessionKey,
    ) -> Result<Option<Vec<SessionStoreEntry>>, SessionStoreError> {
        self.0.load(key).await
    }

    /// See [`SessionStore::list_sessions`].
    ///
    /// # Errors
    ///
    /// Returns the error of the store, or [`UnsupportedStoreOperation`]
    /// when the store does not implement the method.
    pub async fn list_sessions(
        &self,
        project_key: &str,
    ) -> Result<Vec<SessionStoreListEntry>, SessionStoreError> {
        self.0.list_sessions(project_key).await
    }

    /// See [`SessionStore::list_session_summaries`].
    ///
    /// # Errors
    ///
    /// Returns the error of the store, or [`UnsupportedStoreOperation`]
    /// when the store does not implement the method.
    pub async fn list_session_summaries(
        &self,
        project_key: &str,
    ) -> Result<Vec<SessionSummaryEntry>, SessionStoreError> {
        self.0.list_session_summaries(project_key).await
    }

    /// See [`SessionStore::delete`].
    ///
    /// # Errors
    ///
    /// Returns the error of the store, or [`UnsupportedStoreOperation`]
    /// when the store does not implement the method.
    pub async fn delete(&self, key: &SessionKey) -> Result<(), SessionStoreError> {
        self.0.delete(key).await
    }

    /// See [`SessionStore::list_subkeys`].
    ///
    /// # Errors
    ///
    /// Returns the error of the store, or [`UnsupportedStoreOperation`]
    /// when the store does not implement the method.
    pub async fn list_subkeys(
        &self,
        key: &SessionListSubkeysKey,
    ) -> Result<Vec<String>, SessionStoreError> {
        self.0.list_subkeys(key).await
    }
}

impl SessionStore for SharedSessionStore {
    async fn append(
        &self,
        key: &SessionKey,
        entries: &[SessionStoreEntry],
    ) -> Result<(), SessionStoreError> {
        self.0.append(key, entries).await
    }

    async fn load(
        &self,
        key: &SessionKey,
    ) -> Result<Option<Vec<SessionStoreEntry>>, SessionStoreError> {
        self.0.load(key).await
    }

    fn capabilities(&self) -> SessionStoreCapabilities {
        self.0.capabilities()
    }

    async fn list_sessions(
        &self,
        project_key: &str,
    ) -> Result<Vec<SessionStoreListEntry>, SessionStoreError> {
        self.0.list_sessions(project_key).await
    }

    async fn list_session_summaries(
        &self,
        project_key: &str,
    ) -> Result<Vec<SessionSummaryEntry>, SessionStoreError> {
        self.0.list_session_summaries(project_key).await
    }

    async fn delete(&self, key: &SessionKey) -> Result<(), SessionStoreError> {
        self.0.delete(key).await
    }

    async fn list_subkeys(
        &self,
        key: &SessionListSubkeysKey,
    ) -> Result<Vec<String>, SessionStoreError> {
        self.0.list_subkeys(key).await
    }
}

/// A store shared with `Arc` is a store, so the application can keep a
/// handle on the store it hands to the options.
impl<S: SessionStore> SessionStore for std::sync::Arc<S> {
    fn append(
        &self,
        key: &SessionKey,
        entries: &[SessionStoreEntry],
    ) -> impl Future<Output = Result<(), SessionStoreError>> + Send {
        S::append(self, key, entries)
    }

    fn load(
        &self,
        key: &SessionKey,
    ) -> impl Future<Output = Result<Option<Vec<SessionStoreEntry>>, SessionStoreError>> + Send
    {
        S::load(self, key)
    }

    fn capabilities(&self) -> SessionStoreCapabilities {
        S::capabilities(self)
    }

    fn list_sessions(
        &self,
        project_key: &str,
    ) -> impl Future<Output = Result<Vec<SessionStoreListEntry>, SessionStoreError>> + Send {
        S::list_sessions(self, project_key)
    }

    fn list_session_summaries(
        &self,
        project_key: &str,
    ) -> impl Future<Output = Result<Vec<SessionSummaryEntry>, SessionStoreError>> + Send {
        S::list_session_summaries(self, project_key)
    }

    fn delete(
        &self,
        key: &SessionKey,
    ) -> impl Future<Output = Result<(), SessionStoreError>> + Send {
        S::delete(self, key)
    }

    fn list_subkeys(
        &self,
        key: &SessionListSubkeysKey,
    ) -> impl Future<Output = Result<Vec<String>, SessionStoreError>> + Send {
        S::list_subkeys(self, key)
    }
}

impl fmt::Debug for SharedSessionStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SharedSessionStore")
            .field("capabilities", &self.capabilities())
            .finish_non_exhaustive()
    }
}
