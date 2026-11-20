//! One-shot queries: a prompt in, a stream of messages out.

use std::fmt;
use std::pin::Pin;
use std::task::{Context, Poll};

use futures_core::Stream;
use tokio::runtime::Handle;

use crate::control::{ControlSession, Inbox};
use crate::error::{Error, Result};
use crate::message_parser::parse_message;
use crate::prompt::{Prompt, one_shot_text_message};
use crate::transport::SharedTransport;
use crate::types::{BoxFuture, ClaudeAgentOptions, Message};

const ONE_SHOT_SESSION_ID: &str = "";

/// Runs one query against the Claude Code CLI and returns the stream of its
/// messages.
///
/// The prompt is a string, or a stream of input messages wrapped in a
/// [`Prompt`]. All inputs are sent, then all responses are received: for a
/// conversation that reacts to responses, use
/// [`ClaudeSdkClient`](crate::ClaudeSdkClient).
///
/// Nothing runs until the stream is polled. The stream ends after the first
/// error it yields. When the CLI reports an error result and exits with a
/// non-zero code, the last item is [`ErrorKind::ResultError`](crate::ErrorKind::ResultError) carrying the text
/// of that result. Dropping the stream ends the CLI process through the
/// bounded shutdown of the transport, on a detached task.
///
/// The stream must be polled inside a Tokio runtime.
///
/// ```no_run
/// use claude_agent_rust_sdk::{ClaudeAgentOptions, Message, query};
/// use futures_util::StreamExt;
///
/// # async fn demo() -> Result<(), claude_agent_rust_sdk::Error> {
/// let options = ClaudeAgentOptions::builder().max_turns(1).build()?;
/// let mut messages = query("What is 2 + 2?", options);
/// while let Some(message) = messages.next().await {
///     if let Message::Result(result) = message? {
///         println!("{}", result.result.unwrap_or_default());
///     }
/// }
/// # Ok(())
/// # }
/// ```
pub fn query(prompt: impl Into<Prompt>, options: ClaudeAgentOptions) -> QueryStream {
    QueryStream::new(prompt.into(), options, None)
}

/// Runs one query over a custom transport instead of a child process.
///
/// The SDK connects the transport, delivers the prompt through it, and
/// sends hooks, agents, and the other initialize settings on the control
/// channel. The options that become CLI arguments (model, working
/// directory, permission mode, tools) are not applied to a custom
/// transport.
pub fn query_with_transport(
    prompt: impl Into<Prompt>,
    options: ClaudeAgentOptions,
    transport: impl Into<SharedTransport>,
) -> QueryStream {
    QueryStream::new(prompt.into(), options, Some(transport.into()))
}

/// Stream of the messages of one query, returned by [`query`].
///
/// It yields `Result<Message, Error>` and ends after the first error.
#[must_use = "a query does nothing until its stream is polled"]
pub struct QueryStream {
    phase: Phase,
    session: Option<ControlSession>,
    runtime: Option<Handle>,
}

enum Phase {
    Idle(Box<Request>),
    Preparing {
        prepare: BoxFuture<'static, Result<(ControlSession, Inbox)>>,
        prompt: Prompt,
    },
    Starting {
        start: BoxFuture<'static, Result<()>>,
        inbox: Inbox,
    },
    Running(Inbox),
    Closing(BoxFuture<'static, ()>),
    Done,
}

struct Request {
    prompt: Prompt,
    options: ClaudeAgentOptions,
    transport: Option<SharedTransport>,
}

async fn start(session: ControlSession, prompt: Prompt) -> Result<()> {
    session.open().await?;
    match prompt {
        Prompt::Text(text) => {
            session
                .write_user_message(one_shot_text_message(&text, ONE_SHOT_SESSION_ID))
                .await?;
            session.end_input_after_run();
        }
        Prompt::Stream(input) => session.stream_input(input),
    }
    Ok(())
}

impl QueryStream {
    fn new(
        prompt: Prompt,
        options: ClaudeAgentOptions,
        transport: Option<SharedTransport>,
    ) -> Self {
        Self {
            phase: Phase::Idle(Box::new(Request {
                prompt,
                options,
                transport,
            })),
            session: None,
            runtime: None,
        }
    }

