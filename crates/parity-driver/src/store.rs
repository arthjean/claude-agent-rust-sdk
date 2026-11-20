//! The session store of a case with `"session_store": "memory"`: an
//! in-memory store that journals every append the SDK makes, and counts
//! them, so that the `message` event of a result can tell how many appends
//! the application could rely on when the result reached it.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use claude_agent_rust_sdk::{
    InMemorySessionStore, SessionKey, SessionListSubkeysKey, SessionStore,
    SessionStoreCapabilities, SessionStoreEntry, SessionStoreError, SessionStoreListEntry,
    SessionSummaryEntry,
};
use serde_json::Value;

use crate::journal;

/// [`InMemorySessionStore`] with every append written to the journal as a
/// `store_append` event, before the store applies it.
pub(crate) struct JournaledStore(InMemorySessionStore);

static CONFIGURED: AtomicBool = AtomicBool::new(false);
static APPENDS: AtomicUsize = AtomicUsize::new(0);

impl JournaledStore {
    pub(crate) fn new() -> Self {
        CONFIGURED.store(true, Ordering::SeqCst);
        Self(InMemorySessionStore::new())
    }
}

/// The number of appends made so far, when the case has a store.
pub(crate) fn appends() -> Option<usize> {
    CONFIGURED
        .load(Ordering::SeqCst)
        .then(|| APPENDS.load(Ordering::SeqCst))
}

impl SessionStore for JournaledStore {
    async fn append(
        &self,
        key: &SessionKey,
        entries: &[SessionStoreEntry],
    ) -> Result<(), SessionStoreError> {
        journal::write(
            "store_append",
            journal::fields([
                ("key", journal::key(key)),
                (
                    "entries",
                    entries.iter().cloned().map(Value::Object).collect(),
                ),
            ]),
        );
        let outcome = self.0.append(key, entries).await;
        APPENDS.fetch_add(1, Ordering::SeqCst);
        outcome
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
