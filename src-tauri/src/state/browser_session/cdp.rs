//! A minimal Chrome DevTools Protocol client.
//!
//! Only what browser surfaces need: request/response correlation, flattened
//! target sessions, and an event stream. The connection owns one websocket and
//! two background tasks; every caller talks to it through [`CdpConnection`].

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::{broadcast, mpsc, oneshot, Mutex};
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::Message;

/// Ceiling on how long any single protocol call may take.
pub const CDP_CALL_TIMEOUT: Duration = Duration::from_secs(30);
/// Buffered protocol events.
///
/// Sized to absorb a screencast burst *and* a page load's network events
/// without dropping a navigation. Lagging here is not a dropped frame: the
/// session pump cannot know whether a `Page.frameNavigated` was among the
/// discarded messages, so it has to invalidate every outstanding snapshot ref.
/// A page load emits several hundred `Network.*` events, which is why this is
/// well above what frames alone would need.
const EVENT_CHANNEL_CAPACITY: usize = 2048;

/// Synthetic event published when the websocket closes.
///
/// A subscriber cannot detect closure by the channel ending: the sender lives
/// in the connection, which subscribers hold alive. Without an explicit signal
/// a crashed browser would leave every reader waiting forever.
pub const DISCONNECTED_METHOD: &str = "Wardian.disconnected";

/// A protocol event addressed to a specific target session, or to the browser.
#[derive(Debug, Clone)]
pub struct CdpEvent {
    pub session_id: Option<String>,
    pub method: String,
    pub params: Value,
}

/// Why a protocol call failed.
#[derive(Debug, Clone)]
pub enum CdpError {
    /// The websocket closed or was never established.
    Disconnected,
    /// The call exceeded [`CDP_CALL_TIMEOUT`].
    Timeout { method: String },
    /// The browser answered with an error object.
    Protocol {
        method: String,
        code: i64,
        message: String,
    },
    /// The browser answered with something this client cannot read.
    Malformed { method: String, detail: String },
}

impl std::fmt::Display for CdpError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CdpError::Disconnected => write!(formatter, "the browser connection is closed"),
            CdpError::Timeout { method } => {
                write!(
                    formatter,
                    "{method} did not answer within {} seconds",
                    CDP_CALL_TIMEOUT.as_secs()
                )
            }
            CdpError::Protocol {
                method,
                code,
                message,
            } => write!(formatter, "{method} failed ({code}): {message}"),
            CdpError::Malformed { method, detail } => {
                write!(
                    formatter,
                    "{method} returned an unreadable response: {detail}"
                )
            }
        }
    }
}

impl std::error::Error for CdpError {}

#[derive(Debug, Deserialize)]
struct ProtocolErrorBody {
    #[serde(default)]
    code: i64,
    #[serde(default)]
    message: String,
}

/// Splits one inbound protocol frame into either a command reply or an event.
///
/// Kept free of I/O so the routing rules can be tested directly.
pub(crate) enum InboundFrame {
    Reply {
        id: u64,
        result: Result<Value, (i64, String)>,
    },
    Event(CdpEvent),
    Ignored,
}

pub(crate) fn classify_frame(text: &str) -> InboundFrame {
    let Ok(value) = serde_json::from_str::<Value>(text) else {
        return InboundFrame::Ignored;
    };
    if let Some(id) = value.get("id").and_then(Value::as_u64) {
        if let Some(error) = value.get("error") {
            let body: ProtocolErrorBody =
                serde_json::from_value(error.clone()).unwrap_or(ProtocolErrorBody {
                    code: 0,
                    message: error.to_string(),
                });
            return InboundFrame::Reply {
                id,
                result: Err((body.code, body.message)),
            };
        }
        return InboundFrame::Reply {
            id,
            result: Ok(value.get("result").cloned().unwrap_or_else(|| json!({}))),
        };
    }
    match value.get("method").and_then(Value::as_str) {
        Some(method) => InboundFrame::Event(CdpEvent {
            session_id: value
                .get("sessionId")
                .and_then(Value::as_str)
                .map(str::to_string),
            method: method.to_string(),
            params: value.get("params").cloned().unwrap_or_else(|| json!({})),
        }),
        None => InboundFrame::Ignored,
    }
}

