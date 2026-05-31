use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};

pub const REMOTE_SETTINGS_SCHEMA_VERSION: u8 = 1;
pub const REMOTE_AUDIT_SCHEMA_VERSION: u8 = 1;
pub const REMOTE_DEVICE_STORE_SCHEMA_VERSION: u8 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RemoteAccessStatus {
    Disabled,
    Enabled,
    NeedsRepair,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RemoteGatewayConfig {
    pub schema_version: u8,
    pub enabled: bool,
    pub canonical_origin: String,
    pub loopback_host: String,
    pub loopback_port: u16,
    pub gateway_identity_public_key: String,
    pub gateway_identity_fingerprint: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RemoteSetupOverallStatus {
    Disabled,
    NeedsAction,
    Ready,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RemoteSetupCheckStatus {
    Ok,
    Warning,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RemoteSetupCheck {
    pub id: String,
    pub label: String,
    pub status: RemoteSetupCheckStatus,
    pub message: String,
    pub details: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RemoteSetupCommandHint {
    pub label: String,
    pub command: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RemoteSetupCheckResult {
    pub overall_status: RemoteSetupOverallStatus,
    pub checks: Vec<RemoteSetupCheck>,
    pub inferred_origin: Option<String>,
    pub serve_target: Option<String>,
    pub setup_command: Option<RemoteSetupCommandHint>,
}

#[derive(Debug, Clone, Default)]
pub struct RemoteRuntimeState {
    pub pairing_offers: HashMap<String, PairingOfferRecord>,
    pub pending_pairing_requests: HashMap<String, PendingPairingRequestRecord>,
    pub auth_challenges: HashMap<String, AuthChallengeRecord>,
    pub sessions: HashMap<String, RemoteSessionRecord>,
    pub websocket_tickets: HashMap<String, WebSocketTicketRecord>,
    pub active_status_streams: HashMap<String, usize>,
    pub rate_limits: HashMap<String, RateLimitBucket>,
}

#[derive(Debug, Clone)]
pub struct PairingOfferRecord {
    pub offer_id: String,
    pub nonce: String,
    pub canonical_origin: String,
    pub expires_at_ms: i64,
    pub used: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PendingPairingDecision {
    Pending,
    Approved,
    Rejected,
}

#[derive(Debug, Clone)]
pub struct PendingPairingRequestRecord {
    pub request_id: String,
    pub pairing_offer_id: String,
    pub device_id: String,
    pub device_label: String,
    pub public_key_spki_der_base64: String,
    pub public_key_fingerprint: String,
    pub canonical_origin: String,
    pub submitted_at_ms: i64,
    pub expires_at_ms: i64,
    pub decision: PendingPairingDecision,
    pub paired_at: Option<String>,
}

#[derive(Debug, Clone)]
pub struct AuthChallengeRecord {
    pub challenge_id: String,
    pub device_id: String,
    pub nonce: String,
    pub canonical_origin: String,
    pub expires_at_ms: i64,
    pub used: bool,
}

#[derive(Debug, Clone)]
pub struct RemoteSessionRecord {
    pub session_id: String,
    pub device_id: String,
    pub created_at_ms: i64,
    pub last_seen_at_ms: i64,
    pub expires_at_ms: i64,
    pub absolute_expires_at_ms: i64,
    pub csrf_nonce: String,
    pub revoked: bool,
}

#[derive(Debug, Clone)]
pub struct WebSocketTicketRecord {
    pub ticket: String,
    pub session_id: String,
    pub device_id: String,
    pub stream: String,
    pub canonical_origin: String,
    pub expires_at_ms: i64,
    pub used: bool,
}

#[derive(Debug, Clone, Default)]
pub struct RateLimitBucket {
    pub attempts: VecDeque<i64>,
    pub locked_until_ms: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RemoteAuditRecord {
    pub schema_version: u8,
    pub event_id: String,
    pub timestamp: String,
    pub request_id: String,
    pub device_id: Option<String>,
    pub session_id: Option<String>,
    pub origin: Option<String>,
    pub event_type: String,
    pub action: String,
    pub target_type: Option<String>,
    pub target_id: Option<String>,
    pub outcome: String,
    pub error_code: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PairingQrPayload {
    pub gateway_origin: String,
    pub pairing_offer_id: String,
    pub expires_at: String,
    pub nonce: String,
    pub server_identity_fingerprint: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DeviceRecord {
    pub device_id: String,
    pub label: String,
    pub public_key_spki_der_base64: String,
    pub public_key_fingerprint: String,
    pub created_at: String,
    pub last_used_at: Option<String>,
    pub revoked_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RemoteDeviceStore {
    pub schema_version: u8,
    pub devices: Vec<DeviceRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AuthChallengeResponse {
    pub challenge_id: String,
    pub device_id: String,
    pub origin: String,
    pub server_identity_fingerprint: String,
    pub nonce: String,
    pub expires_at: String,
    pub audience: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PairingSubmitRequest {
    pub pairing_offer_id: String,
    pub nonce: String,
    pub device_label: String,
    pub public_key_spki_der_base64: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PairingSubmitResponse {
    pub status: String,
    pub pairing_request_id: String,
    pub device_id: String,
    pub public_key_fingerprint: String,
    pub paired_at: Option<String>,
    pub expires_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RemotePendingPairingRequest {
    pub request_id: String,
    pub device_label: String,
    pub public_key_fingerprint: String,
    pub canonical_origin: String,
    pub submitted_at: String,
    pub expires_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AuthChallengeRequest {
    pub device_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AuthSessionRequest {
    pub challenge_id: String,
    pub device_id: String,
    pub signature_der_base64: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AuthSessionResponse {
    pub csrf_nonce: String,
    pub expires_at: String,
    pub absolute_expires_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RemoteAgentSummary {
    pub session_id: String,
    pub session_name: String,
    pub agent_class: String,
    pub provider: String,
    pub workspace: String,
    pub status: String,
    pub latest_text: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RemoteTerminalSnapshot {
    pub cursor: String,
    pub text: String,
    pub truncated: bool,
    pub omitted_bytes: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RemoteAgentActionRequest {
    pub action: String,
    pub target: String,
    pub prompt: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RemoteWebSocketTicketRequest {
    pub stream: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RemoteWebSocketTicketResponse {
    pub ticket: String,
    pub expires_at: String,
}
