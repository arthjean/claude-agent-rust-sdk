//! Test support, behind the `testing` feature.
//!
//! [`MockTransport`] is an in-memory [`Transport`]: it records what is
//! written to it and plays the frames a test scripts, so protocol code runs
//! without a CLI process.
//!
//! ```
//! use claude_agent_rust_sdk::testing::MockTransport;
//! use claude_agent_rust_sdk::transport::Transport;
//! use serde_json::json;
//!
//! # async fn demo() -> Result<(), claude_agent_rust_sdk::Error> {
//! let transport = MockTransport::new();
//! transport.push_frame(json!({"type": "system", "subtype": "init"}));
//! transport.connect().await?;
//! transport.write("{\"type\":\"user\"}\n").await?;
//! assert_eq!(transport.written_frames(), [json!({"type": "user"})]);
//! # Ok(())
//! # }
//! ```
//!
//! [`run_session_store_conformance`] checks that a
//! [`SessionStore`](crate::SessionStore) adapter honors the storage
//! contract.

mod conformance;

pub use conformance::{
    ConformanceFailure, run_session_store_conformance, run_session_store_conformance_skipping,
};

use std::collections::VecDeque;
use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::task::{Poll, Waker};

use futures_util::stream;
use serde_json::Value;

use crate::error::{Error, ErrorKind, Result};
use crate::transport::{MessageStream, Transport};

type Responder = Arc<dyn Fn(&Value) -> Vec<Value> + Send + Sync>;

/// In-memory [`Transport`] that records written frames and plays scripted
/// ones.
///
/// Cloning the transport shares its state, so a test keeps one clone to
/// script and inspect while the code under test owns another.
///
/// Connecting again after a close reopens the transport, so one mock can
/// serve several sessions in a row.
///
/// Scripted frames are yielded by [`read_messages`](Transport::read_messages)
/// in the order they were pushed, whether they were pushed before or after
/// the stream was created. The stream ends after a scripted error, after
/// [`finish`](Self::finish), when the transport is closed, or, with
/// [`finish_when_input_ends`](Self::finish_when_input_ends), when the input
/// is ended.
#[derive(Clone, Default)]
pub struct MockTransport {
    state: Arc<Mutex<MockState>>,
}

#[derive(Default)]
struct MockState {
    written: Vec<String>,
    scripted: VecDeque<Result<Value>>,
    finished: bool,
    finish_when_input_ends: bool,
    connected: bool,
    closed: bool,
    input_ended: bool,
    connect_count: usize,
    close_count: usize,
    connect_error: Option<String>,
    write_error: Option<String>,
    responder: Option<Responder>,
    reader: Option<Waker>,
}

impl MockState {
    fn wake_reader(&mut self) {
        if let Some(reader) = self.reader.take() {
            reader.wake();
        }
    }
}

impl MockTransport {
    /// Creates a transport with nothing scripted.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates a transport that plays these frames and then ends its
    /// stream.
    #[must_use]
    pub fn with_frames(frames: impl IntoIterator<Item = Value>) -> Self {
        let transport = Self::new();
        for frame in frames {
            transport.push_frame(frame);
        }
        transport.finish();
        transport
    }

    fn state(&self) -> MutexGuard<'_, MockState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Scripts one frame for the message stream.
    pub fn push_frame(&self, frame: Value) {
        let mut state = self.state();
        state.scripted.push_back(Ok(frame));
        state.wake_reader();
    }

    /// Scripts an error for the message stream, which ends after it.
    pub fn push_error(&self, error: Error) {
        let mut state = self.state();
        state.scripted.push_back(Err(error));
        state.wake_reader();
    }

    /// Ends the message stream once the scripted frames are consumed.
    pub fn finish(&self) {
        let mut state = self.state();
        state.finished = true;
        state.wake_reader();
    }

    /// Makes the message stream end once the input is ended, the way the
    /// CLI exits when its standard input closes.
    pub fn finish_when_input_ends(&self) {
        let mut state = self.state();
        state.finish_when_input_ends = true;
        if state.input_ended {
            state.finished = true;
            state.wake_reader();
        }
    }

    /// Registers a function called with each written frame that parses as
    /// JSON. The frames it returns are scripted as replies.
    pub fn respond_with(&self, responder: impl Fn(&Value) -> Vec<Value> + Send + Sync + 'static) {
        self.state().responder = Some(Arc::new(responder));
    }

