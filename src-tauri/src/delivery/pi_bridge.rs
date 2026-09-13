//! Generation-bound task delivery into an already-running Pi TUI.
//!
//! The bridge is deliberately narrower than the provider RPC transport.  Rust
//! owns the loopback listener and launch credential, while the bundled Pi
//! extension owns the supported in-process `sendMessage` call.  A successful
//! result means that Pi emitted the matching custom message into its loop; it
//! does not synthesize a provider turn or a canonical reply.

use std::collections::HashSet;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use rand::RngCore;
use serde::de::{DeserializeSeed, Error as DeError, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, oneshot, Notify};

use wardian_core::native_transport::{
    NativeMessageEnvelope, NativeMessageOperation, NativeSessionBinding,
    NativeTransportCapabilities,
};

pub const ENV_CONFIG: &str = "WARDIAN_PI_BRIDGE_CONFIG";
const MAX_CONFIG_BYTES: usize = 8192;
const MAX_FRAME_BYTES: usize = 65_536;
const MAX_BODY_BYTES: usize = 32_768;
const MAX_SESSION_FILE_BYTES: usize = 4096;
const MAX_GENERATION: u64 = 9_007_199_254_740_991;
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);
const DELIVERY_TIMEOUT: Duration = Duration::from_secs(35);

// These are embedded so installed bundles do not depend on a developer
// checkout.  materialize_extension publishes the exact three runtime modules
// into the generation-owned session directory before Pi starts.
const PI_INDEX: &str = include_str!("../../resources/pi-messaging/index.mjs");
const PI_BRIDGE: &str = include_str!("../../resources/pi-messaging/bridge.mjs");
const PI_PROTOCOL: &str = include_str!("../../resources/pi-messaging/protocol.mjs");

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PiBridgeBinding {
    pub target_agent_id: String,
    pub generation: u64,
    pub session_id: String,
    pub session_file: String,
}

#[allow(clippy::large_enum_variant)]
#[derive(Debug)]
enum BridgeCommand {
    Deliver {
        envelope: NativeMessageEnvelope,
        reply: oneshot::Sender<Result<PiBridgeReceipt, PiBridgeError>>,
    },
    Shutdown,
}

/// Result of the Pi bridge's provider-visible boundary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PiBridgeReceipt {
    pub binding: NativeSessionBinding,
    pub delivery_state: String,
}

#[derive(Clone, Debug)]
pub struct PiBridgeError {
    pub code: &'static str,
    pub message: String,
    pub provider_boundary_crossed: bool,
}

impl fmt::Display for PiBridgeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.message)
    }
}

impl std::error::Error for PiBridgeError {}

impl PiBridgeError {
    fn closed(message: impl Into<String>) -> Self {
        Self {
            code: "bridge_closed",
            message: message.into(),
            provider_boundary_crossed: false,
        }
    }

    fn unavailable(message: impl Into<String>) -> Self {
        Self {
            code: "bridge_unavailable",
            message: message.into(),
            provider_boundary_crossed: false,
        }
    }

    fn uncertain(message: impl Into<String>) -> Self {
        Self {
            code: "submitted_unconfirmed",
            message: message.into(),
            provider_boundary_crossed: true,
        }
    }

    fn rejected(message: impl Into<String>) -> Self {
        Self {
            code: "provider_rejected",
            message: message.into(),
            provider_boundary_crossed: false,
        }
    }
}

/// The launch plan owns the private credential until the manager has spawned
/// the exact Pi child.  It is intentionally not serializable or loggable.
pub struct PiBridgeLaunchPlan {
    extension_path: PathBuf,
    config: String,
    owner: Arc<PiBridgeOwner>,
    attached: bool,
}

impl fmt::Debug for PiBridgeLaunchPlan {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PiBridgeLaunchPlan")
            .field("extension_path", &self.extension_path)
            .field("target_agent_id", &self.owner.binding.target_agent_id)
            .field("generation", &self.owner.binding.generation)
            .field("session_id", &self.owner.binding.session_id)
            .finish_non_exhaustive()
    }
}

