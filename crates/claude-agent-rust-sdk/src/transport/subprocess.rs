//! Transport that runs the CLI as a child process.

use std::ffi::OsString;
use std::fmt;
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::Duration;

use futures_util::FutureExt;
use futures_util::future::Shared;
use futures_util::stream;
use serde_json::Value;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::process::{ChildStdin, Command};
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;
use tokio::time::{Instant, timeout, timeout_at};

use super::command::build_arguments;
use super::environment::{active_trace_context, build_environment};
use super::framing::{LineFramer, oversize_error, parse_stdout_line};
use super::{MessageStream, Transport};
use crate::cli::{Platform, find_cli, reject_windows_batch_cli};
use crate::error::{Error, Result};
use crate::process::{self, Exit, ProcessHandle, wait_for_exit_code};
use crate::types::{BoxFuture, ClaudeAgentOptions, StderrCallback};

const DEFAULT_MAX_BUFFER_SIZE: usize = 1024 * 1024;
const MESSAGE_QUEUE_CAPACITY: usize = 100;
const STDOUT_CHUNK_SIZE: usize = 64 * 1024;
const STDERR_CHUNK_SIZE: usize = 8 * 1024;
const SKIP_VERSION_CHECK_VARIABLE: &str = "CLAUDE_AGENT_SDK_SKIP_VERSION_CHECK";
const VERBATIM_PROMPTS_MINIMUM_CLI_VERSION: (u64, u64, u64) = (2, 1, 248);
const VERSION_COMPONENT_MAX_DIGITS: usize = 9;
const CLOSE_BUDGET: Duration = Duration::from_secs(20);
const WRITE_LOCK_LIMIT: Duration = Duration::from_secs(5);
const STDERR_STOP_LIMIT: Duration = Duration::from_secs(1);
const OUTPUT_DRAIN_LIMIT: Duration = Duration::from_secs(1);
const SKIPPED_TAIL_PREVIEW_CHARS: usize = 200;
/// Bytes of error output a process error keeps, the last lines first kept.
const STDERR_TAIL_BYTES: usize = 8 * 1024;
/// Time the end of the error output is awaited once the process failed.
const STDERR_TAIL_WAIT: Duration = Duration::from_millis(200);

type SharedClose = Shared<BoxFuture<'static, ()>>;

/// Transport that spawns the CLI as a child process and exchanges
/// newline-delimited JSON over its standard input and output.
///
/// # Process lifecycle
///
/// The CLI is started from an argument vector, never through a shell. It
/// runs in its own process group (Unix) or in a Job Object without a
/// console window (Windows), so that it and its descendants end with the
/// session. As a consequence the CLI does not receive the terminal's
/// Ctrl+C: this transport owns its lifecycle.
///
/// [`close`](Self::close) closes the standard input, waits up to 5 s for
/// the CLI to exit, then terminates it and waits up to 5 s, then kills it.
/// The shutdown runs on a detached task: it finishes even when the future
/// returned by `close` is dropped. Dropping the transport without calling
/// `close` starts the same shutdown in the background. On Linux the CLI
/// also receives `SIGTERM` if this process dies; on Windows the Job Object
/// ends it.
///
/// # Message framing
///
/// One task owns the standard output. It splits on line feeds only,
/// reassembles messages split across reads, skips blank lines and lines
/// that do not start with `{`, and stops at the first line that looks like
/// JSON but does not parse. A message longer than `max_buffer_size` bytes
/// (default 1,048,576) is an error. The limit counts bytes, where the
/// upstream Python SDK counts characters.
///
/// # Cancellation
///
/// Called directly, [`write`](Self::write) must run to completion:
/// dropping its future midway can leave a partial message on the standard
/// input of the CLI. [`query`](crate::query) and
/// [`ClaudeSdkClient`](crate::ClaudeSdkClient) never do that: they write
/// through one task that finishes every frame it starts, so their own
/// futures can be dropped at any point.
///
/// # Reconnection
///
/// [`connect`](Self::connect) after [`close`](Self::close) waits for the
/// shutdown of the previous process, then starts a new one, as the upstream
/// transport does. So a [`ClaudeSdkClient`](crate::ClaudeSdkClient) built
/// over this transport connects again after a disconnect. Each connection
/// has its own state: [`process_id`](Self::process_id) names the current
/// process and [`read_messages`](Self::read_messages) hands out its output.
pub struct SubprocessTransport {
    options: Arc<ClaudeAgentOptions>,
    skip_version_check: bool,
    connecting: tokio::sync::Mutex<()>,
    inner: Mutex<Arc<Inner>>,
}

/// The state of one connection. A reconnect replaces it whole.
struct Inner {
    shared: Arc<SharedState>,
    stdin: tokio::sync::Mutex<Option<ChildStdin>>,
    state: Mutex<State>,
}

impl Inner {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            shared: Arc::new(SharedState::default()),
            stdin: tokio::sync::Mutex::new(None),
            state: Mutex::new(State::Idle),
        })
    }
}

#[derive(Default)]
struct SharedState {
    stderr_tail: StderrTail,
    ready: AtomicBool,
    closing: AtomicBool,
    /// Wakes a write still blocked on a full pipe once `close` has waited
    /// [`WRITE_LOCK_LIMIT`] for it, so the write gives up the pipe and
    /// stdin is closed, as upstream closes it without the write lock.
    close_requested: tokio::sync::Notify,
    /// The error the process ended with, kept typed so a later write can
    /// name it as its source, as upstream raises `from self._exit_error`.
    exit_error: Mutex<Option<Error>>,
    exit: OnceLock<watch::Receiver<Option<Exit>>>,
    process_id: OnceLock<u32>,
}

enum State {
    Idle,
    Connected(Box<Connection>),
    Closed(SharedClose),
}

