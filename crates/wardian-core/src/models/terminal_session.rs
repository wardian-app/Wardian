use serde::{Deserialize, Serialize};

/// Maximum UTF-8 byte length for externally supplied terminal identifiers.
pub const MAX_TERMINAL_IDENTIFIER_BYTES: usize = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalGeometry {
    pub rows: u16,
    pub cols: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalClientKind {
    Desktop,
    Remote,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalVisibility {
    Visible,
    Hidden,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalRenderState {
    Mounted,
    Suspended,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalRequestedInteraction {
    Interactive,
    ReadOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalInteractionCapability {
    Interactive,
    ReadOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalRuntimeState {
    Live,
    Paused,
    Terminated,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalPresentationRegistration {
    pub presentation_id: String,
    pub session_id: String,
    pub client_kind: TerminalClientKind,
    pub desired_geometry: Option<TerminalGeometry>,
    pub visibility: TerminalVisibility,
    pub render_state: TerminalRenderState,
    pub requested_interaction: TerminalRequestedInteraction,
    pub observed_lease_epoch: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalPresentationUpdateRequest {
    pub presentation_id: String,
    pub session_id: String,
    pub runtime_generation: u64,
    pub desired_geometry: Option<TerminalGeometry>,
    pub visibility: TerminalVisibility,
    pub render_state: TerminalRenderState,
    pub requested_interaction: TerminalRequestedInteraction,
    pub observed_lease_epoch: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalPresentationState {
    pub presentation_id: String,
    pub client_kind: TerminalClientKind,
    pub desired_geometry: Option<TerminalGeometry>,
    pub visibility: TerminalVisibility,
    pub render_state: TerminalRenderState,
    pub interaction_capability: TerminalInteractionCapability,
    pub interaction_sequence: u64,
    pub requires_resync: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalPendingActivationState {
    pub presentation_id: String,
    pub previous_owner_presentation_id: Option<String>,
    pub runtime_generation: u64,
    pub lease_epoch: u64,
    pub activation_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalBrokerState {
    pub session_id: String,
    pub runtime_generation: u64,
    pub lease_epoch: u64,
    pub stream_sequence: u64,
    pub interaction_sequence: u64,
    pub geometry: TerminalGeometry,
    pub owner_presentation_id: Option<String>,
    pub pending_activation: Option<TerminalPendingActivationState>,
    pub runtime_state: TerminalRuntimeState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalPresentationRegistrationResult {
    pub presentation: TerminalPresentationState,
    pub broker_state: TerminalBrokerState,
    pub initial_snapshot: TerminalSnapshot,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalPresentationUpdateResult {
    pub presentation: TerminalPresentationState,
    pub broker_state: TerminalBrokerState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalPresentationViewportRequest {
    pub session_id: String,
    pub presentation_id: String,
    pub runtime_generation: u64,
    pub cols: u16,
    pub rows: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalLeaseIdentity {
    pub session_id: String,
    pub presentation_id: String,
    pub runtime_generation: u64,
    pub lease_epoch: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalActivationBeginRequest {
    pub session_id: String,
    pub presentation_id: String,
    pub runtime_generation: u64,
    pub observed_lease_epoch: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalActivationAckRequest {
    pub session_id: String,
    pub presentation_id: String,
    pub runtime_generation: u64,
    pub lease_epoch: u64,
    pub activation_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalLeaseDecisionStatus {
    Accepted,
    Rejected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalLeaseRejectionReason {
    RuntimeUnavailable,
    GenerationChanged,
    LeaseEpochChanged,
    PresentationNotFound,
    PresentationIneligible,
    PendingActivation,
    NotOwner,
    StaleActivation,
    ResyncNotRequired,
    StaleOwnerResync,
    StaleGeometrySequence,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalLeaseDecision {
    pub status: TerminalLeaseDecisionStatus,
    pub reason: Option<TerminalLeaseRejectionReason>,
    pub runtime_generation: u64,
    pub lease_epoch: u64,
    pub owner_presentation_id: Option<String>,
}

impl TerminalLeaseDecision {
    pub fn accepted(
        runtime_generation: u64,
        lease_epoch: u64,
        owner_presentation_id: Option<String>,
    ) -> Self {
        Self {
            status: TerminalLeaseDecisionStatus::Accepted,
            reason: None,
            runtime_generation,
            lease_epoch,
            owner_presentation_id,
        }
    }

    pub fn rejected(
        reason: TerminalLeaseRejectionReason,
        runtime_generation: u64,
        lease_epoch: u64,
        owner_presentation_id: Option<String>,
    ) -> Self {
        Self {
            status: TerminalLeaseDecisionStatus::Rejected,
            reason: Some(reason),
            runtime_generation,
            lease_epoch,
            owner_presentation_id,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalActivationBeginResult {
    pub decision: TerminalLeaseDecision,
    pub activation_id: Option<String>,
    pub snapshot: Option<TerminalSnapshot>,
    pub sequence_barrier: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalActivationAckResult {
    pub decision: TerminalLeaseDecision,
    pub broker_state: TerminalBrokerState,
    pub snapshot: Option<TerminalSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalOwnerResyncBeginRequest {
    pub session_id: String,
    pub presentation_id: String,
    pub runtime_generation: u64,
    pub lease_epoch: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalOwnerResyncBeginResult {
    pub decision: TerminalLeaseDecision,
    pub resync_id: Option<String>,
    pub snapshot: Option<TerminalSnapshot>,
    pub sequence_barrier: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalOwnerResyncAckRequest {
    pub session_id: String,
    pub presentation_id: String,
    pub runtime_generation: u64,
    pub lease_epoch: u64,
    pub resync_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalOwnerResyncAckResult {
    pub decision: TerminalLeaseDecision,
    pub broker_state: TerminalBrokerState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalInputRequest {
    pub lease: TerminalLeaseIdentity,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalGeometryRequest {
    pub lease: TerminalLeaseIdentity,
    pub geometry_sequence: u64,
    pub geometry: TerminalGeometry,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalGeometryCommitResult {
    pub decision: TerminalLeaseDecision,
    pub geometry_sequence: u64,
    pub geometry: TerminalGeometry,
    pub snapshot: Option<TerminalSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalSnapshot {
    pub snapshot_id: String,
    pub session_id: String,
    pub runtime_generation: u64,
    pub sequence_barrier: u64,
    pub geometry: TerminalGeometry,
    pub terminal_state_base64: String,
    pub visible_grid: String,
    pub scrollback: Vec<String>,
    #[serde(default)]
    pub formatted_scrollback: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalBrokerEvent {
    pub sequence: u64,
    pub runtime_generation: u64,
    #[serde(flatten)]
    pub event: TerminalBrokerEventKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TerminalBrokerEventKind {
    Output {
        bytes: Vec<u8>,
    },
    Geometry {
        geometry: TerminalGeometry,
        geometry_sequence: u64,
    },
    Ownership {
        owner_presentation_id: Option<String>,
        lease_epoch: u64,
        activation_id: Option<String>,
    },
    Lifecycle {
        lifecycle: TerminalSessionLifecycleEvent,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalSessionLifecycleEvent {
    RuntimeStarted,
    RuntimePaused,
    RuntimeResumed,
    RuntimeReplaced,
    RuntimeTerminated,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalSessionLifecycleNotification {
    pub session_id: String,
    pub runtime_generation: u64,
    pub lifecycle: TerminalSessionLifecycleEvent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalEventSubscriptionRequest {
    pub session_id: String,
    pub consumer_id: String,
    pub client_kind: TerminalClientKind,
    pub runtime_generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalEventSubscriptionResult {
    pub broker_state: TerminalBrokerState,
    pub initial_snapshot: TerminalSnapshot,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalEventReadRequest {
    pub session_id: String,
    pub consumer_id: String,
    pub runtime_generation: u64,
    pub after_sequence: u64,
    pub max_events: u16,
    pub max_bytes: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalEventBatchStatus {
    Events,
    Gap,
    GenerationChanged,
    Terminated,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalEventBatch {
    pub status: TerminalEventBatchStatus,
    pub runtime_generation: u64,
    pub events: Vec<TerminalBrokerEvent>,
    pub next_sequence: u64,
    pub available_from_sequence: u64,
    pub latest_sequence: u64,
    pub recovery_snapshot: Option<TerminalSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalEventAckRequest {
    pub session_id: String,
    pub consumer_id: String,
    pub runtime_generation: u64,
    pub applied_sequence: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalEventAckResult {
    pub accepted_sequence: u64,
    pub latest_sequence: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalEventUnsubscribeRequest {
    pub session_id: String,
    pub consumer_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalEventsReady {
    pub session_id: String,
    pub runtime_generation: u64,
    pub latest_sequence: u64,
}
