//! Carries the context of the caller into the tasks the crate spawns.
//!
//! Upstream starts each task in a copy of the `contextvars` of the code
//! that started it, so a callback sees the OpenTelemetry context that was
//! active when the session connected. A tokio task starts with no
//! `tracing` span and no OpenTelemetry context, so every task that may run
//! application code (a permission callback, a hook, an in-process MCP
//! tool, a session store, a stderr callback, an input stream) is spawned
//! through [`spawn`] or wrapped with [`in_current_context`]: the task
//! enters the `tracing` span current where it was spawned and, with the
//! `otel` feature, runs under the OpenTelemetry context current there. A
//! task spawned from such a task inherits the same context in turn, so the
//! handlers the reader starts run in the context of the caller that opened
//! the session.

use std::future::Future;

use tokio::task::JoinHandle;
use tracing::Instrument;

/// Wraps a future so that it runs in the `tracing` span and, with the
/// `otel` feature, the OpenTelemetry context current at this call.
#[cfg(feature = "otel")]
pub(crate) fn in_current_context<F: Future>(future: F) -> impl Future<Output = F::Output> {
    use opentelemetry::context::FutureExt as _;
    future.in_current_span().with_current_context()
}

/// Wraps a future so that it runs in the `tracing` span current at this
/// call.
#[cfg(not(feature = "otel"))]
pub(crate) fn in_current_context<F: Future>(future: F) -> impl Future<Output = F::Output> {
    future.in_current_span()
}

/// Spawns a task that runs in the context current at this call, as
/// [`in_current_context`] describes.
pub(crate) fn spawn<F>(future: F) -> JoinHandle<F::Output>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    tokio::spawn(in_current_context(future))
}