#[derive(Debug)]
struct PendingCall {
    session_id: Option<String>,
    attachment_target: Option<String>,
    reply: oneshot::Sender<Result<Value, (i64, String)>>,
}

#[derive(Debug, Default)]
struct PendingCalls {
    requests: HashMap<u64, PendingCall>,
    /// Recorded from successful attach replies before waking their callers,
    /// including targets that are still being equipped by the actor.
    target_sessions: HashMap<String, String>,
    retired_targets: HashSet<String>,
    retired_sessions: HashSet<String>,
}

type PendingMap = Arc<Mutex<PendingCalls>>;

/// An open DevTools Protocol connection to one browser process.
#[derive(Debug)]
pub struct CdpConnection {
    next_id: AtomicU64,
    /// Set when the socket closes, so later calls fail immediately instead of
    /// each waiting out the full call timeout against a dead browser.
    closed: AtomicBool,
    outbound: mpsc::UnboundedSender<Message>,
    pending: PendingMap,
    events: broadcast::Sender<CdpEvent>,
}

impl CdpConnection {
    /// Connects to a browser's websocket endpoint and starts pumping frames.
    pub async fn connect(websocket_url: &str) -> Result<Arc<Self>, CdpError> {
        let (stream, _response) = tokio_tungstenite::connect_async(websocket_url)
            .await
            .map_err(|_| CdpError::Disconnected)?;
        let (mut sink, mut source) = stream.split();
        let (outbound, mut outbound_rx) = mpsc::unbounded_channel::<Message>();
        let (events, _) = broadcast::channel(EVENT_CHANNEL_CAPACITY);
        let pending: PendingMap = Arc::new(Mutex::new(PendingCalls::default()));

        let connection = Arc::new(Self {
            next_id: AtomicU64::new(1),
            closed: AtomicBool::new(false),
            outbound,
            pending: Arc::clone(&pending),
            events: events.clone(),
        });
        // The reader owns a handle so it can mark the connection closed; the
        // task ends when the socket does, so this cycle always resolves.
        let connection_closed = Arc::clone(&connection);

        tokio::spawn(async move {
            while let Some(message) = outbound_rx.recv().await {
                if sink.send(message).await.is_err() {
                    break;
                }
            }
            let _ = sink.close().await;
        });

        tokio::spawn(async move {
            while let Some(Ok(message)) = source.next().await {
                let text = match message {
                    Message::Text(text) => text.to_string(),
                    Message::Binary(bytes) => match String::from_utf8(bytes.to_vec()) {
                        Ok(text) => text,
                        Err(_) => continue,
                    },
                    Message::Close(_) => break,
                    _ => continue,
                };
                match classify_frame(&text) {
                    InboundFrame::Reply { id, result } => {
                        let mut pending = pending.lock().await;
                        if let Some(call) = pending.requests.remove(&id) {
                            if let (Some(target), Ok(body)) = (&call.attachment_target, &result) {
                                if let Some(session_id) =
                                    body.get("sessionId").and_then(Value::as_str)
                                {
                                    pending
                                        .target_sessions
                                        .insert(target.clone(), session_id.to_string());
                                }
                            }
                            let _ = call.reply.send(result);
                        }
                    }
                    InboundFrame::Event(event) => {
                        let _ = events.send(event);
                    }
                    InboundFrame::Ignored => {}
                }
            }
            // Fail every in-flight call rather than leaving callers to time out
            // one by one after the socket is already gone.
            connection_closed.closed.store(true, Ordering::Release);
            for (_, call) in pending.lock().await.requests.drain() {
                let _ = call.reply.send(Err((-1, "connection closed".to_string())));
            }
            let _ = events.send(CdpEvent {
                session_id: None,
                method: DISCONNECTED_METHOD.to_string(),
                params: json!({}),
            });
        });

        Ok(connection)
    }