impl PiBridgeLaunchPlan {
    pub fn materialize_extension(session_file: &Path, generation: u64) -> Result<PathBuf, String> {
        if !session_file.is_absolute() || generation == 0 || generation > MAX_GENERATION {
            return Err("Pi bridge publication requires an absolute valid generation".to_string());
        }
        let root = session_file
            .parent()
            .ok_or_else(|| "Pi bridge session file has no parent directory".to_string())?
            .join(".wardian-pi-bridge")
            .join(generation.to_string());
        std::fs::create_dir_all(&root).map_err(|error| {
            format!("Pi bridge generation directory could not be created: {error}")
        })?;
        for (name, contents) in [
            ("index.mjs", PI_INDEX),
            ("bridge.mjs", PI_BRIDGE),
            ("protocol.mjs", PI_PROTOCOL),
        ] {
            let path = root.join(name);
            match std::fs::read_to_string(&path) {
                Ok(existing) if existing == contents => {}
                Ok(_) => {
                    return Err(format!(
                        "Pi bridge generation file was changed: {}",
                        path.display()
                    ))
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    std::fs::write(&path, contents).map_err(|write_error| {
                        format!("Pi bridge module could not be published: {write_error}")
                    })?;
                }
                Err(error) => return Err(format!("Pi bridge module could not be read: {error}")),
            }
        }
        Ok(root.join("index.mjs"))
    }

    pub async fn prepare(
        target_agent_id: String,
        generation: u64,
        session_id: String,
        session_file: PathBuf,
        extension_path: PathBuf,
    ) -> Result<Self, String> {
        validate_id(&target_agent_id)
            .map_err(|error| format!("invalid Pi bridge target: {error}"))?;
        validate_id(&session_id).map_err(|error| format!("invalid Pi bridge session: {error}"))?;
        if generation == 0 || generation > MAX_GENERATION {
            return Err("Pi bridge generation is outside the JavaScript-safe range".to_string());
        }
        if !session_file.is_absolute() {
            return Err("Pi bridge session file must be absolute".to_string());
        }
        if session_file.as_os_str().to_string_lossy().len() > MAX_SESSION_FILE_BYTES
            || session_file
                .as_os_str()
                .to_string_lossy()
                .chars()
                .any(|character| character.is_control())
        {
            return Err("Pi bridge session file is outside the bounded path contract".to_string());
        }
        if !extension_path.is_absolute() || !extension_path.is_file() {
            return Err("Pi bridge extension is not an owned absolute file".to_string());
        }

        let listener = TcpListener::bind(("127.0.0.1", 0))
            .await
            .map_err(|error| format!("Pi bridge listener could not bind: {error}"))?;
        let port = listener
            .local_addr()
            .map_err(|error| format!("Pi bridge listener address unavailable: {error}"))?
            .port();
        let token = random_token();
        let session_file_text = session_file.to_string_lossy().into_owned();
        let binding = PiBridgeBinding {
            target_agent_id,
            generation,
            session_id,
            session_file: session_file_text.clone(),
        };
        let config = serde_json::json!({
            "version": 1,
            "host": "127.0.0.1",
            "port": port,
            "token": token,
            "target_id": binding.target_agent_id,
            "generation": binding.generation,
            "session_id": binding.session_id,
            "session_file": binding.session_file,
        });
        let config = serde_json::to_string(&config)
            .map_err(|error| format!("Pi bridge config could not be encoded: {error}"))?;
        if config.len() > MAX_CONFIG_BYTES {
            return Err("Pi bridge config exceeds its bounded size".to_string());
        }

        let (command_tx, command_rx) = mpsc::channel(8);
        let owner = Arc::new(PiBridgeOwner {
            binding,
            token,
            command_tx,
            close_notify: Notify::new(),
            process_id: AtomicU32::new(0),
            closed: AtomicBool::new(false),
            ready: AtomicBool::new(false),
        });
        let listener_owner = Arc::clone(&owner);
        tokio::spawn(async move {
            run_listener(listener, listener_owner, command_rx).await;
        });
        Ok(Self {
            extension_path,
            config,
            owner,
            attached: false,
        })
    }

    pub fn extension_path(&self) -> &Path {
        &self.extension_path
    }

    pub fn config(&self) -> &str {
        &self.config
    }

    pub fn owner(&self) -> Arc<PiBridgeOwner> {
        Arc::clone(&self.owner)
    }

    /// Transfer cleanup responsibility to the spawned child/generation owner.
    pub fn attached(&mut self) {
        self.attached = true;
    }

    pub fn register_process(&self, process_id: u32) {
        self.owner.register_process(process_id);
    }
}

