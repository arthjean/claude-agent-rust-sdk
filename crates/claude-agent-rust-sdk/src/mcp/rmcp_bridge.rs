//! Bridge to a server built with the `rmcp` crate.
//!
//! The server runs over an in-memory pair of message channels, so `rmcp`
//! itself dispatches every method the server implements. One session exists
//! per configured server for the lifetime of a query: it is started by the
//! first message and closed with the query. When the server stops underneath
//! the CLI, later messages are answered with an error naming the server and
//! the cause, until a new `initialize` starts it again.
//!
//! A request the CLI cancels is answered here, with the error `Request
//! cancelled`: `rmcp` passes the cancellation on to the handler and then
//! sends no response for that request, whatever the handler returns.
//!
//! Nothing carries traffic the server initiates to the CLI: notifications
//! are dropped, a `ping` is answered here, and every other request is
//! refused, so the caller in the server fails at once instead of waiting.

use std::collections::HashMap;
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use rmcp::model::{ClientJsonRpcMessage, JsonRpcMessage, ServerJsonRpcMessage};
use rmcp::service::{QuitReason, RoleServer};
use rmcp::transport::Transport;
use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio::time::Instant;

use super::server::display_id;
use crate::types::BoxFuture;

/// Time the end of a session waits for the server to stop.
pub(crate) use super::server::SHUTDOWN_GRACE;

const METHOD_NOT_FOUND: i64 = -32601;
/// The code of the answer to a cancelled call: the Python `mcp` package
/// (1.x, which upstream runs) answers `0`, not the `-32800` of MCP 2.
const REQUEST_CANCELLED: i64 = 0;

#[derive(Debug, thiserror::Error)]
#[error("the in-process MCP channel is closed")]
pub(crate) struct ChannelClosed;

/// Server side of the in-memory channel pair.
pub(crate) struct ChannelTransport {
    incoming: mpsc::UnboundedReceiver<ClientJsonRpcMessage>,
    outgoing: mpsc::UnboundedSender<ServerJsonRpcMessage>,
}

impl Transport<RoleServer> for ChannelTransport {
    type Error = ChannelClosed;

    fn send(
        &mut self,
        item: ServerJsonRpcMessage,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send + 'static {
        std::future::ready(self.outgoing.send(item).map_err(|_| ChannelClosed))
    }

    fn receive(&mut self) -> impl Future<Output = Option<ClientJsonRpcMessage>> + Send {
        self.incoming.recv()
    }

    fn close(&mut self) -> impl Future<Output = Result<(), Self::Error>> + Send {
        self.incoming.close();
        std::future::ready(Ok(()))
    }
}

type Serve = dyn Fn(ChannelTransport) -> BoxFuture<'static, Result<(), String>> + Send + Sync;

/// An `rmcp` server handler with its type erased: a function that serves it
/// over one transport until the service ends.
#[derive(Clone)]
pub(crate) struct RmcpServer {
    serve: Arc<Serve>,
}

impl RmcpServer {
    pub(crate) fn new<S: rmcp::ServerHandler>(service: S) -> Self {
        let service = Arc::new(service);
        Self {
            serve: Arc::new(move |transport| {
                let service = Arc::clone(&service);
                Box::pin(async move {
                    let running = rmcp::serve_server(service, transport)
                        .await
                        .map_err(|error| error.to_string())?;
                    match running.waiting().await {
                        Ok(QuitReason::JoinError(error)) | Err(error) => Err(error.to_string()),
                        Ok(_) => Ok(()),
                    }
                })
            }),
        }
    }

    #[cfg(test)]
    pub(crate) fn from_fn(
        serve: impl Fn(ChannelTransport) -> BoxFuture<'static, Result<(), String>>
        + Send
        + Sync
        + 'static,
    ) -> Self {
        Self {
            serve: Arc::new(serve),
        }
    }
}

struct Pending {
    id: Value,
    method: String,
    waiter: oneshot::Sender<Result<Value, String>>,
}

#[derive(Default)]
struct SessionState {
    pending: HashMap<String, Pending>,
    finished: bool,
    failure: Option<String>,
}

impl SessionState {
    fn finish(&mut self, name: &str) {
        self.finished = true;
        for (_, pending) in self.pending.drain() {
            let _ = pending.waiter.send(Err(session_closed(name)));
        }
    }
}

