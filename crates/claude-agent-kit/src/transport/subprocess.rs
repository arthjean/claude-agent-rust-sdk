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
use super::environment::build_environment;
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
/// [`write`](Self::write) must run to completion: dropping its future
/// midway can leave a partial message on the standard input of the CLI.
///
/// A transport serves one connection: it cannot connect again once closed.
pub struct SubprocessTransport {
    options: ClaudeAgentOptions,
    skip_version_check: bool,
    inner: Arc<Inner>,
}

struct Inner {
    shared: Arc<SharedState>,
    connecting: tokio::sync::Mutex<()>,
    stdin: tokio::sync::Mutex<Option<ChildStdin>>,
    state: Mutex<State>,
}

#[derive(Default)]
struct SharedState {
    ready: AtomicBool,
    closing: AtomicBool,
    exit_error: Mutex<Option<String>>,
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
            options,
            skip_version_check,
            inner: Arc::new(Inner {
                shared: Arc::new(SharedState::default()),
                connecting: tokio::sync::Mutex::new(()),
                stdin: tokio::sync::Mutex::new(None),
                state: Mutex::new(State::Idle),
            }),
        }
    }

    /// Builds the arguments [`connect`](Self::connect) passes to the CLI,
    /// without the executable itself.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Config`] for an invalid skill name, a plugin that
    /// is not local, or, on Windows, a resume or session value containing a
    /// `cmd.exe` metacharacter. Returns [`Error::CliConnection`] when the
    /// settings to merge with the sandbox options cannot be loaded.
    pub fn command_arguments(&self) -> Result<Vec<OsString>> {
        build_arguments(&self.options, Platform::current())
    }

    /// Identifier of the CLI process, once connected.
    #[must_use]
    pub fn process_id(&self) -> Option<u32> {
        self.inner.shared.process_id.get().copied()
    }

    /// Starts the CLI process. Connecting twice is a no-op.
    ///
    /// # Errors
    ///
    /// Returns [`Error::CliNotFound`] when the executable is missing,
    /// [`Error::CliConnection`] when it is a batch script on Windows, when
    /// the working directory does not exist, when the `user` option names
    /// an unknown user or is set on Windows, when the process cannot start,
    /// or when the transport is already closed, and the errors of
    /// [`command_arguments`](Self::command_arguments).
    pub async fn connect(&self) -> Result<()> {
        let _connecting = self.inner.connecting.lock().await;
        match &*lock(&self.inner.state) {
            State::Idle => {}
            State::Connected(_) => return Ok(()),
            State::Closed(_) => return Err(Error::cli_connection("Transport is closed")),
        }

        let cli_path = match &self.options.cli_path {
            Some(path) => path.clone(),
            None => tokio::task::spawn_blocking(find_cli)
                .await
                .map_err(|error| {
                    Error::cli_connection(format!("CLI discovery failed: {error}"))
                })??,
        };
        reject_windows_batch_cli(&cli_path, Platform::current())?;
        let arguments = self.command_arguments()?;
        let environment =
            build_environment(&self.options, std::env::vars_os(), Platform::current());

        let mut command = Command::new(&cli_path);
        command
            .args(arguments)
            .env_clear()
            .envs(environment.into_entries())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(if self.options.stderr.is_some() {
                Stdio::piped()
            } else {
                Stdio::inherit()
            });
        if let Some(cwd) = &self.options.cwd {
            command.current_dir(cwd);
        }
        if let Some(user) = &self.options.user {
            process::run_as_user(&mut command, user).map_err(|error| {
                Error::cli_connection(format!("Failed to start Claude CLI: {error}"))
            })?;
        }

        let spawned = process::spawn(command)
            .await
            .map_err(|error| self.spawn_error(&cli_path, &error))?;
        let process::Spawned { streams, handle } = spawned;
        let shared = &self.inner.shared;
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
                let task = tokio::spawn(forward_stderr(
                    stream,
                    callback,
                    self.max_buffer_size(),
                    stopped,
                ));
                StderrTask { stop, task }
            });

        *self.inner.stdin.lock().await = streams.stdin;
        *lock(&self.inner.state) = State::Connected(Box::new(Connection {
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
    /// Returns [`Error::CliConnection`] when the transport is not ready,
    /// when the process has exited, or when the write fails.
    pub async fn write(&self, data: &str) -> Result<()> {
        let mut stdin = self.inner.stdin.lock().await;
        let shared = &self.inner.shared;
        let Some(pipe) = stdin
            .as_mut()
            .filter(|_| shared.ready.load(Ordering::SeqCst))
        else {
            return Err(Error::cli_connection(
                "ProcessTransport is not ready for writing",
            ));
        };
        if let Some(exit) = shared.exit.get().and_then(|exit| *exit.borrow()) {
            return Err(Error::cli_connection(format!(
                "Cannot write to terminated process (exit code: {})",
                exit.code
            )));
        }
        if let Some(error) = &*lock(&shared.exit_error) {
            return Err(Error::cli_connection(format!(
                "Cannot write to process that exited with error: {error}"
            )));
        }
        let written = async {
            pipe.write_all(data.as_bytes()).await?;
            pipe.flush().await
        };
        if let Err(error) = written.await {
            shared.ready.store(false, Ordering::SeqCst);
            let message = format!("Failed to write to process stdin: {error}");
            *lock(&shared.exit_error) = Some(message.clone());
            return Err(Error::cli_connection(message));
        }
        Ok(())
    }

    /// Returns the stream of messages read from the standard output.
    ///
    /// The stream is handed out once. It yields [`Error::JsonDecode`] for a
    /// corrupt or oversize message and [`Error::Process`] when the CLI
    /// exits with a non-zero code, and ends after either. Before
    /// [`connect`](Self::connect), or on a second call, the stream yields
    /// one [`Error::CliConnection`].
    pub fn read_messages(&self) -> MessageStream {
        let messages = match &mut *lock(&self.inner.state) {
            State::Connected(connection) => connection.messages.take(),
            State::Idle | State::Closed(_) => None,
        };
        match messages {
            Some(messages) => Box::pin(stream::unfold(messages, |mut messages| async move {
                messages.recv().await.map(|message| (message, messages))
            })),
            None => Box::pin(stream::once(std::future::ready(Err(
                Error::cli_connection("Not connected"),
            )))),
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
        let closing = {
            let mut state = lock(&self.inner.state);
            match std::mem::replace(&mut *state, State::Idle) {
                State::Idle => {
                    self.inner.shared.ready.store(false, Ordering::SeqCst);
                    return Ok(());
                }
                State::Closed(closing) => {
                    *state = State::Closed(closing.clone());
                    closing
                }
                State::Connected(connection) => {
                    let shutdown = tokio::spawn(shut_down(Arc::clone(&self.inner), *connection));
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
        self.inner.shared.ready.load(Ordering::SeqCst)
    }

    /// Closes the standard input of the CLI.
    ///
    /// # Errors
    ///
    /// This implementation never fails.
    pub async fn end_input(&self) -> Result<()> {
        let mut stdin = self.inner.stdin.lock().await;
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

    fn spawn_error(&self, cli_path: &Path, error: &std::io::Error) -> Error {
        if let Some(cwd) = &self.options.cwd
            && !cwd.exists()
        {
            return Error::cli_connection(format!(
                "Working directory does not exist: {}",
                cwd.display()
            ));
        }
        if error.kind() == std::io::ErrorKind::NotFound {
            return Error::cli_not_found(
                Some("Claude CLI not found at"),
                Some(&cli_path.to_string_lossy()),
            );
        }
        Error::cli_connection(format!("Failed to start Claude CLI: {error}"))
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

    match timeout(WRITE_LOCK_LIMIT, inner.stdin.lock()).await {
        Ok(mut stdin) => {
            shared.ready.store(false, Ordering::SeqCst);
            stdin.take();
        }
        Err(_) => shared.ready.store(false, Ordering::SeqCst),
    }

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
            Ok(0) | Err(_) => break,
            Ok(read) => read,
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
        let error = Error::process(
            format!("Command failed with exit code {code}"),
            Some(code),
            Some("Check stderr output for details".to_owned()),
        );
        *lock(&context.shared.exit_error) = Some(error.to_string());
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
        if version.is_none() {
            tracing::warn!(
                "Claude CLI at {cli_path} is too old to report its version, and may be \
                 unsupported in the Agent SDK. Minimum required version is {}. Some features \
                 may not work correctly.",
                crate::MINIMUM_CLI_VERSION,
            );
        }
        if self.verbatim_prompts
            && version.is_none_or(|version| version < VERBATIM_PROMPTS_MINIMUM_CLI_VERSION)
        {
            let (major, minor, patch) = VERBATIM_PROMPTS_MINIMUM_CLI_VERSION;
            let reported = reported.and_then(Value::as_str).unwrap_or("unknown");
            tracing::warn!(
                "verbatim_prompts is enabled, but Claude CLI version {reported} at {cli_path} \
                 ignores it: prompts will still have @path mentions expanded and slash commands \
                 dispatched. Version {major}.{minor}.{patch} or later is required.",
            );
        }
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

async fn forward_stderr<R>(
    mut stream: R,
    callback: StderrCallback,
    max_buffer_size: usize,
    mut stopped: oneshot::Receiver<()>,
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
            emit_stderr_line(&callback, &line).await;
        }
        if framer.pending_len() > max_buffer_size {
            emit_stderr_line(&callback, &framer.flush()).await;
        }
    }
    emit_stderr_line(&callback, &framer.flush()).await;
}

async fn emit_stderr_line(callback: &StderrCallback, line: &[u8]) {
    let text = String::from_utf8_lossy(line);
    let text = text.trim_end();
    if text.is_empty() {
        return;
    }
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
        Exit, OutputContext, SharedState, VersionCheck, forward_stderr, parse_version, read_output,
    };
    use crate::error::{Error, Result};
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
        assert!(matches!(error, Error::JsonDecode { .. }));
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
        assert!(matches!(items[0], Err(Error::JsonDecode { .. })));
    }

    #[tokio::test]
    async fn the_limit_counts_bytes_not_characters() {
        let line = format!("{}\n", json!({"data": "\u{65e5}".repeat(200)}));
        assert!(line.chars().count() < 512);
        assert!(line.len() > 512);
        let items = read_all([line], 512, 0).await;
        assert!(matches!(items[0], Err(Error::JsonDecode { .. })));
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
        assert!(matches!(items[0], Err(Error::JsonDecode { .. })));
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
            "Command failed with exit code 3 (exit code: 3)\nError output: Check stderr output \
             for details"
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
        forward_stderr(Chunks::new(["a\nb\n"]), callback, 1024, stopped).await;
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
        let task = tokio::spawn(forward_stderr(reader, callback, 1024, stopped));
        writer.write_all(b"Error: model overloaded").await.unwrap();
        tokio::task::yield_now().await;
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        stop.send(()).unwrap();
        task.await.unwrap();
        assert_eq!(*received.lock().unwrap(), ["Error: model overloaded"]);
        drop(writer);
    }
}