impl Drop for PiBridgeLaunchPlan {
    fn drop(&mut self) {
        if !self.attached {
            self.owner.close();
        }
    }
}

pub struct PiBridgeOwner {
    binding: PiBridgeBinding,
    token: String,
    command_tx: mpsc::Sender<BridgeCommand>,
    close_notify: Notify,
    process_id: AtomicU32,
    closed: AtomicBool,
    ready: AtomicBool,
}

impl fmt::Debug for PiBridgeOwner {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PiBridgeOwner")
            .field("target_agent_id", &self.binding.target_agent_id)
            .field("generation", &self.binding.generation)
            .field("session_id", &self.binding.session_id)
            .field("process_id", &self.process_id.load(Ordering::Acquire))
            .field("ready", &self.ready.load(Ordering::Acquire))
            .finish()
    }
}

impl PiBridgeOwner {
    pub fn binding(&self) -> &PiBridgeBinding {
        &self.binding
    }

    pub fn is_ready(&self) -> bool {
        !self.closed.load(Ordering::Acquire) && self.ready.load(Ordering::Acquire)
    }

    pub fn register_process(&self, process_id: u32) {
        if process_id != 0 {
            self.process_id.store(process_id, Ordering::Release);
        }
    }

    pub fn close(&self) {
        self.closed.store(true, Ordering::Release);
        self.ready.store(false, Ordering::Release);
        self.close_notify.notify_waiters();
        let _ = self.command_tx.try_send(BridgeCommand::Shutdown);
    }

    pub async fn deliver(
        &self,
        envelope: NativeMessageEnvelope,
    ) -> Result<PiBridgeReceipt, PiBridgeError> {
        if !self.is_ready() {
            return Err(PiBridgeError::unavailable(
                "Pi TUI bridge has not completed its authenticated ready handshake",
            ));
        }
        if envelope.operation != NativeMessageOperation::StartTurn {
            return Err(PiBridgeError::rejected(
                "Pi TUI bridge supports canonical task start only",
            ));
        }
        if envelope
            .sender_agent_id
            .as_deref()
            .is_none_or(|sender| validate_id(sender).is_err())
        {
            return Err(PiBridgeError::rejected(
                "Pi TUI bridge requires a canonical sender attribution",
            ));
        }
        if envelope.body.is_empty() || envelope.body.len() > MAX_BODY_BYTES {
            return Err(PiBridgeError::rejected(
                "Pi TUI bridge task body is outside the bounded size",
            ));
        }
        let (reply_tx, reply_rx) = oneshot::channel();
        self.command_tx
            .send(BridgeCommand::Deliver {
                envelope,
                reply: reply_tx,
            })
            .await
            .map_err(|_| PiBridgeError::closed("Pi TUI bridge command owner stopped"))?;
        match tokio::time::timeout(DELIVERY_TIMEOUT, reply_rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(PiBridgeError::uncertain(
                "Pi TUI bridge ended without a delivery result",
            )),
            Err(_) => Err(PiBridgeError::uncertain(
                "Pi TUI bridge delivery deadline expired without a correlated observation",
            )),
        }
    }

    pub(crate) fn session_binding(&self) -> NativeSessionBinding {
        NativeSessionBinding {
            target_agent_id: self.binding.target_agent_id.clone(),
            generation: self.binding.generation,
            provider: "pi".to_string(),
            transport: "pi_tui_bridge".to_string(),
            provider_session_id: Some(self.binding.session_id.clone()),
            capabilities: NativeTransportCapabilities {
                provider: "pi".to_string(),
                transport: "pi_tui_bridge".to_string(),
                protocol_version: "1".to_string(),
                persistent_session: true,
                positive_turn_start: true,
                late_reconciliation: true,
                cancellation: false,
                invalidate_premise: false,
                approval_requests: false,
                max_payload_bytes: Some(MAX_BODY_BYTES as u64),
                execution_timeout_ms: Some(DELIVERY_TIMEOUT.as_millis() as u64),
            },
            observed_at: chrono::Utc::now().to_rfc3339(),
        }
    }
}

pub(crate) fn is_valid_identifier(value: &str) -> bool {
    validate_id(value).is_ok()
}