fn session_closed(name: &str) -> String {
    format!("SDK MCP server '{name}' session closed")
}

/// The key, in the table of pending requests, of the request that a
/// `notifications/cancelled` message cancels.
fn cancelled_request(message: &Value) -> Option<String> {
    if message.get("method").and_then(Value::as_str) != Some("notifications/cancelled") {
        return None;
    }
    let id = message.get("params")?.get("requestId")?;
    Some(id.to_string())
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// One run of the server over one channel pair.
struct Session {
    name: String,
    state: Arc<Mutex<SessionState>>,
    to_server: Mutex<Option<mpsc::UnboundedSender<ClientJsonRpcMessage>>>,
    task: Mutex<Option<JoinHandle<()>>>,
}

impl Session {
    fn start(name: &str, server: &RmcpServer) -> Arc<Self> {
        let (to_server, incoming) = mpsc::unbounded_channel();
        let (outgoing, from_server) = mpsc::unbounded_channel();
        let state = Arc::new(Mutex::new(SessionState::default()));
        let serve = (server.serve)(ChannelTransport { incoming, outgoing });
        let task = crate::caller_context::spawn(run(
            name.to_owned(),
            serve,
            from_server,
            to_server.downgrade(),
            Arc::clone(&state),
        ));
        Arc::new(Self {
            name: name.to_owned(),
            state,
            to_server: Mutex::new(Some(to_server)),
            task: Mutex::new(Some(task)),
        })
    }

    fn finished(&self) -> bool {
        lock(&self.state).finished
            || lock(&self.task)
                .as_ref()
                .is_none_or(JoinHandle::is_finished)
    }

    fn failure(&self) -> Option<String> {
        lock(&self.state).failure.clone()
    }

    async fn submit(&self, message: &Value) -> Result<Option<Value>, String> {
        let parsed: ClientJsonRpcMessage = serde_json::from_value(message.clone())
            .map_err(|error| format!("Invalid JSON-RPC message: {error}"))?;
        let cancelled = match &parsed {
            JsonRpcMessage::Notification(_) => cancelled_request(message),
            _ => None,
        };
        let mut waiting = None;
        if matches!(parsed, JsonRpcMessage::Request(_)) {
            let id = message.get("id").cloned().unwrap_or(Value::Null);
            let key = id.to_string();
            let (waiter, response) = oneshot::channel();
            let mut state = lock(&self.state);
            if state.finished {
                return Err(session_closed(&self.name));
            }
            if state.pending.contains_key(&key) {
                return Err(format!(
                    "Request id {} is already in flight",
                    display_id(&id)
                ));
            }
            let method = message
                .get("method")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            state
                .pending
                .insert(key.clone(), Pending { id, method, waiter });
            waiting = Some((key, response));
        }
        let sent = lock(&self.to_server)
            .as_ref()
            .is_some_and(|to_server| to_server.send(parsed).is_ok());
        if !sent {
            if let Some((key, _)) = &waiting {
                lock(&self.state).pending.remove(key);
            }
            return Err(session_closed(&self.name));
        }
        let Some((_, response)) = waiting else {
            if let Some(key) = cancelled {
                self.end_cancelled(&key);
            }
            return Ok(None);
        };
        match response.await {
            Ok(Ok(response)) => Ok(Some(response)),
            Ok(Err(failure)) => Err(failure),
            Err(_) => Err(session_closed(&self.name)),
        }
    }

    /// Answers a request the CLI cancelled and frees its id. The server is
    /// told of the cancellation and sends no response for that request, so
    /// this answer is the only one the request gets; a response the server
    /// had already sent is dropped as one to an unknown request.
    fn end_cancelled(&self, key: &str) {
        let pending = lock(&self.state).pending.remove(key);
        let Some(pending) = pending else {
            return;
        };
        let answer = json!({
            "jsonrpc": "2.0",
            "id": pending.id,
            "error": {"code": REQUEST_CANCELLED, "message": "Request cancelled"},
        });
        if pending.waiter.send(Ok(answer)).is_err() {
            tracing::debug!(
                "Request {key} of SDK MCP server '{}' was cancelled after its caller gave up",
                self.name
            );
        }
    }

    /// Ends the session: cancels the requests still running, closes the
    /// input of the server, and waits for it to stop, for at most
    /// [`SHUTDOWN_GRACE`].
    async fn close(&self) {
        let deadline = Instant::now() + SHUTDOWN_GRACE;
        let to_server = lock(&self.to_server).take();
        if let Some(to_server) = to_server {
            let in_flight: Vec<Value> = lock(&self.state)
                .pending
                .values()
                .filter(|pending| pending.method != "initialize")
                .map(|pending| pending.id.clone())
                .collect();
            for id in in_flight {
                let cancellation = serde_json::from_value(json!({
                    "jsonrpc": "2.0",
                    "method": "notifications/cancelled",
                    "params": {"requestId": id, "reason": "connection closing"},
                }));
                if let Ok(cancellation) = cancellation {
                    let _ = to_server.send(cancellation);
                }
            }
        }
        let task = lock(&self.task).take();
        let Some(mut task) = task else {
            return;
        };
        if tokio::time::timeout_at(deadline, &mut task).await.is_err() {
            tracing::warn!(
                "SDK MCP server '{}' did not stop within {} s of being closed; no longer \
                 waiting for it",
                self.name,
                SHUTDOWN_GRACE.as_secs()
            );
            task.abort();
            lock(&self.state).finish(&self.name);
        }
    }
}

async fn run(
    name: String,
    mut serve: BoxFuture<'static, Result<(), String>>,
    mut from_server: mpsc::UnboundedReceiver<ServerJsonRpcMessage>,
    to_server: mpsc::WeakUnboundedSender<ClientJsonRpcMessage>,
    state: Arc<Mutex<SessionState>>,
) {
    let _finish = FinishOnDrop {
        name: name.clone(),
        state: Arc::clone(&state),
    };
    let mut outcome = None;
    loop {
        tokio::select! {
            result = &mut serve, if outcome.is_none() => outcome = Some(result),
            message = from_server.recv() => match message {
                Some(message) => deliver(&name, &message, &to_server, &state),
                None => break,
            },
        }
    }
    if let Some(Err(failure)) = &outcome {
        tracing::error!("SDK MCP server '{name}' failed: {failure}");
        lock(&state).failure = Some(failure.clone());
    }
    lock(&state).finish(&name);
    if outcome.is_none() {
        match tokio::time::timeout(SHUTDOWN_GRACE, &mut serve).await {
            Ok(Err(failure)) => {
                tracing::error!("SDK MCP server '{name}' failed: {failure}");
                lock(&state).failure = Some(failure);
            }
            Ok(Ok(())) => {}
            Err(_) => tracing::warn!(
                "SDK MCP server '{name}' closed its output but did not stop within {} s",
                SHUTDOWN_GRACE.as_secs()
            ),
        }
    }
}

struct FinishOnDrop {
    name: String,
    state: Arc<Mutex<SessionState>>,
}

impl Drop for FinishOnDrop {
    fn drop(&mut self) {
        lock(&self.state).finish(&self.name);
    }
}

fn deliver(
    name: &str,
    message: &ServerJsonRpcMessage,
    to_server: &mpsc::WeakUnboundedSender<ClientJsonRpcMessage>,
    state: &Mutex<SessionState>,
) {
    let payload = match serde_json::to_value(message) {
        Ok(payload) => payload,
        Err(error) => {
            tracing::debug!("SDK MCP server '{name}' sent a message that does not encode: {error}");
            return;
        }
    };
    let Some(method) = payload.get("method").and_then(Value::as_str) else {
        let key = payload.get("id").unwrap_or(&Value::Null).to_string();
        let pending = lock(state).pending.remove(&key);
        match pending {
            Some(pending) => {
                if pending.waiter.send(Ok(payload)).is_err() {
                    tracing::debug!(
                        "Dropping a late response to request {key} from SDK MCP server '{name}'"
                    );
                }
            }
            None => tracing::debug!(
                "Dropping a response to unknown request {key} from SDK MCP server '{name}'"
            ),
        }
        return;
    };
    let Some(id) = payload.get("id") else {
        tracing::debug!("Dropping a {method} notification from SDK MCP server '{name}'");
        return;
    };
    let answer = if method == "ping" {
        json!({"jsonrpc": "2.0", "id": id, "result": {}})
    } else {
        tracing::warn!(
            "SDK MCP server '{name}' sent a {method} request to the client; requests to the \
             client are not supported for SDK servers, refusing it"
        );
        json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": {
                "code": METHOD_NOT_FOUND,
                "message": format!("{method} is not supported for SDK servers"),
            },
        })
    };
    if let (Some(to_server), Ok(answer)) = (to_server.upgrade(), serde_json::from_value(answer)) {
        let _ = to_server.send(answer);
    }
}

