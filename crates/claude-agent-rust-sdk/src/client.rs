//! Interactive client: a long-lived session with the Claude Code CLI.

use std::fmt;
use std::marker::PhantomData;
use std::pin::Pin;
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll};

use futures_core::Stream;
use futures_util::{StreamExt, stream};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use tokio::runtime::Handle;

use crate::control::{CONTROL_REQUEST_TIMEOUT, ControlSession, Inbox};
use crate::error::{Error, ErrorKind, Result};
use crate::message_parser::parse_message;
use crate::prompt::{Prompt, text_message, with_default_session};
use crate::transport::SharedTransport;
use crate::types::{
    ClaudeAgentOptions, ContextUsageResponse, McpStatusResponse, Message, PermissionMode,
};

const DEFAULT_SESSION_ID: &str = "default";
const NOT_CONNECTED: &str = "Not connected. Call connect() first.";

/// Client for a multi-turn, interactive conversation with the Claude Code CLI.
///
/// The client keeps one session open: send a prompt with
/// [`query`](Self::query), read the answer with
/// [`receive_response`](Self::receive_response), and send follow-ups in the
/// same context. Settings change mid-session through the control methods.
/// For a single prompt whose inputs are all known upfront, [`query`]
/// is simpler.
///
/// Every method takes `&self`, so one task can read messages while another
/// sends a prompt or an interrupt. Every method other than
/// [`connect`](Self::connect) and [`disconnect`](Self::disconnect) fails
/// with [`ErrorKind::NotConnected`](crate::ErrorKind::NotConnected) before the client is connected.
///
/// Messages wait in a queue until they are read. While they wait, the
/// control methods, hooks, and permission callbacks keep working, so an
/// [`interrupt`](Self::interrupt) can be sent from the loop that reads the
/// messages. Past 16 MiB of unread messages the session stops reading the
/// CLI output until the application reads again, and control traffic
/// waits with it.
///
/// Dropping a connected client ends the CLI process through the bounded
/// shutdown of the transport, on a detached task. Call
/// [`disconnect`](Self::disconnect) to wait for it.
///
/// ```no_run
/// use claude_agent_rust_sdk::{ClaudeAgentOptions, ClaudeSdkClient, Message};
/// use futures_util::StreamExt;
///
/// # async fn demo() -> Result<(), claude_agent_rust_sdk::Error> {
/// let client = ClaudeSdkClient::new(ClaudeAgentOptions::default());
/// client.connect().await?;
///
/// client.query("What is the capital of France?").await?;
/// let mut response = client.receive_response();
/// while let Some(message) = response.next().await {
///     if let Message::Assistant(assistant) = message? {
///         println!("{:?}", assistant.content);
///     }
/// }
/// drop(response);
///
/// client.set_permission_mode(claude_agent_rust_sdk::PermissionMode::AcceptEdits).await?;
/// client.query("Now write it to capital.txt").await?;
/// client.disconnect().await?;
/// # Ok(())
/// # }
/// ```
///
/// [`query`]: crate::query()
pub struct ClaudeSdkClient {
    options: ClaudeAgentOptions,
    transport: Option<SharedTransport>,
    lifecycle: tokio::sync::Mutex<()>,
    connection: Mutex<Option<Arc<Connection>>>,
    /// The session a [`connect`](Self::connect) is opening, published
    /// before the handshake as upstream publishes its query, so a
    /// concurrent [`disconnect`](Self::disconnect) can close it instead of
    /// waiting for the handshake to time out.
    connecting: Mutex<Option<ControlSession>>,
}

struct Connection {
    session: ControlSession,
    inbox: tokio::sync::Mutex<Inbox>,
    runtime: Option<Handle>,
}

impl Drop for Connection {
    fn drop(&mut self) {
        if let Some(runtime) = &self.runtime {
            self.session.clone().close_detached(runtime);
        }
    }
}

fn not_connected() -> Error {
    Error::not_connected(NOT_CONNECTED)
}

impl ClaudeSdkClient {
    /// Creates a client. Nothing runs until [`connect`](Self::connect).
    #[must_use]
    pub fn new(options: ClaudeAgentOptions) -> Self {
        Self {
            options,
            transport: None,
            lifecycle: tokio::sync::Mutex::new(()),
            connection: Mutex::new(None),
            connecting: Mutex::new(None),
        }
    }