async fn run_listener(
    listener: TcpListener,
    owner: Arc<PiBridgeOwner>,
    mut commands: mpsc::Receiver<BridgeCommand>,
) {
    if owner.closed.load(Ordering::Acquire) {
        return;
    }
    let accepted = tokio::select! {
        _ = owner.close_notify.notified() => return,
        accepted = tokio::time::timeout(HANDSHAKE_TIMEOUT, listener.accept()) => accepted,
    };
    let Ok(Ok((mut stream, _peer))) = accepted else {
        owner.close();
        return;
    };
    if owner.closed.load(Ordering::Acquire) {
        return;
    }
    let authenticated = tokio::select! {
        _ = owner.close_notify.notified() => return,
        authenticated = authenticate(&owner, &mut stream) => authenticated,
    };
    let Ok(mut session) = authenticated else {
        owner.close();
        return;
    };
    owner.ready.store(true, Ordering::Release);

    while owner.is_ready() {
        if let Some(message_id) = session.awaiting_settled.clone() {
            if wait_for_settled(&owner, &mut stream, &mut session, &message_id)
                .await
                .is_err()
            {
                owner.close();
                break;
            }
        }
        let command = tokio::select! {
            command = commands.recv() => command,
            _ = owner.close_notify.notified() => break,
            closed = peer_closed(&stream) => {
                if closed {
                    owner.close();
                    break;
                }
                continue;
            }
        };
        let Some(command) = command else { break };
        match command {
            BridgeCommand::Shutdown => break,
            BridgeCommand::Deliver { envelope, reply } => {
                let result = deliver_one(&owner, &mut stream, &mut session, &envelope).await;
                let should_close = matches!(
                    result.as_ref(),
                    Err(error) if error.code != "provider_rejected"
                );
                let reply_dropped = reply.send(result).is_err();
                if should_close || reply_dropped {
                    owner.close();
                    break;
                }
            }
        }
    }
    owner.ready.store(false, Ordering::Release);
}

async fn authenticate(
    owner: &PiBridgeOwner,
    stream: &mut TcpStream,
) -> Result<BridgeSession, PiBridgeError> {
    let hello = tokio::time::timeout(HANDSHAKE_TIMEOUT, read_frame(stream))
        .await
        .map_err(|_| PiBridgeError::unavailable("Pi TUI bridge hello timed out"))?
        .map_err(|error| {
            PiBridgeError::unavailable(format!("Pi TUI bridge hello failed: {error}"))
        })?;
    exact_keys(
        &hello,
        &[
            "version",
            "target_id",
            "generation",
            "session_id",
            "runtime_nonce",
            "seq",
            "type",
            "token",
            "pid",
            "session_file",
        ],
    )
    .map_err(PiBridgeError::unavailable)?;
    if hello["version"] != 1
        || hello["target_id"] != owner.binding.target_agent_id
        || hello["generation"].as_u64() != Some(owner.binding.generation)
        || hello["session_id"] != owner.binding.session_id
        || hello["seq"].as_u64() != Some(1)
        || hello["type"] != "hello"
        || hello["session_file"] != owner.binding.session_file
    {
        return Err(PiBridgeError::unavailable(
            "Pi TUI bridge hello binding was rejected",
        ));
    }
    let token = hello["token"].as_str().unwrap_or_default();
    if token.len() != 64
        || !token
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        || token.as_bytes().ct_eq(owner.token.as_bytes()).unwrap_u8() != 1
    {
        return Err(PiBridgeError::unavailable(
            "Pi TUI bridge credential was rejected",
        ));
    }
    let claimed_pid = hello["pid"]
        .as_u64()
        .and_then(|pid| u32::try_from(pid).ok());
    let expected_pid = wait_for_process_id(owner).await;
    if claimed_pid.is_none() || expected_pid == 0 || claimed_pid != Some(expected_pid) {
        return Err(PiBridgeError::unavailable(
            "Pi TUI bridge child identity was rejected",
        ));
    }
    let runtime_nonce = hello["runtime_nonce"].as_str().unwrap_or_default();
    if runtime_nonce.is_empty() || runtime_nonce.len() > 128 || !runtime_nonce.is_ascii() {
        return Err(PiBridgeError::unavailable(
            "Pi TUI bridge runtime nonce was rejected",
        ));
    }
    let mut welcome = common_frame(owner, runtime_nonce, 1, "welcome");
    welcome.insert("token".to_string(), Value::String(owner.token.clone()));
    write_frame(stream, &welcome).await.map_err(|error| {
        PiBridgeError::unavailable(format!("Pi TUI bridge welcome failed: {error}"))
    })?;

    let ready = tokio::time::timeout(HANDSHAKE_TIMEOUT, read_frame(stream))
        .await
        .map_err(|_| PiBridgeError::unavailable("Pi TUI bridge ready timed out"))?
        .map_err(|error| {
            PiBridgeError::unavailable(format!("Pi TUI bridge ready failed: {error}"))
        })?;
    exact_keys(
        &ready,
        &[
            "version",
            "target_id",
            "generation",
            "session_id",
            "runtime_nonce",
            "seq",
            "type",
            "capabilities",
        ],
    )
    .map_err(PiBridgeError::unavailable)?;
    if !common_matches(owner, &ready, runtime_nonce, 2, "ready")
        || ready["capabilities"]
            != serde_json::json!({"task": true, "information": false, "cancel": false, "completion": false})
    {
        return Err(PiBridgeError::unavailable(
            "Pi TUI bridge capability handshake was rejected",
        ));
    }
    Ok(BridgeSession {
        runtime_nonce: runtime_nonce.to_string(),
        tx_sequence: 1,
        rx_sequence: 2,
        awaiting_settled: None,
    })
}