    /// Subscribes to every protocol event on this connection.
    pub fn subscribe(&self) -> broadcast::Receiver<CdpEvent> {
        self.events.subscribe()
    }

    /// Issues a browser-scoped command.
    pub async fn call(&self, method: &str, params: Value) -> Result<Value, CdpError> {
        self.dispatch(method, params, None).await
    }

    /// Issues a command scoped to an attached target session.
    pub async fn call_session(
        &self,
        session_id: &str,
        method: &str,
        params: Value,
    ) -> Result<Value, CdpError> {
        self.dispatch(method, params, Some(session_id)).await
    }

    /// True once the websocket has closed.
    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }

    /// Fails calls belonging to one destroyed target without closing its browser.
    ///
    /// Retirement and dispatch registration share a lock: a call either enters
    /// before retirement and is failed here, or sees the retired identity and
    /// fails before enqueueing. Attach requests carry their target identity so
    /// destruction also covers the interval before a flattened session exists.
    /// Tombstones prevent later work on those identities; late replies have no
    /// pending recipient. Other target and browser-scoped calls stay live.
    pub(super) async fn retire_target(&self, target_id: &str) {
        let mut pending = self.pending.lock().await;
        pending.retired_targets.insert(target_id.to_string());
        let session_id = pending.target_sessions.remove(target_id);
        if let Some(session_id) = &session_id {
            pending.retired_sessions.insert(session_id.clone());
        }
        let ids: Vec<_> = pending
            .requests
            .iter()
            .filter_map(|(id, call)| {
                (call.attachment_target.as_deref() == Some(target_id)
                    || session_id
                        .as_deref()
                        .is_some_and(|session_id| call.session_id.as_deref() == Some(session_id)))
                .then_some(*id)
            })
            .collect();
        for id in ids {
            if let Some(call) = pending.requests.remove(&id) {
                let _ = call
                    .reply
                    .send(Err((-32000, "target session is closed".to_string())));
            }
        }
    }

    /// Whether destruction has already retired this target, including an
    /// attach that completed before the actor could publish its stack entry.
    pub(super) async fn target_is_retired(&self, target_id: &str) -> bool {
        self.pending
            .lock()
            .await
            .retired_targets
            .contains(target_id)
    }

    async fn dispatch(
        &self,
        method: &str,
        params: Value,
        session_id: Option<&str>,
    ) -> Result<Value, CdpError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let attachment_target = (method == "Target.attachToTarget")
            .then(|| params.get("targetId").and_then(Value::as_str))
            .flatten()
            .map(str::to_string);
        let mut envelope = json!({ "id": id, "method": method, "params": params });
        if let Some(session_id) = session_id {
            envelope["sessionId"] = json!(session_id);
        }
        let (sender, receiver) = oneshot::channel();
        {
            // Registered under the same lock the reader drains under, and
            // re-checked there. The reader sets `closed` before taking this
            // lock, so a disconnect either drains this sender or is visible
            // here — an insert can never land after the drain and then wait
            // out the full call timeout with nobody left to answer it.
            let mut pending = self.pending.lock().await;
            if self.is_closed() {
                return Err(CdpError::Disconnected);
            }
            if session_id.is_some_and(|session_id| pending.retired_sessions.contains(session_id))
                || attachment_target
                    .as_ref()
                    .is_some_and(|target_id| pending.retired_targets.contains(target_id))
            {
                return Err(CdpError::Protocol {
                    method: method.to_string(),
                    code: -32000,
                    message: "target session is closed".to_string(),
                });
            }
            pending.requests.insert(
                id,
                PendingCall {
                    session_id: session_id.map(str::to_string),
                    attachment_target,
                    reply: sender,
                },
            );
            // Enqueueing is synchronous, so retirement cannot fall between
            // registration and enqueueing and leave new work on a dead target.
            if self
                .outbound
                .send(Message::Text(envelope.to_string().into()))
                .is_err()
            {
                pending.requests.remove(&id);
                return Err(CdpError::Disconnected);
            }
        }

        match timeout(CDP_CALL_TIMEOUT, receiver).await {
            Ok(Ok(Ok(result))) => Ok(result),
            Ok(Ok(Err((code, message)))) => Err(CdpError::Protocol {
                method: method.to_string(),
                code,
                message,
            }),
            Ok(Err(_)) => Err(CdpError::Disconnected),
            Err(_) => {
                self.pending.lock().await.requests.remove(&id);
                Err(CdpError::Timeout {
                    method: method.to_string(),
                })
            }
        }
    }
}