    /// Creates a client over a custom transport instead of a child process.
    ///
    /// The options that become CLI arguments are not applied to a custom
    /// transport; hooks, agents, and the other initialize settings are
    /// still sent on the control channel.
    ///
    /// Every [`connect`](Self::connect) reuses this transport: it closes
    /// the previous session, which closes the transport, then connects the
    /// transport again. A [`SubprocessTransport`](crate::SubprocessTransport)
    /// starts a new process each time.
    #[must_use]
    pub fn with_transport(
        options: ClaudeAgentOptions,
        transport: impl Into<SharedTransport>,
    ) -> Self {
        Self {
            transport: Some(transport.into()),
            ..Self::new(options)
        }
    }

    /// Starts the session: connects the transport and performs the
    /// initialize handshake, which registers the hooks.
    ///
    /// On a client that is already connected, the previous session is
    /// closed first.
    ///
    /// # Errors
    ///
    /// Returns the errors of the transport, [`ErrorKind::Config`](crate::ErrorKind::Config) for options
    /// the CLI cannot honor, [`ErrorKind::ControlTimeout`](crate::ErrorKind::ControlTimeout) when the CLI does not
    /// answer the handshake, and [`ErrorKind::Unsupported`](crate::ErrorKind::Unsupported) when a hook
    /// matcher asks to block on failure and the CLI cannot honor it. After
    /// an error nothing stays running.
    pub async fn connect(&self) -> Result<()> {
        self.open(None).await
    }

    /// Starts the session like [`connect`](Self::connect), then sends a
    /// first prompt.
    ///
    /// A text prompt is written at once and the standard input stays open.
    /// An input stream is written in the background, and the standard
    /// input closes once the run of its last message has ended.
    ///
    /// # Errors
    ///
    /// Returns the errors of [`connect`](Self::connect) and of the first
    /// write.
    pub async fn connect_with_prompt(&self, prompt: impl Into<Prompt>) -> Result<()> {
        self.open(Some(prompt.into())).await
    }

    async fn open(&self, prompt: Option<Prompt>) -> Result<()> {
        let _lifecycle = self.lifecycle.lock().await;
        if let Some(previous) = self.take_connection() {
            previous.close().await;
        }
        let (session, inbox) =
            ControlSession::prepare(&self.options, self.transport.clone()).await?;
        let connection = Arc::new(Connection {
            session,
            inbox: tokio::sync::Mutex::new(inbox),
            runtime: Handle::try_current().ok(),
        });
        let session = &connection.session;
        *self.connecting_slot() = Some(session.clone());
        let opened = async {
            session.open().await?;
            match prompt {
                None => {}
                Some(Prompt::Text(text)) => {
                    session
                        .write_user_message(text_message(&text, DEFAULT_SESSION_ID))
                        .await?;
                }
                Some(Prompt::Stream(input)) => session.stream_input(input),
            }
            Ok(())
        };
        let opened = opened.await;
        self.connecting_slot().take();
        if let Err(error) = opened {
            connection.close().await;
            return Err(error);
        }
        *self.slot() = Some(connection);
        Ok(())
    }

    fn connecting_slot(&self) -> std::sync::MutexGuard<'_, Option<ControlSession>> {
        self.connecting
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    fn slot(&self) -> std::sync::MutexGuard<'_, Option<Arc<Connection>>> {
        self.connection
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    fn take_connection(&self) -> Option<Arc<Connection>> {
        self.slot().take()
    }

    fn connection(&self) -> Result<Arc<Connection>> {
        self.slot().clone().ok_or_else(not_connected)
    }

    /// Sends a prompt in the session `default`.
    ///
    /// The standard input stays open, so more prompts can follow. With an
    /// input stream, each message that names no session gets `default`.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::NotConnected`](crate::ErrorKind::NotConnected) when the client is not connected,
    /// when an input stream fails, or when a write fails.
    pub async fn query(&self, prompt: impl Into<Prompt>) -> Result<()> {
        self.query_with_session_id(prompt, DEFAULT_SESSION_ID).await
    }

    /// Sends a prompt in the given session. See [`query`](Self::query).
    ///
    /// # Errors
    ///
    /// Returns the errors of [`query`](Self::query).
    pub async fn query_with_session_id(
        &self,
        prompt: impl Into<Prompt>,
        session_id: &str,
    ) -> Result<()> {
        let connection = self.connection()?;
        let session = &connection.session;
        match prompt.into() {
            Prompt::Text(text) => {
                session
                    .write_user_message(text_message(&text, session_id))
                    .await
            }
            Prompt::Stream(mut input) => {
                while let Some(message) = input.next().await {
                    let message = message.map_err(|error| {
                        Error::new(ErrorKind::PromptStream, "Prompt stream failed")
                            .with_source(error)
                    })?;
                    session
                        .write_user_message(with_default_session(message, session_id))
                        .await?;
                }
                Ok(())
            }
        }
    }