struct BridgeSession {
    runtime_nonce: String,
    tx_sequence: u64,
    rx_sequence: u64,
    awaiting_settled: Option<String>,
}

async fn deliver_one(
    owner: &PiBridgeOwner,
    stream: &mut TcpStream,
    session: &mut BridgeSession,
    envelope: &NativeMessageEnvelope,
) -> Result<PiBridgeReceipt, PiBridgeError> {
    let sender = envelope.sender_agent_id.as_deref().ok_or_else(|| {
        PiBridgeError::rejected("Pi TUI bridge requires a canonical sender attribution")
    })?;
    let body_sha256 = format!("{:x}", Sha256::digest(envelope.body.as_bytes()));
    let delivery = serde_json::json!({
        "message_id": envelope.message_id,
        "interaction_id": envelope.interaction_id,
        "sender_id": sender,
        "kind": "task",
        "body": envelope.body,
        "body_sha256": body_sha256,
    });
    if validate_id(&envelope.message_id).is_err()
        || validate_id(&envelope.interaction_id).is_err()
        || envelope.target_agent_id != owner.binding.target_agent_id
        || envelope.generation != owner.binding.generation
    {
        return Err(PiBridgeError::rejected(
            "Pi TUI bridge task binding was rejected",
        ));
    }
    let mut frame = common_frame(
        owner,
        &session.runtime_nonce,
        session.tx_sequence + 1,
        "deliver",
    );
    frame.insert("delivery".to_string(), delivery);
    write_frame(stream, &frame).await.map_err(|error| {
        PiBridgeError::uncertain(format!("Pi TUI bridge deliver write failed: {error}"))
    })?;
    session.tx_sequence += 1;
    let mut crossed = false;
    loop {
        let event = tokio::time::timeout(DELIVERY_TIMEOUT, read_frame(stream))
            .await
            .map_err(|_| PiBridgeError::uncertain("Pi TUI bridge observation timed out"))?
            .map_err(|error| {
                if crossed {
                    PiBridgeError::uncertain(format!("Pi TUI bridge observation failed: {error}"))
                } else {
                    PiBridgeError::unavailable(format!(
                        "Pi TUI bridge rejected before submission: {error}"
                    ))
                }
            })?;
        let event_type = event["type"].as_str().unwrap_or_default();
        let expected_keys = match event_type {
            "received" | "submitted_unconfirmed" | "observed_consumption" | "observed_settled" => {
                vec![
                    "version",
                    "target_id",
                    "generation",
                    "session_id",
                    "runtime_nonce",
                    "seq",
                    "type",
                    "message_id",
                ]
            }
            "rejected" => vec![
                "version",
                "target_id",
                "generation",
                "session_id",
                "runtime_nonce",
                "seq",
                "type",
                "message_id",
                "reason",
            ],
            _ => {
                return Err(PiBridgeError::uncertain(
                    "Pi TUI bridge sent an unknown event",
                ))
            }
        };
        exact_keys(&event, &expected_keys).map_err(|error| {
            PiBridgeError::uncertain(format!("Pi TUI bridge event shape failed: {error}"))
        })?;
        session.rx_sequence = session
            .rx_sequence
            .checked_add(1)
            .ok_or_else(|| PiBridgeError::uncertain("Pi TUI bridge sequence overflow"))?;
        if !common_matches(
            owner,
            &event,
            &session.runtime_nonce,
            session.rx_sequence,
            event_type,
        ) || event["message_id"] != envelope.message_id
        {
            return Err(PiBridgeError::uncertain(
                "Pi TUI bridge event binding failed",
            ));
        }
        match event_type {
            "received" => {}
            "submitted_unconfirmed" => crossed = true,
            "observed_consumption" => {
                session.awaiting_settled = Some(envelope.message_id.clone());
                return Ok(PiBridgeReceipt {
                    binding: owner.session_binding(),
                    delivery_state: "provider_visible".to_string(),
                });
            }
            "observed_settled" => {
                return Err(PiBridgeError::uncertain(
                    "Pi TUI bridge settled before consumption was observed",
                ));
            }
            "rejected" => {
                let reason = event["reason"].as_str().unwrap_or("rejected");
                return if crossed {
                    Err(PiBridgeError::uncertain(format!(
                        "Pi TUI bridge rejected after submission: {reason}"
                    )))
                } else {
                    Err(PiBridgeError::rejected(format!(
                        "Pi TUI bridge rejected task: {reason}"
                    )))
                };
            }
            _ => unreachable!(),
        }
    }
}