struct Connection {
    process: ProcessHandle,
    messages: Option<mpsc::Receiver<Result<Value>>>,
    reader: JoinHandle<()>,
    stderr: Option<StderrTask>,
}

struct StderrTask {
    stop: oneshot::Sender<()>,
    task: JoinHandle<()>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl SubprocessTransport {
    /// Creates a transport for these options. Nothing is spawned until
    /// [`connect`](Self::connect).
    #[must_use]
    pub fn new(options: ClaudeAgentOptions) -> Self {
        let skip_version_check =
            std::env::var_os(SKIP_VERSION_CHECK_VARIABLE).is_some_and(|value| !value.is_empty());
        Self {
            options: Arc::new(options),
            skip_version_check,
            connecting: tokio::sync::Mutex::new(()),
            inner: Mutex::new(Inner::new()),
        }
    }

    /// The state of the current connection.
    fn inner(&self) -> Arc<Inner> {
        Arc::clone(&lock(&self.inner))
    }

    /// Builds the arguments [`connect`](Self::connect) passes to the CLI,
    /// without the executable itself.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::Config`](crate::ErrorKind::Config) for an invalid skill name, a plugin that
    /// is not local, or, on Windows, a resume or session value containing a
    /// `cmd.exe` metacharacter. Returns [`ErrorKind::Spawn`](crate::ErrorKind::Spawn) when the
    /// settings to merge with the sandbox options cannot be loaded.
    pub fn command_arguments(&self) -> Result<Vec<OsString>> {
        build_arguments(&self.options, Platform::current())
    }

    /// Identifier of the CLI process, once connected.
    #[must_use]
    pub fn process_id(&self) -> Option<u32> {
        self.inner().shared.process_id.get().copied()
    }

    /// Starts the CLI process. Connecting twice is a no-op; connecting
    /// after [`close`](Self::close) waits for the previous process to end,
    /// then starts a new one.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::CliNotFound`](crate::ErrorKind::CliNotFound) when the executable is missing,
    /// [`ErrorKind::Spawn`](crate::ErrorKind::Spawn) when it is a batch script on Windows, when the working
    /// directory does not exist, when the `user` option names an unknown
    /// user or is set on Windows, or when the process cannot start, and the
    /// errors of
    /// [`command_arguments`](Self::command_arguments).
    pub async fn connect(&self) -> Result<()> {
        let _connecting = self.connecting.lock().await;
        let previous = match &*lock(&self.inner().state) {
            State::Idle => None,
            State::Connected(_) => return Ok(()),
            State::Closed(closing) => Some(closing.clone()),
        };
        if let Some(closing) = previous {
            closing.await;
            *lock(&self.inner) = Inner::new();
        }
        let inner = self.inner();

        let trace_context = active_trace_context();
        let options = Arc::clone(&self.options);
        let (cli_path, command) =
            crate::blocking::run(move || prepare_command(&options, &trace_context)).await??;

        let spawned = process::spawn(command)
            .await
            .map_err(|error| self.spawn_error(&cli_path, error))?;
        let process::Spawned { streams, handle } = spawned;
        let shared = &inner.shared;
        let _ = shared.exit.set(handle.exit());
        if let Some(id) = handle.id() {
            let _ = shared.process_id.set(id);
        }

        let (sender, messages) = mpsc::channel(MESSAGE_QUEUE_CAPACITY);
        let reader = match streams.stdout {
            Some(stdout) => tokio::spawn(read_output(
                stdout,
                sender,
                OutputContext {
                    shared: Arc::clone(shared),
                    exit: handle.exit(),
                    max_buffer_size: self.max_buffer_size(),
                    version_check: VersionCheck {
                        done: self.skip_version_check,
                        cli_path,
                        verbatim_prompts: self.options.verbatim_prompts,
                    },
                },
            )),
            None => tokio::spawn(std::future::ready(())),
        };
        let stderr = self
            .options
            .stderr
            .clone()
            .zip(streams.stderr)
            .map(|(callback, stream)| {
                let (stop, stopped) = oneshot::channel();
                shared.stderr_tail.start();
                let task = crate::caller_context::spawn(forward_stderr(
                    stream,
                    callback,
                    self.max_buffer_size(),
                    stopped,
                    shared.stderr_tail.clone(),
                ));
                StderrTask { stop, task }
            });

        *inner.stdin.lock().await = streams.stdin;
        *lock(&inner.state) = State::Connected(Box::new(Connection {
            process: handle,
            messages: Some(messages),
            reader,
            stderr,
        }));
        shared.ready.store(true, Ordering::SeqCst);
        Ok(())
    }

    /// Writes raw data to the standard input of the CLI.
    ///
    /// Writes are serialized: concurrent callers never interleave.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::NotConnected`](crate::ErrorKind::NotConnected) when the transport is not ready, and
    /// [`ErrorKind::ConnectionClosed`](crate::ErrorKind::ConnectionClosed) when the process has exited or the write
    /// fails.
    pub async fn write(&self, data: &str) -> Result<()> {
        let inner = self.inner();
        let mut stdin = inner.stdin.lock().await;
        let shared = &inner.shared;
        let Some(pipe) = stdin
            .as_mut()
            .filter(|_| shared.ready.load(Ordering::SeqCst))
        else {
            return Err(Error::not_connected(
                "ProcessTransport is not ready for writing",
            ));
        };
        if let Some(exit) = shared.exit.get().and_then(|exit| *exit.borrow()) {
            return Err(Error::connection_closed(format!(
                "Cannot write to terminated process (exit code: {})",
                exit.code
            )));
        }
        if let Some(error) = lock(&shared.exit_error).clone() {
            return Err(Error::connection_closed(format!(
                "Cannot write to process that exited with error: {error}"
            ))
            .with_source(error));
        }
        let close_requested = shared.close_requested.notified();
        tokio::pin!(close_requested);
        close_requested.as_mut().enable();
        if shared.closing.load(Ordering::SeqCst) {
            return Err(Error::connection_closed("The transport is closing"));
        }
        let written = async {
            pipe.write_all(data.as_bytes()).await?;
            pipe.flush().await
        };
        let written = tokio::select! {
            biased;
            written = written => written,
            () = close_requested => {
                shared.ready.store(false, Ordering::SeqCst);
                stdin.take();
                return Err(Error::connection_closed(
                    "The transport closed while a write to the CLI was blocked",
                ));
            }
        };
        if let Err(error) = written {
            shared.ready.store(false, Ordering::SeqCst);
            let failure =
                Error::connection_closed(format!("Failed to write to process stdin: {error}"))
                    .with_source(error);
            *lock(&shared.exit_error) = Some(failure.clone());
            return Err(failure);
        }
        Ok(())
    }

    /// Returns the stream of messages read from the standard output.
    ///
    /// The stream is handed out once per connection. It yields [`ErrorKind::JsonDecode`](crate::ErrorKind::JsonDecode) for a
    /// corrupt or oversize message and [`ErrorKind::Process`](crate::ErrorKind::Process) when the CLI
    /// exits with a non-zero code, and ends after either. Before
    /// [`connect`](Self::connect), or on a second call, the stream yields
    /// one [`ErrorKind::NotConnected`](crate::ErrorKind::NotConnected).
    pub fn read_messages(&self) -> MessageStream {
        let messages = match &mut *lock(&self.inner().state) {
            State::Connected(connection) => connection.messages.take(),
            State::Idle | State::Closed(_) => None,
        };
        match messages {
            Some(messages) => Box::pin(stream::unfold(messages, |mut messages| async move {
                messages.recv().await.map(|message| (message, messages))
            })),
            None => Box::pin(stream::once(std::future::ready(Err(Error::not_connected(
                "Not connected",
            ))))),
        }
    }

    /// Closes the standard input, then ends the CLI process and its
    /// descendants within a bounded time (20 s in the worst case).
    ///
    /// The shutdown runs on a detached task, so it completes even when this
    /// future is dropped. Calling `close` again waits for that same
    /// shutdown.
    ///
    /// # Errors
    ///
    /// This implementation never fails.
    pub async fn close(&self) -> Result<()> {
        let inner = self.inner();
        let closing = {
            let mut state = lock(&inner.state);
            match std::mem::replace(&mut *state, State::Idle) {
                State::Idle => {
                    inner.shared.ready.store(false, Ordering::SeqCst);
                    return Ok(());
                }
                State::Closed(closing) => {
                    *state = State::Closed(closing.clone());
                    closing
                }
                State::Connected(connection) => {
                    let shutdown = tokio::spawn(shut_down(Arc::clone(&inner), *connection));
                    let closing: BoxFuture<'static, ()> = Box::pin(async move {
                        let _ = shutdown.await;
                    });
                    let closing = closing.shared();
                    *state = State::Closed(closing.clone());
                    closing
                }
            }
        };
        closing.await;
        Ok(())
    }

    /// Whether the process is running and accepts writes.
    #[must_use]
    pub fn is_ready(&self) -> bool {
        self.inner().shared.ready.load(Ordering::SeqCst)
    }

    /// Closes the standard input of the CLI.
    ///
    /// # Errors
    ///
    /// This implementation never fails.
    pub async fn end_input(&self) -> Result<()> {
        let inner = self.inner();
        let mut stdin = inner.stdin.lock().await;
        if let Some(mut pipe) = stdin.take() {
            let _ = pipe.shutdown().await;
        }
        Ok(())
    }

    fn max_buffer_size(&self) -> usize {
        self.options
            .max_buffer_size
            .unwrap_or(DEFAULT_MAX_BUFFER_SIZE)
    }

    fn spawn_error(&self, cli_path: &Path, error: std::io::Error) -> Error {
        if let Some(cwd) = working_directory(&self.options)
            && !cwd.exists()
        {
            return Error::spawn(format!(
                "Working directory does not exist: {}",
                cwd.display()
            ));
        }
        if error.kind() == std::io::ErrorKind::NotFound {
            return Error::cli_not_found(
                Some("Claude Code not found at"),
                Some(&cli_path.to_string_lossy()),
            );
        }
        Error::spawn("Failed to start Claude Code").with_source(error)
    }
}