/// Reads a required string field out of a protocol result.
pub fn required_str(method: &str, value: &Value, field: &str) -> Result<String, CdpError> {
    value
        .get(field)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| CdpError::Malformed {
            method: method.to_string(),
            detail: format!("missing {field}"),
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;
    use tokio_tungstenite::accept_async;

    #[tokio::test]
    async fn retirement_preserves_live_calls_and_rejects_new_dead_session_calls() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let endpoint = format!("ws://{}", listener.local_addr().expect("address"));
        let (ready_tx, ready_rx) = oneshot::channel();
        let (release_tx, release_rx) = oneshot::channel();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accept");
            let mut socket = accept_async(stream).await.expect("websocket");
            let Some(Ok(Message::Text(text))) = socket.next().await else {
                panic!("attach request");
            };
            let attach: Value = serde_json::from_str(text.as_ref()).expect("attach JSON");
            assert_eq!(attach["params"]["targetId"], "popup-target");
            socket
                .send(Message::Text(
                    json!({ "id": attach["id"], "result": { "sessionId": "popup-session" } })
                        .to_string()
                        .into(),
                ))
                .await
                .expect("attach reply");
            let mut requests = Vec::new();
            for _ in 0..4 {
                let Some(Ok(Message::Text(text))) = socket.next().await else {
                    panic!("held request");
                };
                requests.push(serde_json::from_str::<Value>(text.as_ref()).expect("request JSON"));
            }
            ready_tx.send(()).expect("ready receipt");
            release_rx.await.expect("release replies");
            // Popup replies arrive after retirement; they cannot resurrect callers.
            requests.sort_by_key(|request| request["sessionId"] != "popup-session");
            for request in requests {
                socket
                    .send(Message::Text(
                        json!({ "id": request["id"], "result": { "method": request["method"] } })
                            .to_string()
                            .into(),
                    ))
                    .await
                    .expect("late/live reply");
            }
            let Some(Ok(Message::Text(text))) = socket.next().await else {
                panic!("fresh live request");
            };
            let fresh: Value = serde_json::from_str(text.as_ref()).expect("fresh JSON");
            assert_eq!(fresh["sessionId"], "base-session");
            assert_eq!(fresh["method"], "Fake.fresh");
            socket
                .send(Message::Text(
                    json!({ "id": fresh["id"], "result": { "live": true } })
                        .to_string()
                        .into(),
                ))
                .await
                .expect("fresh reply");
            socket.close(None).await.expect("close");
        });
        let connection = CdpConnection::connect(&endpoint).await.expect("connect");
        connection
            .call(
                "Target.attachToTarget",
                json!({ "targetId": "popup-target", "flatten": true }),
            )
            .await
            .expect("attach");
        let mut popup_calls = Vec::new();
        for method in ["Fake.popupOne", "Fake.popupTwo"] {
            let connection = Arc::clone(&connection);
            popup_calls.push(tokio::spawn(async move {
                connection
                    .call_session("popup-session", method, json!({}))
                    .await
            }));
        }
        let base_connection = Arc::clone(&connection);
        let base = tokio::spawn(async move {
            base_connection
                .call_session("base-session", "Fake.base", json!({}))
                .await
        });
        let browser_connection = Arc::clone(&connection);
        let browser =
            tokio::spawn(async move { browser_connection.call("Fake.browser", json!({})).await });
        timeout(Duration::from_secs(2), ready_rx)
            .await
            .expect("wire receipt")
            .expect("ready");
        connection.retire_target("foreign-target").await;
        assert!(popup_calls.iter().all(|call| !call.is_finished()));
        assert!(!base.is_finished());
        assert!(!browser.is_finished());
        connection.retire_target("popup-target").await;
        connection.retire_target("popup-target").await;
        for call in popup_calls {
            let error = timeout(Duration::from_secs(2), call)
                .await
                .expect("retired caller joins")
                .expect("caller task")
                .expect_err("retired call");
            assert!(matches!(error, CdpError::Protocol { code: -32000, .. }));
            assert!(error.to_string().contains("target session is closed"));
        }
        assert!(!base.is_finished());
        assert!(!browser.is_finished());
        assert!(matches!(
            connection
                .call_session("popup-session", "Fake.mustNotReachWire", json!({}))
                .await,
            Err(CdpError::Protocol { code: -32000, .. })
        ));
        assert!(matches!(
            connection
                .call(
                    "Target.attachToTarget",
                    json!({ "targetId": "popup-target" })
                )
                .await,
            Err(CdpError::Protocol { code: -32000, .. })
        ));
        release_tx.send(()).expect("release");
        assert_eq!(
            timeout(Duration::from_secs(2), base)
                .await
                .expect("base joins")
                .expect("task")
                .expect("base reply")["method"],
            "Fake.base"
        );
        assert_eq!(
            timeout(Duration::from_secs(2), browser)
                .await
                .expect("browser joins")
                .expect("task")
                .expect("browser reply")["method"],
            "Fake.browser"
        );
        let fresh = timeout(
            Duration::from_secs(2),
            connection.call_session("base-session", "Fake.fresh", json!({})),
        )
        .await
        .expect("fresh response")
        .expect("fresh live call");
        assert_eq!(fresh["live"], true);
        timeout(Duration::from_secs(2), server)
            .await
            .expect("server joins")
            .expect("server task");
        assert!(connection.pending.lock().await.requests.is_empty());
    }

    #[tokio::test]
    async fn retirement_before_attach_reply_prevents_late_attachment_registration() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let endpoint = format!("ws://{}", listener.local_addr().expect("address"));
        let (ready_tx, ready_rx) = oneshot::channel();
        let (release_tx, release_rx) = oneshot::channel();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accept");
            let mut socket = accept_async(stream).await.expect("websocket");
            let Some(Ok(Message::Text(text))) = socket.next().await else {
                panic!("attach request")
            };
            let attach: Value = serde_json::from_str(text.as_ref()).expect("JSON");
            ready_tx.send(()).expect("wire receipt");
            release_rx.await.expect("release");
            socket
                .send(Message::Text(
                    json!({ "id": attach["id"], "result": { "sessionId": "late-session" } })
                        .to_string()
                        .into(),
                ))
                .await
                .expect("late reply");
            let Some(Ok(Message::Text(text))) = socket.next().await else {
                panic!("live browser request")
            };
            let live: Value = serde_json::from_str(text.as_ref()).expect("JSON");
            assert_eq!(live["method"], "Fake.browserStillLive");
            socket
                .send(Message::Text(
                    json!({ "id": live["id"], "result": { "live": true } })
                        .to_string()
                        .into(),
                ))
                .await
                .expect("live reply");
            socket.close(None).await.expect("close");
        });
        let connection = CdpConnection::connect(&endpoint).await.expect("connect");
        let attaching_connection = Arc::clone(&connection);
        let attaching = tokio::spawn(async move {
            attaching_connection
                .call(
                    "Target.attachToTarget",
                    json!({ "targetId": "closing-target" }),
                )
                .await
        });
        timeout(Duration::from_secs(2), ready_rx)
            .await
            .expect("wire receipt")
            .expect("ready");
        connection.retire_target("closing-target").await;
        assert!(matches!(
            timeout(Duration::from_secs(2), attaching)
                .await
                .expect("attach joins")
                .expect("caller task"),
            Err(CdpError::Protocol { code: -32000, .. })
        ));
        assert!(matches!(
            connection
                .call(
                    "Target.attachToTarget",
                    json!({ "targetId": "closing-target" })
                )
                .await,
            Err(CdpError::Protocol { code: -32000, .. })
        ));
        // Retirement wins before registration too, without putting an attach on the wire.
        connection.retire_target("already-gone-target").await;
        assert!(matches!(
            connection
                .call(
                    "Target.attachToTarget",
                    json!({ "targetId": "already-gone-target" })
                )
                .await,
            Err(CdpError::Protocol { code: -32000, .. })
        ));
        release_tx.send(()).expect("release");
        let live = timeout(
            Duration::from_secs(2),
            connection.call("Fake.browserStillLive", json!({})),
        )
        .await
        .expect("live joins")
        .expect("live reply");
        assert_eq!(live["live"], true);
        timeout(Duration::from_secs(2), server)
            .await
            .expect("server joins")
            .expect("server task");
        let pending = connection.pending.lock().await;
        assert!(!pending.target_sessions.contains_key("closing-target"));
        assert!(pending.requests.is_empty());
    }

    #[test]
    fn classifies_a_successful_reply() {
        match classify_frame(r#"{"id":7,"result":{"targetId":"t1"}}"#) {
            InboundFrame::Reply { id, result } => {
                assert_eq!(id, 7);
                assert_eq!(result.expect("ok")["targetId"], "t1");
            }
            _ => panic!("expected a reply"),
        }
    }

    #[test]
    fn classifies_a_reply_with_no_result_body() {
        match classify_frame(r#"{"id":8}"#) {
            InboundFrame::Reply { id, result } => {
                assert_eq!(id, 8);
                assert_eq!(result.expect("ok"), json!({}));
            }
            _ => panic!("expected a reply"),
        }
    }

    #[test]
    fn classifies_a_protocol_error() {
        match classify_frame(r#"{"id":9,"error":{"code":-32000,"message":"nope"}}"#) {
            InboundFrame::Reply { id, result } => {
                assert_eq!(id, 9);
                let (code, message) = result.expect_err("error");
                assert_eq!(code, -32000);
                assert_eq!(message, "nope");
            }
            _ => panic!("expected a reply"),
        }
    }

    #[test]
    fn classifies_a_session_scoped_event() {
        match classify_frame(
            r#"{"method":"Page.frameNavigated","sessionId":"s1","params":{"frame":{}}}"#,
        ) {
            InboundFrame::Event(event) => {
                assert_eq!(event.method, "Page.frameNavigated");
                assert_eq!(event.session_id.as_deref(), Some("s1"));
            }
            _ => panic!("expected an event"),
        }
    }

    #[test]
    fn classifies_a_browser_scoped_event_without_a_session() {
        match classify_frame(r#"{"method":"Target.targetCreated","params":{}}"#) {
            InboundFrame::Event(event) => {
                assert_eq!(event.session_id, None);
                assert_eq!(event.params, json!({}));
            }
            _ => panic!("expected an event"),
        }
    }

    #[test]
    fn ignores_frames_that_are_neither_replies_nor_events() {
        assert!(matches!(classify_frame("not json"), InboundFrame::Ignored));
        assert!(matches!(
            classify_frame(r#"{"hello":1}"#),
            InboundFrame::Ignored
        ));
    }

    #[test]
    fn required_str_reports_the_missing_field_by_name() {
        let error =
            required_str("Target.createTarget", &json!({}), "targetId").expect_err("missing field");
        assert!(error.to_string().contains("missing targetId"));
    }

    #[test]
    fn a_protocol_error_names_the_failing_method() {
        let error = CdpError::Protocol {
            method: "Page.navigate".to_string(),
            code: -32000,
            message: "Cannot navigate to invalid URL".to_string(),
        };
        assert_eq!(
            error.to_string(),
            "Page.navigate failed (-32000): Cannot navigate to invalid URL"
        );
    }
}