async fn wait_for_settled(
    owner: &PiBridgeOwner,
    stream: &mut TcpStream,
    session: &mut BridgeSession,
    message_id: &str,
) -> Result<(), PiBridgeError> {
    let read = tokio::time::timeout(DELIVERY_TIMEOUT, read_frame(stream));
    let event = tokio::select! {
        _ = owner.close_notify.notified() => {
            return Err(PiBridgeError::closed("Pi TUI bridge owner was disposed"));
        }
        event = read => event
            .map_err(|_| PiBridgeError::uncertain("Pi TUI bridge settlement deadline expired"))?
            .map_err(|error| PiBridgeError::uncertain(format!("Pi TUI bridge settlement read failed: {error}")))?,
    };
    exact_keys(
        &event,
        &[
            "version",
            "target_id",
            "generation",
            "session_id",
            "runtime_nonce",
            "seq",
            "type",
            "message_id",
        ],
    )
    .map_err(|error| {
        PiBridgeError::uncertain(format!("Pi TUI bridge settlement shape failed: {error}"))
    })?;
    session.rx_sequence = session
        .rx_sequence
        .checked_add(1)
        .ok_or_else(|| PiBridgeError::uncertain("Pi TUI bridge sequence overflow"))?;
    if !common_matches(
        owner,
        &event,
        &session.runtime_nonce,
        session.rx_sequence,
        "observed_settled",
    ) || event["message_id"] != message_id
    {
        return Err(PiBridgeError::uncertain(
            "Pi TUI bridge settlement binding failed",
        ));
    }
    session.awaiting_settled = None;
    Ok(())
}

async fn peer_closed(stream: &TcpStream) -> bool {
    if stream.readable().await.is_err() {
        return true;
    }
    let mut probe = [0_u8; 1];
    match stream.try_read(&mut probe) {
        Ok(_) => true,
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => false,
        Err(_) => true,
    }
}

