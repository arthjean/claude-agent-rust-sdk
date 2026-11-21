//! Control protocol on top of a [`Transport`](crate::Transport).
//!
//! One reader task routes every frame the CLI emits. Control traffic never
//! reaches the application: responses complete the pending request with the
//! same id, requests each start a handler task, cancel requests abort the
//! matching handler, and transcript mirror frames go to the mirror. Every
//! other frame is meant for the application.
//!
//! # Back-pressure
//!
//! The application reads its frames from a queue of 100. The control
//! traffic must keep flowing while the application is not reading, since
//! the application may be waiting on that very traffic: an `interrupt`
//! sent from the loop that reads the messages waits for its response, and
//! the CLI waits for the answers of hooks and permission callbacks before
//! it goes on. So a full queue never stops the reader at once. Frames for
//! the application that do not fit are held by the reader, in order,
//! while it goes on reading and routing control traffic, and are handed
//! over as the application frees room.
//!
//! Memory stays bounded: once the held frames reach 16 MiB, counted as
//! their JSON text, the reader stops reading the CLI output until the
//! application catches up. The CLI then blocks on its full output pipe,
//! which is the back-pressure upstream applies at once, after 100 frames.
//! Control traffic stalls again only past that budget, the point at which
//! an application that never reads cannot be served without unbounded
//! memory.
//!
//! The flush of the transcript mirror that a `result` frame waits for runs
//! the same way: the reader goes on routing control traffic during the
//! flush, and holds the `result` frame, and every application frame after
//! it, until the flush is done, so the application still sees a result
//! only once its transcript is in the store.
//!
//! The session differs from the upstream Python SDK in deliberate ways,
//! each recorded in `parity/differences.toml`:
//!
//! - control responses and requests are routed while up to 16 MiB of
//!   frames wait for the application, where upstream stops reading the
//!   output after 100 unread frames, so that an `interrupt` or a hook does
//!   not time out behind messages the application has not read yet;
//! - when the output of the CLI ends, pending control requests fail at once
//!   instead of waiting for their timeout;
//! - a control request without a request id and a malformed transcript
//!   mirror frame are ignored instead of stopping the reader;
//! - a `CLAUDE_CODE_STREAM_CLOSE_TIMEOUT` that Python's `int()` rejects
//!   leaves the initialize timeout at 60 s, with a warning, where upstream
//!   fails the connect;
//! - a control error response whose `error` is `null` reads as
//!   `Unknown error` and one that is not a string as its JSON text, where
//!   upstream gives the Python text `None` or the `repr` of the value.
//!
//! Where it behaves as upstream, through other means:
//!
//! - every frame goes to the CLI through one writer task, fed whole frames
//!   by a queue: a caller that drops its future (under a `timeout` or a
//!   `select!`) after its frame was queued still gets the frame written in
//!   full, and a caller dropped before cannot leave half a frame behind.
//!   Upstream gets the same guarantee from anyio, which buffers a whole
//!   frame in one synchronous call before it awaits the drain;
//! - a cancel request stops the application callback and sends no response
//!   for it; a response already being written is finished;
//! - a callback that panics is answered with an error response, as a
//!   Python callback that raises is;
//! - a prompt stream that panics ends the input as one that yields an
//!   error does, so stdin is closed as upstream closes it when the
//!   iterable raises.

mod handlers;
mod initialize;
mod run_end;
/// Whether a JSON value is true as Python's `bool()` reads it: `null`,
/// `false`, zero, and an empty string, array, or object are false, as
/// upstream tests `response.get(...)` in an `if`.
fn truthy(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => false,
        Some(Value::Bool(flag)) => *flag,
        Some(Value::Number(number)) => number.as_f64().is_some_and(|number| number != 0.0),
        Some(Value::String(text)) => !text.is_empty(),
        Some(Value::Array(items)) => !items.is_empty(),
        Some(Value::Object(members)) => !members.is_empty(),
    }
}

#[cfg(test)]
mod tests;

