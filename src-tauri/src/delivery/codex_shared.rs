//! A generation-bound connection to a broker-owned Codex app-server.
//!
//! This module never discovers another daemon, starts a turn during connection,
//! retries a written request, or answers an interactive approval request.

use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::{oneshot, watch, Mutex};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::WebSocketStream;

mod completion;
mod diagnostics;
mod launch_config;
mod launch_model;
mod owner;
mod proxy;
#[cfg(test)]
mod startup_tests;
#[cfg(test)]
pub(crate) mod test_support;
mod version;
pub use owner::{CodexSharedOwner, CodexTuiAttachment};

// Erase only the split writer, preserving each transport's handshake buffers.
type SocketWriter = Box<
    dyn futures_util::Sink<Message, Error = tokio_tungstenite::tungstenite::Error> + Unpin + Send,
>;
type PendingReply = oneshot::Sender<Result<Value, CodexSharedError>>;
pub(super) const STARTUP_TIMEOUT: Duration = Duration::from_secs(120);

/// A request can be uncertain even when the socket write returned an error.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CodexSharedError {
    pub code: String,
    pub message: String,
    pub provider_boundary_crossed: bool,
}

impl std::fmt::Display for CodexSharedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for CodexSharedError {}

impl CodexSharedError {
    fn connection_pending(message: impl Into<String>) -> Self {
        Self {
            code: "connection_pending".into(),
            message: message.into(),
            provider_boundary_crossed: false,
        }
    }

    pub fn unsupported(message: impl Into<String>) -> Self {
        Self {
            code: "unsupported".into(),
            message: message.into(),
            provider_boundary_crossed: false,
        }
    }

    pub(crate) fn uncertain(message: impl Into<String>) -> Self {
        Self {
            code: "submitted_unconfirmed".into(),
            message: message.into(),
            provider_boundary_crossed: true,
        }
    }
}

/// Transport evidence, never a claim that the model consumed a message.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CodexSharedReceipt {
    pub wardian_agent_id: String,
    pub generation: u64,
    pub provider_session_id: String,
    /// Actual initialize-reported version, never the minimum supported version.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_version: Option<String>,
    pub provider_turn_id: Option<String>,
    pub delivery_state: String,
    pub interruption_confirmed: bool,
    pub message_id: Option<String>,
}

pub type CodexPushReceipt = CodexSharedReceipt;
pub type CodexInterruptReceipt = CodexSharedReceipt;

#[derive(Debug, Default)]
pub(crate) struct Observation {
    provider_version: Option<String>,
    thread_id: Option<String>,
    active_turn: Option<String>,
    completed_turn: Option<(String, String)>,
    closed: bool,
    stopped: bool,
    answer: String,
    completions: completion::TurnCompletions,
}

/// Activity is derived only from the bound owner's native turn lifecycle.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum CodexTurnActivity {
    Pending,
    Processing(String),
    Idle(String),
    Closed,
    Stopped,
}

impl Observation {
    /// An intentional close must not turn a healthy session red. Preserve an
    /// already-observed unexpected disconnect so later cleanup cannot mask it.
    fn stop(&mut self) {
        if !self.closed {
            self.stopped = true;
        }
        self.close();
    }
    fn close(&mut self) {
        self.closed = true;
        self.completions.close();
    }
    pub(crate) fn activity(&self) -> CodexTurnActivity {
        if self.stopped {
            CodexTurnActivity::Stopped
        } else if self.closed {
            CodexTurnActivity::Closed
        } else if let Some(id) = &self.active_turn {
            CodexTurnActivity::Processing(id.clone())
        } else if let Some((id, _)) = &self.completed_turn {
            CodexTurnActivity::Idle(id.clone())
        } else {
            // An empty idle server is not proof that its TUI is ready for input.
            CodexTurnActivity::Pending
        }
    }

    /// Only the bound native thread can change interrupt or completion evidence.
    fn observe(&mut self, value: &Value) {
        let params = &value["params"];
        if params["threadId"].as_str() != self.thread_id.as_deref() || self.thread_id.is_none() {
            return;
        }
        if value["method"] == "item/agentMessage/delta"
            && params["turnId"].as_str() == self.active_turn.as_deref()
        {
            if let Some(delta) = params["delta"].as_str() {
                self.answer.push_str(delta);
            }
        }
        let Some(turn_id) = params["turn"]["id"].as_str() else {
            return;
        };
        match value["method"].as_str() {
            Some("turn/started") => {
                self.completions.start(turn_id);
                self.active_turn = Some(turn_id.to_owned());
                self.answer.clear();
            }
            Some("turn/completed") => {
                if self.active_turn.as_deref() != Some(turn_id) {
                    return;
                }
                if let Some(status) = params["turn"]["status"].as_str() {
                    self.active_turn = None;
                    self.completed_turn = Some((turn_id.to_owned(), status.to_owned()));
                    self.completions.finish(turn_id, status, &self.answer);
                }
            }
            _ => {}
        }
    }
}