    /// Returns every message of the session, as they arrive.
    ///
    /// The stream ends when the session ends. Several streams of one client
    /// share the same queue: each message goes to one of them. An error,
    /// such as a message that cannot be parsed, is yielded and ends the
    /// stream, as upstream's iterator raises; a new call resumes with the
    /// next message. Before [`connect`](Self::connect) the stream yields one
    /// [`ErrorKind::NotConnected`](crate::ErrorKind::NotConnected).
    pub fn receive_messages(&self) -> Messages<'_> {
        self.messages(false)
    }

    /// Returns the messages up to and including the next result message,
    /// which completes one response.
    ///
    /// Without a result message the stream continues until the session
    /// ends. Before [`connect`](Self::connect) the stream yields one
    /// [`ErrorKind::NotConnected`](crate::ErrorKind::NotConnected).
    pub fn receive_response(&self) -> Messages<'_> {
        self.messages(true)
    }

    fn messages(&self, until_result: bool) -> Messages<'_> {
        let Ok(connection) = self.connection() else {
            return Messages::new(stream::once(std::future::ready(Err(not_connected()))));
        };
        Messages::new(stream::unfold(
            Some(connection),
            move |connection| async move {
                let connection = connection?;
                let frame = connection.inbox.lock().await.recv().await?;
                let message = frame.and_then(parse_message);
                let ends =
                    message.is_err() || (until_result && matches!(message, Ok(Message::Result(_))));
                Some((message, (!ends).then_some(connection)))
            },
        ))
    }

    async fn control(&self, request: Value) -> Result<Value> {
        self.connection()?
            .session
            .send_control_request(request, CONTROL_REQUEST_TIMEOUT)
            .await
    }

    /// Interrupts the current turn.
    ///
    /// # Errors
    ///
    /// Every control method returns [`ErrorKind::NotConnected`](crate::ErrorKind::NotConnected) or
    /// [`ErrorKind::ConnectionClosed`](crate::ErrorKind::ConnectionClosed) when the
    /// client is not connected or the session ends, [`ErrorKind::Control`](crate::ErrorKind::Control) when
    /// the CLI refuses the request, and [`ErrorKind::ControlTimeout`](crate::ErrorKind::ControlTimeout) when the
    /// CLI does not answer within 60 s.
    pub async fn interrupt(&self) -> Result<()> {
        self.control(json!({"subtype": "interrupt"})).await?;
        Ok(())
    }

    /// Changes the permission mode of the session.
    ///
    /// # Errors
    ///
    /// See [`interrupt`](Self::interrupt).
    pub async fn set_permission_mode(&self, mode: PermissionMode) -> Result<()> {
        self.control(json!({"subtype": "set_permission_mode", "mode": mode.as_str()}))
            .await?;
        Ok(())
    }

    /// Changes the model of the session. `None` selects the default model.
    ///
    /// # Errors
    ///
    /// See [`interrupt`](Self::interrupt).
    pub async fn set_model(&self, model: Option<&str>) -> Result<()> {
        self.control(json!({"subtype": "set_model", "model": model}))
            .await?;
        Ok(())
    }

    /// Rewinds the tracked files to their state at a user message.
    ///
    /// It needs the `enable_file_checkpointing` option. The id is the
    /// `uuid` of a [`UserMessage`](crate::UserMessage) received in this
    /// session, which the CLI emits with the extra argument
    /// `replay-user-messages`.
    ///
    /// # Errors
    ///
    /// See [`interrupt`](Self::interrupt).
    pub async fn rewind_files(&self, user_message_id: &str) -> Result<()> {
        self.control(json!({"subtype": "rewind_files", "user_message_id": user_message_id}))
            .await?;
        Ok(())
    }

    /// Reconnects an MCP server that failed or was disconnected.
    ///
    /// # Errors
    ///
    /// See [`interrupt`](Self::interrupt).
    pub async fn reconnect_mcp_server(&self, server_name: &str) -> Result<()> {
        self.control(json!({"subtype": "mcp_reconnect", "serverName": server_name}))
            .await?;
        Ok(())
    }

    /// Enables or disables an MCP server. Disabling a server disconnects it
    /// and removes its tools.
    ///
    /// # Errors
    ///
    /// See [`interrupt`](Self::interrupt).
    pub async fn toggle_mcp_server(&self, server_name: &str, enabled: bool) -> Result<()> {
        self.control(json!({
            "subtype": "mcp_toggle",
            "serverName": server_name,
            "enabled": enabled,
        }))
        .await?;
        Ok(())
    }

    /// Stops a running task. The id comes from the `task_started` system
    /// message. The end of the task is then reported in the message stream
    /// by a `task_updated` message with a terminal status.
    ///
    /// # Errors
    ///
    /// See [`interrupt`](Self::interrupt).
    pub async fn stop_task(&self, task_id: &str) -> Result<()> {
        self.control(json!({"subtype": "stop_task", "task_id": task_id}))
            .await?;
        Ok(())
    }

    /// Returns the connection status of every configured MCP server.
    ///
    /// # Errors
    ///
    /// See [`interrupt`](Self::interrupt). Returns [`ErrorKind::MessageParse`](crate::ErrorKind::MessageParse)
    /// when the response does not have the expected shape.
    #[doc(alias = "get_mcp_status")]
    pub async fn mcp_status(&self) -> Result<McpStatusResponse> {
        typed(
            self.control(json!({"subtype": "mcp_status"})).await?,
            "mcp_status",
        )
    }

    /// Returns the breakdown of the context window by category, the data
    /// behind the `/context` command of the CLI.
    ///
    /// # Errors
    ///
    /// See [`interrupt`](Self::interrupt). Returns [`ErrorKind::MessageParse`](crate::ErrorKind::MessageParse)
    /// when the response does not have the expected shape.
    #[doc(alias = "get_context_usage")]
    pub async fn context_usage(&self) -> Result<ContextUsageResponse> {
        typed(
            self.control(json!({"subtype": "get_context_usage"}))
                .await?,
            "get_context_usage",
        )
    }

    /// Returns the response of the CLI to the initialize handshake: the
    /// available commands, the output styles, and the capabilities.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::NotConnected`](crate::ErrorKind::NotConnected) when the client is not connected.
    #[doc(alias = "get_server_info")]
    pub fn server_info(&self) -> Result<Option<Value>> {
        Ok(self.connection()?.session.server_info())
    }

    /// Ends the session and waits for the transport to close.
    ///
    /// It is safe on a client that never connected, and calling it again
    /// does nothing. During a [`connect`](Self::connect) it closes the
    /// session being opened, so the connect fails at once with
    /// [`ErrorKind::ConnectionClosed`](crate::ErrorKind::ConnectionClosed)
    /// instead of waiting for the handshake to time out.
    ///
    /// # Errors
    ///
    /// Returns the error of the transport when closing it fails.
    pub async fn disconnect(&self) -> Result<()> {
        let connecting = self.connecting_slot().take();
        if let Some(session) = connecting {
            session.close().await?;
        }
        let _lifecycle = self.lifecycle.lock().await;
        match self.take_connection() {
            Some(connection) => connection.session.close().await,
            None => Ok(()),
        }
    }
}

