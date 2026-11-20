//! Blocking work moved off the async runtime.
//!
//! File reads and writes, directory walks, a child process waited on (the
//! macOS Keychain reader, `git worktree list`), and user database lookups
//! (`getpwnam_r`, which may query a directory service) block the thread
//! they run on. Run inline in an async function, they would freeze every
//! other task of a `current_thread` runtime, timers included. The async
//! paths of the crate run them through [`run`], on the blocking pool of
//! tokio.

use crate::error::{Error, Result};

/// Runs `work` on the blocking pool and returns what it returns.
///
/// `work` runs in the `tracing` span of the caller. A panic in `work`
/// resumes on the caller, as it would have inline.
///
/// # Errors
///
/// Returns [`ErrorKind::Io`](crate::ErrorKind::Io) when the runtime shuts
/// down before `work` runs.
pub(crate) async fn run<T, F>(work: F) -> Result<T>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    let span = tracing::Span::current();
    match tokio::task::spawn_blocking(move || span.in_scope(work)).await {
        Ok(value) => Ok(value),
        Err(error) => match error.try_into_panic() {
            Ok(payload) => std::panic::resume_unwind(payload),
            Err(error) => Err(Error::io(
                "The runtime shut down before blocking work could run",
                std::io::Error::other(error.to_string()),
            )),
        },
    }
}