/// Sole protocol connection used by the native broker for this owner generation.
/// The reader runs while idle as well as during broker- and TUI-originated turns.
pub struct CodexSharedClient {
    agent_id: String,
    generation: u64,
    writer: Mutex<SocketWriter>,
    pending: Arc<StdMutex<HashMap<String, PendingReply>>>,
    observation: watch::Sender<Observation>,
    reader: Mutex<Option<tokio::task::JoinHandle<()>>>,
    proxy: Option<proxy::OwnedProxy>,
}

impl std::fmt::Debug for CodexSharedClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CodexSharedClient")
            .field("agent_id", &self.agent_id)
            .field("generation", &self.generation)
            .finish_non_exhaustive()
    }
}

impl CodexSharedClient {
    pub(crate) fn observations(&self) -> watch::Receiver<Observation> {
        self.observation.subscribe()
    }

    /// Connect only to the caller's already-created IPv4 loopback endpoint.
    /// A token is supplied by that owner; it is never read from a global daemon.
    pub async fn connect(
        agent_id: String,
        generation: u64,
        endpoint: &str,
        token: &str,
    ) -> Result<Arc<Self>, CodexSharedError> {
        validate_endpoint(endpoint)?;
        if token.is_empty() {
            return Err(CodexSharedError::unsupported(
                "owned endpoint token is missing",
            ));
        }
        let mut request = endpoint
            .into_client_request()
            .map_err(|_| CodexSharedError::unsupported("invalid owned endpoint"))?;
        request.headers_mut().insert(
            "Authorization",
            format!("Bearer {token}")
                .parse()
                .map_err(|_| CodexSharedError::unsupported("invalid endpoint credential"))?,
        );
        let (socket, _) = tokio::time::timeout(
            Duration::from_secs(10),
            tokio_tungstenite::connect_async(request),
        )
        .await
        .map_err(|_| CodexSharedError::connection_pending("owned endpoint connection timed out"))?
        .map_err(|error| match error {
            tokio_tungstenite::tungstenite::Error::Io(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::ConnectionRefused
                        | std::io::ErrorKind::TimedOut
                        | std::io::ErrorKind::ConnectionReset
                        | std::io::ErrorKind::ConnectionAborted
                        | std::io::ErrorKind::Interrupted
                ) =>
            {
                CodexSharedError::connection_pending("owned endpoint is not ready")
            }
            _ => CodexSharedError::unsupported("owned endpoint rejected the connection"),
        })?;
        Ok(Self::from_connected(agent_id, generation, socket, None))
    }

