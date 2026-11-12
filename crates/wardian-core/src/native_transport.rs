pub use crate::claude_mod::NonSubmissionProof;
use serde::{Deserialize, Serialize};

/// Provider-neutral identity and policy attached to one native provider turn.
///
/// Wardian owns every field in this envelope. Provider adapters may project it
/// into provider-native input, but provider session or thread identifiers must
/// never replace `target_agent_id` as an address.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NativeMessageEnvelope {
    pub interaction_id: String,
    pub message_id: String,
    pub target_agent_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sender_agent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_interaction_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caller_idempotency_key: Option<String>,
    pub generation: u64,
    pub operation: NativeMessageOperation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deadline_at: Option<String>,
    pub body: String,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NativeMessageOperation {
    #[default]
    StartTurn,
    /// Exceptional active-turn correction. This is not a general priority or
    /// preemption mechanism.
    InvalidatePremise,
}

/// Capabilities proved by a concrete provider transport for one installed
/// provider version. Unknown operations remain false rather than inferred.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NativeTransportCapabilities {
    pub provider: String,
    pub transport: String,
    pub protocol_version: String,
    pub persistent_session: bool,
    pub positive_turn_start: bool,
    pub late_reconciliation: bool,
    /// Provider-native cancellation. Broker-owned withdrawal and replacement
    /// are reported separately.
    pub cancellation: bool,
    pub invalidate_premise: bool,
    pub approval_requests: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_payload_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_timeout_ms: Option<u64>,
}

impl NativeTransportCapabilities {
    pub fn degraded(provider: impl Into<String>, transport: impl Into<String>) -> Self {
        Self {
            provider: provider.into(),
            transport: transport.into(),
            protocol_version: "unknown".to_string(),
            persistent_session: false,
            positive_turn_start: false,
            late_reconciliation: false,
            cancellation: false,
            invalidate_premise: false,
            approval_requests: false,
            max_payload_bytes: None,
            execution_timeout_ms: None,
        }
    }
}

/// Durable binding between a Wardian runtime generation and provider-owned
/// diagnostic identity.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NativeSessionBinding {
    pub target_agent_id: String,
    pub generation: u64,
    pub provider: String,
    pub transport: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_session_id: Option<String>,
    pub capabilities: NativeTransportCapabilities,
    pub observed_at: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NativeDeliveryPhase {
    Queued,
    Dispatching,
    /// Bytes or a protocol request may have crossed the provider boundary,
    /// but Wardian has no positive provider evidence yet. This phase is never
    /// automatically retried.
    SubmittedUnconfirmed,
    /// The provider acknowledged the protocol request. This does not by itself
    /// prove that a model turn began.
    ProviderAccepted,
    /// Positive provider evidence proves that the addressed turn began.
    TurnStarted,
    Completed,
    FailedBeforeSubmit,
    Failed,
    CancelRequested,
    Cancelled,
    Expired,
    StaleGeneration,
    Withdrawn,
    Superseded,
}

impl NativeDeliveryPhase {
    /// Narrow proof-bearing recovery; ordinary transition rules remain unchanged.
    /// The caller must apply this to the interaction and generation named by the proof.
    pub fn resolve_proven_non_submission(self, _proof: &NonSubmissionProof) -> Option<Self> {
        (self == Self::SubmittedUnconfirmed).then_some(Self::FailedBeforeSubmit)
    }

    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed
                | Self::FailedBeforeSubmit
                | Self::Failed
                | Self::Cancelled
                | Self::Expired
                | Self::StaleGeneration
                | Self::Withdrawn
                | Self::Superseded
        )
    }

    /// Enforces the Wardian-owned delivery state machine. Reconciliation may
    /// advance an unconfirmed submission when late provider evidence arrives;
    /// it may not move a terminal interaction or rewind to a replayable phase.
    pub fn can_transition_to(self, next: Self) -> bool {
        use NativeDeliveryPhase as P;
        if self == next {
            return true;
        }
        match self {
            P::Queued => matches!(
                next,
                P::Dispatching
                    | P::FailedBeforeSubmit
                    | P::Cancelled
                    | P::Expired
                    | P::StaleGeneration
                    | P::Withdrawn
                    | P::Superseded
            ),
            P::Dispatching => matches!(
                next,
                P::SubmittedUnconfirmed
                    | P::ProviderAccepted
                    | P::TurnStarted
                    | P::FailedBeforeSubmit
                    | P::Failed
                    | P::CancelRequested
                    | P::Cancelled
                    | P::Expired
                    | P::StaleGeneration
            ),
            P::SubmittedUnconfirmed => matches!(
                next,
                P::ProviderAccepted
                    | P::TurnStarted
                    | P::Completed
                    | P::Failed
                    | P::CancelRequested
                    | P::Cancelled
                    | P::Expired
                    | P::StaleGeneration
            ),
            P::ProviderAccepted => matches!(
                next,
                P::TurnStarted
                    | P::Completed
                    | P::Failed
                    | P::CancelRequested
                    | P::Cancelled
                    | P::Expired
                    | P::StaleGeneration
            ),
            P::TurnStarted => matches!(
                next,
                P::Completed
                    | P::Failed
                    | P::CancelRequested
                    | P::Cancelled
                    | P::Expired
                    | P::StaleGeneration
            ),
            P::CancelRequested => matches!(
                next,
                P::Cancelled | P::Completed | P::Failed | P::Expired | P::StaleGeneration
            ),
            P::Completed
            | P::FailedBeforeSubmit
            | P::Failed
            | P::Cancelled
            | P::Expired
            | P::StaleGeneration
            | P::Withdrawn
            | P::Superseded => false,
        }
    }

    /// Only messages that have not crossed the provider boundary are safe for
    /// automatic queue recovery.
    pub fn is_automatically_retryable(self) -> bool {
        matches!(self, Self::Queued | Self::Dispatching)
    }
}