/// Routes the JSON-RPC messages of one session to one `rmcp` server.
pub(crate) struct RmcpBridge {
    name: String,
    server: RmcpServer,
    session: tokio::sync::Mutex<Option<Arc<Session>>>,
    closed: AtomicBool,
}

impl RmcpBridge {
    pub(crate) fn new(name: String, server: RmcpServer) -> Self {
        Self {
            name,
            server,
            session: tokio::sync::Mutex::new(None),
            closed: AtomicBool::new(false),
        }
    }

    /// Handles one message from the CLI. Returns the JSON-RPC response to a
    /// request, and nothing for a notification or a response.
    pub(crate) async fn handle(&self, message: &Value) -> Result<Option<Value>, String> {
        if self.closed.load(Ordering::Acquire) {
            return Err(format!("SDK MCP server '{}' is closed", self.name));
        }
        if message.get("method").is_some()
            && message.get("id").is_some()
            && super::server::request_id(message).is_none()
        {
            tracing::debug!(
                "SDK MCP server '{}' got a request whose id is neither an integer nor a string; \
                 it is acknowledged as a notification, as upstream does",
                self.name
            );
            return Ok(None);
        }
        let session = {
            let mut slot = self.session.lock().await;
            match slot.as_ref() {
                Some(session) if !session.finished() => Arc::clone(session),
                Some(stopped) => {
                    if message.get("method").and_then(Value::as_str) != Some("initialize") {
                        let cause = stopped
                            .failure()
                            .unwrap_or_else(|| "its service ended".to_owned());
                        return Err(format!("SDK MCP server '{}' stopped: {cause}", self.name));
                    }
                    let session = Session::start(&self.name, &self.server);
                    if let Some(stopped) = slot.replace(Arc::clone(&session)) {
                        stopped.close().await;
                    }
                    session
                }
                None => {
                    let session = Session::start(&self.name, &self.server);
                    *slot = Some(Arc::clone(&session));
                    session
                }
            }
        };
        session.submit(message).await
    }