impl Transport for SubprocessTransport {
    fn connect(&self) -> impl Future<Output = Result<()>> + Send {
        Self::connect(self)
    }

    fn write(&self, data: &str) -> impl Future<Output = Result<()>> + Send {
        Self::write(self, data)
    }

    fn read_messages(&self) -> MessageStream {
        Self::read_messages(self)
    }

    fn close(&self) -> impl Future<Output = Result<()>> + Send {
        Self::close(self)
    }

    fn is_ready(&self) -> bool {
        Self::is_ready(self)
    }

    fn end_input(&self) -> impl Future<Output = Result<()>> + Send {
        Self::end_input(self)
    }
}

impl fmt::Debug for SubprocessTransport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SubprocessTransport")
            .field("ready", &self.is_ready())
            .field("process_id", &self.process_id())
            .finish_non_exhaustive()
    }
}

/// The working directory of the CLI. An empty path is unset, as upstream
/// treats it: the CLI runs in the directory of this process.
fn working_directory(options: &ClaudeAgentOptions) -> Option<&Path> {
    options
        .cwd
        .as_deref()
        .filter(|cwd| !cwd.as_os_str().is_empty())
}

/// Finds the CLI and builds the command that starts it. It blocks: the
/// discovery and the sandbox settings read the file system, and the
/// `user` option looks the user up in the user database, which may query a
/// directory service. [`SubprocessTransport::connect`] runs it on the
/// blocking pool. The trace context is read before, on the thread of the
/// caller, where the OpenTelemetry context is current.
fn prepare_command(
    options: &ClaudeAgentOptions,
    trace_context: &[(String, String)],
) -> Result<(PathBuf, Command)> {
    let cli_path = match &options.cli_path {
        Some(path) => path.clone(),
        None => find_cli()?,
    };
    reject_windows_batch_cli(&cli_path, Platform::current())?;
    let arguments = build_arguments(options, Platform::current())?;
    let environment = build_environment(
        options,
        std::env::vars_os(),
        Platform::current(),
        trace_context,
    );

    let mut command = Command::new(&cli_path);
    command
        .args(arguments)
        .env_clear()
        .envs(environment.into_entries())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(if options.stderr.is_some() {
            Stdio::piped()
        } else {
            Stdio::inherit()
        });
    if let Some(cwd) = working_directory(options) {
        command.current_dir(cwd);
    }
    if let Some(user) = &options.user {
        process::run_as_user(&mut command, user)
            .map_err(|error| Error::spawn("Failed to start Claude Code").with_source(error))?;
    }
    Ok((cli_path, command))
}