use std::collections::{HashMap, VecDeque};
use std::panic::AssertUnwindSafe;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use futures_util::{FutureExt, StreamExt};
use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot};
use tokio::task::{AbortHandle, JoinHandle};
use uuid::Uuid;

use crate::error::{Error, ErrorKind, Result};
use crate::prompt::{InputStream, stamp_user_message};
use crate::resume::{ResumeDir, materialize_resume_session};
use crate::session_store::SessionKey;
use crate::transport::{SharedTransport, SubprocessTransport};
use crate::types::{BoxFuture, ClaudeAgentOptions};

/// The `tracing` target of the warning that `can_use_tool` is shadowed, the
/// counterpart of the `CanUseToolShadowedWarning` category of the Python
/// SDK: filter it out with `claude_agent_rust_sdk::can_use_tool_shadowed=off`
/// in an `EnvFilter`.
pub(crate) const CAN_USE_TOOL_SHADOWED_TARGET: &str =
    "claude_agent_rust_sdk::can_use_tool_shadowed";

/// The `code` field of that warning, the code the TypeScript SDK gives the
/// same condition.
pub(crate) const CAN_USE_TOOL_SHADOWED_CODE: &str = "CLAUDE_SDK_CAN_USE_TOOL_SHADOWED";

pub(crate) use initialize::SessionConfig;
use run_end::RunEndTracker;

const MESSAGE_QUEUE_CAPACITY: usize = 100;
/// Bytes of JSON text the reader holds for the application beyond the
/// queue before it stops reading the CLI output.
const HELD_FRAMES_BUDGET: usize = 16 * 1024 * 1024;
const REQUEST_ID_RANDOM_CHARS: usize = 8;

/// Time a control request waits for its response.
pub(crate) const CONTROL_REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

/// Queue of the frames meant for the application. It ends when the reader
/// stops, after the error that stopped it, if any.
pub(crate) type Inbox = mpsc::Receiver<Result<Value>>;

/// Receiver of the transcript mirror frames of a session.
pub(crate) trait TranscriptMirror: Send + Sync {
    /// Queues the entries the CLI appended to a transcript file.
    fn enqueue(&self, file_path: String, entries: Vec<Value>);

    /// Writes out everything queued. It never fails.
    fn flush(&self) -> BoxFuture<'_, ()>;

    /// Flushes for the last time.
    fn close(&self) -> BoxFuture<'_, ()>;

    /// Gives the mirror the channel its dropped batches are reported on.
    fn attach(&self, reporter: MirrorErrorReporter) {
        let _ = reporter;
    }
}

/// Delivers `mirror_error` system messages to the application queue.
#[derive(Clone)]
pub(crate) struct MirrorErrorReporter(mpsc::WeakSender<Result<Value>>);

impl MirrorErrorReporter {
    pub(crate) fn new(outbox: mpsc::WeakSender<Result<Value>>) -> Self {
        Self(outbox)
    }

    /// Reports a failed transcript mirror append. It never blocks: with a
    /// full queue the message is logged and dropped.
    pub(crate) fn report(&self, key: Option<&SessionKey>, error: &str) {
        let message = json!({
            "type": "system",
            "subtype": "mirror_error",
            "error": error,
            "key": key,
            "uuid": Uuid::new_v4().to_string(),
            "session_id": key.map_or("", |key| key.session_id.as_str()),
        });
        let delivered = self
            .0
            .upgrade()
            .is_some_and(|outbox| outbox.try_send(Ok(message)).is_ok());
        if !delivered {
            tracing::warn!("Dropping mirror_error message (buffer full): {error}");
        }
    }
}

/// One session of the control protocol: the router, the control channel in
/// both directions, and the tracking of the end of the run.
#[derive(Clone)]
pub(crate) struct ControlSession {
    shared: Arc<Shared>,
}

struct Shared {
    transport: SharedTransport,
    writer: mpsc::UnboundedSender<WriteJob>,
    config: SessionConfig,
    run: RunEndTracker,
    state: Mutex<State>,
    mirror_errors: mpsc::WeakSender<Result<Value>>,
    closing: tokio::sync::Mutex<bool>,
    resume: Mutex<Option<ResumeDir>>,
}