    /// Stops the session, if any. Calling it again does nothing.
    pub(crate) async fn close(&self) {
        self.closed.store(true, Ordering::Release);
        let session = self.session.lock().await.take();
        if let Some(session) = session {
            session.close().await;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use rmcp::model::ServerJsonRpcMessage;
    use rmcp::transport::Transport;
    use serde_json::{Value, json};
    use tokio::sync::Notify;

    use super::{ChannelTransport, RmcpBridge, RmcpServer, SHUTDOWN_GRACE};
    use crate::test_capture as capture;
    use crate::testing::MockTransport;
    use crate::types::{McpSdkServerConfig, McpSdkServerInstance};
    use crate::{ClaudeAgentOptions, ClaudeSdkClient};

    async fn connect(server: RmcpServer) -> (MockTransport, ClaudeSdkClient) {
        let transport = MockTransport::new();
        transport.respond_with(|frame| {
            if frame["type"] == "control_request" {
                vec![json!({
                    "type": "control_response",
                    "response": {
                        "subtype": "success",
                        "request_id": frame["request_id"],
                        "response": {},
                    },
                })]
            } else {
                Vec::new()
            }
        });
        let options = ClaudeAgentOptions::builder()
            .mcp_server(
                "srv",
                McpSdkServerConfig::new(
                    "scripted",
                    McpSdkServerInstance(crate::types::SdkServer::Rmcp(server)),
                ),
            )
            .build()
            .unwrap();
        let client = ClaudeSdkClient::with_transport(options, transport.clone());
        client.connect().await.unwrap();
        (transport, client)
    }

    fn message(id: u64, method: &str) -> Value {
        json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": {"name": "claude-code", "version": "2.1.325"},
            },
        })
    }

    fn post(transport: &MockTransport, request_id: &str, id: u64, method: &str) {
        transport.push_frame(json!({
            "type": "control_request",
            "request_id": request_id,
            "request": {
                "subtype": "mcp_message",
                "server_name": "srv",
                "message": message(id, method),
            },
        }));
    }

    async fn settle() {
        for _ in 0..10 {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    }

    async fn answer(transport: &MockTransport, request_id: &str) -> Value {
        for _ in 0..100_000 {
            let answer = transport
                .written_frames()
                .into_iter()
                .find(|frame| frame["response"]["request_id"] == request_id);
            if let Some(answer) = answer {
                return answer["response"]["response"]["mcp_response"].clone();
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        panic!("control request {request_id} was never answered");
    }

    async fn request(transport: &MockTransport, request_id: &str, id: u64, method: &str) -> Value {
        post(transport, request_id, id, method);
        answer(transport, request_id).await
    }

    fn crashing(crash: &Arc<Notify>, starts: &Arc<AtomicUsize>) -> RmcpServer {
        let crash = Arc::clone(crash);
        let starts = Arc::clone(starts);
        RmcpServer::from_fn(move |mut transport: ChannelTransport| {
            let crash = Arc::clone(&crash);
            starts.fetch_add(1, Ordering::SeqCst);
            Box::pin(async move {
                loop {
                    let message = tokio::select! {
                        () = crash.notified() => return Err("the disk is on fire".to_owned()),
                        message = transport.receive() => message,
                    };
                    let Some(message) = message else {
                        return Ok(());
                    };
                    let message = serde_json::to_value(&message).unwrap();
                    let (Some(id), Some(method)) = (message.get("id"), message.get("method"))
                    else {
                        continue;
                    };
                    let result = if method == "initialize" {
                        handshake()
                    } else {
                        json!({})
                    };
                    respond(&mut transport, id, result).await;
                }
            })
        })
    }

    fn handshake() -> Value {
        json!({
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "serverInfo": {"name": "scripted", "version": "1.0.0"},
        })
    }

    async fn respond(transport: &mut ChannelTransport, id: &Value, result: Value) {
        let response: ServerJsonRpcMessage =
            serde_json::from_value(json!({"jsonrpc": "2.0", "id": id, "result": result})).unwrap();
        transport.send(response).await.unwrap();
    }

    /// A server that completes the handshake, then never returns from the
    /// first other request and never stops: it keeps its transport open and
    /// reads nothing more, neither a cancellation nor the end of its input.
    fn stuck_in_its_first_call(entered: &Arc<Notify>) -> RmcpServer {
        let entered = Arc::clone(entered);
        RmcpServer::from_fn(move |mut transport: ChannelTransport| {
            let entered = Arc::clone(&entered);
            Box::pin(async move {
                loop {
                    let Some(message) = transport.receive().await else {
                        return Ok(());
                    };
                    let message = serde_json::to_value(&message).unwrap();
                    let (Some(id), Some(method)) = (message.get("id"), message.get("method"))
                    else {
                        continue;
                    };
                    if method != "initialize" {
                        entered.notify_one();
                        return std::future::pending::<Result<(), String>>().await;
                    }
                    respond(&mut transport, id, handshake()).await;
                }
            })
        })
    }

    #[tokio::test(start_paused = true)]
    async fn server_that_fails_after_a_good_handshake_stays_stopped_until_a_new_initialize() {
        let crash = Arc::new(Notify::new());
        let starts = Arc::new(AtomicUsize::new(0));
        let (transport, client) = connect(crashing(&crash, &starts)).await;

        let handshake = request(&transport, "mcp-1", 0, "initialize").await;
        assert_eq!(handshake["result"]["serverInfo"]["name"], "scripted");
        let pong = request(&transport, "mcp-2", 1, "ping").await;
        assert_eq!(pong["result"], json!({}));
        assert_eq!(starts.load(Ordering::SeqCst), 1);

        crash.notify_one();
        settle().await;
        for (request_id, id) in [("mcp-3", 2), ("mcp-4", 3)] {
            let stopped = request(&transport, request_id, id, "ping").await;
            assert_eq!(stopped["id"], id);
            assert_eq!(stopped["error"]["code"], -32603);
            assert_eq!(
                stopped["error"]["message"],
                "SDK MCP server 'srv' stopped: the disk is on fire"
            );
        }
        assert_eq!(starts.load(Ordering::SeqCst), 1);

        let restarted = request(&transport, "mcp-5", 4, "initialize").await;
        assert_eq!(restarted["result"]["serverInfo"]["name"], "scripted");
        assert_eq!(starts.load(Ordering::SeqCst), 2);
        let pong = request(&transport, "mcp-6", 5, "ping").await;
        assert_eq!(pong["result"], json!({}));
        client.disconnect().await.unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn request_in_flight_when_the_server_fails_is_answered_with_an_error() {
        let crash = Arc::new(Notify::new());
        let silent = {
            let crash = Arc::clone(&crash);
            RmcpServer::from_fn(move |transport: ChannelTransport| {
                let crash = Arc::clone(&crash);
                Box::pin(async move {
                    let _transport = transport;
                    crash.notified().await;
                    Err("the disk is on fire".to_owned())
                })
            })
        };
        let (transport, client) = connect(silent).await;
        post(&transport, "mcp-1", 0, "ping");
        settle().await;
        crash.notify_one();
        let failed = answer(&transport, "mcp-1").await;
        assert_eq!(failed["id"], 0);
        assert_eq!(failed["error"]["code"], -32603);
        assert_eq!(
            failed["error"]["message"],
            "SDK MCP server 'srv' session closed"
        );
        client.disconnect().await.unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn closing_waits_at_most_the_grace_period_for_a_server_that_does_not_stop() {
        let stuck = RmcpServer::from_fn(|transport: ChannelTransport| {
            Box::pin(async move {
                let _transport = transport;
                std::future::pending::<Result<(), String>>().await
            })
        });
        let (transport, client) = connect(stuck).await;
        post(&transport, "mcp-1", 0, "ping");
        settle().await;

        let (warnings, _guard) = capture::Warnings::install();
        let before = tokio::time::Instant::now();
        client.disconnect().await.unwrap();
        let waited = before.elapsed();
        assert_eq!(SHUTDOWN_GRACE, Duration::from_secs(5));
        assert!(waited >= SHUTDOWN_GRACE, "{waited:?}");
        assert!(
            waited < SHUTDOWN_GRACE + Duration::from_secs(1),
            "{waited:?}"
        );
        let logged = warnings.all();
        assert!(
            logged.iter().any(|warning| warning
                == "SDK MCP server 'srv' did not stop within 5 s of being closed; no longer \
                    waiting for it"),
            "{logged:?}"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn messages_after_close_are_refused_and_start_nothing() {
        let crash = Arc::new(Notify::new());
        let starts = Arc::new(AtomicUsize::new(0));
        let bridge = RmcpBridge::new("srv".to_owned(), crashing(&crash, &starts));
        let initialized = bridge.handle(&message(0, "initialize")).await.unwrap();
        assert_eq!(initialized.unwrap()["result"], handshake());
        assert_eq!(starts.load(Ordering::SeqCst), 1);
        bridge.close().await;

        let listing = json!({"jsonrpc": "2.0", "id": 9, "method": "tools/list"});
        for refused in [listing, message(10, "initialize")] {
            assert_eq!(
                bridge.handle(&refused).await,
                Err("SDK MCP server 'srv' is closed".to_owned())
            );
        }
        assert!(bridge.session.lock().await.is_none());
        assert_eq!(starts.load(Ordering::SeqCst), 1);

        bridge.close().await;
        assert!(bridge.session.lock().await.is_none());
    }

    #[tokio::test(start_paused = true)]
    async fn waiter_on_a_stuck_call_is_failed_once_the_grace_period_is_up() {
        let entered = Arc::new(Notify::new());
        let bridge = Arc::new(RmcpBridge::new(
            "srv".to_owned(),
            stuck_in_its_first_call(&entered),
        ));
        let initialized = bridge.handle(&message(0, "initialize")).await.unwrap();
        assert_eq!(initialized.unwrap()["result"], handshake());

        let call = json!({
            "jsonrpc": "2.0",
            "id": 5,
            "method": "tools/call",
            "params": {"name": "blocked", "arguments": {}},
        });
        let waiter = {
            let bridge = Arc::clone(&bridge);
            tokio::spawn(async move { bridge.handle(&call).await })
        };
        entered.notified().await;

        let before = tokio::time::Instant::now();
        bridge.close().await;
        let waited = before.elapsed();
        assert!(waited >= SHUTDOWN_GRACE, "{waited:?}");
        assert!(
            waited < SHUTDOWN_GRACE + Duration::from_secs(1),
            "{waited:?}"
        );

        let outcome = tokio::time::timeout(Duration::from_secs(1), waiter)
            .await
            .expect("the waiter still hangs after the grace period")
            .unwrap();
        assert_eq!(
            outcome,
            Err("SDK MCP server 'srv' session closed".to_owned())
        );
    }
}