    /// Reuse identical framing, observations and pending-reply semantics for all
    /// connected transports. No protocol request is sent by this constructor.
    fn from_connected<S>(
        agent_id: String,
        generation: u64,
        socket: WebSocketStream<S>,
        proxy: Option<proxy::OwnedProxy>,
    ) -> Arc<Self>
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        let (writer, mut reader) = socket.split();
        let pending: Arc<StdMutex<HashMap<String, PendingReply>>> =
            Arc::new(StdMutex::new(HashMap::new()));
        let (observation, _) = watch::channel(Observation::default());
        let replies = pending.clone();
        let observations = observation.clone();
        let proxy_stop = proxy.as_ref().map(proxy::OwnedProxy::stop_signal);
        let task = tokio::spawn(async move {
            while let Some(Ok(message)) = reader.next().await {
                match message {
                    Message::Text(text) => {
                        let Ok(value) = serde_json::from_str::<Value>(&text) else {
                            break;
                        };
                        if let Some(id) = value["id"].as_str() {
                            // Server requests (approvals) are deliberately not answered.
                            if value.get("method").is_none() {
                                if let Some(reply) = replies.lock().unwrap().remove(id) {
                                    // An admitted RPC can precede turn/started. It
                                    // establishes a completion slot, never activity.
                                    if let Some(id) = value["result"]["turn"]["id"].as_str() {
                                        observations
                                            .send_modify(|state| state.completions.start(id));
                                    }
                                    let _ = reply.send(Ok(value));
                                }
                            }
                        } else {
                            observations.send_modify(|current| current.observe(&value));
                        }
                    }
                    Message::Close(_) => break,
                    _ => {}
                }
            }
            observations.send_modify(Observation::close);
            for (_, reply) in replies.lock().unwrap().drain() {
                let _ = reply.send(Err(CodexSharedError::uncertain(
                    "provider connection ended",
                )));
            }
            if let Some(stop) = proxy_stop {
                stop.send_replace(true);
            }
        });
        Arc::new(Self {
            agent_id,
            generation,
            writer: Mutex::new(Box::new(writer)),
            pending,
            observation,
            reader: Mutex::new(Some(task)),
            proxy,
        })
    }

    /// Initialize one connection and verify the installed version and exact owned home.
    pub async fn initialize(
        &self,
        expected_home: &std::path::Path,
    ) -> Result<String, CodexSharedError> {
        self.initialize_with_timeout(expected_home, Some(STARTUP_TIMEOUT))
            .await
    }

    /// Cold native indexing has no progress protocol or fixed-duration bound.
    /// The owner must keep this wait cancellable and reap its child on exit.
    pub(super) async fn initialize_queued(
        &self,
        expected_home: &std::path::Path,
    ) -> Result<String, CodexSharedError> {
        self.initialize_with_timeout(expected_home, None).await
    }

    async fn initialize_with_timeout(
        &self,
        expected_home: &std::path::Path,
        timeout: Option<Duration>,
    ) -> Result<String, CodexSharedError> {
        let result = self
            .request_with_optional_timeout(
                "initialize",
                json!({
                    "clientInfo": {"name": "wardian", "version": env!("CARGO_PKG_VERSION")},
                    "capabilities": {"experimentalApi": true}
                }),
                timeout,
            )
            .await?;
        let user_agent = result["userAgent"].as_str().unwrap_or_default();
        let version = version::supported_version(user_agent)?;
        let actual = result["codexHome"]
            .as_str()
            .and_then(|path| std::fs::canonicalize(path).ok());
        let expected = std::fs::canonicalize(expected_home).ok();
        if actual.is_none() || actual != expected {
            return Err(CodexSharedError::unsupported(
                "app-server home does not match owner habitat",
            ));
        }
        self.writer
            .lock()
            .await
            .send(Message::Text(
                json!({"method":"initialized"}).to_string().into(),
            ))
            .await
            .map_err(|_| CodexSharedError::uncertain("initialization notification write failed"))?;
        self.observation
            .send_modify(|state| state.provider_version = Some(version.clone()));
        Ok(version)
    }

    /// Exercise the actual no-turn protocol with legitimate integration context
    /// before exposing a ready owner or attaching the TUI. An eligible version
    /// alone is insufficient. No model turn or fabricated tool call is submitted.
    pub(super) async fn install_initial_context(
        &self,
        context: &str,
    ) -> Result<(), CodexSharedError> {
        let receipt = self.receipt("initializing")?;
        let result = self.request_with_timeout("thread/inject_items", json!({
            "threadId":receipt.provider_session_id,"items":[{
                "type":"message","role":"developer",
                "content":[{"type":"input_text","text":context}]
            }]
        }), STARTUP_TIMEOUT).await.map_err(|error| CodexSharedError {
            code: "unsupported".into(),
            message: format!("Codex {} failed required no-turn thread/inject_items capability validation: {}", receipt.provider_version.as_deref().unwrap_or("unknown"), error.message),
            provider_boundary_crossed: error.provider_boundary_crossed,
        })?;
        if !result.is_object() {
            return Err(CodexSharedError::unsupported(
                "Codex thread/inject_items returned an incompatible acknowledgement",
            ));
        }
        Ok(())
    }

    /// Set binding exclusively from a successful thread/start or thread/resume response.
    pub fn bind(&self, result: &Value) -> Result<String, CodexSharedError> {
        let thread = &result["thread"];
        let id = thread["id"]
            .as_str()
            .filter(|id| !id.is_empty())
            .ok_or_else(|| CodexSharedError::unsupported("native thread identity is missing"))?;
        if thread["canAcceptDirectInput"] != true {
            return Err(CodexSharedError::unsupported(
                "native thread did not confirm direct-input capability",
            ));
        }
        let existing = self.observation.borrow().thread_id.clone();
        if existing.as_deref().is_some_and(|existing| existing != id) {
            return Err(CodexSharedError::unsupported(
                "native thread identity changed",
            ));
        }
        self.observation.send_modify(|state| {
            state.thread_id = Some(id.to_owned());
            state.active_turn = thread["turns"].as_array().and_then(|turns| {
                turns
                    .iter()
                    .rev()
                    .find(|turn| turn["status"] == "inProgress")
                    .and_then(|turn| turn["id"].as_str())
                    .map(str::to_owned)
            });
            if let Some(id) = &state.active_turn {
                state.completions.start(id);
            }
        });
        Ok(id.to_owned())
    }

    /// Raw protocol acceptance only. Callers must persist admission before invoking it.
    pub async fn request(&self, method: &str, params: Value) -> Result<Value, CodexSharedError> {
        self.request_with_timeout(method, params, Duration::from_secs(30))
            .await
    }

    pub(super) async fn request_with_timeout(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, CodexSharedError> {
        self.request_with_optional_timeout(method, params, Some(timeout))
            .await
    }

    async fn request_with_optional_timeout(
        &self,
        method: &str,
        params: Value,
        timeout: Option<Duration>,
    ) -> Result<Value, CodexSharedError> {
        if self.observation.borrow().closed {
            return Err(CodexSharedError::unsupported(
                "provider connection is closed",
            ));
        }
        let id = format!("wardian:{}", uuid::Uuid::new_v4());
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(id.clone(), tx);
        let payload = json!({"id":id,"method":method,"params":params});
        let deadline = tokio::time::Instant::now() + timeout.unwrap_or(STARTUP_TIMEOUT);
        if !matches!(
            tokio::time::timeout_at(deadline, async {
                self.writer
                    .lock()
                    .await
                    .send(Message::Text(payload.to_string().into()))
                    .await
            })
            .await,
            Ok(Ok(()))
        ) {
            self.pending.lock().unwrap().remove(&id);
            return Err(CodexSharedError::uncertain(
                "provider request write failed; not replayed",
            ));
        }
        let result = if timeout.is_some() {
            tokio::time::timeout_at(deadline, rx).await.map_err(|_| {
                CodexSharedError::uncertain("provider acknowledgement timed out; not replayed")
            })
        } else {
            Ok(rx.await)
        };
        self.pending.lock().unwrap().remove(&id);
        let value =
            result?.map_err(|_| CodexSharedError::uncertain("provider reply channel ended"))??;
        if let Some(error) = value.get("error") {
            return Err(CodexSharedError {
                code: "provider_rejected".into(),
                message: diagnostics::rejection_message(method, error).into(),
                provider_boundary_crossed: true,
            });
        }
        value
            .get("result")
            .cloned()
            .ok_or_else(|| CodexSharedError::uncertain("provider reply has no result"))
    }

    pub(crate) fn receipt(&self, state: &str) -> Result<CodexSharedReceipt, CodexSharedError> {
        let observation = self.observation.borrow();
        if observation.closed {
            return Err(CodexSharedError::unsupported("owner connection closed"));
        }
        Ok(CodexSharedReceipt {
            wardian_agent_id: self.agent_id.clone(),
            generation: self.generation,
            provider_version: observation.provider_version.clone(),
            provider_session_id: observation
                .thread_id
                .clone()
                .ok_or_else(|| CodexSharedError::unsupported("owner thread unbound"))?,
            provider_turn_id: observation.active_turn.clone(),
            delivery_state: state.into(),
            interruption_confirmed: false,
            message_id: None,
        })
    }

    /// Append genuine host-delivered peer context with no fabricated model tool call.
    pub async fn push(&self, context: Value) -> Result<CodexSharedReceipt, CodexSharedError> {
        let mut receipt = self.receipt("provider_accepted")?;
        self.request(
            "thread/inject_items",
            json!({"threadId":receipt.provider_session_id,
            "items":[{"type":"function_call_output", "call_id":null,
                "name":"wardian_inbox_delivery", "namespace":"wardian",
            "output":context.to_string()}]}),
        )
        .await?;
        // inject_items acknowledges a thread append, not a particular active turn.
        receipt.provider_turn_id = None;
        Ok(receipt)
    }

    /// Submit ordinary native input without presenting it as a canonical peer task.
    /// Broker records retain the full envelope; the provider gets the literal body
    /// and its diagnostic client message identity, not an MCP reply request ID.
    pub(super) async fn native_message(
        &self,
        message_id: &str,
        body: &str,
    ) -> Result<CodexSharedReceipt, CodexSharedError> {
        let mut receipt = self.receipt("provider_accepted")?;
        let result = self
            .request(
                "turn/start",
                json!({"threadId":receipt.provider_session_id,
                    "clientUserMessageId":message_id,
                    "input":[{"type":"text","text":body}]}),
            )
            .await?;
        receipt.provider_turn_id = result["turn"]["id"].as_str().map(str::to_owned);
        receipt.message_id = Some(message_id.to_owned());
        Ok(receipt)
    }

    /// Admit a canonical peer task using active-turn admission or a new idle turn.
    pub async fn followup(
        &self,
        message_id: &str,
        context: &str,
    ) -> Result<CodexSharedReceipt, CodexSharedError> {
        let mut receipt = self.receipt("provider_accepted")?;
        let result = self
            .request(
                "turn/start",
                json!({"threadId":receipt.provider_session_id,
            "input":[], "toolOutput":{"name":"wardian_task_delivery","namespace":"wardian",
                "output":context}}),
            )
            .await?;
        receipt.provider_turn_id = result["turn"]["id"].as_str().map(str::to_owned);
        receipt.message_id = Some(message_id.to_owned());
        Ok(receipt)
    }

    /// Observe the exact admitted turn, including a completion received before its RPC reply.
    pub async fn wait_for_turn(
        &self,
        turn_id: &str,
        timeout: Duration,
    ) -> Result<(String, String), CodexSharedError> {
        let mut completed = self.observation.borrow().completions.subscribe(turn_id)?;
        tokio::time::timeout(timeout, async {
            loop {
                if let Some(result) = completed.borrow_and_update().clone() {
                    return result;
                }
                completed
                    .changed()
                    .await
                    .map_err(|_| CodexSharedError::uncertain("completion observation ended"))?;
            }
        })
        .await
        .map_err(|_| CodexSharedError::uncertain("turn completion timed out; not replayed"))?
    }

    /// Never substitutes Escape or process termination for an exact native interrupt.
    pub async fn interrupt(&self) -> Result<CodexSharedReceipt, CodexSharedError> {
        self.interrupt_expected(None).await
    }

    pub(crate) async fn interrupt_expected(
        &self,
        expected_turn: Option<&str>,
    ) -> Result<CodexSharedReceipt, CodexSharedError> {
        let mut receipt = self.receipt("no_active_turn")?;
        if expected_turn
            .is_some_and(|expected| receipt.provider_turn_id.as_deref() != Some(expected))
        {
            return Err(CodexSharedError::unsupported(
                "requested turn is no longer the observed active turn",
            ));
        }
        let Some(turn_id) = receipt.provider_turn_id.clone() else {
            return Ok(receipt);
        };
        self.request(
            "turn/interrupt",
            json!({"threadId":receipt.provider_session_id,"turnId":turn_id}),
        )
        .await?;
        receipt.delivery_state = "interrupt_requested".into();
        let confirmation = self
            .wait_for_turn(&turn_id, Duration::from_secs(10))
            .await
            .is_ok_and(|(status, _)| status == "interrupted");
        if confirmation {
            receipt.delivery_state = "interrupted".into();
            receipt.interruption_confirmed = true;
        }
        Ok(receipt)
    }

    /// Close this connection and join its proxy, if any. The containing native
    /// owner must separately await the app-server child (a different process).
    pub async fn close(&self) {
        self.observation.send_modify(Observation::stop);
        // Signal before any await: a cancelled close cannot leave the proxy alive
        // behind a blocked writer. Loopback retains its graceful close behavior.
        if let Some(proxy) = &self.proxy {
            proxy.stop();
        }
        let _ = tokio::time::timeout(Duration::from_secs(1), async {
            self.writer.lock().await.close().await
        })
        .await;
        let mut reader_slot = self.reader.lock().await;
        if let Some(reader) = reader_slot.as_mut() {
            reader.abort();
            let _ = reader.await;
        }
        reader_slot.take();
        drop(reader_slot);
        for (_, reply) in self.pending.lock().unwrap().drain() {
            let _ = reply.send(Err(CodexSharedError::uncertain("owner closed")));
        }
        if let Some(proxy) = &self.proxy {
            proxy.close().await;
        }
    }
}