struct State {
    outbox: Option<mpsc::Sender<Result<Value>>>,
    pending: HashMap<String, oneshot::Sender<Result<Value>>>,
    /// The cancel handle of each running control request handler, with
    /// the number of the handler, so a handler that ends removes its own
    /// entry and not the one of a later request with the same id.
    inflight: HashMap<String, (u64, oneshot::Sender<()>)>,
    /// Number of the next control request handler.
    next_handler: u64,
    tasks: Vec<AbortHandle>,
    reader: Option<JoinHandle<()>>,
    reader_end: Option<ReaderEnd>,
    request_counter: u64,
    closed: bool,
    initialization: Option<Value>,
    writer_jobs: Option<mpsc::UnboundedReceiver<WriteJob>>,
}

/// One write to the CLI, run by the writer task.
enum Outgoing {
    /// A whole frame, with its line feed.
    Frame(String),
    /// The end of the input stream.
    EndInput,
}

struct WriteJob {
    what: Outgoing,
    done: oneshot::Sender<Result<()>>,
}

/// Runs every write to the transport, one at a time, in the order queued.
///
/// The loop owns a handle to the transport, not the session, so it ends
/// when the session and every sender are gone.
async fn write_jobs(transport: SharedTransport, mut jobs: mpsc::UnboundedReceiver<WriteJob>) {
    while let Some(job) = jobs.recv().await {
        let result = match job.what {
            Outgoing::Frame(text) => transport.write(&text).await,
            Outgoing::EndInput => transport.end_input().await,
        };
        let _ = job.done.send(result);
    }
}

enum ReaderEnd {
    Clean,
    Failed(Error),
}

fn session_closed() -> Error {
    Error::connection_closed("The session was closed before the control request was answered")
}

fn output_ended() -> Error {
    Error::connection_closed("The CLI output ended before the control request was answered")
}

impl State {
    fn unanswerable(&self) -> Option<Error> {
        if self.closed {
            return Some(session_closed());
        }
        match &self.reader_end {
            None => None,
            Some(ReaderEnd::Clean) => Some(output_ended()),
            Some(ReaderEnd::Failed(error)) => Some(
                Error::connection_closed(format!(
                    "Cannot write to process that exited with error: {error}"
                ))
                .with_source(error.clone()),
            ),
        }
    }

    fn fail_pending(&mut self, error: impl Fn() -> Error) {
        for (_, waiter) in self.pending.drain() {
            let _ = waiter.send(Err(error()));
        }
    }
}

impl Shared {
    fn lock_resume(&self) -> MutexGuard<'_, Option<ResumeDir>> {
        self.resume.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn is_closed(&self) -> bool {
        self.state().closed
    }

    /// Writes a frame through the writer task. Dropping the returned
    /// future never cuts the frame: once queued, it is written in full.
    async fn write_frame(&self, frame: &Value) -> Result<()> {
        self.submit(Outgoing::Frame(format!("{frame}\n"))).await
    }

    async fn submit(&self, what: Outgoing) -> Result<()> {
        let jobs = self.state().writer_jobs.take();
        if let Some(jobs) = jobs {
            let task = crate::caller_context::spawn(write_jobs(self.transport.clone(), jobs));
            let mut state = self.state();
            if state.closed {
                task.abort();
            } else {
                state.tasks.push(task.abort_handle());
            }
        }
        let (done, written) = oneshot::channel();
        self.writer
            .send(WriteJob { what, done })
            .map_err(|_| session_closed())?;
        written.await.unwrap_or_else(|_| Err(session_closed()))
    }

    fn complete_request(&self, frame: &Value) {
        let response = frame.get("response");
        let Some(request_id) = response
            .and_then(|response| response.get("request_id"))
            .and_then(Value::as_str)
        else {
            return;
        };
        let Some(waiter) = self.state().pending.remove(request_id) else {
            return;
        };
        let field = |name: &str| response.and_then(|response| response.get(name));
        let outcome = if field("subtype").and_then(Value::as_str) == Some("error") {
            Err(Error::control(match field("error") {
                Some(Value::String(text)) => text.clone(),
                Some(other) if !other.is_null() => other.to_string(),
                _ => "Unknown error".to_owned(),
            }))
        } else {
            Ok(field("response")
                .filter(|payload| payload.is_object())
                .cloned()
                .unwrap_or_else(|| json!({})))
        };
        let _ = waiter.send(outcome);
    }