async fn shut_down(inner: Arc<Inner>, connection: Connection) {
    let deadline = Instant::now() + CLOSE_BUDGET;
    let shared = &inner.shared;
    shared.closing.store(true, Ordering::SeqCst);
    let Connection {
        process,
        messages,
        mut reader,
        stderr,
    } = connection;

    if let Some(StderrTask { stop, mut task }) = stderr {
        let _ = stop.send(());
        if timeout(STDERR_STOP_LIMIT, &mut task).await.is_err() {
            task.abort();
        }
    }

    let mut stdin = match timeout(WRITE_LOCK_LIMIT, inner.stdin.lock()).await {
        Ok(stdin) => stdin,
        Err(_) => {
            shared.close_requested.notify_waiters();
            inner.stdin.lock().await
        }
    };
    shared.ready.store(false, Ordering::SeqCst);
    stdin.take();
    drop(stdin);

    let exit = process.shut_down(deadline).await;
    let exited_on_its_own = exit.is_some_and(|exit| !exit.forced);
    let drain_until = deadline.min(Instant::now() + OUTPUT_DRAIN_LIMIT);
    if !exited_on_its_own || timeout_at(drain_until, &mut reader).await.is_err() {
        reader.abort();
    }
    drop(messages);
    *lock(&shared.exit_error) = None;
}

struct OutputContext {
    shared: Arc<SharedState>,
    exit: watch::Receiver<Option<Exit>>,
    max_buffer_size: usize,
    version_check: VersionCheck,
}

async fn read_output<R>(
    mut stdout: R,
    sender: mpsc::Sender<Result<Value>>,
    mut context: OutputContext,
) where
    R: AsyncRead + Unpin,
{
    let limit = context.max_buffer_size;
    let mut framer = LineFramer::default();
    let mut chunk = vec![0_u8; STDOUT_CHUNK_SIZE];
    loop {
        let read = match stdout.read(&mut chunk).await {
            Ok(0) => break,
            Ok(read) => read,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) if context.shared.closing.load(Ordering::SeqCst) => break,
            Err(error) => {
                let error = Error::io("Failed to read the output of the CLI", error);
                let _ = sender.send(Err(error)).await;
                return;
            }
        };
        for line in framer.push(&chunk[..read]) {
            if line.len() > limit {
                let _ = sender.send(Err(oversize_error(line.len(), limit))).await;
                return;
            }
            match parse_stdout_line(&line) {
                Ok(None) => {}
                Ok(Some(message)) => {
                    context.version_check.observe(&message);
                    if sender.send(Ok(message)).await.is_err() {
                        return;
                    }
                }
                Err(error) => {
                    let _ = sender.send(Err(error)).await;
                    return;
                }
            }
        }
        if framer.pending_len() > limit {
            let error = oversize_error(framer.pending_len(), limit);
            let _ = sender.send(Err(error)).await;
            return;
        }
    }
    drop(stdout);

    let tail = framer.flush();
    match parse_stdout_line(&tail) {
        Ok(None) => {}
        Ok(Some(message)) => {
            if sender.send(Ok(message)).await.is_err() {
                return;
            }
        }
        Err(_) => {
            let preview: String = String::from_utf8_lossy(&tail)
                .chars()
                .take(SKIPPED_TAIL_PREVIEW_CHARS)
                .collect();
            tracing::debug!(tail = %preview, "dropping truncated JSON at end of CLI stdout");
        }
    }

    let code = wait_for_exit_code(&mut context.exit).await;
    if code != 0 && !context.shared.closing.load(Ordering::SeqCst) {
        let stderr = context.shared.stderr_tail.text().await;
        let error = Error::process(
            format!("Command failed with exit code {code}"),
            Some(code),
            stderr,
        );
        *lock(&context.shared.exit_error) = Some(error.clone());
        let _ = sender.send(Err(error)).await;
    }
}

struct VersionCheck {
    done: bool,
    cli_path: PathBuf,
    verbatim_prompts: bool,
}