    fn close(&mut self) {
        self.phase = match self.session.clone() {
            Some(session) => Phase::Closing(Box::pin(async move {
                if let Err(error) = session.close().await {
                    tracing::debug!("Error closing the session: {error}");
                }
            })),
            None => Phase::Done,
        };
    }

    fn fail(&mut self, error: Error) -> Poll<Option<Result<Message>>> {
        self.close();
        Poll::Ready(Some(Err(error)))
    }
}

impl Stream for QueryStream {
    type Item = Result<Message>;

    fn poll_next(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        loop {
            match std::mem::replace(&mut this.phase, Phase::Done) {
                Phase::Idle(request) => {
                    let Request {
                        prompt,
                        options,
                        transport,
                    } = *request;
                    this.runtime = Handle::try_current().ok();
                    this.phase = Phase::Preparing {
                        prepare: Box::pin(async move {
                            ControlSession::prepare(&options, transport).await
                        }),
                        prompt,
                    };
                }
                Phase::Preparing {
                    mut prepare,
                    prompt,
                } => match prepare.as_mut().poll(context) {
                    Poll::Pending => {
                        this.phase = Phase::Preparing { prepare, prompt };
                        return Poll::Pending;
                    }
                    Poll::Ready(Ok((session, inbox))) => {
                        this.session = Some(session.clone());
                        this.phase = Phase::Starting {
                            start: Box::pin(start(session, prompt)),
                            inbox,
                        };
                    }
                    Poll::Ready(Err(error)) => return Poll::Ready(Some(Err(error))),
                },
                Phase::Starting { mut start, inbox } => match start.as_mut().poll(context) {
                    Poll::Pending => {
                        this.phase = Phase::Starting { start, inbox };
                        return Poll::Pending;
                    }
                    Poll::Ready(Ok(())) => this.phase = Phase::Running(inbox),
                    Poll::Ready(Err(error)) => return this.fail(error),
                },
                Phase::Running(mut inbox) => match inbox.poll_recv(context) {
                    Poll::Pending => {
                        this.phase = Phase::Running(inbox);
                        return Poll::Pending;
                    }
                    Poll::Ready(Some(Ok(frame))) => match parse_message(frame) {
                        Ok(message) => {
                            this.phase = Phase::Running(inbox);
                            return Poll::Ready(Some(Ok(message)));
                        }
                        Err(error) => return this.fail(error),
                    },
                    Poll::Ready(Some(Err(error))) => return this.fail(error),
                    Poll::Ready(None) => this.close(),
                },
                Phase::Closing(mut close) => match close.as_mut().poll(context) {
                    Poll::Pending => {
                        this.phase = Phase::Closing(close);
                        return Poll::Pending;
                    }
                    Poll::Ready(()) => {
                        this.session = None;
                        return Poll::Ready(None);
                    }
                },
                Phase::Done => return Poll::Ready(None),
            }
        }
    }
}

impl Drop for QueryStream {
    fn drop(&mut self) {
        if let (Some(session), Some(runtime)) = (self.session.take(), self.runtime.take()) {
            session.close_detached(&runtime);
        }
    }
}

impl fmt::Debug for QueryStream {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let phase = match self.phase {
            Phase::Idle(_) => "idle",
            Phase::Preparing { .. } => "preparing",
            Phase::Starting { .. } => "starting",
            Phase::Running(_) => "running",
            Phase::Closing(_) => "closing",
            Phase::Done => "done",
        };
        formatter
            .debug_struct("QueryStream")
            .field("phase", &phase)
            .finish_non_exhaustive()
    }
}