    fn mirror(&self, frame: &Value) {
        let Some(mirror) = &self.config.mirror else {
            return;
        };
        match (
            frame.get("filePath").and_then(Value::as_str),
            frame.get("entries").and_then(Value::as_array),
        ) {
            (Some(file_path), Some(entries)) => {
                mirror.enqueue(file_path.to_owned(), entries.clone());
            }
            _ => tracing::warn!("Ignoring a malformed transcript_mirror frame"),
        }
    }

    async fn wait_for_run_end_and_end_input(&self) {
        self.run.wait_for_end().await;
        self.end_input().await;
    }

    async fn end_input(&self) {
        self.run.seal();
        if let Err(error) = self.submit(Outgoing::EndInput).await {
            tracing::debug!("Error closing the input stream: {error}");
        }
    }

    async fn stream_input(&self, mut input: InputStream) {
        let mut written = 0_usize;
        loop {
            let message = match AssertUnwindSafe(input.next()).catch_unwind().await {
                Ok(Some(message)) => message,
                Ok(None) => break,
                Err(_) => {
                    tracing::error!("Prompt stream failed; closing stdin: the stream panicked");
                    break;
                }
            };
            if self.is_closed() {
                break;
            }
            let message = match message {
                Ok(message) => message,
                Err(error) => {
                    tracing::error!("Prompt stream failed; closing stdin: {error}");
                    break;
                }
            };
            self.run.begin_prompt();
            let frame = stamp_user_message(message, self.config.verbatim_prompts);
            if let Err(error) = self.write_frame(&frame).await {
                tracing::error!("Prompt stream failed; closing stdin: {error}");
                break;
            }
            written += 1;
        }
        if written > 0 {
            self.wait_for_run_end_and_end_input().await;
        } else {
            self.end_input().await;
        }
    }
}

/// Ends the run when the reader task stops, however it stops. When it
/// stops without recording its end, which only a panic in code it runs can
/// cause, the pending requests fail at once and the message stream ends
/// with an error instead of just ending.
struct ReaderFinish {
    shared: Arc<Shared>,
    outbox: mpsc::Sender<Result<Value>>,
}

impl Drop for ReaderFinish {
    fn drop(&mut self) {
        let lost = {
            let mut state = self.shared.state();
            if state.reader_end.is_some() || state.closed {
                None
            } else {
                let error = reader_lost();
                state.fail_pending(reader_lost);
                state.reader_end = Some(ReaderEnd::Failed(error.clone()));
                Some(error)
            }
        };
        if let Some(error) = lost {
            let _ = self.outbox.try_send(Err(error));
        }
        self.shared.run.finish();
    }
}

fn reader_lost() -> Error {
    Error::connection_closed("The session stopped reading the CLI output because of a panic")
}

/// Frames for the application that did not fit in its queue, in order,
/// with the size of their JSON text.
#[derive(Default)]
struct Held {
    frames: VecDeque<Value>,
    bytes: usize,
}

impl Held {
    fn push(&mut self, frame: Value) {
        self.bytes += json_size(&frame);
        self.frames.push_back(frame);
    }

    fn pop(&mut self) -> Option<Value> {
        let frame = self.frames.pop_front()?;
        self.bytes = self.bytes.saturating_sub(json_size(&frame));
        Some(frame)
    }

    fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }

    fn is_full(&self) -> bool {
        self.bytes >= HELD_FRAMES_BUDGET
    }
}