impl VersionCheck {
    fn observe(&mut self, message: &Value) {
        if self.done
            || message.get("type").and_then(Value::as_str) != Some("system")
            || message.get("subtype").and_then(Value::as_str) != Some("init")
        {
            return;
        }
        self.done = true;
        let reported = message.get("claude_code_version");
        let version = reported.and_then(Value::as_str).and_then(parse_version);
        let cli_path = self.cli_path.display();
        match version {
            None => tracing::warn!(
                "Claude Code at {cli_path} is too old to report its version, and may be \
                 unsupported in the Agent SDK. Minimum required version is {}. Some features \
                 may not work correctly.",
                crate::MINIMUM_CLI_VERSION,
            ),
            Some(version) => warn_if_untested(version, &cli_path.to_string()),
        }
        if self.verbatim_prompts
            && version.is_none_or(|version| version < VERBATIM_PROMPTS_MINIMUM_CLI_VERSION)
        {
            let (major, minor, patch) = VERBATIM_PROMPTS_MINIMUM_CLI_VERSION;
            let reported = reported.and_then(Value::as_str).unwrap_or("unknown");
            tracing::warn!(
                "verbatim_prompts is enabled, but Claude Code version {reported} at {cli_path} \
                 ignores it: prompts will still have @path mentions expanded and slash commands \
                 dispatched. Claude Code {major}.{minor}.{patch} or later is required.",
            );
        }
    }
}

/// A CLI install, by path and version.
type CliRelease = (String, (u64, u64, u64));

/// The CLI installs already warned about as untested in this process.
static WARNED_UNTESTED: std::sync::Mutex<std::collections::BTreeSet<CliRelease>> =
    std::sync::Mutex::new(std::collections::BTreeSet::new());

/// Warns when the CLI is not [`CLI_VERSION`](crate::CLI_VERSION), the one
/// release this crate is tested against, once per process for each path
/// and version. Python runs the CLI bundled in its wheel, so the two always
/// match there; this crate runs the CLI installed on the machine, which can
/// be any release, and a program that opens many sessions would otherwise
/// log the same warning for each.
fn warn_if_untested(version: (u64, u64, u64), cli_path: &str) {
    let Some(tested) = parse_version(crate::CLI_VERSION) else {
        return;
    };
    if version == tested {
        return;
    }
    let first = WARNED_UNTESTED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert((cli_path.to_owned(), version));
    if !first {
        return;
    }
    let (major, minor, patch) = version;
    let tested_text = crate::CLI_VERSION;
    match version.cmp(&tested) {
        std::cmp::Ordering::Equal => {}
        std::cmp::Ordering::Less => tracing::warn!(
            "Claude Code {major}.{minor}.{patch} at {cli_path} is older than {tested_text}, the \
             version this release of the SDK is tested against: an option it does not know \
             makes it exit with an error. Update Claude Code, or point cli_path at a \
             {tested_text} install."
        ),
        std::cmp::Ordering::Greater => tracing::warn!(
            "Claude Code {major}.{minor}.{patch} at {cli_path} is newer than {tested_text}, the \
             version this release of the SDK is tested against: what it added since may not \
             be handled."
        ),
    }
}

fn parse_version(text: &str) -> Option<(u64, u64, u64)> {
    let (major, rest) = leading_number(text)?;
    let (minor, rest) = leading_number(rest.strip_prefix('.')?)?;
    let patch_digits: String = rest
        .strip_prefix('.')?
        .chars()
        .take_while(char::is_ascii_digit)
        .take(VERSION_COMPONENT_MAX_DIGITS)
        .collect();
    Some((major, minor, patch_digits.parse().ok()?))
}

fn leading_number(text: &str) -> Option<(u64, &str)> {
    let digits = text.bytes().take_while(u8::is_ascii_digit).count();
    if digits == 0 || digits > VERSION_COMPONENT_MAX_DIGITS {
        return None;
    }
    Some((text[..digits].parse().ok()?, &text[digits..]))
}

/// The last lines of error output, kept for [`Error::stderr`] while a
/// task forwards them to the stderr callback.
#[derive(Clone, Default)]
struct StderrTail(Arc<TailState>);

#[derive(Default)]
struct TailState {
    text: Mutex<Option<String>>,
    done: tokio::sync::Notify,
    finished: AtomicBool,
}

impl StderrTail {
    /// Starts the capture; without it, [`Self::text`] stays `None`.
    fn start(&self) {
        *lock(&self.0.text) = Some(String::new());
    }

    fn push(&self, line: &str) {
        let mut text = lock(&self.0.text);
        let Some(text) = text.as_mut() else {
            return;
        };
        text.push_str(line);
        text.push('\n');
        while text.len() > STDERR_TAIL_BYTES {
            match text.find('\n') {
                Some(end) if end + 1 < text.len() => {
                    text.drain(..=end);
                }
                _ => {
                    let mut cut = text.len() - STDERR_TAIL_BYTES;
                    while !text.is_char_boundary(cut) {
                        cut += 1;
                    }
                    text.drain(..cut);
                }
            }
        }
    }

    fn finish(&self) {
        self.0.finished.store(true, Ordering::SeqCst);
        self.0.done.notify_waiters();
    }

    /// The captured text, once the forwarding task has read the end of the
    /// output or [`STDERR_TAIL_WAIT`] has passed; `None` when nothing was
    /// captured.
    async fn text(&self) -> Option<String> {
        if lock(&self.0.text).is_none() {
            return None;
        }
        let done = self.0.done.notified();
        if !self.0.finished.load(Ordering::SeqCst) {
            let _ = timeout(STDERR_TAIL_WAIT, done).await;
        }
        let text = lock(&self.0.text).clone()?;
        let text = text.trim_end();
        (!text.is_empty()).then(|| text.to_owned())
    }
}

