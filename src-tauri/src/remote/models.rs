use serde::{Deserialize, Deserializer, Serialize};
use std::collections::{HashMap, VecDeque};
use wardian_core::control::MessageInputMode;

pub const REMOTE_SETTINGS_SCHEMA_VERSION: u8 = 1;
pub const REMOTE_AUDIT_SCHEMA_VERSION: u8 = 1;
pub const REMOTE_DEVICE_STORE_SCHEMA_VERSION: u8 = 2;
pub const REMOTE_PWA_AUDIENCE: &str = "wardian_remote_pwa";
pub const REMOTE_CLIENT_AUDIENCE: &str = "wardian_remote_client_v1";

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
    /// Exact browser origins allowed to use the paired-client API.
    #[serde(default)]
    pub client_origin_allowlist: Vec<String>,
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
    pub client_auth_challenges: HashMap<String, AuthChallengeRecord>,
    pub sessions: HashMap<String, RemoteSessionRecord>,
    pub client_sessions: HashMap<String, RemoteClientSessionRecord>,
    pub websocket_tickets: HashMap<String, WebSocketTicketRecord>,
    pub active_status_streams: HashMap<String, usize>,
    pub rate_limits: HashMap<String, RateLimitBucket>,
}

#[derive(Debug, Clone)]
pub struct PairingOfferRecord {
    pub offer_id: String,
    pub nonce: String,
    pub canonical_origin: String,
    pub gateway_identity_fingerprint: String,
    pub credential_kind: DeviceCredentialKind,
    pub audience: RemoteCredentialAudience,
    pub client_origin: Option<String>,
    pub read_grants: Vec<RemoteReadGrant>,
    pub expires_at_ms: i64,
    pub used: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PendingPairingDecision {
    Pending,
    /// Reserved while device-store publication runs without the runtime lock.
    Approving,
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
    pub credential_kind: DeviceCredentialKind,
    pub audience: RemoteCredentialAudience,
    pub client_origin: Option<String>,
    pub read_grants: Vec<RemoteReadGrant>,
    pub send_grants: Vec<RemoteSendGrant>,
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
    pub gateway_identity_fingerprint: String,
    pub audience: RemoteCredentialAudience,
    pub client_origin: Option<String>,
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
pub struct RemoteClientSessionRecord {
    /// Random audit identifier; it is distinct from the bearer token and verifier.
    pub session_id: String,
    pub device_id: String,
    pub client_origin: String,
    pub gateway_origin: String,
    pub audience: RemoteCredentialAudience,
    pub gateway_identity_fingerprint: String,
    pub read_grants: Vec<RemoteReadGrant>,
    pub send_grants: Vec<RemoteSendGrant>,
    pub idempotency_namespace: Option<String>,
    pub idempotency_expires_at_ms: Option<i64>,
    pub expires_at_ms: i64,
    /// Keeps a revoked verifier recognizable as device_revoked until expiry.
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
pub struct RemoteClientPairingQrPayload {
    pub gateway_origin: String,
    pub credential_kind: DeviceCredentialKind,
    pub client_origin: String,
    pub read_grants: Vec<RemoteReadGrant>,
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
    pub credential_kind: DeviceCredentialKind,
    pub audience: RemoteCredentialAudience,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub client_origin: Option<String>,
    pub read_grants: Vec<RemoteReadGrant>,
    #[serde(default)]
    pub send_grants: Vec<RemoteSendGrant>,
    pub created_at: String,
    pub last_used_at: Option<String>,
    pub revoked_at: Option<String>,
}

fn deserialize_required_option<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    Option::<String>::deserialize(deserializer)
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
pub struct RemoteClientPairingSubmitResponse {
    pub status: String,
    pub pairing_request_id: String,
    pub device_id: String,
    pub public_key_fingerprint: String,
    pub expires_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RemoteClientPairingSubmitRequest {
    pub pairing_offer_id: String,
    pub nonce: String,
    pub device_label: String,
    pub public_key_spki_der_base64: String,
    #[serde(default)]
    pub requested_scopes: Option<Vec<RemoteClientScope>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RemotePendingPairingRequest {
    pub request_id: String,
    pub device_label: String,
    pub public_key_fingerprint: String,
    pub canonical_origin: String,
    pub credential_kind: DeviceCredentialKind,
    pub audience: RemoteCredentialAudience,
    pub client_origin: Option<String>,
    pub read_grants: Vec<RemoteReadGrant>,
    pub send_grants: Vec<RemoteSendGrant>,
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
pub struct RemoteClientAuthChallengeResponse {
    pub challenge_id: String,
    pub device_id: String,
    pub origin: String,
    pub client_origin: String,
    pub server_identity_fingerprint: String,
    pub nonce: String,
    pub expires_at: String,
    pub audience: String,
    pub signing_payload: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RemoteClientAuthSessionResponse {
    pub token_type: String,
    pub access_token: String,
    pub expires_at: String,
    pub device_id: String,
    pub scopes: Vec<RemoteClientScope>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub idempotency_namespace: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub idempotency_expires_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RemoteClientSharingStatus {
    pub status: String,
    pub device_id: String,
    pub client_origin: String,
    pub audience: String,
    pub read_grants: Vec<RemoteReadGrant>,
    pub send_grants: Vec<RemoteSendGrant>,
    pub expires_at: String,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DeviceCredentialKind {
    #[default]
    Pwa,
    #[serde(rename = "client")]
    PairedClient,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum RemoteCredentialAudience {
    #[default]
    #[serde(rename = "wardian_remote_pwa")]
    Pwa,
    #[serde(rename = "wardian_remote_client_v1")]
    PairedClientV1,
}

impl RemoteCredentialAudience {
    pub fn for_kind(kind: DeviceCredentialKind) -> Self {
        match kind {
            DeviceCredentialKind::Pwa => Self::Pwa,
            DeviceCredentialKind::PairedClient => Self::PairedClientV1,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pwa => REMOTE_PWA_AUDIENCE,
            Self::PairedClientV1 => REMOTE_CLIENT_AUDIENCE,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RemoteReadGrant {
    #[serde(rename = "agents:read")]
    AgentsRead,
    #[serde(rename = "chat:read")]
    ChatRead,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum RemoteSendGrant {
    #[serde(rename = "chat:send")]
    ChatSend,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum RemoteClientScope {
    #[serde(rename = "agents:read")]
    AgentsRead,
    #[serde(rename = "chat:read")]
    ChatRead,
    #[serde(rename = "chat:send")]
    ChatSend,
}

impl From<RemoteReadGrant> for RemoteClientScope {
    fn from(grant: RemoteReadGrant) -> Self {
        match grant {
            RemoteReadGrant::AgentsRead => Self::AgentsRead,
            RemoteReadGrant::ChatRead => Self::ChatRead,
        }
    }
}

impl From<RemoteSendGrant> for RemoteClientScope {
    fn from(grant: RemoteSendGrant) -> Self {
        match grant {
            RemoteSendGrant::ChatSend => Self::ChatSend,
        }
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RemoteClientMessageSubmitRequest {
    pub text: String,
    pub input_mode: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RemoteClientMessageOperation {
    pub operation_id: String,
    pub request_id: String,
    pub session_id: String,
    pub delivery_state: String,
    pub accepted_at: String,
    pub updated_at: String,
    pub idempotency_expires_at: String,
    pub error_code: Option<String>,
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
pub struct RemoteWatchlistResponse {
    pub watchlists: serde_json::Value,
    pub teams: serde_json::Value,
    pub prefs: Option<serde_json::Value>,
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
    pub input_mode: Option<MessageInputMode>,
    pub inbox_item_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RemoteInboxActionRequest {
    pub action: String,
    pub item_id: Option<String>,
    pub choice: Option<String>,
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RemoteAutomationMonitorRun {
    pub run_id: String,
    pub blueprint_id: String,
    pub automation_name: String,
    pub schedule_id: Option<String>,
    pub status: String,
    pub node_count: usize,
    pub completed_node_count: Option<usize>,
    pub failure: Option<String>,
    pub started_at: Option<String>,
    pub updated_at: Option<String>,
    pub completed_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteAutomationMonitorSchedule {
    pub id: String,
    pub blueprint_id: String,
    pub automation_name: String,
    pub schedule: wardian_core::models::ScheduleDefinition,
    pub next_run_epoch_ms: Option<u64>,
    pub is_paused: bool,
    pub last_run_status: Option<String>,
    pub last_run_error: Option<String>,
    pub last_run_epoch_ms: Option<u64>,
    pub target_labels: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteAutomationMonitorSnapshot {
    pub schema_version: u8,
    pub generated_at: String,
    pub active_runs: Vec<RemoteAutomationMonitorRun>,
    pub active_runs_truncated: bool,
    pub active_runs_next_offset: Option<usize>,
    pub recent_runs: Vec<RemoteAutomationMonitorRun>,
    pub recent_runs_truncated: bool,
    pub recent_runs_next_offset: Option<usize>,
    pub schedules: Vec<RemoteAutomationMonitorSchedule>,
    pub schedules_truncated: bool,
    pub schedules_next_offset: Option<usize>,
}