/// Length of the JSON text of a frame, counted without building it.
fn json_size(frame: &Value) -> usize {
    struct Count(usize);
    impl std::io::Write for Count {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0 += bytes.len();
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut count = Count(0);
    let _ = serde_json::to_writer(&mut count, frame);
    count.0
}

/// Waits for the flush in progress, if any, and clears it.
async fn finish_flush(flushing: &mut Option<BoxFuture<'static, ()>>) {
    if let Some(flush) = flushing.as_mut() {
        flush.await;
    }
    *flushing = None;
}

/// Why the routing loop stopped.
enum Stop {
    /// The output ended, cleanly or with an error.
    Ended(Option<Error>),
    /// The application dropped its queue, or the session closed.
    Abandoned,
}

async fn read_messages(shared: Arc<Shared>, outbox: mpsc::Sender<Result<Value>>) {
    let _finish = ReaderFinish {
        shared: Arc::clone(&shared),
        outbox: outbox.clone(),
    };
    let mut frames = shared.transport.read_messages();
    let mut last_error_result: Option<Value> = None;
    let mut held = Held::default();
    let mut flushing: Option<BoxFuture<'static, ()>> = None;
    let stop = loop {
        let deliverable = flushing.is_none() && !held.is_empty();
        tokio::select! {
            biased;
            () = finish_flush(&mut flushing), if flushing.is_some() => {}
            permit = outbox.reserve(), if deliverable => {
                let Ok(permit) = permit else {
                    break Stop::Abandoned;
                };
                if let Some(frame) = held.pop() {
                    permit.send(Ok(frame));
                }
            }
            frame = frames.next(), if !held.is_full() => {
                let Some(frame) = frame else {
                    break Stop::Ended(None);
                };
                if shared.is_closed() {
                    break Stop::Abandoned;
                }
                let frame = match frame {
                    Ok(frame) => frame,
                    Err(error) => break Stop::Ended(Some(error)),
                };
                let Some(frame) = route(&shared, frame, &mut last_error_result, &mut flushing)
                else {
                    continue;
                };
                if flushing.is_some() || !held.is_empty() {
                    held.push(frame);
                    continue;
                }
                match outbox.try_send(Ok(frame)) {
                    Ok(()) => {}
                    Err(mpsc::error::TrySendError::Full(Ok(frame))) => held.push(frame),
                    Err(mpsc::error::TrySendError::Full(Err(_))) => {}
                    Err(mpsc::error::TrySendError::Closed(_)) => break Stop::Abandoned,
                }
            }
        }
    };
    let (failure, deliver) = match stop {
        Stop::Ended(failure) => (
            failure.map(|error| exit_error(error, last_error_result)),
            true,
        ),
        Stop::Abandoned => (None, false),
    };
    {
        let mut state = shared.state();
        match &failure {
            Some(error) => {
                state.fail_pending(|| error.clone());
                state.reader_end = Some(ReaderEnd::Failed(error.clone()));
            }
            None => {
                state.fail_pending(output_ended);
                state.reader_end = Some(ReaderEnd::Clean);
            }
        }
    }
    finish_flush(&mut flushing).await;
    if deliver {
        deliver_the_rest(&outbox, held, failure).await;
    }
    if let Some(mirror) = &shared.config.mirror {
        mirror.flush().await;
    }
}

/// Hands the held frames to the application once the output has ended,
/// then the error that ended it, if any. Nothing is left to route, so
/// waiting for the application blocks nothing.
async fn deliver_the_rest(
    outbox: &mpsc::Sender<Result<Value>>,
    mut held: Held,
    failure: Option<Error>,
) {
    while let Some(frame) = held.pop() {
        if outbox.send(Ok(frame)).await.is_err() {
            return;
        }
    }
    if let Some(error) = failure {
        let _ = outbox.send(Err(error)).await;
    }
}

/// Routes one frame. Returns the frame when it is meant for the
/// application. A `result` frame starts the flush of the transcript
/// mirror, which the frame then waits for.
fn route(
    shared: &Arc<Shared>,
    frame: Value,
    last_error_result: &mut Option<Value>,
    flushing: &mut Option<BoxFuture<'static, ()>>,
) -> Option<Value> {
    let is_session_state = |frame: &Value| {
        frame.get("subtype").and_then(Value::as_str) == Some("session_state_changed")
    };
    match frame.get("type").and_then(Value::as_str) {
        Some("control_response") => {
            shared.complete_request(&frame);
            None
        }
        Some("control_request") => {
            handlers::spawn(shared, frame);
            None
        }
        Some("control_cancel_request") => {
            handlers::cancel(shared, &frame);
            None
        }
        Some("transcript_mirror") => {
            shared.mirror(&frame);
            None
        }
        Some("system") => {
            if shared.run.observe_system(&frame) {
                return None;
            }
            if !is_session_state(&frame) {
                *last_error_result = None;
            }
            Some(frame)
        }
        Some("result") => {
            if let Some(mirror) = &shared.config.mirror {
                let mirror = Arc::clone(mirror);
                let previous = flushing.take();
                *flushing = Some(Box::pin(async move {
                    if let Some(previous) = previous {
                        previous.await;
                    }
                    mirror.flush().await;
                }));
            }
            shared.run.observe_result();
            let is_error = frame.get("is_error").and_then(Value::as_bool) == Some(true);
            *last_error_result = is_error.then(|| frame.clone());
            Some(frame)
        }
        kind => {
            *last_error_result = None;
            let on_main_thread = frame.get("parent_tool_use_id").is_none_or(Value::is_null);
            if matches!(kind, Some("assistant" | "stream_event")) && on_main_thread {
                shared.run.observe_main_thread_activity();
            }
            Some(frame)
        }
    }
}

/// The CLI ends a failed run with an error result and then exits with a
/// non-zero code on purpose. The exit alone says nothing, so it is replaced
/// by an error carrying what the CLI already reported, with the process
/// error as its source, as upstream sets `__cause__`.
fn exit_error(error: Error, last_error_result: Option<Value>) -> Error {
    match (error, last_error_result) {
        (error, Some(result)) if error.kind() == ErrorKind::Process => {
            let exit_code = error.exit_code();
            tracing::debug!(
                "Replacing the process error (exit code {exit_code:?}) with the error result"
            );
            Error::result_error(result, exit_code).with_source(error)
        }
        (error, _) => {
            tracing::error!("Fatal error in message reader: {error}");
            error
        }
    }
}

struct PendingRequest<'a> {
    shared: &'a Shared,
    request_id: &'a str,
}