async fn forward_stderr<R>(
    mut stream: R,
    callback: StderrCallback,
    max_buffer_size: usize,
    mut stopped: oneshot::Receiver<()>,
    tail: StderrTail,
) where
    R: AsyncRead + Unpin,
{
    let mut framer = LineFramer::default();
    let mut chunk = vec![0_u8; STDERR_CHUNK_SIZE];
    loop {
        let read = tokio::select! {
            biased;
            _ = &mut stopped => break,
            read = stream.read(&mut chunk) => read,
        };
        let read = match read {
            Ok(0) | Err(_) => break,
            Ok(read) => read,
        };
        for line in framer.push(&chunk[..read]) {
            emit_stderr_line(&callback, &tail, &line).await;
        }
        if framer.pending_len() > max_buffer_size {
            emit_stderr_line(&callback, &tail, &framer.flush()).await;
        }
    }
    emit_stderr_line(&callback, &tail, &framer.flush()).await;
    tail.finish();
}

async fn emit_stderr_line(callback: &StderrCallback, tail: &StderrTail, line: &[u8]) {
    let text = String::from_utf8_lossy(line);
    let text = text.trim_end();
    if text.is_empty() {
        return;
    }
    tail.push(text);
    let call = AssertUnwindSafe(|| callback.call(text.to_owned()));
    let Ok(pending) = std::panic::catch_unwind(call) else {
        tracing::debug!("stderr callback panicked; continuing");
        return;
    };
    if AssertUnwindSafe(pending).catch_unwind().await.is_err() {
        tracing::debug!("stderr callback panicked; continuing");
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::path::PathBuf;
    use std::pin::Pin;
    use std::sync::{Arc, Mutex};
    use std::task::{Context, Poll};

    use serde_json::{Value, json};
    use tokio::io::{AsyncRead, AsyncWriteExt, ReadBuf};
    use tokio::sync::{mpsc, oneshot, watch};

    use super::{
        Exit, OutputContext, SharedState, StderrTail, VersionCheck, forward_stderr, parse_version,
        read_output,
    };
    use crate::error::{Error, ErrorKind, Result};
    use crate::types::StderrCallback;

    struct Chunks(VecDeque<Vec<u8>>);

    impl Chunks {
        fn new(chunks: impl IntoIterator<Item = impl AsRef<[u8]>>) -> Self {
            Self(
                chunks
                    .into_iter()
                    .map(|chunk| chunk.as_ref().to_vec())
                    .collect(),
            )
        }
    }

    impl AsyncRead for Chunks {
        fn poll_read(
            mut self: Pin<&mut Self>,
            _context: &mut Context<'_>,
            buffer: &mut ReadBuf<'_>,
        ) -> Poll<std::io::Result<()>> {
            if let Some(mut chunk) = self.0.pop_front() {
                let fits = chunk.len().min(buffer.remaining());
                let rest = chunk.split_off(fits);
                buffer.put_slice(&chunk);
                if !rest.is_empty() {
                    self.0.push_front(rest);
                }
            }
            Poll::Ready(Ok(()))
        }
    }

    async fn read_all(
        chunks: impl IntoIterator<Item = impl AsRef<[u8]>>,
        max_buffer_size: usize,
        exit_code: i32,
    ) -> Vec<Result<Value>> {
        let (sender, mut receiver) = mpsc::channel(1024);
        let (_exit_sender, exit) = watch::channel(Some(Exit {
            code: exit_code,
            forced: false,
        }));
        let context = OutputContext {
            shared: Arc::new(SharedState::default()),
            exit,
            max_buffer_size,
            version_check: VersionCheck {
                done: true,
                cli_path: PathBuf::from("/usr/bin/claude"),
                verbatim_prompts: false,
            },
        };
        read_output(Chunks::new(chunks), sender, context).await;
        let mut items = Vec::new();
        while let Some(item) = receiver.recv().await {
            items.push(item);
        }
        items
    }

    fn messages(items: Vec<Result<Value>>) -> Vec<Value> {
        items.into_iter().map(Result::unwrap).collect()
    }

    const MEBIBYTE: usize = 1024 * 1024;

    struct Failing(Option<Vec<u8>>);

    impl AsyncRead for Failing {
        fn poll_read(
            mut self: Pin<&mut Self>,
            _context: &mut Context<'_>,
            buffer: &mut ReadBuf<'_>,
        ) -> Poll<std::io::Result<()>> {
            match self.0.take() {
                Some(chunk) => {
                    buffer.put_slice(&chunk);
                    Poll::Ready(Ok(()))
                }
                None => Poll::Ready(Err(std::io::Error::other("pipe broke"))),
            }
        }
    }

    #[tokio::test]
    async fn a_read_error_on_the_output_ends_the_stream_with_that_error() {
        let (sender, mut receiver) = mpsc::channel(16);
        let (_exit_sender, exit) = watch::channel(Some(Exit {
            code: 0,
            forced: false,
        }));
        let context = OutputContext {
            shared: Arc::new(SharedState::default()),
            exit,
            max_buffer_size: MEBIBYTE,
            version_check: VersionCheck {
                done: true,
                cli_path: PathBuf::from("/usr/bin/claude"),
                verbatim_prompts: false,
            },
        };
        let first = json!({"type": "system", "subtype": "init"});
        read_output(
            Failing(Some(format!("{first}\n").into_bytes())),
            sender,
            context,
        )
        .await;
        assert_eq!(receiver.recv().await.unwrap().unwrap(), first);
        let error = receiver.recv().await.unwrap().unwrap_err();
        assert_eq!(error.kind(), ErrorKind::Io);
        assert_eq!(error.to_string(), "Failed to read the output of the CLI");
        assert_eq!(
            std::error::Error::source(&error).map(ToString::to_string),
            Some("pipe broke".to_owned())
        );
        assert!(receiver.recv().await.is_none());
    }

    #[tokio::test]
    async fn multiple_json_objects_in_one_read() {
        let first = json!({"type": "message", "id": "msg1", "content": "First message"});
        let second = json!({"type": "result", "id": "res1", "content": "Second message"});
        let chunk = format!("{first}\n{second}");
        assert_eq!(
            messages(read_all([chunk], MEBIBYTE, 0).await),
            [first, second]
        );
    }

    #[tokio::test]
    async fn json_with_embedded_newlines() {
        let first = json!({"type": "message", "content": "Line 1\nLine 2\nLine 3"});
        let second = json!({"type": "result", "data": "Some\nMultiline\nContent"});
        let chunk = format!("{first}\n{second}");
        assert_eq!(
            messages(read_all([chunk], MEBIBYTE, 0).await),
            [first, second]
        );
    }

    #[tokio::test]
    async fn multiple_newlines_between_objects() {
        let first = json!({"type": "message", "id": "msg1"});
        let second = json!({"type": "result", "id": "res1"});
        let chunk = format!("{first}\n\n\n{second}\n");
        assert_eq!(
            messages(read_all([chunk], MEBIBYTE, 0).await),
            [first, second]
        );
    }

    #[tokio::test]
    async fn json_split_across_multiple_reads() {
        let message = json!({
            "type": "assistant",
            "message": {"content": [
                {"type": "text", "text": "x".repeat(1000)},
                {"type": "tool_use", "id": "tool_123", "name": "Read",
                 "input": {"file_path": "/test.txt"}},
            ]},
        });
        let complete = format!("{message}\n");
        let chunks = [&complete[..100], &complete[100..250], &complete[250..]];
        assert_eq!(messages(read_all(chunks, MEBIBYTE, 0).await), [message]);
    }

    #[tokio::test]
    async fn large_minified_json_split_into_64_kib_chunks() {
        let rows: Vec<Value> = (0..2000)
            .map(|index| json!({"id": index, "value": "x".repeat(100)}))
            .collect();
        let message = json!({
            "type": "user",
            "message": {"role": "user", "content": [
                {"tool_use_id": "toolu_016fed1NhiaMLqnEvrj5NUaj", "type": "tool_result",
                 "content": json!({"data": rows}).to_string()},
            ]},
        });
        let complete = format!("{message}\n");
        let chunks: Vec<&[u8]> = complete.as_bytes().chunks(64 * 1024).collect();
        assert!(chunks.len() > 1);
        assert_eq!(messages(read_all(chunks, MEBIBYTE, 0).await), [message]);
    }

    #[tokio::test]
    async fn pending_message_over_the_limit_is_an_error_and_stops_the_stream() {
        let huge = format!("{{\"data\": \"{}", "x".repeat(MEBIBYTE + 1000));
        let items = read_all([huge.as_str(), "\n{\"type\": \"late\"}\n"], MEBIBYTE, 0).await;
        assert_eq!(items.len(), 1);
        let error = items.into_iter().next().unwrap().unwrap_err();
        assert_eq!(error.kind(), ErrorKind::JsonDecode);
        assert!(
            error
                .to_string()
                .contains("exceeded maximum buffer size of 1048576 bytes")
        );
    }

    #[tokio::test]
    async fn buffer_size_option_is_honored() {
        let huge = format!("{{\"data\": \"{}", "x".repeat(600));
        let items = read_all([huge], 512, 0).await;
        let error = items.into_iter().next().unwrap().unwrap_err();
        assert!(
            error
                .to_string()
                .contains("maximum buffer size of 512 bytes")
        );
    }

    #[tokio::test]
    async fn complete_oversized_line_still_raises() {
        let line = format!("{}\n", json!({"data": "x".repeat(600)}));
        let items = read_all([line], 512, 0).await;
        assert_eq!(items.len(), 1);
        assert_eq!(
            items[0].as_ref().err().map(Error::kind),
            Some(ErrorKind::JsonDecode)
        );
    }

    #[tokio::test]
    async fn the_limit_counts_bytes_not_characters() {
        let line = format!("{}\n", json!({"data": "\u{65e5}".repeat(200)}));
        assert!(line.chars().count() < 512);
        assert!(line.len() > 512);
        let items = read_all([line], 512, 0).await;
        assert_eq!(
            items[0].as_ref().err().map(Error::kind),
            Some(ErrorKind::JsonDecode)
        );
    }

    #[tokio::test]
    async fn mixed_complete_and_split_json() {
        let first = json!({"type": "system", "subtype": "start"});
        let large = json!({
            "type": "assistant",
            "message": {"content": [{"type": "text", "text": "y".repeat(5000)}]},
        });
        let last = json!({"type": "system", "subtype": "end"});
        let large_text = large.to_string();
        let chunks = [
            format!("{first}\n"),
            large_text[..1000].to_owned(),
            large_text[1000..3000].to_owned(),
            format!("{}\n{last}\n", &large_text[3000..]),
        ];
        assert_eq!(
            messages(read_all(chunks, MEBIBYTE, 0).await),
            [first, large, last]
        );
    }

    #[tokio::test]
    async fn non_json_lines_are_skipped_even_when_interleaved() {
        let first = json!({"type": "system", "subtype": "init"});
        let second = json!({"type": "result", "subtype": "success"});
        let chunk = format!(
            "[SandboxDebug] Seccomp filtering not available\n{first}\n\
             [SandboxDebug] Another debug line\nplain text\n{second}\n"
        );
        assert_eq!(
            messages(read_all([chunk], MEBIBYTE, 0).await),
            [first, second]
        );
    }

    #[tokio::test]
    async fn whitespace_at_a_chunk_boundary_is_preserved() {
        let message = json!({"type": "assistant", "text": "hello   world  and  more"});
        let complete = format!("{message}\n");
        let split = complete.find("   world").unwrap() + 1;
        let chunks = [&complete[..split], &complete[split..]];
        assert_eq!(messages(read_all(chunks, MEBIBYTE, 0).await), [message]);
    }

    #[tokio::test]
    async fn malformed_complete_line_raises_and_stops() {
        let items = read_all(
            ["{\"type\": \"assistant\", \"broken\n{\"type\": \"late\"}\n"],
            MEBIBYTE,
            0,
        )
        .await;
        assert_eq!(items.len(), 1);
        assert_eq!(
            items[0].as_ref().err().map(Error::kind),
            Some(ErrorKind::JsonDecode)
        );
    }

    #[tokio::test]
    async fn final_message_without_trailing_newline_is_yielded() {
        let first = json!({"type": "system", "subtype": "init"});
        let last = json!({"type": "result", "subtype": "success"});
        let chunks = [format!("{first}\n"), last.to_string()];
        assert_eq!(messages(read_all(chunks, MEBIBYTE, 0).await), [first, last]);
    }

    #[tokio::test]
    async fn truncated_final_line_is_dropped_not_raised() {
        let first = json!({"type": "system", "subtype": "init"});
        let chunks = [format!("{first}\n"), "{\"type\": \"assis".to_owned()];
        assert_eq!(messages(read_all(chunks, MEBIBYTE, 0).await), [first]);
    }

    #[tokio::test]
    async fn non_zero_exit_code_ends_the_stream_with_a_process_error() {
        let first = json!({"type": "system", "subtype": "init"});
        let mut items = read_all([format!("{first}\n")], MEBIBYTE, 3).await;
        assert_eq!(items.len(), 2);
        let error = items.pop().unwrap().unwrap_err();
        assert_eq!(error.exit_code(), Some(3));
        assert_eq!(
            error.to_string(),
            "Command failed with exit code 3 (exit code: 3)"
        );
        assert_eq!(
            error.stderr(),
            None,
            "no stderr callback, so nothing was captured"
        );
    }

    #[test]
    fn version_parsing_follows_the_upstream_pattern() {
        assert_eq!(parse_version("2.0.20"), Some((2, 0, 20)));
        assert_eq!(parse_version("99.99.99 (Claude Code)"), Some((99, 99, 99)));
        assert_eq!(parse_version("2.1.248-beta"), Some((2, 1, 248)));
        assert_eq!(parse_version("1234567890.1.1"), None);
        assert_eq!(parse_version("v2.0.0"), None);
        assert_eq!(parse_version("2.0"), None);
        assert_eq!(parse_version("None"), None);
        assert!(parse_version("2.1.247").unwrap() < (2, 1, 248));
    }

    fn recording_callback() -> (StderrCallback, Arc<Mutex<Vec<String>>>) {
        let received = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&received);
        let callback = StderrCallback::from_sync(move |line| {
            sink.lock().unwrap().push(line.to_owned());
        });
        (callback, received)
    }

    #[tokio::test]
    async fn stderr_callback_panic_does_not_stop_the_loop() {
        let received = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&received);
        let callback = StderrCallback::from_sync(move |line| {
            let first = {
                let mut lines = sink.lock().unwrap();
                lines.push(line.to_owned());
                lines.len() == 1
            };
            assert!(!first, "simulated handler failure");
        });
        let (_stop, stopped) = oneshot::channel();
        forward_stderr(
            Chunks::new(["line 1\nline 2\n", "line 3"]),
            callback,
            1024,
            stopped,
            StderrTail::default(),
        )
        .await;
        assert_eq!(*received.lock().unwrap(), ["line 1", "line 2", "line 3"]);
    }

    #[tokio::test]
    async fn stderr_async_callback_panic_does_not_stop_the_loop() {
        let received = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&received);
        let callback = StderrCallback::new(move |line: String| {
            let sink = Arc::clone(&sink);
            async move {
                let first = {
                    let mut lines = sink.lock().unwrap();
                    lines.push(line);
                    lines.len() == 1
                };
                assert!(!first, "simulated handler failure");
            }
        });
        let (_stop, stopped) = oneshot::channel();
        forward_stderr(
            Chunks::new(["a\nb\n"]),
            callback,
            1024,
            stopped,
            StderrTail::default(),
        )
        .await;
        assert_eq!(*received.lock().unwrap(), ["a", "b"]);
    }

    #[tokio::test]
    async fn stderr_line_split_across_chunks_is_reassembled() {
        let (callback, received) = recording_callback();
        let (_stop, stopped) = oneshot::channel();
        forward_stderr(
            Chunks::new([
                "a warning that got ",
                "split across two reads\nnext line  \n\n",
            ]),
            callback,
            1024,
            stopped,
            StderrTail::default(),
        )
        .await;
        assert_eq!(
            *received.lock().unwrap(),
            ["a warning that got split across two reads", "next line"]
        );
    }

    #[tokio::test]
    async fn stderr_line_without_newline_is_flushed_at_the_buffer_limit() {
        let (callback, received) = recording_callback();
        let (_stop, stopped) = oneshot::channel();
        forward_stderr(
            Chunks::new(["aaaaa", "aaaaa", "aaaaa", "bbb\n"]),
            callback,
            10,
            stopped,
            StderrTail::default(),
        )
        .await;
        assert_eq!(
            *received.lock().unwrap(),
            ["a".repeat(15), "bbb".to_owned()]
        );
    }

    #[tokio::test]
    async fn stderr_pending_line_is_flushed_when_the_task_is_stopped() {
        let (callback, received) = recording_callback();
        let (stop, stopped) = oneshot::channel();
        let (mut writer, reader) = tokio::io::duplex(1024);
        let task = tokio::spawn(forward_stderr(
            reader,
            callback,
            1024,
            stopped,
            StderrTail::default(),
        ));
        writer.write_all(b"Error: model overloaded").await.unwrap();
        tokio::task::yield_now().await;
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        stop.send(()).unwrap();
        task.await.unwrap();
        assert_eq!(*received.lock().unwrap(), ["Error: model overloaded"]);
        drop(writer);
    }
}