/// Current durable projection for one Wardian-owned native delivery.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NativeDeliveryRecord {
    pub envelope: NativeMessageEnvelope,
    pub canonical_hash: String,
    pub provider: String,
    pub transport: String,
    pub phase: NativeDeliveryPhase,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_request_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_turn_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NativeDeliveryEvidence {
    pub interaction_id: String,
    pub message_id: String,
    pub target_agent_id: String,
    pub generation: u64,
    pub provider: String,
    pub transport: String,
    pub phase: NativeDeliveryPhase,
    pub source: NativeEvidenceSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_request_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_turn_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    pub observed_at: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NativeEvidenceSource {
    WardianQueue,
    ProviderResponse,
    ProviderEvent,
    ProviderTranscript,
    Reconciler,
    Deadline,
    Caller,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NativeDeliveryErrorCode {
    IdempotencyConflict,
    DeadlineExpired,
    StaleGeneration,
    UnsupportedProvider,
    UnsupportedOperation,
    CapabilityUnavailable,
    TransportUnavailable,
    FailedBeforeSubmit,
    SubmittedUnconfirmed,
    InvalidTransition,
    NotFound,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unconfirmed_submission_is_not_automatically_retryable() {
        assert!(NativeDeliveryPhase::Queued.is_automatically_retryable());
        assert!(NativeDeliveryPhase::Dispatching.is_automatically_retryable());
        assert!(!NativeDeliveryPhase::SubmittedUnconfirmed.is_automatically_retryable());
        assert!(!NativeDeliveryPhase::ProviderAccepted.is_automatically_retryable());
        assert!(!NativeDeliveryPhase::TurnStarted.is_automatically_retryable());
    }

    #[test]
    fn late_evidence_can_reconcile_unconfirmed_delivery() {
        assert!(NativeDeliveryPhase::SubmittedUnconfirmed
            .can_transition_to(NativeDeliveryPhase::TurnStarted));
        assert!(NativeDeliveryPhase::SubmittedUnconfirmed
            .can_transition_to(NativeDeliveryPhase::Completed));
    }

    #[test]
    fn queued_work_can_be_withdrawn_but_submitted_work_cannot() {
        assert!(NativeDeliveryPhase::Queued.can_transition_to(NativeDeliveryPhase::Withdrawn));
        assert!(!NativeDeliveryPhase::SubmittedUnconfirmed
            .can_transition_to(NativeDeliveryPhase::Withdrawn));
    }

    #[test]
    fn terminal_phases_never_reopen() {
        for phase in [
            NativeDeliveryPhase::Completed,
            NativeDeliveryPhase::FailedBeforeSubmit,
            NativeDeliveryPhase::Failed,
            NativeDeliveryPhase::Cancelled,
            NativeDeliveryPhase::Expired,
            NativeDeliveryPhase::StaleGeneration,
            NativeDeliveryPhase::Withdrawn,
            NativeDeliveryPhase::Superseded,
        ] {
            assert!(phase.is_terminal());
            assert!(!phase.can_transition_to(NativeDeliveryPhase::Queued));
        }
    }

    #[test]
    fn envelope_round_trips_with_provider_neutral_identity() {
        let envelope = NativeMessageEnvelope {
            interaction_id: "ask_1".into(),
            message_id: "msg_1".into(),
            target_agent_id: "wardian-agent".into(),
            sender_agent_id: Some("orchestrator".into()),
            parent_interaction_id: None,
            caller_idempotency_key: Some("caller-1".into()),
            generation: 7,
            operation: NativeMessageOperation::StartTurn,
            deadline_at: Some("2026-08-28T06:00:00.000Z".into()),
            body: "Review the patch".into(),
        };

        let encoded = serde_json::to_string(&envelope).expect("serialize envelope");
        let decoded: NativeMessageEnvelope =
            serde_json::from_str(&encoded).expect("deserialize envelope");

        assert_eq!(decoded, envelope);
        assert!(!encoded.contains("provider_session_id"));
    }
}