impl Drop for PendingRequest<'_> {
    fn drop(&mut self) {
        self.shared.state().pending.remove(self.request_id);
    }
}

impl ControlSession {
    /// Creates a session over a transport, with the queue the application
    /// reads. Nothing runs until [`open`](Self::open).
    pub(crate) fn new(transport: SharedTransport, config: SessionConfig) -> (Self, Inbox) {
        let (outbox, inbox) = mpsc::channel(MESSAGE_QUEUE_CAPACITY);
        let (writer, writer_jobs) = mpsc::unbounded_channel();
        let shared = Arc::new(Shared {
            writer,
            run: RunEndTracker::new(config.run_end_ceiling_ms, config.bidirectional),
            transport,
            config,
            mirror_errors: outbox.downgrade(),
            state: Mutex::new(State {
                outbox: Some(outbox),
                pending: HashMap::new(),
                inflight: HashMap::new(),
                next_handler: 0,
                tasks: Vec::new(),
                reader: None,
                reader_end: None,
                request_counter: 0,
                closed: false,
                initialization: None,
                writer_jobs: Some(writer_jobs),
            }),
            closing: tokio::sync::Mutex::new(false),
            resume: Mutex::new(None),
        });
        if let Some(mirror) = &shared.config.mirror {
            mirror.attach(MirrorErrorReporter::new(shared.mirror_errors.clone()));
        }
        (Self { shared }, inbox)
    }

    /// Creates the session of these options. Without a transport, the CLI
    /// runs as a child process.
    pub(crate) fn for_options(
        options: &ClaudeAgentOptions,
        transport: Option<SharedTransport>,
    ) -> Result<(Self, Inbox)> {
        let options = options.for_session()?;
        let config = SessionConfig::from_options(&options)?;
        let transport =
            transport.unwrap_or_else(|| SharedTransport::new(SubprocessTransport::new(options)));
        Ok(Self::new(transport, config))
    }