async fn wait_for_process_id(owner: &PiBridgeOwner) -> u32 {
    for _ in 0..500 {
        let process_id = owner.process_id.load(Ordering::Acquire);
        if process_id != 0 {
            return process_id;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    0
}

fn common_frame(
    owner: &PiBridgeOwner,
    runtime_nonce: &str,
    sequence: u64,
    frame_type: &str,
) -> Map<String, Value> {
    Map::from_iter([
        ("version".to_string(), Value::from(1)),
        (
            "target_id".to_string(),
            Value::String(owner.binding.target_agent_id.clone()),
        ),
        (
            "generation".to_string(),
            Value::from(owner.binding.generation),
        ),
        (
            "session_id".to_string(),
            Value::String(owner.binding.session_id.clone()),
        ),
        (
            "runtime_nonce".to_string(),
            Value::String(runtime_nonce.to_string()),
        ),
        ("seq".to_string(), Value::from(sequence)),
        ("type".to_string(), Value::String(frame_type.to_string())),
    ])
}

fn common_matches(
    owner: &PiBridgeOwner,
    frame: &Value,
    runtime_nonce: &str,
    sequence: u64,
    frame_type: &str,
) -> bool {
    frame["version"] == 1
        && frame["target_id"] == owner.binding.target_agent_id
        && frame["generation"].as_u64() == Some(owner.binding.generation)
        && frame["session_id"] == owner.binding.session_id
        && frame["runtime_nonce"] == runtime_nonce
        && frame["seq"].as_u64() == Some(sequence)
        && frame["type"] == frame_type
}

async fn read_frame(stream: &mut TcpStream) -> Result<Value, String> {
    let mut prefix = [0_u8; 4];
    stream
        .read_exact(&mut prefix)
        .await
        .map_err(|error| error.to_string())?;
    let size = u32::from_be_bytes(prefix) as usize;
    if size == 0 || size > MAX_FRAME_BYTES {
        return Err("frame size is outside the bounded contract".to_string());
    }
    let mut body = vec![0_u8; size];
    stream
        .read_exact(&mut body)
        .await
        .map_err(|error| error.to_string())?;
    parse_strict_json(&body)
}

async fn write_frame(stream: &mut TcpStream, frame: &Map<String, Value>) -> Result<(), String> {
    let body = serde_json::to_vec(frame).map_err(|error| error.to_string())?;
    if body.is_empty() || body.len() > MAX_FRAME_BYTES {
        return Err("frame size is outside the bounded contract".to_string());
    }
    let size = u32::try_from(body.len()).map_err(|_| "frame size overflow".to_string())?;
    stream
        .write_all(&size.to_be_bytes())
        .await
        .map_err(|error| error.to_string())?;
    stream
        .write_all(&body)
        .await
        .map_err(|error| error.to_string())?;
    stream.flush().await.map_err(|error| error.to_string())
}

fn exact_keys(value: &Value, keys: &[&str]) -> Result<(), String> {
    let Value::Object(map) = value else {
        return Err("frame is not an object".to_string());
    };
    if map.len() != keys.len() || keys.iter().any(|key| !map.contains_key(*key)) {
        return Err("frame has an unexpected shape".to_string());
    }
    Ok(())
}

fn validate_id(value: &str) -> Result<(), &'static str> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        Err("identifier is not ASCII bounded")
    } else {
        Ok(())
    }
}

fn random_token() -> String {
    let mut bytes = [0_u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

struct StrictJsonSeed;

impl<'de> DeserializeSeed<'de> for StrictJsonSeed {
    type Value = Value;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_any(StrictJsonVisitor)
    }
}

struct StrictJsonVisitor;

impl<'de> Visitor<'de> for StrictJsonVisitor {
    type Value = Value;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("an unambiguous JSON value")
    }

    fn visit_bool<E>(self, value: bool) -> Result<Self::Value, E> {
        Ok(Value::Bool(value))
    }

    fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E>
    where
        E: DeError,
    {
        Ok(Value::Number(value.into()))
    }

    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E>
    where
        E: DeError,
    {
        Ok(Value::Number(value.into()))
    }

    fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E>
    where
        E: DeError,
    {
        serde_json::Number::from_f64(value)
            .map(Value::Number)
            .ok_or_else(|| E::custom("non-finite JSON number"))
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E> {
        Ok(Value::String(value.to_string()))
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E> {
        Ok(Value::String(value))
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E> {
        Ok(Value::Null)
    }

    fn visit_none<E>(self) -> Result<Self::Value, E> {
        Ok(Value::Null)
    }

    fn visit_some<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        StrictJsonSeed.deserialize(deserializer)
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut values = Vec::new();
        while let Some(value) = sequence.next_element_seed(StrictJsonSeed)? {
            values.push(value);
        }
        Ok(Value::Array(values))
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut keys = HashSet::new();
        let mut values = Map::new();
        while let Some(key) = map.next_key::<String>()? {
            if !keys.insert(key.clone()) {
                return Err(A::Error::custom("duplicate JSON object key"));
            }
            values.insert(key, map.next_value_seed(StrictJsonSeed)?);
        }
        Ok(Value::Object(values))
    }
}

fn parse_strict_json(bytes: &[u8]) -> Result<Value, String> {
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    let value = StrictJsonSeed
        .deserialize(&mut deserializer)
        .map_err(|error| error.to_string())?;
    deserializer.end().map_err(|error| error.to_string())?;
    Ok(value)
}
