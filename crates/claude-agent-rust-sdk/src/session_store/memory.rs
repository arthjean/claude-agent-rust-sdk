//! The in-memory reference store.

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use indexmap::IndexMap;

use super::{
    SessionKey, SessionListSubkeysKey, SessionStore, SessionStoreCapabilities, SessionStoreEntry,
    SessionStoreError, SessionStoreListEntry, SessionSummaryEntry,
};
use crate::sessions::{fold_session_summary, now_millis};

#[derive(Default)]
struct State {
    entries: IndexMap<String, Vec<SessionStoreEntry>>,
    mtimes: HashMap<String, i64>,
    summaries: IndexMap<(String, String), SessionSummaryEntry>,
    last_mtime: i64,
}

impl State {
    fn next_mtime(&mut self) -> i64 {
        let now = now_millis().max(self.last_mtime + 1);
        self.last_mtime = now;
        now
    }

    fn remove(&mut self, key: &str) {
        self.entries.shift_remove(key);
        self.mtimes.remove(key);
    }
}

fn key_to_string(key: &SessionKey) -> String {
    match key.subpath.as_deref().filter(|subpath| !subpath.is_empty()) {
        Some(subpath) => format!("{}/{}/{subpath}", key.project_key, key.session_id),
        None => format!("{}/{}", key.project_key, key.session_id),
    }
}

/// [`SessionStore`] held in memory, for tests and development.
///
/// It implements every optional method, keeps a summary per session with
/// [`fold_session_summary`](crate::fold_session_summary), and stamps
/// strictly increasing modification times. Its content is lost when the
/// process exits. Clones share the same content.
///
/// ```
/// use claude_agent_kit::{InMemorySessionStore, SessionKey, SessionStore};
///
/// # async fn demo() -> Result<(), claude_agent_kit::SessionStoreError> {
/// let store = InMemorySessionStore::new();
/// let key = SessionKey::new("project", "session");
/// let entry = serde_json::json!({"type": "user", "uuid": "u1"});
/// store.append(&key, &[entry.as_object().cloned().unwrap_or_default()]).await?;
/// assert_eq!(store.load(&key).await?.map(|entries| entries.len()), Some(1));
/// # Ok(())
/// # }
/// ```
#[derive(Clone, Default)]
pub struct InMemorySessionStore {
    state: Arc<Mutex<State>>,
}

impl InMemorySessionStore {
    /// Creates an empty store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Every entry stored under a key, empty when none was appended.
    #[must_use]
    pub fn get_entries(&self, key: &SessionKey) -> Vec<SessionStoreEntry> {
        self.state()
            .entries
            .get(&key_to_string(key))
            .cloned()
            .unwrap_or_default()
    }

    /// Number of main transcripts stored, subagent transcripts excluded.
    #[must_use]
    pub fn size(&self) -> usize {
        self.state()
            .entries
            .keys()
            .filter(|key| {
                key.split_once('/')
                    .is_some_and(|(_, rest)| !rest.contains('/'))
            })
            .count()
    }

    /// Removes everything.
    pub fn clear(&self) {
        *self.state() = State::default();
    }
}

impl fmt::Debug for InMemorySessionStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("InMemorySessionStore")
            .field("size", &self.size())
            .finish_non_exhaustive()
    }
}

impl SessionStore for InMemorySessionStore {
    async fn append(
        &self,
        key: &SessionKey,
        entries: &[SessionStoreEntry],
    ) -> Result<(), SessionStoreError> {
        let mut state = self.state();
        let text_key = key_to_string(key);
        state
            .entries
            .entry(text_key.clone())
            .or_default()
            .extend(entries.iter().cloned());
        let now = state.next_mtime();
        if key.subpath.is_none() {
            let summary_key = (key.project_key.clone(), key.session_id.clone());
            let mut folded = fold_session_summary(state.summaries.get(&summary_key), key, entries);
            folded.mtime = now;
            state.summaries.insert(summary_key, folded);
        }
        state.mtimes.insert(text_key, now);
        Ok(())
    }

    async fn load(
        &self,
        key: &SessionKey,
    ) -> Result<Option<Vec<SessionStoreEntry>>, SessionStoreError> {
        Ok(self.state().entries.get(&key_to_string(key)).cloned())
    }

    fn capabilities(&self) -> SessionStoreCapabilities {
        SessionStoreCapabilities::all()
    }

    async fn list_sessions(
        &self,
        project_key: &str,
    ) -> Result<Vec<SessionStoreListEntry>, SessionStoreError> {
        let state = self.state();
        let prefix = format!("{project_key}/");
        Ok(state
            .entries
            .keys()
            .filter_map(|key| {
                let rest = key.strip_prefix(&prefix)?;
                (!rest.contains('/')).then(|| {
                    SessionStoreListEntry::new(rest, state.mtimes.get(key).copied().unwrap_or(0))
                })
            })
            .collect())
    }

    async fn list_session_summaries(
        &self,
        project_key: &str,
    ) -> Result<Vec<SessionSummaryEntry>, SessionStoreError> {
        Ok(self
            .state()
            .summaries
            .iter()
            .filter(|((project, _), _)| project == project_key)
            .map(|(_, summary)| summary.clone())
            .collect())
    }

    async fn delete(&self, key: &SessionKey) -> Result<(), SessionStoreError> {
        let mut state = self.state();
        state.remove(&key_to_string(key));
        if key.subpath.is_none() {
            state
                .summaries
                .shift_remove(&(key.project_key.clone(), key.session_id.clone()));
            let prefix = format!("{}/{}/", key.project_key, key.session_id);
            let subkeys: Vec<String> = state
                .entries
                .keys()
                .filter(|stored| stored.starts_with(&prefix))
                .cloned()
                .collect();
            for subkey in subkeys {
                state.remove(&subkey);
            }
        }
        Ok(())
    }

    async fn list_subkeys(
        &self,
        key: &SessionListSubkeysKey,
    ) -> Result<Vec<String>, SessionStoreError> {
        let prefix = format!("{}/{}/", key.project_key, key.session_id);
        Ok(self
            .state()
            .entries
            .keys()
            .filter_map(|stored| stored.strip_prefix(&prefix).map(str::to_owned))
            .collect())
    }
}