    /// Creates the session of these options like
    /// [`for_options`](Self::for_options), after loading the session to
    /// resume from the store of the options, when there is one, into a
    /// temporary config directory the child runs with. The directory belongs
    /// to the session and is removed when it closes; with a custom
    /// transport nothing is loaded, since the options never reach the
    /// transport.
    pub(crate) async fn prepare(
        options: &ClaudeAgentOptions,
        transport: Option<SharedTransport>,
    ) -> Result<(Self, Inbox)> {
        let shadowed = options.can_use_tool_shadowed_warning();
        let options = options.for_session()?;
        if let Some(warning) = shadowed {
            tracing::warn!(
                target: CAN_USE_TOOL_SHADOWED_TARGET,
                code = CAN_USE_TOOL_SHADOWED_CODE,
                "{warning}"
            );
        }
        let materialized = match transport {
            None => materialize_resume_session(&options).await?,
            Some(_) => None,
        };
        let Some(materialized) = materialized else {
            return Self::for_options(&options, transport);
        };
        let (session, inbox) = Self::for_options(&materialized.apply(&options), transport)?;
        *session.shared.lock_resume() = Some(materialized.directory);
        Ok((session, inbox))
    }

    /// Closes the session on a detached task, for the paths that cannot
    /// wait: a dropped stream or a dropped client.
    pub(crate) fn close_detached(self, runtime: &tokio::runtime::Handle) {
        runtime.spawn(crate::caller_context::in_current_context(async move {
            if let Err(error) = self.close().await {
                tracing::debug!("Error closing the session: {error}");
            }
        }));
    }

    /// Connects the transport, starts the reader, and performs the
    /// initialize handshake, which registers the hooks before any prompt.
    pub(crate) async fn open(&self) -> Result<()> {
        self.shared.transport.connect().await?;
        self.start();
        self.initialize().await?;
        Ok(())
    }

    fn start(&self) {
        let mut state = self.shared.state();
        if state.closed || state.reader.is_some() {
            return;
        }
        if let Some(outbox) = state.outbox.take() {
            state.reader = Some(crate::caller_context::spawn(read_messages(
                Arc::clone(&self.shared),
                outbox,
            )));
        }
    }

    async fn initialize(&self) -> Result<Value> {
        let config = &self.shared.config;
        let response = self
            .send_control_request(config.initialize_request.clone(), config.initialize_timeout)
            .await?;
        if config.blocks_on_failure {
            if !truthy(response.get("hook_on_failure_supported")) {
                return Err(Error::unsupported(
                    "The process answering initialize does not honor on_failure on hook \
                     matchers, so a hook that fails would not block. Update Claude Code if it \
                     is old, or remove on_failure=\"block\" from your hooks.",
                ));
            }
            if response.get("hooks_applied") == Some(&Value::Bool(false)) {
                return Err(Error::unsupported(
                    "This session was already running and did not register your hooks, so a \
                     hook set to on_failure=\"block\" would never run. Start a new session, or \
                     remove on_failure=\"block\" from your hooks.",
                ));
            }
        }
        self.shared.state().initialization = Some(response.clone());
        Ok(response)
    }

    /// The response to the initialize request, once the handshake is done.
    pub(crate) fn server_info(&self) -> Option<Value> {
        self.shared.state().initialization.clone()
    }

    /// Sends a control request and waits for its response.
    pub(crate) async fn send_control_request(
        &self,
        request: Value,
        limit: Duration,
    ) -> Result<Value> {
        let shared = &*self.shared;
        let subtype = request
            .get("subtype")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let (request_id, response) = {
            let mut state = shared.state();
            if let Some(error) = state.unanswerable() {
                return Err(error);
            }
            state.request_counter += 1;
            let random = Uuid::new_v4().simple().to_string();
            let request_id = format!(
                "req_{}_{}",
                state.request_counter,
                &random[..REQUEST_ID_RANDOM_CHARS]
            );
            let (waiter, response) = oneshot::channel();
            state.pending.insert(request_id.clone(), waiter);
            (request_id, response)
        };
        let _pending = PendingRequest {
            shared,
            request_id: &request_id,
        };
        let exchange = async {
            shared
                .write_frame(&json!({
                    "type": "control_request",
                    "request_id": request_id,
                    "request": request,
                }))
                .await?;
            response.await.unwrap_or_else(|_| Err(session_closed()))
        };
        tokio::time::timeout(limit, exchange)
            .await
            .unwrap_or_else(|_| Err(Error::control_timeout(subtype)))
    }