impl Connection {
    async fn close(&self) {
        if let Err(error) = self.session.close().await {
            tracing::debug!("Error closing the session: {error}");
        }
    }
}

fn typed<T: DeserializeOwned>(response: Value, subtype: &str) -> Result<T> {
    T::deserialize(&response).map_err(|error| {
        Error::message_parse(
            format!("Invalid {subtype} response: {error}"),
            Some(response),
        )
    })
}

impl fmt::Debug for ClaudeSdkClient {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ClaudeSdkClient")
            .field("connected", &self.slot().is_some())
            .finish_non_exhaustive()
    }
}

/// Stream of the messages of a [`ClaudeSdkClient`] session.
///
/// It yields `Result<Message, Error>`, and keeps returning `None` once it
/// has ended.
#[must_use = "streams do nothing unless polled"]
pub struct Messages<'a> {
    inner: Pin<Box<dyn Stream<Item = Result<Message>> + Send>>,
    client: PhantomData<&'a ClaudeSdkClient>,
}

impl Messages<'_> {
    fn new(stream: impl Stream<Item = Result<Message>> + Send + 'static) -> Self {
        Self {
            inner: Box::pin(stream.fuse()),
            client: PhantomData,
        }
    }
}

impl Stream for Messages<'_> {
    type Item = Result<Message>;

    fn poll_next(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.inner.as_mut().poll_next(context)
    }
}

impl fmt::Debug for Messages<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Messages(..)")
    }
}