impl Drop for CodexSharedClient {
    fn drop(&mut self) {
        if let Some(proxy) = &self.proxy {
            proxy.stop();
        }
        if let Some(reader) = self.reader.get_mut().take() {
            reader.abort();
        }
    }
}

fn validate_endpoint(endpoint: &str) -> Result<(), CodexSharedError> {
    let Some(address) = endpoint.strip_prefix("ws://") else {
        return Err(CodexSharedError::unsupported(
            "owner endpoint must be loopback ws",
        ));
    };
    let parsed = address.parse::<std::net::SocketAddr>().ok();
    if parsed.is_none_or(|address| {
        address.ip() != std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST) || address.port() == 0
    }) {
        return Err(CodexSharedError::unsupported(
            "owner endpoint must be exact IPv4 loopback and nonzero port",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intentional_owner_shutdown_does_not_mask_an_unexpected_disconnect() {
        let mut normal = Observation::default();
        normal.stop();
        normal.close(); // the socket reader can observe EOF after close starts
        assert_eq!(normal.activity(), CodexTurnActivity::Stopped);

        let mut unexpected = Observation::default();
        unexpected.close();
        unexpected.stop(); // cleanup must preserve the original failure
        assert_eq!(unexpected.activity(), CodexTurnActivity::Closed);
    }

    #[tokio::test]
    async fn eligible_newer_version_still_requires_actual_no_turn_capability_ack() {
        tokio::time::timeout(Duration::from_secs(5), async {
            for response in [json!({"result":{}}), json!({"result":null}), json!({"error":{"code":-32601,"message":"method not found"}})] {
                let expected_success = response["result"].is_object();
                let test_home = tempfile::tempdir().unwrap();
                let owned_home = test_home.path().canonicalize().unwrap();
                let reported_home = owned_home.clone();
                let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
                let endpoint = format!("ws://{}", listener.local_addr().unwrap());
                let server = tokio::spawn(async move {
                    let (stream, _) = listener.accept().await.unwrap();
                    let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
                    let init: Value = serde_json::from_str(socket.next().await.unwrap().unwrap().to_text().unwrap()).unwrap();
                    assert_eq!(init["method"], "initialize");
                    socket.send(Message::Text(json!({"id":init["id"],"result":{"userAgent":"wardian/0.154.0 (simulated server)","codexHome":reported_home}}).to_string().into())).await.unwrap();
                    let initialized: Value = serde_json::from_str(socket.next().await.unwrap().unwrap().to_text().unwrap()).unwrap();
                    assert_eq!(initialized["method"], "initialized");
                    let inject: Value = serde_json::from_str(socket.next().await.unwrap().unwrap().to_text().unwrap()).unwrap();
                    assert_eq!(inject["method"], "thread/inject_items");
                    assert_eq!(inject["params"]["threadId"], "owned");
                    assert_eq!(inject["params"]["items"], json!([{"type":"message","role":"developer","content":[{"type":"input_text","text":"legitimate managed integration context"}]}]));
                    let mut reply = response;
                    reply["id"] = inject["id"].clone();
                    socket.send(Message::Text(reply.to_string().into())).await.unwrap();
                    // Neither rejection nor success may trigger a model turn,
                    // alternative composer delivery, or uncertain replay.
                    assert!(matches!(socket.next().await, Some(Ok(Message::Close(_))) | None));
                });
                let client = CodexSharedClient::connect("wardian-id".into(), 7, &endpoint, "test-owned-token").await.unwrap();
                assert_eq!(client.initialize(&owned_home).await.unwrap(), "0.154.0");
                assert!(client.bind(&json!({"thread":{"id":"owned","turns":[]}})).is_err());
                client.bind(&json!({"thread":{"id":"owned","canAcceptDirectInput":true,"turns":[]}})).unwrap();
                assert_eq!(client.receipt("initializing").unwrap().provider_version.as_deref(), Some("0.154.0"));
                let result = client.install_initial_context("legitimate managed integration context").await;
                assert_eq!(result.is_ok(), expected_success);
                if let Err(error) = result { assert_eq!(error.code, "unsupported"); }
                assert_eq!(client.observation.borrow().activity(), CodexTurnActivity::Pending);
                client.close().await;
                server.await.unwrap();
            }
        }).await.unwrap();
    }

    #[tokio::test]
    async fn ordinary_native_input_preserves_literal_body_identity_and_uncertain_boundary() {
        tokio::time::timeout(Duration::from_secs(5), async {
            for close_before_ack in [false, true] {
                let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
                let endpoint = format!("ws://{}", listener.local_addr().unwrap());
                let body = "  Human task: wait for the peer reply.\nLiteral λ中, \\\"quoted\\\", {request_id: not-a-frame}\n";
                let server = tokio::spawn(async move {
                    let (stream, _) = listener.accept().await.unwrap();
                    let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
                    let request: Value = serde_json::from_str(socket.next().await.unwrap().unwrap().to_text().unwrap()).unwrap();
                    assert_eq!(request["method"], "turn/start");
                    assert_eq!(request["params"], json!({
                        "threadId":"owned",
                        "clientUserMessageId":"int_initial",
                        "input":[{"type":"text","text":body}]
                    }));
                    assert!(request["params"].get("toolOutput").is_none());
                    if close_before_ack {
                        socket.close(None).await.unwrap();
                        // A written request without acknowledgement stays uncertain;
                        // the client must not reconnect and replay ordinary input.
                        assert!(tokio::time::timeout(Duration::from_millis(50), listener.accept()).await.is_err());
                    } else {
                        for value in [
                            json!({"method":"turn/started","params":{"threadId":"owned","turn":{"id":"input-turn"}}}),
                            json!({"method":"turn/completed","params":{"threadId":"owned","turn":{"id":"input-turn","status":"completed"}}}),
                            json!({"id":request["id"],"result":{"turn":{"id":"input-turn"}}}),
                        ] {
                            socket.send(Message::Text(value.to_string().into())).await.unwrap();
                        }
                        // Completion does not synthesize a followup or peer reply.
                        assert!(matches!(socket.next().await, Some(Ok(Message::Close(_))) | None));
                    }
                });
                let client = CodexSharedClient::connect("wardian-id".into(), 7, &endpoint, "test-owned-token").await.unwrap();
                client.bind(&json!({"thread":{"id":"owned","canAcceptDirectInput":true,"turns":[]}})).unwrap();
                let result = client.native_message("int_initial", body).await;
                if close_before_ack {
                    let error = result.unwrap_err();
                    assert_eq!(error.code, "submitted_unconfirmed");
                    assert!(error.provider_boundary_crossed);
                } else {
                    let receipt = result.unwrap();
                    assert_eq!(receipt.message_id.as_deref(), Some("int_initial"));
                    assert_eq!(receipt.wardian_agent_id, "wardian-id");
                    assert_eq!(receipt.generation, 7);
                    assert_eq!(receipt.provider_session_id, "owned");
                    assert_eq!(receipt.provider_turn_id.as_deref(), Some("input-turn"));
                    assert_eq!(client.wait_for_turn("input-turn", Duration::from_secs(1)).await.unwrap().0, "completed");
                }
                client.close().await;
                server.await.unwrap();
            }
        }).await.unwrap();
    }

    #[tokio::test]
    async fn admitted_rpc_before_started_reserves_completion_without_active_evidence() {
        tokio::time::timeout(Duration::from_secs(5), async {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let endpoint = format!("ws://{}", listener.local_addr().unwrap());
            let (advance_tx, advance_rx) = oneshot::channel();
            let frame = json!({"schema_version":1,"kind":"task","interaction_id":"task-id","body":"  literal\nUnicode λ\t  ","request_id":"task-id"}).to_string();
            let expected_frame = frame.clone();
            let server = tokio::spawn(async move {
                let (stream, _) = listener.accept().await.unwrap();
                let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
                let request: Value = serde_json::from_str(socket.next().await.unwrap().unwrap().to_text().unwrap()).unwrap();
                assert_eq!(request["method"], "turn/start");
                assert_eq!(request["params"]["input"], json!([]));
                assert_eq!(request["params"]["toolOutput"], json!({"name":"wardian_task_delivery","namespace":"wardian","output":expected_frame}));
                socket.send(Message::Text(json!({"id":request["id"],"result":{"turn":{"id":"late","status":"inProgress"}}}).to_string().into())).await.unwrap();
                advance_rx.await.unwrap();
                for value in [
                    json!({"method":"turn/started","params":{"threadId":"owned","turn":{"id":"late"}}}),
                    json!({"method":"item/agentMessage/delta","params":{"threadId":"owned","turnId":"late","delta":"exact answer"}}),
                    json!({"method":"turn/completed","params":{"threadId":"owned","turn":{"id":"late","status":"completed"}}}),
                ] { socket.send(Message::Text(value.to_string().into())).await.unwrap(); }
                let _ = socket.next().await;
            });
            let client = CodexSharedClient::connect("wardian-id".into(), 7, &endpoint, "test-owned-token").await.unwrap();
            client.bind(&json!({"thread":{"id":"owned","canAcceptDirectInput":true,"turns":[]}})).unwrap();
            let receipt = client.followup("task-id", &frame).await.unwrap();
            assert_eq!(receipt.provider_turn_id.as_deref(), Some("late"));
            assert_eq!(client.observation.borrow().activity(), CodexTurnActivity::Pending);
            assert!(client.receipt("observed").unwrap().provider_turn_id.is_none());
            let completed = client.observation.borrow().completions.subscribe("late").unwrap();
            assert!(completed.borrow().is_none());
            advance_tx.send(()).unwrap();
            assert_eq!(client.wait_for_turn("late", Duration::from_secs(1)).await.unwrap(), ("completed".into(), "exact answer".into()));
            client.close().await;
            server.await.unwrap();
        }).await.unwrap();
    }

    #[tokio::test]
    async fn idle_push_and_tui_turns_share_continuous_exact_turn_observation() {
        // An owned local protocol peer exercises the actual client pump, not a
        // provider/model. The retained fixture separately proves native shape.
        tokio::time::timeout(Duration::from_secs(5), async {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let endpoint = format!("ws://{}", listener.local_addr().unwrap());
            let (advance_tx, advance_rx) = oneshot::channel();
            let server = tokio::spawn(async move {
                let (stream, _) = listener.accept().await.unwrap();
                let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
                let push: Value = serde_json::from_str(socket.next().await.unwrap().unwrap().to_text().unwrap()).unwrap();
                assert_eq!(push["method"], "thread/inject_items");
                assert_eq!(push["params"]["threadId"], "owned");
                let item = &push["params"]["items"][0];
                assert_eq!(item["call_id"], Value::Null);
                assert_eq!(item["name"], "wardian_inbox_delivery");
                assert_eq!(item["namespace"], "wardian");
                assert_eq!(item["type"], "function_call_output");
                assert!(item["output"].is_string());
                // These turn events originated in the attached TUI, with no
                // broker turn/start request. Queue both before the client wakes.
                for value in [
                    json!({"method":"turn/started","params":{"threadId":"owned","turn":{"id":"previous"}}}),
                    json!({"method":"turn/completed","params":{"threadId":"owned","turn":{"id":"previous","status":"completed"}}}),
                    json!({"id":push["id"],"result":{}}),
                ] { socket.send(Message::Text(value.to_string().into())).await.unwrap(); }
                advance_rx.await.unwrap();
                socket.send(Message::Text(json!({"method":"turn/started","params":{"threadId":"owned","turn":{"id":"tui-active"}}}).to_string().into())).await.unwrap();
                let interrupt: Value = serde_json::from_str(socket.next().await.unwrap().unwrap().to_text().unwrap()).unwrap();
                assert_eq!(interrupt["method"], "turn/interrupt");
                assert_eq!(interrupt["params"], json!({"threadId":"owned","turnId":"tui-active"}));
                for value in [
                    json!({"id":interrupt["id"],"result":{}}),
                    json!({"method":"turn/completed","params":{"threadId":"owned","turn":{"id":"tui-active","status":"interrupted"}}}),
                    json!({"method":"turn/started","params":{"threadId":"owned","turn":{"id":"next"}}}),
                    json!({"method":"turn/completed","params":{"threadId":"owned","turn":{"id":"next","status":"completed"}}}),
                ] { socket.send(Message::Text(value.to_string().into())).await.unwrap(); }
                let _ = socket.next().await;
            });
            let client = CodexSharedClient::connect("wardian-id".into(), 7, &endpoint, "test-owned-token").await.unwrap();
            client.bind(&json!({"thread":{"id":"owned","canAcceptDirectInput":true,"turns":[]}})).unwrap();
            let mut observations = client.observations();
            let receipt = client.push(json!({"message_id":"info","context":"literal information"})).await.unwrap();
            assert!(receipt.provider_turn_id.is_none());
            assert_eq!(observations.borrow().activity(), CodexTurnActivity::Idle("previous".into()));
            advance_tx.send(()).unwrap();
            loop {
                if observations.borrow_and_update().activity() == CodexTurnActivity::Processing("tui-active".into()) { break; }
                observations.changed().await.unwrap();
            }
            let receipt = client.interrupt().await.unwrap();
            assert_eq!(receipt.provider_turn_id.as_deref(), Some("tui-active"));
            assert!(receipt.interruption_confirmed);
            assert_eq!(client.wait_for_turn("previous", Duration::from_secs(1)).await.unwrap().0, "completed");
            client.close().await;
            server.await.unwrap();
        }).await.unwrap();
    }

    #[test]
    fn owner_endpoint_rejects_daemons_and_foreign_addresses() {
        assert!(validate_endpoint("ws://127.0.0.1:12345").is_ok());
        for endpoint in [
            "ws://localhost:12345",
            "ws://0.0.0.0:12345",
            "ws://127.0.0.1:0",
            "unix://",
            "ws://127.0.0.1:12345/path",
            "wss://example.com:443",
        ] {
            assert!(validate_endpoint(endpoint).is_err(), "{endpoint}");
        }
    }

    #[test]
    fn only_bound_native_turn_controls_interrupt_evidence() {
        let mut state = Observation {
            thread_id: Some("owned".into()),
            ..Default::default()
        };
        state.observe(
            &json!({"method":"turn/started","params":{"threadId":"foreign","turn":{"id":"x"}}}),
        );
        assert!(state.active_turn.is_none());
        state.observe(
            &json!({"method":"turn/started","params":{"threadId":"owned","turn":{"id":"current"}}}),
        );
        state.observe(&json!({"method":"turn/completed","params":{"threadId":"owned","turn":{"id":"old","status":"interrupted"}}}));
        assert_eq!(state.active_turn.as_deref(), Some("current"));
        state.observe(&json!({"method":"turn/completed","params":{"threadId":"owned","turn":{"id":"current","status":"interrupted"}}}));
        assert!(state.active_turn.is_none());
        assert_eq!(
            state.completed_turn,
            Some(("current".into(), "interrupted".into()))
        );
    }
}