    /// Writes one user message, marked as client composed when the session
    /// delivers prompts verbatim.
    pub(crate) async fn write_user_message(&self, message: Value) -> Result<()> {
        let frame = stamp_user_message(message, self.shared.config.verbatim_prompts);
        self.shared.write_frame(&frame).await
    }

    /// Closes the standard input once the run has ended, in the background.
    pub(crate) fn end_input_after_run(&self) {
        let shared = Arc::clone(&self.shared);
        self.track(crate::caller_context::spawn(async move {
            shared.wait_for_run_end_and_end_input().await;
        }));
    }

    /// Writes the messages of an input stream in the background, then
    /// closes the standard input once the run of the last one has ended.
    pub(crate) fn stream_input(&self, input: InputStream) {
        let shared = Arc::clone(&self.shared);
        self.track(crate::caller_context::spawn(async move {
            shared.stream_input(input).await;
        }));
    }

    fn track(&self, task: JoinHandle<()>) {
        let mut state = self.shared.state();
        if state.closed {
            task.abort();
            return;
        }
        state.tasks.retain(|task| !task.is_finished());
        state.tasks.push(task.abort_handle());
    }

    /// Reports a failed transcript mirror append as a `mirror_error` system
    /// message. It never blocks: with a full queue the message is logged
    /// and dropped.
    #[cfg(test)]
    pub(crate) fn report_mirror_error(&self, key: Option<&SessionKey>, error: &str) {
        MirrorErrorReporter::new(self.shared.mirror_errors.clone()).report(key, error);
    }

    /// Stops the session and closes the transport. Calling it again does
    /// nothing.
    ///
    /// Handlers and input tasks stop, the reader stops, every pending
    /// control request fails, the in-process MCP servers stop, and the queue
    /// ends once the application has read what it already holds.
    ///
    /// A close whose future is dropped part-way is finished by the next
    /// call: what is still to stop stays registered until it is stopped.
    pub(crate) async fn close(&self) -> Result<()> {
        let shared = &self.shared;
        let mut closed = shared.closing.lock().await;
        if *closed {
            return Ok(());
        }
        shared.run.mark_closed();
        shared.state().closed = true;
        if let Some(mirror) = &shared.config.mirror {
            mirror.close().await;
        }
        let (reader, tasks, inflight, outbox) = {
            let mut state = shared.state();
            (
                state.reader.take(),
                std::mem::take(&mut state.tasks),
                std::mem::take(&mut state.inflight),
                state.outbox.take(),
            )
        };
        for task in tasks {
            task.abort();
        }
        for (_, (_, cancel)) in inflight {
            let _ = cancel.send(());
        }
        drop(outbox);
        if let Some(reader) = reader {
            reader.abort();
            let _ = reader.await;
        }
        shared.state().fail_pending(session_closed);
        shared.config.mcp.close().await;
        let result = shared.transport.close().await;
        let resume = shared.lock_resume().take();
        if let Some(resume) = resume {
            resume.remove().await;
        }
        *closed = true;
        result
    }

    #[cfg(test)]
    pub(crate) fn with_mirror(
        transport: SharedTransport,
        mut config: SessionConfig,
        mirror: Arc<dyn TranscriptMirror>,
    ) -> (Self, Inbox) {
        config.mirror = Some(mirror);
        Self::new(transport, config)
    }

    #[cfg(test)]
    pub(crate) fn handlers_in_flight(&self) -> usize {
        self.shared.state().inflight.len()
    }

    #[cfg(test)]
    pub(crate) fn run(&self) -> &RunEndTracker {
        &self.shared.run
    }
}
