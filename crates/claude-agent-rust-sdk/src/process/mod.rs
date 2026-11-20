//! Spawning and supervision of the CLI process tree.
//!
//! A supervisor task owns the child. It publishes the exit code, and runs
//! the bounded shutdown when asked to or when every handle to the process
//! is dropped: wait, then terminate, then kill, each step limited to
//! [`SHUTDOWN_STEP`]. Because the task is detached, the shutdown finishes
//! even when the caller that requested it is cancelled.
//!
//! When the task itself is dropped, as when the runtime shuts down because
//! the program returns from `main`, nothing can wait any more: on Unix the
//! process group gets `SIGTERM`, so the CLI ends its transcript and exits,
//! and the child is not killed on drop; on Windows the Job Object, closed
//! with its handle, ends every process at once.

pub(crate) mod sys;

use std::io;
use std::process::ExitStatus;
use std::time::Duration;

use tokio::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command};
use tokio::sync::{oneshot, watch};
use tokio::time::{Instant, timeout, timeout_at};

use sys::ProcessTree;

pub(crate) use sys::run_as_user;

pub(crate) const SHUTDOWN_STEP: Duration = Duration::from_secs(5);
const UNKNOWN_EXIT_CODE: i32 = -1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Exit {
    pub(crate) code: i32,
    pub(crate) forced: bool,
}

#[derive(Debug)]
pub(crate) struct ProcessHandle {
    shutdown: Option<oneshot::Sender<()>>,
    exit: watch::Receiver<Option<Exit>>,
    id: Option<u32>,
}

impl ProcessHandle {
    pub(crate) fn id(&self) -> Option<u32> {
        self.id
    }

    pub(crate) fn exit(&self) -> watch::Receiver<Option<Exit>> {
        self.exit.clone()
    }

    pub(crate) async fn shut_down(mut self, deadline: Instant) -> Option<Exit> {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        match timeout_at(deadline, self.exit.wait_for(Option::is_some)).await {
            Ok(Ok(exit)) => *exit,
            _ => None,
        }
    }
}

pub(crate) async fn wait_for_exit_code(exit: &mut watch::Receiver<Option<Exit>>) -> i32 {
    match exit.wait_for(Option::is_some).await {
        Ok(exit) => exit.map_or(UNKNOWN_EXIT_CODE, |exit| exit.code),
        Err(_) => UNKNOWN_EXIT_CODE,
    }
}

pub(crate) struct Spawned {
    pub(crate) streams: Streams,
    pub(crate) handle: ProcessHandle,
}

pub(crate) struct Streams {
    pub(crate) stdin: Option<ChildStdin>,
    pub(crate) stdout: Option<ChildStdout>,
    pub(crate) stderr: Option<ChildStderr>,
}

pub(crate) async fn spawn(mut command: Command) -> io::Result<Spawned> {
    command.kill_on_drop(cfg!(windows));
    sys::prepare(&mut command);
    let mut child = spawn_child(command).await?;
    let tree = ProcessTree::attach(&child);
    let id = child.id();
    let streams = Streams {
        stdin: child.stdin.take(),
        stdout: child.stdout.take(),
        stderr: child.stderr.take(),
    };

    let (shutdown, shutdown_requested) = oneshot::channel();
    let (exit_sender, exit) = watch::channel(None);
    tokio::spawn(supervise(
        Supervised { tree, child },
        shutdown_requested,
        exit_sender,
    ));

    Ok(Spawned {
        streams,
        handle: ProcessHandle {
            shutdown: Some(shutdown),
            exit,
            id,
        },
    })
}

#[cfg(not(target_os = "linux"))]
async fn spawn_child(mut command: Command) -> io::Result<Child> {
    command.spawn()
}

#[cfg(target_os = "linux")]
async fn spawn_child(command: Command) -> io::Result<Child> {
    spawn_thread::spawn(command).await
}

/// Spawning thread for Linux.
///
/// The kernel delivers the parent-death signal when the thread that
/// spawned the child exits, not when the whole process does. Spawning from
/// one thread that lives as long as the process ties the signal to the
/// death of the process, whichever runtime thread asked for the spawn.
#[cfg(target_os = "linux")]
mod spawn_thread {
    use std::io;
    use std::sync::OnceLock;
    use std::sync::mpsc::{Sender, channel};

    use tokio::process::{Child, Command};
    use tokio::runtime::Handle;
    use tokio::sync::oneshot;

    const THREAD_NAME: &str = "claude-agent-rust-sdk-spawn";

    type Request = (Command, Handle, oneshot::Sender<io::Result<Child>>);

    fn queue() -> Option<&'static Sender<Request>> {
        static QUEUE: OnceLock<Option<Sender<Request>>> = OnceLock::new();
        QUEUE
            .get_or_init(|| {
                let (sender, receiver) = channel::<Request>();
                std::thread::Builder::new()
                    .name(THREAD_NAME.to_owned())
                    .spawn(move || {
                        for (mut command, runtime, reply) in receiver {
                            let _context = runtime.enter();
                            let _ = reply.send(command.spawn());
                        }
                    })
                    .ok()
                    .map(|_| sender)
            })
            .as_ref()
    }

    pub(super) async fn spawn(mut command: Command) -> io::Result<Child> {
        let Some(queue) = queue() else {
            return command.spawn();
        };
        let (reply, spawned) = oneshot::channel();
        match queue.send((command, Handle::current(), reply)) {
            Ok(()) => spawned
                .await
                .unwrap_or_else(|_| Err(io::Error::other("the spawning thread stopped"))),
            Err(rejected) => {
                let (mut command, _, _) = rejected.0;
                command.spawn()
            }
        }
    }
}

struct Supervised {
    tree: ProcessTree,
    child: Child,
}

impl Supervised {
    async fn shut_down(&mut self) -> (io::Result<ExitStatus>, bool) {
        if let Ok(status) = timeout(SHUTDOWN_STEP, self.child.wait()).await {
            return (status, false);
        }
        self.tree.terminate(&mut self.child);
        if let Ok(status) = timeout(SHUTDOWN_STEP, self.child.wait()).await {
            return (status, true);
        }
        self.tree.kill(&mut self.child);
        match timeout(SHUTDOWN_STEP, self.child.wait()).await {
            Ok(status) => (status, true),
            Err(elapsed) => (Err(io::Error::new(io::ErrorKind::TimedOut, elapsed)), true),
        }
    }
}

async fn supervise(
    mut supervised: Supervised,
    mut shutdown_requested: oneshot::Receiver<()>,
    exit: watch::Sender<Option<Exit>>,
) {
    let (status, forced) = tokio::select! {
        status = supervised.child.wait() => (status, false),
        _ = &mut shutdown_requested => supervised.shut_down().await,
    };
    match status {
        Ok(status) => {
            supervised.tree.sweep();
            let _ = exit.send(Some(Exit {
                code: exit_code(status),
                forced,
            }));
        }
        Err(error) => {
            tracing::warn!(%error, "the Claude CLI process could not be reaped");
        }
    }
}

#[cfg(unix)]
fn exit_code(status: ExitStatus) -> i32 {
    use std::os::unix::process::ExitStatusExt;

    status
        .code()
        .or_else(|| status.signal().map(|signal| -signal))
        .unwrap_or(UNKNOWN_EXIT_CODE)
}

#[cfg(not(unix))]
fn exit_code(status: ExitStatus) -> i32 {
    status.code().unwrap_or(UNKNOWN_EXIT_CODE)
}