    /// Makes [`connect`](Transport::connect) fail with
    /// [`ErrorKind::Spawn`] carrying this message.
    pub fn fail_connect(&self, message: impl Into<String>) {
        self.state().connect_error = Some(message.into());
    }

    /// Makes every later [`write`](Transport::write) fail with
    /// [`ErrorKind::ConnectionClosed`] carrying this message.
    pub fn fail_writes(&self, message: impl Into<String>) {
        self.state().write_error = Some(message.into());
    }

    /// Everything written so far, as raw strings.
    #[must_use]
    pub fn written(&self) -> Vec<String> {
        self.state().written.clone()
    }

    /// Everything written so far, parsed line by line as JSON. Blank lines
    /// and lines that do not parse are left out.
    #[must_use]
    pub fn written_frames(&self) -> Vec<Value> {
        self.state()
            .written
            .iter()
            .flat_map(|data| data.lines())
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect()
    }

    /// Number of scripted frames and errors not read yet.
    #[must_use]
    pub fn pending_frames(&self) -> usize {
        self.state().scripted.len()
    }

    /// Number of calls to [`connect`](Transport::connect).
    #[must_use]
    pub fn connect_count(&self) -> usize {
        self.state().connect_count
    }

    /// Number of calls to [`close`](Transport::close).
    #[must_use]
    pub fn close_count(&self) -> usize {
        self.state().close_count
    }

    /// Whether [`end_input`](Transport::end_input) was called.
    #[must_use]
    pub fn input_ended(&self) -> bool {
        self.state().input_ended
    }

    /// Whether the transport is closed: [`close`](Transport::close) was
    /// called and [`connect`](Transport::connect) was not called since.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.state().closed
    }
}

impl Transport for MockTransport {
    async fn connect(&self) -> Result<()> {
        let mut state = self.state();
        state.connect_count += 1;
        if let Some(message) = &state.connect_error {
            return Err(Error::new(ErrorKind::Spawn, message.clone()));
        }
        state.connected = true;
        state.closed = false;
        Ok(())
    }

    async fn write(&self, data: &str) -> Result<()> {
        let responder = {
            let mut state = self.state();
            if !state.connected || state.closed {
                return Err(Error::new(
                    ErrorKind::NotConnected,
                    "ProcessTransport is not ready for writing",
                ));
            }
            if let Some(message) = &state.write_error {
                return Err(Error::new(ErrorKind::ConnectionClosed, message.clone()));
            }
            state.written.push(data.to_owned());
            state.responder.clone()
        };
        let Some(responder) = responder else {
            return Ok(());
        };
        let replies: Vec<Value> = data
            .lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .flat_map(|frame| responder(&frame))
            .collect();
        for reply in replies {
            self.push_frame(reply);
        }
        Ok(())
    }

    fn read_messages(&self) -> MessageStream {
        let state = Arc::clone(&self.state);
        let mut ended = false;
        Box::pin(stream::poll_fn(move |context| {
            if ended {
                return Poll::Ready(None);
            }
            let mut state = state.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some(message) = state.scripted.pop_front() {
                ended = message.is_err();
                return Poll::Ready(Some(message));
            }
            if state.finished || state.closed {
                ended = true;
                return Poll::Ready(None);
            }
            state.reader = Some(context.waker().clone());
            Poll::Pending
        }))
    }

    async fn close(&self) -> Result<()> {
        let mut state = self.state();
        state.close_count += 1;
        state.closed = true;
        state.connected = false;
        state.wake_reader();
        Ok(())
    }

    fn is_ready(&self) -> bool {
        let state = self.state();
        state.connected && !state.closed
    }

    async fn end_input(&self) -> Result<()> {
        let mut state = self.state();
        state.input_ended = true;
        if state.finish_when_input_ends {
            state.finished = true;
            state.wake_reader();
        }
        Ok(())
    }
}

impl fmt::Debug for MockTransport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let state = self.state();
        formatter
            .debug_struct("MockTransport")
            .field("connected", &state.connected)
            .field("closed", &state.closed)
            .field("written", &state.written.len())
            .field("scripted", &state.scripted.len())
            .finish_non_exhaustive()
    }
}
