//! Options of the session functions that take more than a session id and a
//! directory.
//!
//! Python passes these as keyword arguments with defaults; each struct
//! here holds one function family's arguments, with the same defaults, so
//! a call names what it sets: `list_sessions(ListSessionsOptions::new().limit(20))`.

use std::path::PathBuf;

use super::store::MAX_PENDING_ENTRIES;
use crate::types::macros::setters;

/// Options of [`list_sessions`](crate::list_sessions) and
/// [`list_sessions_from_store`](crate::list_sessions_from_store).
///
/// The default lists every project, with no limit, no offset, and the git
/// worktrees of the project included, as upstream does.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ListSessionsOptions {
    /// Project to list. `None`, or an empty path, lists every project on
    /// disk; a store lists the project of the current directory.
    pub directory: Option<PathBuf>,
    /// Most sessions to return; `None` or 0 returns them all.
    pub limit: Option<usize>,
    /// Sessions to skip, newest first, before the limit applies.
    pub offset: usize,
    /// Whether the sessions of the other git worktrees of the project's
    /// repository are listed too. It has no effect on
    /// [`list_sessions_from_store`](crate::list_sessions_from_store): store
    /// keys hold no worktree layout.
    pub include_worktrees: bool,
}

impl Default for ListSessionsOptions {
    fn default() -> Self {
        Self {
            directory: None,
            limit: None,
            offset: 0,
            include_worktrees: true,
        }
    }
}

impl ListSessionsOptions {
    /// Creates the default options.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    setters! {
        opt directory: PathBuf;
        val limit: usize;
        plain offset: usize;
        plain include_worktrees: bool;
    }
}

/// Options of [`get_session_messages`](crate::get_session_messages),
/// [`get_subagent_messages`](crate::get_subagent_messages), and their
/// `_from_store` counterparts.
///
/// The default searches every project (a store: the project of the current
/// directory) and returns every message.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct SessionMessagesOptions {
    /// Project the session belongs to. `None` searches every project on
    /// disk; a store reads the project of the current directory.
    pub directory: Option<PathBuf>,
    /// Most messages to return; `None` or 0 returns them all.
    pub limit: Option<usize>,
    /// Messages to skip, oldest first, before the limit applies.
    pub offset: usize,
}

impl SessionMessagesOptions {
    /// Creates the default options.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    setters! {
        opt directory: PathBuf;
        val limit: usize;
        plain offset: usize;
    }
}

/// Options of [`fork_session`](crate::fork_session) and
/// [`fork_session_via_store`](crate::fork_session_via_store).
///
/// The default copies the whole transcript and titles the fork after its
/// source.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct ForkSessionOptions {
    /// Project the session belongs to. `None` searches every project on
    /// disk; a store reads the project of the current directory.
    pub directory: Option<PathBuf>,
    /// Id of the last message to copy, included. `None` or an empty id
    /// copies the whole transcript.
    pub up_to_message_id: Option<String>,
    /// Title of the fork. `None` or a blank title uses the title of the
    /// source followed by ` (fork)`.
    pub title: Option<String>,
}

impl ForkSessionOptions {
    /// Creates the default options.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    setters! {
        opt directory: PathBuf;
        opt up_to_message_id: String;
        opt title: String;
    }
}

/// Options of [`import_session_to_store`](crate::import_session_to_store).
///
/// The default searches every project, copies the subagent transcripts,
/// and appends 500 entries at a time, as upstream does.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ImportSessionOptions {
    /// Project the session belongs to. `None` searches every project on
    /// disk.
    pub directory: Option<PathBuf>,
    /// Whether the transcripts under `<session>/subagents/` are copied too.
    pub include_subagents: bool,
    /// Most entries per append. 0 means the default, 500.
    pub batch_size: usize,
}

impl Default for ImportSessionOptions {
    fn default() -> Self {
        Self {
            directory: None,
            include_subagents: true,
            batch_size: MAX_PENDING_ENTRIES,
        }
    }
}

impl ImportSessionOptions {
    /// Creates the default options.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    setters! {
        opt directory: PathBuf;
        plain include_subagents: bool;
    }

    /// Sets `batch_size`, the most entries per append; an append also
    /// stops at 1 MiB of lines. 0 means the default, 500, as in Python.
    #[must_use]
    pub fn batch_size(mut self, value: usize) -> Self {
        self.batch_size = value;
        self
    }
}
