//! Transports: raw message exchange with the CLI.
//!
//! A [`Transport`] moves newline-delimited JSON between this process and a
//! CLI, without interpreting it. [`SubprocessTransport`] is the standard
//! implementation: it spawns the CLI as a child process. A custom
//! implementation, such as a remote connection, implements the trait and is
//! handed over as a [`SharedTransport`].

mod command;
mod environment;
mod framing;
mod subprocess;

use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use futures_core::Stream;
use serde_json::Value;

use crate::error::Result;
use crate::types::BoxFuture;

pub use subprocess::SubprocessTransport;

/// Stream of the JSON messages read from a transport.
///
/// The stream ends after the first error it yields.
pub type MessageStream = Pin<Box<dyn Stream<Item = Result<Value>> + Send>>;

/// Raw message exchange with a CLI process or service.
///
/// Methods take `&self` so one transport can be written to from several
/// tasks while another task reads from it. An implementation needs interior
/// mutability for its connection state.
///
/// This low-level interface follows the upstream SDK and may change with
/// it.
pub trait Transport: Send + Sync + 'static {
    /// Connects the transport. A subprocess transport starts its process.
    ///
    /// A [`ClaudeSdkClient`](crate::ClaudeSdkClient) that connects again
    /// calls `connect` on the same transport after [`close`](Self::close),
    /// so a transport that can serve one connection only should fail here.
    fn connect(&self) -> impl Future<Output = Result<()>> + Send;

    /// Writes raw data, usually one JSON message followed by a line feed.
    ///
    /// A session of this crate calls `write` and
    /// [`end_input`](Self::end_input) from one task, one call at a time and
    /// in order, and drops a pending write only when the session closes.
    fn write(&self, data: &str) -> impl Future<Output = Result<()>> + Send;

    /// Returns the stream of parsed messages. It is meant to be called once
    /// per connection.
    fn read_messages(&self) -> MessageStream;

    /// Closes the connection and releases its resources.
    ///
    /// An implementation must bound its own waits: callers do not cancel a
    /// close that hangs.
    fn close(&self) -> impl Future<Output = Result<()>> + Send;

    /// Whether the transport can send and receive messages.
    fn is_ready(&self) -> bool;

    /// Ends the input stream. A subprocess transport closes the standard
    /// input of its process.
    fn end_input(&self) -> impl Future<Output = Result<()>> + Send;
}

trait ErasedTransport: Send + Sync {
    fn connect(&self) -> BoxFuture<'_, Result<()>>;
    fn write<'a>(&'a self, data: &'a str) -> BoxFuture<'a, Result<()>>;
    fn read_messages(&self) -> MessageStream;
    fn close(&self) -> BoxFuture<'_, Result<()>>;
    fn is_ready(&self) -> bool;
    fn end_input(&self) -> BoxFuture<'_, Result<()>>;
}

impl<T: Transport> ErasedTransport for T {
    fn connect(&self) -> BoxFuture<'_, Result<()>> {
        Box::pin(Transport::connect(self))
    }

    fn write<'a>(&'a self, data: &'a str) -> BoxFuture<'a, Result<()>> {
        Box::pin(Transport::write(self, data))
    }

    fn read_messages(&self) -> MessageStream {
        Transport::read_messages(self)
    }

    fn close(&self) -> BoxFuture<'_, Result<()>> {
        Box::pin(Transport::close(self))
    }

    fn is_ready(&self) -> bool {
        Transport::is_ready(self)
    }

    fn end_input(&self) -> BoxFuture<'_, Result<()>> {
        Box::pin(Transport::end_input(self))
    }
}

/// Shared handle to a [`Transport`] of any type.
///
/// Cloning the handle shares the same transport. Any [`Transport`] converts
/// into it with `into()`.
#[derive(Clone)]
pub struct SharedTransport(Arc<dyn ErasedTransport>);

impl SharedTransport {
    /// Wraps a transport.
    pub fn new(transport: impl Transport) -> Self {
        Self(Arc::new(transport))
    }

    /// See [`Transport::connect`].
    ///
    /// # Errors
    ///
    /// Returns the error of the transport.
    pub async fn connect(&self) -> Result<()> {
        self.0.connect().await
    }

    /// See [`Transport::write`].
    ///
    /// # Errors
    ///
    /// Returns the error of the transport.
    pub async fn write(&self, data: &str) -> Result<()> {
        self.0.write(data).await
    }

    /// See [`Transport::read_messages`].
    pub fn read_messages(&self) -> MessageStream {
        self.0.read_messages()
    }

    /// See [`Transport::close`].
    ///
    /// # Errors
    ///
    /// Returns the error of the transport.
    pub async fn close(&self) -> Result<()> {
        self.0.close().await
    }

    /// See [`Transport::is_ready`].
    #[must_use]
    pub fn is_ready(&self) -> bool {
        self.0.is_ready()
    }

    /// See [`Transport::end_input`].
    ///
    /// # Errors
    ///
    /// Returns the error of the transport.
    pub async fn end_input(&self) -> Result<()> {
        self.0.end_input().await
    }
}

impl<T: Transport> From<T> for SharedTransport {
    fn from(transport: T) -> Self {
        Self::new(transport)
    }
}

impl fmt::Debug for SharedTransport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SharedTransport")
            .field("ready", &self.is_ready())
            .finish_non_exhaustive()
    }
}
