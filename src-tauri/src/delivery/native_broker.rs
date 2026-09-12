use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::{mpsc, oneshot, Mutex};
use wardian_core::models::AgentConfig;
use wardian_core::native_transport::{
    NativeDeliveryErrorCode, NativeDeliveryEvidence, NativeDeliveryPhase, NativeDeliveryRecord,
    NativeEvidenceSource, NativeMessageEnvelope, NativeMessageOperation, NativeSessionBinding,
    NativeTransportCapabilities,
};

use super::native_session::{NativeProtocolEvent, NativeProtocolEventKind, NativeProviderProtocol};
use crate::providers::{CodexProvider, PiProvider, ProviderFactory};

mod codex;

const SESSION_COMMAND_CAPACITY: usize = 64;
const BOOTSTRAP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
const PROTOCOL_PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);
const STDERR_TAIL_BYTES: usize = 4 * 1024;

#[derive(Debug, Clone)]
pub struct NativeDeliveryAdmission {
    pub interaction_id: String,
    pub message_id: String,
    pub target_agent_id: String,
    pub sender_agent_id: Option<String>,
    pub provider: String,
    pub generation: u64,
    pub operation: NativeMessageOperation,
    pub caller_idempotency_key: Option<String>,
    pub parent_interaction_id: Option<String>,
    pub deadline_at: Option<String>,
    pub body: String,
}

#[derive(Debug, Clone)]
pub struct NativeSessionSpec {
    pub target_agent_id: String,
    pub provider: String,
    pub generation: u64,
    pub workspace: PathBuf,
    pub config: AgentConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NativeDispatchReceipt {
    pub record: NativeDeliveryRecord,
    pub binding: NativeSessionBinding,
    pub capabilities: NativeTransportCapabilities,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NativeBrokerError {
    pub code: NativeDeliveryErrorCode,
    pub message: String,
    pub provider_boundary_crossed: bool,
}

impl std::fmt::Display for NativeBrokerError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.message)
    }
}

impl std::error::Error for NativeBrokerError {}

#[derive(Debug)]
struct NativeSessionHandle {
    generation: u64,
    provider: String,
    capabilities: NativeTransportCapabilities,
    tx: mpsc::Sender<SessionCommand>,
    shared_codex: Option<Arc<super::codex_shared::CodexSharedOwner>>,
    stopped: Option<tokio::sync::watch::Receiver<bool>>,
}

#[derive(Debug)]
enum SessionCommand {
    Submit {
        record: Box<NativeDeliveryRecord>,
        reply: oneshot::Sender<Result<NativeDispatchReceipt, NativeBrokerError>>,
    },
    Cancel {
        interaction_id: String,
        reply: oneshot::Sender<Result<NativeDeliveryRecord, NativeBrokerError>>,
    },
    Shutdown,
}

#[derive(Debug)]
struct ActiveTurn {
    record: NativeDeliveryRecord,
    receipt: Option<oneshot::Sender<Result<NativeDispatchReceipt, NativeBrokerError>>>,
    positive_start_seen: bool,
    response_text: String,
}

#[derive(Debug)]
struct ActiveCorrection {
    record: NativeDeliveryRecord,
    receipt: Option<oneshot::Sender<Result<NativeDispatchReceipt, NativeBrokerError>>>,
}

struct NativeRuntime {
    child: Child,
    stdin: ChildStdin,
    lines: Lines<BufReader<ChildStdout>>,
    stderr_tail: Arc<Mutex<VecDeque<u8>>>,
    binding: NativeSessionBinding,
    // The lease revokes the provider's memory capability on runtime shutdown.
    _memory_capability: Option<wardian_core::memory::MemoryCapabilityLease>,
}

type CodexCreationRevisionMap =
    HashMap<(String, u64), std::sync::Weak<tokio::sync::watch::Sender<u64>>>;

/// Tracks only live creation requests, including workers not yet at the owner gate.
/// Registration and disposal revisions are synchronous; neither holds this lock
/// across an await. Expired weak entries are pruned rather than kept as tombstones.
#[derive(Debug, Default)]
struct CodexCreationRegistry {
    revisions: std::sync::Mutex<CodexCreationRevisionMap>,
}

/// Keeps its generation's revision alive until the detached creation worker ends.
#[derive(Debug)]
struct CodexCreationRequest {
    revision: Arc<tokio::sync::watch::Sender<u64>>,
    observed_revision: u64,
}

impl CodexCreationRegistry {
    /// Capture before the creation request's first await or detached spawn.
    fn register(
        &self,
        agent_id: &str,
        generation: u64,
    ) -> Result<CodexCreationRequest, &'static str> {
        let mut revisions = self
            .revisions
            .lock()
            .map_err(|_| "Codex creation registry lock poisoned")?;
        revisions.retain(|_, revision| revision.strong_count() > 0);
        let revision = revisions
            .entry((agent_id.to_owned(), generation))
            .or_default();
        let sender = match revision.upgrade() {
            Some(sender) => sender,
            None => {
                let (sender, _) = tokio::sync::watch::channel(0_u64);
                let sender = Arc::new(sender);
                *revision = Arc::downgrade(&sender);
                sender
            }
        };
        let observed_revision = *sender.borrow();
        Ok(CodexCreationRequest {
            revision: sender,
            observed_revision,
        })
    }

    /// Cancel requests already registered at this boundary, before awaiting the
    /// owner gate. An exact-generation stop must not invalidate a newer generation.
    fn cancel(&self, agent_id: &str, generation: Option<u64>) -> Result<(), &'static str> {
        let mut revisions = self
            .revisions
            .lock()
            .map_err(|_| "Codex creation registry lock poisoned")?;
        revisions.retain(|(target, current_generation), revision| {
            let Some(sender) = revision.upgrade() else {
                return false;
            };
            if target == agent_id && generation.is_none_or(|value| value == *current_generation) {
                sender.send_modify(|value| *value = value.wrapping_add(1));
            }
            true
        });
        Ok(())
    }
}

impl CodexCreationRequest {
    fn is_cancelled(&self) -> bool {
        *self.revision.borrow() != self.observed_revision
    }

    /// Observe cancellation even if disposal completed before this future started.
    async fn cancelled(&self) {
        let mut revision = self.revision.subscribe();
        let _ = revision
            .wait_for(|value| *value != self.observed_revision)
            .await;
    }
}

#[cfg(test)]
#[derive(Debug, Default)]
struct CodexCreationTestBarrier {
    reached: tokio::sync::Notify,
    release: tokio::sync::Notify,
}

#[cfg(test)]
#[derive(Debug, Default)]
struct CodexCreationTestHooks {
    before_gate: std::sync::Mutex<HashMap<u64, Arc<CodexCreationTestBarrier>>>,
    reject_start: std::sync::atomic::AtomicBool,
    start_attempts: std::sync::atomic::AtomicUsize,
}

#[derive(Debug, Default)]
pub struct NativeDeliveryBroker {
    mutation_lock: Mutex<()>,
    sessions: Mutex<HashMap<String, NativeSessionHandle>>,
    owner_gates: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    owner_changes: tokio::sync::Notify,
    codex_creations: CodexCreationRegistry,
    #[cfg(test)]
    codex_creation_test: CodexCreationTestHooks,
    shutting_down: std::sync::atomic::AtomicBool,
}

impl NativeDeliveryBroker {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn admit(
        &self,
        request: NativeDeliveryAdmission,
    ) -> Result<NativeDeliveryRecord, NativeBrokerError> {
        let _mutation = self.mutation_lock.lock().await;
        if deadline_has_passed(request.deadline_at.as_deref()) {
            return Err(error(
                NativeDeliveryErrorCode::DeadlineExpired,
                "native delivery deadline already expired",
                false,
            ));
        }
        let protocol =
            NativeProviderProtocol::for_provider(&request.provider).ok_or_else(|| {
                error(
                    NativeDeliveryErrorCode::UnsupportedProvider,
                    format!("{} has no Wardian native transport", request.provider),
                    false,
                )
            })?;
        let canonical_hash = canonical_hash(&request);
        if let Some(key) = request
            .caller_idempotency_key
            .as_deref()
            .map(str::trim)
            .filter(|key| !key.is_empty())
        {
            let existing = wardian_core::db::native_delivery_by_idempotency(
                request.sender_agent_id.as_deref(),
                &request.target_agent_id,
                request.operation,
                key,
            )
            .map_err(db_error)?;
            if let Some(existing) = existing {
                if existing.canonical_hash == canonical_hash {
                    return Ok(existing);
                }
                return Err(error(
                    NativeDeliveryErrorCode::IdempotencyConflict,
                    format!("idempotency key {key:?} was already used with a different request"),
                    false,
                ));
            }
        }
        let now = now();
        let record = NativeDeliveryRecord {
            envelope: NativeMessageEnvelope {
                interaction_id: request.interaction_id,
                message_id: request.message_id,
                target_agent_id: request.target_agent_id,
                sender_agent_id: request.sender_agent_id,
                parent_interaction_id: request.parent_interaction_id,
                caller_idempotency_key: request.caller_idempotency_key,
                generation: request.generation,
                operation: request.operation,
                deadline_at: request.deadline_at,
                body: request.body,
            },
            canonical_hash,
            provider: protocol.provider().to_string(),
            transport: protocol.transport().to_string(),
            phase: NativeDeliveryPhase::Queued,
            provider_request_id: None,
            provider_turn_id: None,
            detail: None,
            created_at: now.clone(),
            updated_at: now,
        };
        wardian_core::db::upsert_native_delivery(&record).map_err(db_error)?;
        self.append_evidence(&record, NativeEvidenceSource::WardianQueue, "admitted")?;
        Ok(record)
    }

    pub async fn dispatch(
        self: &Arc<Self>,
        spec: NativeSessionSpec,
        record: NativeDeliveryRecord,
    ) -> Result<NativeDispatchReceipt, NativeBrokerError> {
        if record.envelope.generation != spec.generation {
            let stale = self
                .advance(
                    &record.envelope.interaction_id,
                    NativeDeliveryPhase::StaleGeneration,
                    NativeEvidenceSource::WardianQueue,
                    None,
                    None,
                    Some(format!(
                        "admitted generation {} no longer matches dispatch generation {}",
                        record.envelope.generation, spec.generation
                    )),
                    "stale_generation",
                )
                .await?;
            return Err(error(
                NativeDeliveryErrorCode::StaleGeneration,
                stale
                    .detail
                    .unwrap_or_else(|| "stale generation".to_string()),
                false,
            ));
        }
        let handle = self.ensure_session(spec).await?;
        if let Some(owner) = handle.shared_codex {
            return self
                .dispatch_shared_codex(owner, record, handle.capabilities)
                .await;
        }
        let (reply_tx, reply_rx) = oneshot::channel();
        handle
            .tx
            .send(SessionCommand::Submit {
                record: Box::new(record),
                reply: reply_tx,
            })
            .await
            .map_err(|_| {
                error(
                    NativeDeliveryErrorCode::TransportUnavailable,
                    "native session actor stopped before accepting queued work",
                    false,
                )
            })?;
        reply_rx.await.map_err(|_| {
            error(
                NativeDeliveryErrorCode::TransportUnavailable,
                "native session actor stopped without a delivery receipt",
                false,
            )
        })?
    }

    pub async fn cancel(
        &self,
        interaction_id: &str,
    ) -> Result<NativeDeliveryRecord, NativeBrokerError> {
        let record = self.get(interaction_id)?;
        if record.phase == NativeDeliveryPhase::Queued {
            return self
                .advance(
                    interaction_id,
                    NativeDeliveryPhase::Cancelled,
                    NativeEvidenceSource::Caller,
                    None,
                    None,
                    Some("cancelled before provider submission".to_string()),
                    "cancel_queued",
                )
                .await;
        }
        let sessions = self.sessions.lock().await;
        let handle = sessions
            .get(&record.envelope.target_agent_id)
            .ok_or_else(|| {
                error(
                    NativeDeliveryErrorCode::CapabilityUnavailable,
                    "no active native session can receive cancellation",
                    record.phase != NativeDeliveryPhase::Queued,
                )
            })?;
        if !handle.capabilities.cancellation {
            return Err(error(
                NativeDeliveryErrorCode::CapabilityUnavailable,
                format!(
                    "{} native transport does not support cancellation",
                    handle.provider
                ),
                true,
            ));
        }
        if let Some(owner) = handle.shared_codex.clone() {
            if handle.generation != record.envelope.generation {
                return Err(error(
                    NativeDeliveryErrorCode::StaleGeneration,
                    "cancellation generation changed",
                    false,
                ));
            }
            let turn_id = record.provider_turn_id.as_deref().ok_or_else(|| {
                error(
                    NativeDeliveryErrorCode::CapabilityUnavailable,
                    "cancellation lacks an exact native turn",
                    false,
                )
            })?;
            drop(sessions);
            let receipt = owner
                .client
                .interrupt_expected(Some(turn_id))
                .await
                .map_err(codex::shared_error)?;
            if receipt.interruption_confirmed {
                return self
                    .advance(
                        interaction_id,
                        NativeDeliveryPhase::Cancelled,
                        NativeEvidenceSource::ProviderEvent,
                        None,
                        receipt.provider_turn_id,
                        None,
                        "shared_cancelled",
                    )
                    .await;
            }
            return self.get(interaction_id);
        }
        let (reply_tx, reply_rx) = oneshot::channel();
        handle
            .tx
            .send(SessionCommand::Cancel {
                interaction_id: interaction_id.to_string(),
                reply: reply_tx,
            })
            .await
            .map_err(|_| {
                error(
                    NativeDeliveryErrorCode::TransportUnavailable,
                    "native session actor stopped before cancellation",
                    true,
                )
            })?;
        drop(sessions);
        reply_rx.await.map_err(|_| {
            error(
                NativeDeliveryErrorCode::TransportUnavailable,
                "native session actor stopped without cancellation acknowledgement",
                true,
            )
        })?
    }

    pub async fn withdraw(
        &self,
        interaction_id: &str,
    ) -> Result<NativeDeliveryRecord, NativeBrokerError> {
        self.advance(
            interaction_id,
            NativeDeliveryPhase::Withdrawn,
            NativeEvidenceSource::Caller,
            None,
            None,
            Some("withdrawn while still queued".to_string()),
            "withdraw",
        )
        .await
    }

    pub async fn supersede(
        &self,
        interaction_id: &str,
    ) -> Result<NativeDeliveryRecord, NativeBrokerError> {
        self.advance(
            interaction_id,
            NativeDeliveryPhase::Superseded,
            NativeEvidenceSource::Caller,
            None,
            None,
            Some("replaced by a new queued interaction".to_string()),
            "supersede",
        )
        .await
    }

    pub async fn replace(
        &self,
        interaction_id: &str,
        body: String,
        idempotency_key: String,
        deadline_at: Option<String>,
    ) -> Result<NativeDeliveryRecord, NativeBrokerError> {
        let _mutation = self.mutation_lock.lock().await;
        let mut superseded = self.get(interaction_id)?;
        if superseded.phase != NativeDeliveryPhase::Queued {
            return Err(error(
                NativeDeliveryErrorCode::InvalidTransition,
                format!(
                    "native interaction {interaction_id} is {:?}; only queued work can be replaced",
                    superseded.phase
                ),
                superseded.phase != NativeDeliveryPhase::Queued,
            ));
        }
        if idempotency_key.trim().is_empty() {
            return Err(error(
                NativeDeliveryErrorCode::IdempotencyConflict,
                "replacement requires a non-empty caller idempotency key",
                false,
            ));
        }
        if deadline_has_passed(deadline_at.as_deref()) {
            return Err(error(
                NativeDeliveryErrorCode::DeadlineExpired,
                "replacement deadline already expired",
                false,
            ));
        }
        let replacement_id = format!("int_native_{}", uuid::Uuid::new_v4().simple());
        let message_id = format!("msg_native_{}", uuid::Uuid::new_v4().simple());
        let request = NativeDeliveryAdmission {
            interaction_id: replacement_id.clone(),
            message_id,
            target_agent_id: superseded.envelope.target_agent_id.clone(),
            sender_agent_id: superseded.envelope.sender_agent_id.clone(),
            provider: superseded.provider.clone(),
            generation: superseded.envelope.generation,
            operation: superseded.envelope.operation,
            caller_idempotency_key: Some(idempotency_key),
            parent_interaction_id: Some(superseded.envelope.interaction_id.clone()),
            deadline_at,
            body,
        };
        let canonical_hash = canonical_hash(&request);
        if let Some(existing) = wardian_core::db::native_delivery_by_idempotency(
            request.sender_agent_id.as_deref(),
            &request.target_agent_id,
            request.operation,
            request
                .caller_idempotency_key
                .as_deref()
                .expect("replacement key"),
        )
        .map_err(db_error)?
        {
            if existing.canonical_hash == canonical_hash {
                return Ok(existing);
            }
            return Err(error(
                NativeDeliveryErrorCode::IdempotencyConflict,
                "replacement idempotency key conflicts with an existing request",
                false,
            ));
        }
        let now = now();
        let replacement = NativeDeliveryRecord {
            envelope: NativeMessageEnvelope {
                interaction_id: replacement_id,
                message_id: request.message_id,
                target_agent_id: request.target_agent_id,
                sender_agent_id: request.sender_agent_id,
                parent_interaction_id: request.parent_interaction_id,
                caller_idempotency_key: request.caller_idempotency_key,
                generation: request.generation,
                operation: request.operation,
                deadline_at: request.deadline_at,
                body: request.body,
            },
            canonical_hash,
            provider: superseded.provider.clone(),
            transport: superseded.transport.clone(),
            phase: NativeDeliveryPhase::Queued,
            provider_request_id: None,
            provider_turn_id: None,
            detail: Some(format!("replacement for {interaction_id}")),
            created_at: now.clone(),
            updated_at: now.clone(),
        };
        superseded.phase = NativeDeliveryPhase::Superseded;
        superseded.detail = Some(format!(
            "replaced by {}",
            replacement.envelope.interaction_id
        ));
        superseded.updated_at = now;
        wardian_core::db::replace_native_delivery(&superseded, &replacement).map_err(db_error)?;
        self.append_evidence(&superseded, NativeEvidenceSource::Caller, "superseded")?;
        self.append_evidence(
            &replacement,
            NativeEvidenceSource::WardianQueue,
            "replacement_admitted",
        )?;
        Ok(replacement)
    }

    pub fn get(&self, interaction_id: &str) -> Result<NativeDeliveryRecord, NativeBrokerError> {
        wardian_core::db::native_delivery(interaction_id)
            .map_err(db_error)?
            .ok_or_else(|| {
                error(
                    NativeDeliveryErrorCode::NotFound,
                    format!("native interaction not found: {interaction_id}"),
                    false,
                )
            })
    }

    pub fn evidence(
        &self,
        interaction_id: &str,
        limit: usize,
    ) -> Result<Vec<NativeDeliveryEvidence>, NativeBrokerError> {
        wardian_core::db::list_native_delivery_evidence(interaction_id, limit).map_err(db_error)
    }

    pub async fn dispose_agent(&self, target_agent_id: &str) -> Result<(), NativeBrokerError> {
        let _gate = self.lock_owner_for_stop(target_agent_id, None).await?;
        self.stop_registered_owner(target_agent_id, None).await
    }

    pub async fn recover_after_restart(
        &self,
        limit: usize,
    ) -> Result<Vec<NativeDeliveryRecord>, NativeBrokerError> {
        let records = wardian_core::db::list_native_deliveries(limit).map_err(db_error)?;
        let mut queued = Vec::new();
        for record in records {
            match record.phase {
                NativeDeliveryPhase::Queued
                    if deadline_has_passed(record.envelope.deadline_at.as_deref()) =>
                {
                    let _ = self
                        .advance(
                            &record.envelope.interaction_id,
                            NativeDeliveryPhase::Expired,
                            NativeEvidenceSource::Deadline,
                            None,
                            None,
                            Some("deadline expired while Wardian was stopped".to_string()),
                            "restart_expired",
                        )
                        .await?;
                }
                NativeDeliveryPhase::Queued => queued.push(record),
                NativeDeliveryPhase::Dispatching => {
                    let _ = self
                        .advance(
                            &record.envelope.interaction_id,
                            NativeDeliveryPhase::SubmittedUnconfirmed,
                            NativeEvidenceSource::Reconciler,
                            record.provider_request_id.clone(),
                            record.provider_turn_id.clone(),
                            Some(
                                "Wardian restarted across the provider submission boundary; payload was not replayed"
                                    .to_string(),
                            ),
                            "restart_uncertain",
                        )
                        .await?;
                }
                _ => {}
            }
        }
        Ok(queued)
    }

    async fn ensure_session(
        self: &Arc<Self>,
        spec: NativeSessionSpec,
    ) -> Result<NativeSessionHandle, NativeBrokerError> {
        let _gate = self
            .owner_gate(&spec.target_agent_id)
            .await
            .lock_owned()
            .await;
        let protocol = NativeProviderProtocol::for_provider(&spec.provider).ok_or_else(|| {
            error(
                NativeDeliveryErrorCode::UnsupportedProvider,
                format!("{} has no Wardian native transport", spec.provider),
                false,
            )
        })?;
        {
            let sessions = self.sessions.lock().await;
            if let Some(existing) = sessions.get(&spec.target_agent_id) {
                if existing.generation == spec.generation && existing.provider == spec.provider {
                    return Ok(existing.clone());
                }
            }
        }
        if spec.provider == "codex" {
            return Err(error(NativeDeliveryErrorCode::CapabilityUnavailable,
                "Codex requires a lifecycle-prepared shared owner; embedded sessions require explicit restart", false));
        }
        let protocol_version = probe_protocol_version(&spec, protocol).await;
        let sessions = self.sessions.lock().await;
        if let Some(existing) = sessions.get(&spec.target_agent_id) {
            if existing.generation == spec.generation && existing.provider == spec.provider {
                return Ok(existing.clone());
            }
        }
        drop(sessions);
        self.stop_registered_owner(&spec.target_agent_id, None)
            .await?;
        let capabilities = protocol.capabilities(protocol_version);
        let (tx, rx) = mpsc::channel(SESSION_COMMAND_CAPACITY);
        let (stopped_tx, stopped_rx) = tokio::sync::watch::channel(false);
        let handle = NativeSessionHandle {
            generation: spec.generation,
            provider: spec.provider.clone(),
            capabilities: capabilities.clone(),
            tx,
            shared_codex: None,
            stopped: Some(stopped_rx),
        };
        self.sessions
            .lock()
            .await
            .insert(spec.target_agent_id.clone(), handle.clone());
        let broker = self.clone();
        tokio::spawn(async move {
            run_session_actor(broker, spec, protocol, capabilities, rx).await;
            let _ = stopped_tx.send(true);
        });
        Ok(handle)
    }

    #[allow(clippy::too_many_arguments)]
    async fn advance(
        &self,
        interaction_id: &str,
        next: NativeDeliveryPhase,
        source: NativeEvidenceSource,
        provider_request_id: Option<String>,
        provider_turn_id: Option<String>,
        detail: Option<String>,
        evidence_key: &str,
    ) -> Result<NativeDeliveryRecord, NativeBrokerError> {
        let _mutation = self.mutation_lock.lock().await;
        let mut record = self.get(interaction_id)?;
        let evidence = NativeDeliveryEvidence {
            interaction_id: record.envelope.interaction_id.clone(),
            message_id: record.envelope.message_id.clone(),
            target_agent_id: record.envelope.target_agent_id.clone(),
            generation: record.envelope.generation,
            provider: record.provider.clone(),
            transport: record.transport.clone(),
            phase: next,
            source,
            provider_request_id: provider_request_id.clone(),
            provider_turn_id: provider_turn_id.clone(),
            detail: detail.clone(),
            observed_at: now(),
        };
        append_evidence_with_key(&evidence, evidence_key)?;
        if record.phase == next || phase_rank(record.phase) > phase_rank(next) {
            return Ok(record);
        }
        if !record.phase.can_transition_to(next) {
            return Err(error(
                NativeDeliveryErrorCode::InvalidTransition,
                format!(
                    "cannot advance native delivery from {:?} to {:?}",
                    record.phase, next
                ),
                record.phase != NativeDeliveryPhase::Queued,
            ));
        }
        record.phase = next;
        record.provider_request_id = provider_request_id.or(record.provider_request_id);
        record.provider_turn_id = provider_turn_id.or(record.provider_turn_id);
        if detail.is_some() {
            record.detail = detail;
        }
        record.updated_at = evidence.observed_at;
        wardian_core::db::upsert_native_delivery(&record).map_err(db_error)?;
        Ok(record)
    }

    fn append_evidence(
        &self,
        record: &NativeDeliveryRecord,
        source: NativeEvidenceSource,
        key: &str,
    ) -> Result<(), NativeBrokerError> {
        let evidence = NativeDeliveryEvidence {
            interaction_id: record.envelope.interaction_id.clone(),
            message_id: record.envelope.message_id.clone(),
            target_agent_id: record.envelope.target_agent_id.clone(),
            generation: record.envelope.generation,
            provider: record.provider.clone(),
            transport: record.transport.clone(),
            phase: record.phase,
            source,
            provider_request_id: record.provider_request_id.clone(),
            provider_turn_id: record.provider_turn_id.clone(),
            detail: record.detail.clone(),
            observed_at: now(),
        };
        append_evidence_with_key(&evidence, key)
    }
}

impl Clone for NativeSessionHandle {
    fn clone(&self) -> Self {
        Self {
            generation: self.generation,
            provider: self.provider.clone(),
            capabilities: self.capabilities.clone(),
            tx: self.tx.clone(),
            shared_codex: self.shared_codex.clone(),
            stopped: self.stopped.clone(),
        }
    }
}

async fn run_session_actor(
    broker: Arc<NativeDeliveryBroker>,
    spec: NativeSessionSpec,
    protocol: NativeProviderProtocol,
    capabilities: NativeTransportCapabilities,
    mut rx: mpsc::Receiver<SessionCommand>,
) {
    let mut runtime: Option<NativeRuntime> = None;
    let mut active: Option<ActiveTurn> = None;
    let mut corrections: HashMap<String, ActiveCorrection> = HashMap::new();
    let mut pending = VecDeque::new();
    loop {
        if active.is_none() {
            let command = match pending.pop_front() {
                Some(command) => Some(command),
                None => rx.recv().await,
            };
            match command {
                Some(SessionCommand::Submit { record, reply }) => {
                    let current = match broker.get(&record.envelope.interaction_id) {
                        Ok(current) => current,
                        Err(failure) => {
                            let _ = reply.send(Err(failure));
                            continue;
                        }
                    };
                    if current.phase != NativeDeliveryPhase::Queued {
                        let _ = reply.send(Err(error(
                            NativeDeliveryErrorCode::InvalidTransition,
                            format!(
                                "native interaction {} is {:?} and is no longer dispatchable",
                                current.envelope.interaction_id, current.phase
                            ),
                            false,
                        )));
                        continue;
                    }
                    let record = current;
                    if deadline_has_passed(record.envelope.deadline_at.as_deref()) {
                        let result = broker
                            .advance(
                                &record.envelope.interaction_id,
                                NativeDeliveryPhase::Expired,
                                NativeEvidenceSource::Deadline,
                                None,
                                None,
                                Some("deadline expired while queued".to_string()),
                                "queue_deadline",
                            )
                            .await;
                        let _ = reply.send(Err(result.err().unwrap_or_else(|| {
                            error(
                                NativeDeliveryErrorCode::DeadlineExpired,
                                "native delivery expired while queued",
                                false,
                            )
                        })));
                        continue;
                    }
                    if runtime.is_none() {
                        match start_runtime(&spec, protocol, &capabilities).await {
                            Ok(opened) => runtime = Some(opened),
                            Err(failure) => {
                                let _ = broker
                                    .advance(
                                        &record.envelope.interaction_id,
                                        NativeDeliveryPhase::FailedBeforeSubmit,
                                        NativeEvidenceSource::WardianQueue,
                                        None,
                                        None,
                                        Some(failure.message.clone()),
                                        "startup_failed",
                                    )
                                    .await;
                                let _ = reply.send(Err(failure));
                                continue;
                            }
                        }
                    }
                    let opened = runtime.as_mut().expect("runtime initialized");
                    let dispatching = broker
                        .advance(
                            &record.envelope.interaction_id,
                            NativeDeliveryPhase::Dispatching,
                            NativeEvidenceSource::WardianQueue,
                            None,
                            None,
                            None,
                            "dispatching",
                        )
                        .await;
                    let Ok(dispatching) = dispatching else {
                        let _ = reply.send(Err(dispatching.unwrap_err()));
                        continue;
                    };
                    let request = match protocol.submit_request(
                        &dispatching.envelope,
                        opened.binding.provider_session_id.as_deref(),
                        None,
                    ) {
                        Ok(request) => request,
                        Err(err) => {
                            let failure = error(
                                NativeDeliveryErrorCode::UnsupportedOperation,
                                err.to_string(),
                                false,
                            );
                            let _ = broker
                                .advance(
                                    &dispatching.envelope.interaction_id,
                                    NativeDeliveryPhase::FailedBeforeSubmit,
                                    NativeEvidenceSource::WardianQueue,
                                    None,
                                    None,
                                    Some(failure.message.clone()),
                                    "encode_failed",
                                )
                                .await;
                            let _ = reply.send(Err(failure));
                            continue;
                        }
                    };
                    let request_id = protocol_request_id(&request);
                    if let Err(failure) = write_json(&mut opened.stdin, &request).await {
                        let _ = broker
                            .advance(
                                &dispatching.envelope.interaction_id,
                                NativeDeliveryPhase::SubmittedUnconfirmed,
                                NativeEvidenceSource::WardianQueue,
                                request_id,
                                None,
                                Some(failure.message.clone()),
                                "write_uncertain",
                            )
                            .await;
                        let _ = reply.send(Err(failure));
                        stop_native_runtime(&mut runtime).await;
                        continue;
                    }
                    let submitted = broker
                        .advance(
                            &dispatching.envelope.interaction_id,
                            NativeDeliveryPhase::SubmittedUnconfirmed,
                            NativeEvidenceSource::WardianQueue,
                            request_id,
                            None,
                            Some(
                                "provider request written; awaiting positive evidence".to_string(),
                            ),
                            "request_written",
                        )
                        .await
                        .unwrap_or(dispatching);
                    active = Some(ActiveTurn {
                        record: submitted,
                        receipt: Some(reply),
                        positive_start_seen: false,
                        response_text: String::new(),
                    });
                }
                Some(SessionCommand::Cancel {
                    interaction_id,
                    reply,
                }) => {
                    let _ = reply.send(Err(error(
                        NativeDeliveryErrorCode::InvalidTransition,
                        format!("interaction {interaction_id} is not active"),
                        false,
                    )));
                }
                Some(SessionCommand::Shutdown) | None => break,
            }
            continue;
        }

        let opened = runtime.as_mut().expect("active turn has runtime");
        tokio::select! {
            command = rx.recv() => {
                match command {
                    Some(SessionCommand::Submit { record, reply }) => {
                        if record.envelope.operation == NativeMessageOperation::StartTurn {
                            pending.push_back(SessionCommand::Submit { record, reply });
                            continue;
                        }
                        if !capabilities.invalidate_premise {
                            let _ = reply.send(Err(error(
                                NativeDeliveryErrorCode::CapabilityUnavailable,
                                format!("{} does not support invalidate-premise steering", spec.provider),
                                false,
                            )));
                            continue;
                        }
                        let active_turn_id = active
                            .as_ref()
                            .and_then(|turn| turn.record.provider_turn_id.as_deref());
                        let dispatching = match broker.advance(
                            &record.envelope.interaction_id,
                            NativeDeliveryPhase::Dispatching,
                            NativeEvidenceSource::Caller,
                            None,
                            active_turn_id.map(str::to_string),
                            Some("explicit invalidate-premise correction".to_string()),
                            "correction_dispatching",
                        ).await {
                            Ok(record) => record,
                            Err(failure) => {
                                let _ = reply.send(Err(failure));
                                continue;
                            }
                        };
                        let request = match protocol.submit_request(
                            &dispatching.envelope,
                            opened.binding.provider_session_id.as_deref(),
                            active_turn_id,
                        ) {
                            Ok(request) => request,
                            Err(err) => {
                                let failure = error(
                                    NativeDeliveryErrorCode::CapabilityUnavailable,
                                    err.to_string(),
                                    false,
                                );
                                let _ = broker.advance(
                                    &dispatching.envelope.interaction_id,
                                    NativeDeliveryPhase::FailedBeforeSubmit,
                                    NativeEvidenceSource::Caller,
                                    None,
                                    active_turn_id.map(str::to_string),
                                    Some(failure.message.clone()),
                                    "correction_encode_failed",
                                ).await;
                                let _ = reply.send(Err(failure));
                                continue;
                            }
                        };
                        let request_id = protocol_request_id(&request);
                        if let Err(failure) = write_json(&mut opened.stdin, &request).await {
                            let _ = broker.advance(
                                &dispatching.envelope.interaction_id,
                                NativeDeliveryPhase::SubmittedUnconfirmed,
                                NativeEvidenceSource::Caller,
                                request_id,
                                active_turn_id.map(str::to_string),
                                Some(failure.message.clone()),
                                "correction_write_uncertain",
                            ).await;
                            let _ = reply.send(Err(failure));
                            continue;
                        }
                        let submitted = broker.advance(
                            &dispatching.envelope.interaction_id,
                            NativeDeliveryPhase::SubmittedUnconfirmed,
                            NativeEvidenceSource::Caller,
                            request_id,
                            active_turn_id.map(str::to_string),
                            Some("correction written; awaiting provider acceptance".to_string()),
                            "correction_written",
                        ).await.unwrap_or(dispatching);
                        corrections.insert(
                            submitted.envelope.interaction_id.clone(),
                            ActiveCorrection { record: submitted, receipt: Some(reply) },
                        );
                    }
                    Some(SessionCommand::Cancel { interaction_id, reply }) => {
                        let current_id = active.as_ref().map(|turn| turn.record.envelope.interaction_id.as_str());
                        if current_id != Some(interaction_id.as_str()) {
                            let _ = reply.send(Err(error(
                                NativeDeliveryErrorCode::InvalidTransition,
                                format!("interaction {interaction_id} is not the active native turn"),
                                false,
                            )));
                            continue;
                        }
                        let cancel = match protocol.cancel_request(
                            &interaction_id,
                            opened.binding.provider_session_id.as_deref(),
                            active
                                .as_ref()
                                .and_then(|turn| turn.record.provider_turn_id.as_deref()),
                        ) {
                            Ok(cancel) => cancel,
                            Err(err) => {
                                let _ = reply.send(Err(error(
                                    NativeDeliveryErrorCode::CapabilityUnavailable,
                                    err.to_string(),
                                    true,
                                )));
                                continue;
                            }
                        };
                        let advanced = broker.advance(
                            &interaction_id,
                            NativeDeliveryPhase::CancelRequested,
                            NativeEvidenceSource::Caller,
                            protocol_request_id(&cancel),
                            active.as_ref().and_then(|turn| turn.record.provider_turn_id.clone()),
                            Some("provider cancellation requested".to_string()),
                            "cancel_requested",
                        ).await;
                        match advanced {
                            Ok(record) => {
                                if let Err(failure) = write_json(&mut opened.stdin, &cancel).await {
                                    let _ = reply.send(Err(failure));
                                } else {
                                    if let Some(turn) = active.as_mut() { turn.record = record.clone(); }
                                    let _ = reply.send(Ok(record));
                                }
                            }
                            Err(failure) => { let _ = reply.send(Err(failure)); }
                        }
                    }
                    Some(SessionCommand::Shutdown) | None => {
                        if let Some(turn) = active.as_mut() {
                            if let Some(receipt) = turn.receipt.take() {
                                let _ = receipt.send(Err(error(
                                    NativeDeliveryErrorCode::SubmittedUnconfirmed,
                                    "native session disposed after possible provider submission",
                                    true,
                                )));
                            }
                        }
                        fail_corrections(
                            &broker,
                            &mut corrections,
                            "native session disposed after possible correction submission",
                        ).await;
                        break;
                    }
                }
            }
            line = opened.lines.next_line() => {
                match line {
                    Ok(Some(line)) => {
                        let value = match serde_json::from_str::<serde_json::Value>(&line) {
                            Ok(value) => value,
                            Err(err) => {
                                if let Some(mut turn) = active.take() {
                                    let message = format!("malformed native provider framing: {err}");
                                    let _ = broker.advance(
                                        &turn.record.envelope.interaction_id,
                                        NativeDeliveryPhase::SubmittedUnconfirmed,
                                        NativeEvidenceSource::ProviderEvent,
                                        turn.record.provider_request_id.clone(),
                                        turn.record.provider_turn_id.clone(),
                                        Some(message.clone()),
                                        "malformed_provider_framing",
                                    ).await;
                                    if let Some(receipt) = turn.receipt.take() {
                                        let _ = receipt.send(Err(error(
                                            NativeDeliveryErrorCode::SubmittedUnconfirmed,
                                            message,
                                            true,
                                        )));
                                    }
                                }
                                fail_corrections(
                                    &broker,
                                    &mut corrections,
                                    "malformed framing after possible correction submission",
                                ).await;
                                stop_native_runtime(&mut runtime).await;
                                continue;
                            }
                        };
                        if let Err(err) = answer_reverse_request(protocol, &value, &spec.config, &mut opened.stdin).await {
                            if let Some(turn) = active.as_mut() { turn.record.detail = Some(err); }
                        }
                        let events = match protocol.parse_line(&line) {
                            Ok(events) => events,
                            Err(err) => {
                                if let Some(mut turn) = active.take() {
                                    let message = format!("invalid native provider event: {err}");
                                    let _ = broker.advance(
                                        &turn.record.envelope.interaction_id,
                                        NativeDeliveryPhase::SubmittedUnconfirmed,
                                        NativeEvidenceSource::ProviderEvent,
                                        turn.record.provider_request_id.clone(),
                                        turn.record.provider_turn_id.clone(),
                                        Some(message.clone()),
                                        "invalid_provider_event",
                                    ).await;
                                    if let Some(receipt) = turn.receipt.take() {
                                        let _ = receipt.send(Err(error(
                                            NativeDeliveryErrorCode::SubmittedUnconfirmed,
                                            message,
                                            true,
                                        )));
                                    }
                                }
                                fail_corrections(
                                    &broker,
                                    &mut corrections,
                                    "invalid event after possible correction submission",
                                ).await;
                                stop_native_runtime(&mut runtime).await;
                                continue;
                            },
                        };
                        for event in events {
                            let correction_id = event
                                .request_id
                                .as_deref()
                                .filter(|id| corrections.contains_key(*id))
                                .map(str::to_string);
                            if let Some(correction_id) = correction_id {
                                if let Some(correction) = corrections.get_mut(&correction_id) {
                                    apply_correction_event(
                                        &broker,
                                        &capabilities,
                                        opened,
                                        correction,
                                        event,
                                    ).await;
                                }
                                continue;
                            }
                            apply_protocol_event(
                                &broker,
                                &spec,
                                &capabilities,
                                opened,
                                active.as_mut().expect("active turn"),
                                event,
                            ).await;
                        }
                        let terminal = active.as_ref().is_some_and(|turn| turn.record.phase.is_terminal());
                        if terminal {
                            let mut turn = active.take().expect("terminal turn");
                            if let Some(receipt) = turn.receipt.take() {
                                let result = if turn.positive_start_seen {
                                    Ok(NativeDispatchReceipt {
                                        record: turn.record.clone(),
                                        binding: opened.binding.clone(),
                                        capabilities: capabilities.clone(),
                                    })
                                } else {
                                    Err(error(
                                        NativeDeliveryErrorCode::SubmittedUnconfirmed,
                                        "provider reached a terminal event without positive turn-start evidence",
                                        true,
                                    ))
                                };
                                let _ = receipt.send(result);
                            }
                            for (_, mut correction) in corrections.drain() {
                                if correction.record.phase == NativeDeliveryPhase::ProviderAccepted {
                                    if let Ok(record) = broker.advance(
                                        &correction.record.envelope.interaction_id,
                                        NativeDeliveryPhase::Completed,
                                        NativeEvidenceSource::ProviderEvent,
                                        correction.record.provider_request_id.clone(),
                                        turn.record.provider_turn_id.clone(),
                                        Some("corrected active turn completed".to_string()),
                                        "correction_turn_completed",
                                    ).await {
                                        correction.record = record;
                                    }
                                }
                                if let Some(receipt) = correction.receipt.take() {
                                    let _ = receipt.send(Err(error(
                                        NativeDeliveryErrorCode::SubmittedUnconfirmed,
                                        "active turn ended before correction acceptance was confirmed",
                                        true,
                                    )));
                                }
                            }
                        }
                    }
                    Ok(None) | Err(_) => {
                        if let Some(mut turn) = active.take() {
                            if let Some(receipt) = turn.receipt.take() {
                                let _ = receipt.send(Err(error(
                                    NativeDeliveryErrorCode::SubmittedUnconfirmed,
                                    "provider process ended after possible submission and before confirmation",
                                    true,
                                )));
                            }
                        }
                        fail_corrections(
                            &broker,
                            &mut corrections,
                            "provider process ended after possible correction submission",
                        ).await;
                        stop_native_runtime(&mut runtime).await;
                    }
                }
            }
        }
    }
    stop_native_runtime(&mut runtime).await;
}

/// A stopping registry slot cannot be released on a best-effort kill. Keep the
/// child handle until exit is observed, including EOF/write-failure recovery.
async fn stop_native_runtime(runtime: &mut Option<NativeRuntime>) {
    if let Some(opened) = runtime.as_mut() {
        loop {
            if matches!(opened.child.try_wait(), Ok(Some(_))) {
                break;
            }
            let _ = opened.child.start_kill();
            if matches!(
                tokio::time::timeout(std::time::Duration::from_secs(1), opened.child.wait()).await,
                Ok(Ok(_))
            ) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        }
    }
    runtime.take();
}

async fn fail_corrections(
    broker: &NativeDeliveryBroker,
    corrections: &mut HashMap<String, ActiveCorrection>,
    message: &str,
) {
    for (_, mut correction) in corrections.drain() {
        let _ = broker
            .advance(
                &correction.record.envelope.interaction_id,
                NativeDeliveryPhase::SubmittedUnconfirmed,
                NativeEvidenceSource::ProviderEvent,
                correction.record.provider_request_id.clone(),
                correction.record.provider_turn_id.clone(),
                Some(message.to_string()),
                "correction_session_lost",
            )
            .await;
        if let Some(receipt) = correction.receipt.take() {
            let _ = receipt.send(Err(error(
                NativeDeliveryErrorCode::SubmittedUnconfirmed,
                message,
                true,
            )));
        }
    }
}

async fn apply_protocol_event(
    broker: &NativeDeliveryBroker,
    spec: &NativeSessionSpec,
    capabilities: &NativeTransportCapabilities,
    runtime: &mut NativeRuntime,
    active: &mut ActiveTurn,
    event: NativeProtocolEvent,
) {
    if let Some(session_id) = event.provider_session_id.as_deref() {
        if !session_id.trim().is_empty() {
            runtime.binding.provider_session_id = Some(session_id.to_string());
            runtime.binding.observed_at = now();
            let _ = wardian_core::db::upsert_native_session_binding(&runtime.binding);
        }
    }
    if !event_matches_active(&event, active) {
        return;
    }
    if matches!(
        event.kind,
        NativeProtocolEventKind::Progress | NativeProtocolEventKind::TurnStarted
    ) {
        if let Some(text) = event.text.as_deref() {
            if event.cumulative_text {
                // The provider re-sends the whole answer on every update, so
                // appending would duplicate it.
                active.response_text.clear();
                active.response_text.push_str(text);
            } else {
                active.response_text.push_str(text);
            }
        }
    }
    let Some(mut next) = event.delivery_phase() else {
        return;
    };
    if active.record.phase == NativeDeliveryPhase::CancelRequested
        && matches!(
            next,
            NativeDeliveryPhase::Completed | NativeDeliveryPhase::Failed
        )
    {
        next = NativeDeliveryPhase::Cancelled;
    }
    let detail = event
        .detail
        .clone()
        .or(event.text.clone())
        .or_else(|| (!active.response_text.is_empty()).then(|| active.response_text.clone()));
    let key = format!(
        "provider:{:?}:{}:{}",
        event.kind,
        event.request_id.as_deref().unwrap_or(""),
        event.provider_turn_id.as_deref().unwrap_or("")
    );
    if let Ok(record) = broker
        .advance(
            &active.record.envelope.interaction_id,
            next,
            NativeEvidenceSource::ProviderEvent,
            event.request_id.clone(),
            event.provider_turn_id.clone(),
            detail,
            &key,
        )
        .await
    {
        active.record = record;
    }
    if event.kind == NativeProtocolEventKind::TurnStarted {
        active.positive_start_seen = true;
        if let Some(receipt) = active.receipt.take() {
            let _ = receipt.send(Ok(NativeDispatchReceipt {
                record: active.record.clone(),
                binding: runtime.binding.clone(),
                capabilities: capabilities.clone(),
            }));
        }
    }
    let _ = spec;
}

async fn apply_correction_event(
    broker: &NativeDeliveryBroker,
    capabilities: &NativeTransportCapabilities,
    runtime: &NativeRuntime,
    correction: &mut ActiveCorrection,
    event: NativeProtocolEvent,
) {
    let next = match event.kind {
        NativeProtocolEventKind::ProviderAccepted | NativeProtocolEventKind::TurnStarted => {
            NativeDeliveryPhase::ProviderAccepted
        }
        NativeProtocolEventKind::TurnFailed | NativeProtocolEventKind::ProtocolError => {
            NativeDeliveryPhase::Failed
        }
        _ => return,
    };
    let key = format!(
        "correction:{:?}:{}",
        event.kind,
        event.request_id.as_deref().unwrap_or_default()
    );
    let Ok(record) = broker
        .advance(
            &correction.record.envelope.interaction_id,
            next,
            NativeEvidenceSource::ProviderEvent,
            event.request_id,
            event
                .provider_turn_id
                .or_else(|| correction.record.provider_turn_id.clone()),
            event.detail.or(event.text),
            &key,
        )
        .await
    else {
        return;
    };
    correction.record = record.clone();
    if let Some(receipt) = correction.receipt.take() {
        let result = if next == NativeDeliveryPhase::ProviderAccepted {
            Ok(NativeDispatchReceipt {
                record,
                binding: runtime.binding.clone(),
                capabilities: capabilities.clone(),
            })
        } else {
            Err(error(
                NativeDeliveryErrorCode::TransportUnavailable,
                "provider rejected invalidate-premise correction",
                true,
            ))
        };
        let _ = receipt.send(result);
    }
}

fn event_matches_active(event: &NativeProtocolEvent, active: &ActiveTurn) -> bool {
    let Some(request_id) = event.request_id.as_deref() else {
        return true;
    };
    request_id == active.record.envelope.interaction_id
        || request_id == active.record.envelope.message_id
        || active.record.provider_turn_id.as_deref() == Some(request_id)
        || request_id.starts_with("cancel:")
}

async fn start_runtime(
    spec: &NativeSessionSpec,
    protocol: NativeProviderProtocol,
    capabilities: &NativeTransportCapabilities,
) -> Result<NativeRuntime, NativeBrokerError> {
    let prior = wardian_core::db::latest_native_session_binding(&spec.target_agent_id)
        .map_err(db_error)?
        .filter(|binding| binding.provider == spec.provider);
    let (mut command, memory_capability) = native_command(spec, protocol, prior.as_ref())?;
    let mut child = command.spawn().map_err(|err| {
        error(
            NativeDeliveryErrorCode::TransportUnavailable,
            format!("failed to start {} native transport: {err}", spec.provider),
            false,
        )
    })?;
    let stdin = child.stdin.take().ok_or_else(|| {
        error(
            NativeDeliveryErrorCode::TransportUnavailable,
            "native provider process did not expose stdin",
            false,
        )
    })?;
    let stdout = child.stdout.take().ok_or_else(|| {
        error(
            NativeDeliveryErrorCode::TransportUnavailable,
            "native provider process did not expose stdout",
            false,
        )
    })?;
    let stderr_tail = Arc::new(Mutex::new(VecDeque::with_capacity(STDERR_TAIL_BYTES)));
    if let Some(mut stderr) = child.stderr.take() {
        let stderr_tail = Arc::clone(&stderr_tail);
        tokio::spawn(async move {
            let mut buffer = [0_u8; 1024];
            while let Ok(read) = stderr.read(&mut buffer).await {
                if read == 0 {
                    break;
                }
                let mut tail = stderr_tail.lock().await;
                tail.extend(buffer[..read].iter().copied());
                while tail.len() > STDERR_TAIL_BYTES {
                    tail.pop_front();
                }
            }
        });
    }
    let provider_session_id = prior
        .as_ref()
        .and_then(|binding| binding.provider_session_id.clone())
        .or_else(|| configured_provider_session_id(protocol, &spec.config));
    let mut runtime = NativeRuntime {
        child,
        stdin,
        lines: BufReader::new(stdout).lines(),
        stderr_tail,
        binding: NativeSessionBinding {
            target_agent_id: spec.target_agent_id.clone(),
            generation: spec.generation,
            provider: protocol.provider().to_string(),
            transport: protocol.transport().to_string(),
            provider_session_id,
            capabilities: capabilities.clone(),
            observed_at: now(),
        },
        _memory_capability: memory_capability,
    };
    for request in protocol.bootstrap_requests(
        &spec.target_agent_id,
        &spec.workspace.to_string_lossy(),
        runtime.binding.provider_session_id.as_deref(),
    ) {
        let is_opencode_load = protocol == NativeProviderProtocol::OpenCodeAcp
            && request.get("method").and_then(|value| value.as_str()) == Some("session/load");
        if let Err(failure) =
            apply_bootstrap_request(&mut runtime, protocol, &spec.provider, &request).await
        {
            if !is_opencode_load {
                return Err(failure);
            }
            let replacement = serde_json::json!({
                "jsonrpc": "2.0",
                "id": request.get("id").cloned().unwrap_or(serde_json::Value::String(format!("wardian:session:{}", spec.target_agent_id))),
                "method": "session/new",
                "params": {"cwd": spec.workspace.to_string_lossy(), "mcpServers": []}
            });
            runtime.binding.provider_session_id = None;
            apply_bootstrap_request(&mut runtime, protocol, &spec.provider, &replacement).await?;
        }
    }
    // OpenCode's ACP command takes no model or agent flags, so the operator's
    // selections are applied over the protocol once the session exists.
    // Skipping either would silently use the provider's defaults.
    if protocol == NativeProviderProtocol::OpenCodeAcp {
        if let (Some(session_id), Some(model)) = (
            runtime.binding.provider_session_id.clone(),
            spec.config
                .model
                .as_deref()
                .map(str::trim)
                .filter(|model| !model.is_empty()),
        ) {
            let request = serde_json::json!({
                "jsonrpc": "2.0",
                "id": format!("wardian:model:{}", spec.target_agent_id),
                "method": "session/set_model",
                "params": {"sessionId": session_id, "modelId": model}
            });
            apply_bootstrap_request(&mut runtime, protocol, &spec.provider, &request).await?;
        }
        if let Some(agent) = spec
            .config
            .opencode_config()
            .agent
            .as_deref()
            .map(str::trim)
            .filter(|agent| !agent.is_empty())
        {
            let session_id = runtime
                .binding
                .provider_session_id
                .as_deref()
                .ok_or_else(|| {
                    error(
                        NativeDeliveryErrorCode::TransportUnavailable,
                        "OpenCode ACP did not bind a session before configured agent selection",
                        false,
                    )
                })?;
            let request = protocol
                .set_session_mode_request(
                    &format!("wardian:agent:{}", spec.target_agent_id),
                    Some(session_id),
                    agent,
                )
                .map_err(|failure| {
                    error(
                        NativeDeliveryErrorCode::TransportUnavailable,
                        format!("OpenCode ACP agent selection could not be prepared: {failure}"),
                        false,
                    )
                })?;
            apply_bootstrap_request(&mut runtime, protocol, &spec.provider, &request).await?;
        }
    }
    runtime.binding.observed_at = now();
    wardian_core::db::upsert_native_session_binding(&runtime.binding).map_err(db_error)?;
    Ok(runtime)
}

async fn apply_bootstrap_request(
    runtime: &mut NativeRuntime,
    protocol: NativeProviderProtocol,
    provider: &str,
    request: &serde_json::Value,
) -> Result<(), NativeBrokerError> {
    let request_id = protocol_request_id(request);
    write_json(&mut runtime.stdin, request).await?;
    let response = tokio::time::timeout(BOOTSTRAP_TIMEOUT, async {
        loop {
            let line = runtime
                .lines
                .next_line()
                .await
                .map_err(|err| err.to_string())?
                .ok_or_else(|| "provider exited during native bootstrap".to_string())?;
            let value: serde_json::Value =
                serde_json::from_str(&line).map_err(|err| err.to_string())?;
            for event in protocol.parse_line(&line).map_err(|err| err.to_string())? {
                if let Some(session_id) = event.provider_session_id {
                    runtime.binding.provider_session_id = Some(session_id);
                }
            }
            if protocol_request_id(&value) == request_id {
                if let Some(err) = value.get("error") {
                    return Err(format!("native bootstrap request failed: {err}"));
                }
                return Ok(());
            }
        }
    })
    .await
    .map_err(|_| {
        error(
            NativeDeliveryErrorCode::TransportUnavailable,
            // Name the stage. A generic provider label cannot distinguish a
            // stall in `initialize` from one in `thread/start` or
            // `thread/resume`, and those have entirely different causes.
            format!(
                "{provider} native bootstrap timed out during {} after {}s",
                bootstrap_stage(request),
                BOOTSTRAP_TIMEOUT.as_secs()
            ),
            false,
        )
    })?;
    match response {
        Ok(()) => Ok(()),
        Err(message) => {
            let detail = bootstrap_failure_detail(runtime, request, &message).await;
            Err(error(
                NativeDeliveryErrorCode::TransportUnavailable,
                detail,
                false,
            ))
        }
    }
}

fn configured_provider_session_id(
    protocol: NativeProviderProtocol,
    config: &AgentConfig,
) -> Option<String> {
    if !matches!(
        protocol,
        NativeProviderProtocol::CodexAppServer | NativeProviderProtocol::OpenCodeAcp
    ) {
        return None;
    }
    config
        .resume_session
        .as_deref()
        .map(str::trim)
        .filter(|session_id| !session_id.is_empty())
        .map(str::to_string)
}

async fn bootstrap_failure_detail(
    runtime: &mut NativeRuntime,
    request: &serde_json::Value,
    message: &str,
) -> String {
    // Give the stderr reader a scheduling opportunity after process exit.
    tokio::task::yield_now().await;
    let request_name = bootstrap_stage(request);
    let status = runtime
        .child
        .try_wait()
        .ok()
        .flatten()
        .map(|status| format!("; exit_status={status}"))
        .unwrap_or_default();
    let stderr = {
        let tail = runtime.stderr_tail.lock().await;
        String::from_utf8_lossy(&tail.iter().copied().collect::<Vec<_>>()).to_string()
    };
    let stderr = bounded_diagnostic(&stderr);
    if stderr.is_empty() {
        format!("{message} for {request_name}{status}")
    } else {
        format!("{message} for {request_name}{status}; stderr={stderr}")
    }
}

/// The protocol stage a bootstrap request represents, for diagnostics.
///
/// Both JSON-RPC style requests (`method`) and Pi's typed envelopes (`type`)
/// are covered; anything unrecognized reports `bootstrap` rather than leaking
/// request content into an error message.
fn bootstrap_stage(request: &serde_json::Value) -> &str {
    request
        .get("method")
        .or_else(|| request.get("type"))
        .and_then(|value| value.as_str())
        .unwrap_or("bootstrap")
}

fn bounded_diagnostic(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(1024)
        .collect()
}

fn native_command(
    spec: &NativeSessionSpec,
    protocol: NativeProviderProtocol,
    prior: Option<&NativeSessionBinding>,
) -> Result<(Command, Option<wardian_core::memory::MemoryCapabilityLease>), NativeBrokerError> {
    #[cfg(test)]
    if let Some(script) = std::env::var_os("WARDIAN_NATIVE_TEST_SCRIPT") {
        let mut command = Command::new("node");
        command
            .arg(script)
            .current_dir(&spec.workspace)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let memory_enabled = crate::utils::memory_feature_enabled();
        let memory_capability = crate::manager::headless::apply_headless_identity_env(
            &mut command,
            &spec.target_agent_id,
            Some(&spec.target_agent_id),
            memory_enabled,
        );
        return Ok((command, memory_capability));
    }
    let provider = ProviderFactory::resolve(&spec.provider)
        .map_err(|message| error(NativeDeliveryErrorCode::UnsupportedProvider, message, false))?;
    let (program, mut args) = provider.get_executable();
    let mut config = spec.config.clone();
    if let Some(session_id) = prior.and_then(|binding| binding.provider_session_id.clone()) {
        config.resume_session = Some(session_id);
    }
    if protocol == NativeProviderProtocol::ClaudeStreamJson {
        reconcile_claude_native_session(&mut config, claude_session_file_exists);
    }
    if protocol == NativeProviderProtocol::PiRpc {
        let session_dir = PiProvider::session_dir(&config.session_id);
        reconcile_pi_native_session(&mut config, |session_id| {
            session_dir
                .as_deref()
                .and_then(|dir| PiProvider::session_file(dir, session_id))
                .is_some()
        });
    }
    let is_resume = config
        .resume_session
        .as_deref()
        .is_some_and(|id| !id.trim().is_empty());
    match protocol {
        NativeProviderProtocol::ClaudeStreamJson => {
            args.extend(provider.get_spawn_args(&config, is_resume));
            push_flag(&mut args, "--print");
            push_flag(&mut args, "--include-partial-messages");
            push_flag(&mut args, "--replay-user-messages");
            push_flag_value(&mut args, "--permission-prompt-tool", "stdio");
        }
        NativeProviderProtocol::CodexAppServer => {
            args.extend(codex_app_server_args(&config)?);
        }
        NativeProviderProtocol::AntigravityStreamJson => {
            args.extend(provider.get_spawn_args(&config, is_resume));
            // `agy --print` accepts an optional prompt value. A bare flag consumes the
            // following `--input-format` token as that prompt, so bind the empty prompt
            // explicitly for a persistent stdin-driven stream.
            push_flag(&mut args, "--print=");
            push_flag_value(&mut args, "--input-format", "stream-json");
            push_flag_value(&mut args, "--output-format", "stream-json");
        }
        NativeProviderProtocol::OpenCodeAcp => {
            args.extend(opencode_acp_args(&config));
        }
        NativeProviderProtocol::PiRpc => {
            let pi_args =
                remove_flag_value(provider.get_spawn_args(&config, is_resume), "--tui-mode");
            args.extend(pi_args);
            push_flag_value(&mut args, "--mode", "rpc");
        }
    }
    let mut command = Command::new(program);
    command
        .args(args)
        .current_dir(&spec.workspace)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    crate::manager::apply_managed_cli_path_to_process(&mut command);
    crate::manager::apply_process_provider_runtime_env(&spec.provider, &mut command).map_err(
        |message| {
            error(
                NativeDeliveryErrorCode::TransportUnavailable,
                message,
                false,
            )
        },
    )?;
    let memory_enabled = crate::utils::memory_feature_enabled();
    let memory_capability = crate::manager::headless::apply_headless_identity_env(
        &mut command,
        &spec.target_agent_id,
        Some(&spec.target_agent_id),
        memory_enabled,
    );
    for (key, value) in crate::manager::worktree_build_env(&config) {
        command.env(key, value);
    }
    if protocol == NativeProviderProtocol::CodexAppServer {
        if let Some(root) = habitat_root(&config) {
            command.env("CODEX_HOME", crate::utils::fs::habitat_codex_home(&root));
        }
    }
    if protocol == NativeProviderProtocol::OpenCodeAcp {
        if let Ok(values) = crate::manager::opencode::opencode_env(
            &spec.workspace,
            &config.agent_class,
            prior.and_then(|binding| binding.provider_session_id.as_deref()),
            Some(&config),
        ) {
            for (key, value) in values {
                command.env(key, value);
            }
        }
    }
    Ok((command, memory_capability))
}

fn reconcile_claude_native_session(
    config: &mut AgentConfig,
    session_exists: impl FnOnce(&str) -> bool,
) {
    let Some(resume_session) = config
        .resume_session
        .as_deref()
        .map(str::trim)
        .filter(|session_id| !session_id.is_empty())
        .map(str::to_string)
    else {
        return;
    };
    if session_exists(&resume_session) {
        return;
    }

    // Claude emits its init event before the first transcript is durable. If
    // Wardian pauses during that window, the persisted provider UUID is valid
    // for a new session but `--resume` fails with "No conversation found".
    // Preserve the provider UUID and start it with `--session-id` instead.
    config.resume_session = None;
    config.fresh_provider_session_id = Some(resume_session);
}

fn reconcile_pi_native_session(
    config: &mut AgentConfig,
    session_exists: impl FnOnce(&str) -> bool,
) {
    let Some(resume_session) = config
        .resume_session
        .as_deref()
        .map(str::trim)
        .filter(|session_id| !session_id.is_empty())
        .map(str::to_string)
    else {
        return;
    };
    if session_exists(&resume_session) {
        return;
    }

    // Pi's `--session` only resumes an existing transcript, while
    // `--session-id` preserves the provider UUID and creates it when absent.
    // A freshly paused Wardian agent may have an ID before Pi has flushed the
    // first transcript, so retain the identity and switch launch mode.
    config.resume_session = None;
    config.fresh_provider_session_id = Some(resume_session);
}

fn claude_session_file_exists(session_id: &str) -> bool {
    if session_id.is_empty()
        || session_id.contains('/')
        || session_id.contains('\\')
        || session_id.contains("..")
    {
        return false;
    }
    let Some(projects_root) = dirs::home_dir().map(|home| home.join(".claude").join("projects"))
    else {
        return false;
    };
    let Ok(projects) = std::fs::read_dir(projects_root) else {
        return false;
    };
    let transcript_name = format!("{session_id}.jsonl");
    projects
        .flatten()
        .take(10_000)
        .any(|project| project.path().join(&transcript_name).is_file())
}

async fn probe_protocol_version(
    spec: &NativeSessionSpec,
    protocol: NativeProviderProtocol,
) -> String {
    #[cfg(test)]
    if std::env::var_os("WARDIAN_NATIVE_TEST_SCRIPT").is_some() {
        return "test-fixture".to_string();
    }
    let Ok(provider) = ProviderFactory::resolve(&spec.provider) else {
        return "bootstrap-only".to_string();
    };
    let (program, args) = provider.get_executable();
    if let Some(version) = node_package_version(&program, &args).await {
        return version;
    }
    let mut command = Command::new(program);
    command
        .args(args)
        .arg("--version")
        .current_dir(&spec.workspace)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    crate::manager::apply_managed_cli_path_to_process(&mut command);
    let _ = crate::manager::apply_process_provider_runtime_env(&spec.provider, &mut command);
    let output = match tokio::time::timeout(PROTOCOL_PROBE_TIMEOUT, command.output()).await {
        Ok(Ok(output)) => output,
        _ => return "bootstrap-only".to_string(),
    };
    let version = if output.stdout.is_empty() {
        String::from_utf8_lossy(&output.stderr)
    } else {
        String::from_utf8_lossy(&output.stdout)
    };
    let version = version.lines().next().unwrap_or_default().trim();
    if version.is_empty() {
        format!("{}:bootstrap-only", protocol.transport())
    } else {
        version.to_string()
    }
}

async fn node_package_version(program: &str, args: &[String]) -> Option<String> {
    let executable = std::path::Path::new(program)
        .file_name()
        .and_then(|name| name.to_str())?;
    if !executable.eq_ignore_ascii_case("node") && !executable.eq_ignore_ascii_case("node.exe") {
        return None;
    }
    let script = args.first().map(PathBuf::from)?;
    if !matches!(
        script.extension().and_then(|extension| extension.to_str()),
        Some("js" | "cjs" | "mjs")
    ) {
        return None;
    }
    for parent in script.ancestors().skip(1).take(8) {
        let package = parent.join("package.json");
        let Ok(contents) = tokio::fs::read_to_string(package).await else {
            continue;
        };
        let Ok(metadata) = serde_json::from_str::<serde_json::Value>(&contents) else {
            continue;
        };
        if let Some(version) = metadata
            .get("version")
            .and_then(|version| version.as_str())
            .map(str::trim)
            .filter(|version| !version.is_empty())
        {
            return Some(version.to_string());
        }
    }
    None
}

async fn answer_reverse_request(
    protocol: NativeProviderProtocol,
    value: &serde_json::Value,
    config: &AgentConfig,
    stdin: &mut ChildStdin,
) -> Result<(), String> {
    if protocol == NativeProviderProtocol::ClaudeStreamJson
        && value.get("type").and_then(|value| value.as_str()) == Some("control_request")
        && value
            .pointer("/request/subtype")
            .and_then(|value| value.as_str())
            == Some("can_use_tool")
    {
        let request_id = value
            .get("request_id")
            .and_then(|value| value.as_str())
            .ok_or_else(|| "Claude permission request has no request_id".to_string())?;
        let tool_name = value
            .pointer("/request/tool_name")
            .and_then(|value| value.as_str())
            .unwrap_or_default();
        let claude = config.claude_config();
        let explicitly_denied = claude
            .disallowed_tools
            .as_ref()
            .is_some_and(|tools| tools.iter().any(|tool| tool == tool_name));
        let explicitly_allowed = claude
            .allowed_tools
            .as_ref()
            .is_some_and(|tools| tools.iter().any(|tool| tool == tool_name));
        let allow = !explicitly_denied
            && (explicitly_allowed
                || claude
                    .permission_mode
                    .as_deref()
                    .is_none_or(|mode| mode == "bypassPermissions"));
        let decision = if allow {
            serde_json::json!({
                "type": "control_response",
                "response": {
                    "subtype": "success",
                    "request_id": request_id,
                    "response": {
                        "behavior": "allow",
                        "updatedInput": value.pointer("/request/input").cloned().unwrap_or(serde_json::Value::Null),
                        "updatedPermissions": []
                    }
                }
            })
        } else {
            serde_json::json!({
                "type": "control_response",
                "response": {
                    "subtype": "success",
                    "request_id": request_id,
                    "response": {
                        "behavior": "deny",
                        "message": "Wardian native delivery did not authorize this tool",
                        "interrupt": false
                    }
                }
            })
        };
        return write_json(stdin, &decision)
            .await
            .map_err(|failure| failure.message);
    }
    let Some(id) = value.get("id") else {
        return Ok(());
    };
    let method = value
        .get("method")
        .and_then(|value| value.as_str())
        .unwrap_or_default();
    let response = match protocol {
        NativeProviderProtocol::CodexAppServer
            if method.contains("requestApproval") || method.contains("request_approval") =>
        {
            let allow = config.codex_config().approval_policy.as_deref() == Some("never")
                || config.codex_config().full_auto.unwrap_or(false);
            serde_json::json!({"id": id, "result": {"decision": if allow { "accept" } else { "decline" }}})
        }
        NativeProviderProtocol::OpenCodeAcp if method == "session/request_permission" => {
            let options = value
                .pointer("/params/options")
                .and_then(|value| value.as_array())
                .cloned()
                .unwrap_or_default();
            let auto = config.opencode_config().auto.unwrap_or(false);
            let selected = options.iter().find(|option| {
                let kind = option
                    .get("kind")
                    .and_then(|value| value.as_str())
                    .unwrap_or_default();
                if auto {
                    kind.starts_with("allow")
                } else {
                    kind.starts_with("reject")
                }
            });
            match selected
                .and_then(|option| option.get("optionId").or_else(|| option.get("option_id")))
            {
                Some(option_id) => {
                    serde_json::json!({"jsonrpc":"2.0", "id": id, "result": {"outcome": {"outcome":"selected", "optionId": option_id}}})
                }
                None => {
                    serde_json::json!({"jsonrpc":"2.0", "id": id, "result": {"outcome": {"outcome":"cancelled"}}})
                }
            }
        }
        _ => return Ok(()),
    };
    write_json(stdin, &response)
        .await
        .map_err(|failure| failure.message)
}

async fn write_json(
    stdin: &mut ChildStdin,
    value: &serde_json::Value,
) -> Result<(), NativeBrokerError> {
    let mut bytes = serde_json::to_vec(value).map_err(|err| {
        error(
            NativeDeliveryErrorCode::FailedBeforeSubmit,
            format!("failed to encode native provider request: {err}"),
            false,
        )
    })?;
    bytes.push(b'\n');
    stdin.write_all(&bytes).await.map_err(|err| {
        error(
            NativeDeliveryErrorCode::SubmittedUnconfirmed,
            format!("native provider write failed after bytes may have crossed: {err}"),
            true,
        )
    })?;
    stdin.flush().await.map_err(|err| {
        error(
            NativeDeliveryErrorCode::SubmittedUnconfirmed,
            format!("native provider flush failed after bytes may have crossed: {err}"),
            true,
        )
    })
}

fn canonical_hash(request: &NativeDeliveryAdmission) -> String {
    let canonical = serde_json::json!({
        "target_agent_id": request.target_agent_id,
        "sender_agent_id": request.sender_agent_id,
        "provider": request.provider,
        "generation": request.generation,
        "operation": request.operation,
        "parent_interaction_id": request.parent_interaction_id,
        "deadline_at": request.deadline_at,
        "body": request.body,
    });
    format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&canonical).unwrap_or_default())
    )
}

fn append_evidence_with_key(
    evidence: &NativeDeliveryEvidence,
    key: &str,
) -> Result<(), NativeBrokerError> {
    let event_key = format!(
        "{}:{}:{}:{}:{}",
        evidence.interaction_id,
        key,
        evidence.provider_request_id.as_deref().unwrap_or(""),
        evidence.provider_turn_id.as_deref().unwrap_or(""),
        evidence.detail.as_deref().unwrap_or("")
    );
    let event_id = format!("nev_{:x}", Sha256::digest(event_key.as_bytes()));
    wardian_core::db::append_native_delivery_evidence(&event_id, evidence)
        .map(|_| ())
        .map_err(db_error)
}

fn protocol_request_id(value: &serde_json::Value) -> Option<String> {
    value.get("id").and_then(|id| match id {
        serde_json::Value::String(value) => Some(value.clone()),
        serde_json::Value::Number(value) => Some(value.to_string()),
        _ => None,
    })
}

fn deadline_has_passed(deadline: Option<&str>) -> bool {
    deadline
        .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
        .is_some_and(|deadline| deadline.with_timezone(&Utc) <= Utc::now())
}

fn phase_rank(phase: NativeDeliveryPhase) -> u8 {
    match phase {
        NativeDeliveryPhase::Queued => 0,
        NativeDeliveryPhase::Dispatching => 1,
        NativeDeliveryPhase::SubmittedUnconfirmed => 2,
        NativeDeliveryPhase::ProviderAccepted => 3,
        NativeDeliveryPhase::TurnStarted => 4,
        NativeDeliveryPhase::CancelRequested => 5,
        NativeDeliveryPhase::Completed
        | NativeDeliveryPhase::FailedBeforeSubmit
        | NativeDeliveryPhase::Failed
        | NativeDeliveryPhase::Cancelled
        | NativeDeliveryPhase::Expired
        | NativeDeliveryPhase::StaleGeneration
        | NativeDeliveryPhase::Withdrawn
        | NativeDeliveryPhase::Superseded => 6,
    }
}

fn habitat_root(config: &AgentConfig) -> Option<PathBuf> {
    if let Some(wardian_home) = crate::utils::fs::get_wardian_home() {
        let habitat = wardian_home
            .join("agents")
            .join(config.session_id.trim())
            .join("habitat");
        if !config.session_id.trim().is_empty() && habitat.is_dir() {
            return Some(habitat);
        }
    }

    config
        .system_include_directories
        .as_ref()?
        .iter()
        .find_map(|include| {
            let include = PathBuf::from(include);
            let projected_habitat = include.join("habitat");
            if projected_habitat.is_dir() {
                Some(projected_habitat)
            } else if include.join(".codex").exists() || include.join(".claude").exists() {
                Some(include)
            } else {
                None
            }
        })
}

/// Arguments for `opencode acp`.
///
/// `acp` is its own command and accepts only `--print-logs`, `--log-level`,
/// `--pure`, `--port` and `--hostname`. Reusing the interactive spawn args put
/// `--model` (and `--agent`, `--auto`, `--session`) in front of it, so the CLI
/// rejected the unknown option, printed its usage and exited before the
/// initialize handshake could run. The model is not dropped: it is applied over
/// the protocol with `session/set_model` and `session/set_mode` once the
/// session exists.
fn opencode_acp_args(config: &AgentConfig) -> Vec<String> {
    let mut args = Vec::new();
    if config.debug.unwrap_or(false) {
        args.push("--print-logs".to_string());
    }
    args.push("acp".to_string());
    args
}

fn codex_app_server_args(config: &AgentConfig) -> Result<Vec<String>, NativeBrokerError> {
    CodexProvider::new()
        .shared_server_args(config)
        .map_err(|message| error(NativeDeliveryErrorCode::UnsupportedProvider, message, false))
}

fn push_flag(args: &mut Vec<String>, flag: &str) {
    if !args.iter().any(|value| value == flag) {
        args.push(flag.to_string());
    }
}

fn push_flag_value(args: &mut Vec<String>, flag: &str, value: &str) {
    if !args.iter().any(|candidate| candidate == flag) {
        args.push(flag.to_string());
        args.push(value.to_string());
    }
}

fn remove_flag_value(args: Vec<String>, flag: &str) -> Vec<String> {
    let mut result = Vec::with_capacity(args.len());
    let mut iter = args.into_iter();
    while let Some(value) = iter.next() {
        if value == flag {
            let _ = iter.next();
        } else {
            result.push(value);
        }
    }
    result
}

fn now() -> String {
    Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

fn db_error(error: impl std::fmt::Display) -> NativeBrokerError {
    NativeBrokerError {
        code: NativeDeliveryErrorCode::TransportUnavailable,
        message: format!("native delivery persistence failed: {error}"),
        provider_boundary_crossed: false,
    }
}

fn error(
    code: NativeDeliveryErrorCode,
    message: impl Into<String>,
    provider_boundary_crossed: bool,
) -> NativeBrokerError {
    NativeBrokerError {
        code,
        message: message.into(),
        provider_boundary_crossed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct NativeTestScriptGuard;

    impl Drop for NativeTestScriptGuard {
        fn drop(&mut self) {
            unsafe {
                std::env::remove_var("WARDIAN_NATIVE_TEST_SCRIPT");
                std::env::remove_var("WARDIAN_NATIVE_TEST_LOG");
            }
        }
    }

    struct NativeHomeGuard {
        previous_home: Option<std::ffi::OsString>,
    }

    impl NativeHomeGuard {
        fn set(home: &std::path::Path) -> Self {
            let previous_home = std::env::var_os("WARDIAN_HOME");
            std::env::set_var("WARDIAN_HOME", home);
            Self { previous_home }
        }
    }

    impl Drop for NativeHomeGuard {
        fn drop(&mut self) {
            match self.previous_home.take() {
                Some(value) => std::env::set_var("WARDIAN_HOME", value),
                None => std::env::remove_var("WARDIAN_HOME"),
            }
        }
    }

    fn test_admission(id: &str, key: &str, body: &str) -> NativeDeliveryAdmission {
        NativeDeliveryAdmission {
            interaction_id: id.to_string(),
            message_id: format!("message-{id}"),
            target_agent_id: "agent-native-test".to_string(),
            sender_agent_id: Some("orchestrator-test".to_string()),
            provider: "claude".to_string(),
            generation: 1,
            operation: NativeMessageOperation::StartTurn,
            caller_idempotency_key: Some(key.to_string()),
            parent_interaction_id: None,
            deadline_at: None,
            body: body.to_string(),
        }
    }

    #[test]
    fn canonical_hash_ignores_server_generated_ids() {
        let request = NativeDeliveryAdmission {
            interaction_id: "one".into(),
            message_id: "message-one".into(),
            target_agent_id: "agent".into(),
            sender_agent_id: Some("orchestrator".into()),
            provider: "codex".into(),
            generation: 4,
            operation: NativeMessageOperation::StartTurn,
            caller_idempotency_key: Some("key".into()),
            parent_interaction_id: None,
            deadline_at: None,
            body: "review".into(),
        };
        let mut retry = request.clone();
        retry.interaction_id = "two".into();
        retry.message_id = "message-two".into();
        assert_eq!(canonical_hash(&request), canonical_hash(&retry));
    }

    /// `opencode acp` exits 1 and prints its usage when an interactive flag
    /// such as `--model` precedes it, which is what stopped ACP negotiation
    /// before initialize. The command must carry nothing it does not accept.
    #[test]
    fn opencode_acp_command_carries_no_interactive_flags() {
        let mut config = AgentConfig {
            provider: "opencode".to_string(),
            model: Some("opencode/mimo-v2.5-free".to_string()),
            ..Default::default()
        };

        assert_eq!(opencode_acp_args(&config), vec!["acp".to_string()]);

        config.debug = Some(true);
        assert_eq!(
            opencode_acp_args(&config),
            vec!["--print-logs".to_string(), "acp".to_string()],
            "--print-logs is one of the flags acp does accept"
        );
    }

    #[test]
    fn codex_app_server_command_uses_config_override_for_model() {
        let config = AgentConfig {
            provider: "codex".into(),
            model: Some("gpt-5.6-luna".into()),
            provider_config: wardian_core::models::ProviderConfig::Codex(
                wardian_core::models::CodexProviderConfig {
                    reasoning_effort: Some("high".into()),
                    sandbox_mode: Some("workspace-write".into()),
                    approval_policy: Some("on-request".into()),
                    ..Default::default()
                },
            ),
            ..Default::default()
        };

        let args = codex_app_server_args(&config).expect("Codex app-server arguments");
        assert_eq!(args.first().map(String::as_str), Some("app-server"));
        assert!(args
            .windows(2)
            .any(|pair| { pair[0] == "-c" && pair[1] == "model=\"gpt-5.6-luna\"" }));
        assert!(args
            .windows(2)
            .any(|pair| { pair[0] == "-c" && pair[1] == "model_reasoning_effort=\"high\"" }));
        assert!(!args.iter().any(|arg| arg == "--model"));
    }

    /// A bootstrap stall in `initialize` and one in `thread/resume` have
    /// entirely different causes, so the recorded diagnostic must name which.
    #[test]
    fn bootstrap_stage_names_the_request_rather_than_the_provider() {
        assert_eq!(
            bootstrap_stage(&serde_json::json!({"id": "x", "method": "initialize"})),
            "initialize"
        );
        assert_eq!(
            bootstrap_stage(&serde_json::json!({"id": "x", "method": "thread/resume"})),
            "thread/resume"
        );
        assert_eq!(
            bootstrap_stage(&serde_json::json!({"id": "x", "method": "session/new"})),
            "session/new"
        );
        // Pi uses typed envelopes rather than a method name.
        assert_eq!(
            bootstrap_stage(&serde_json::json!({"id": "x", "type": "get_state"})),
            "get_state"
        );
        // Unrecognized shapes must not leak request content.
        assert_eq!(
            bootstrap_stage(&serde_json::json!({"id": "x", "params": {"secret": "value"}})),
            "bootstrap"
        );
    }

    #[test]
    fn flag_replacement_preserves_unrelated_provider_args() {
        assert_eq!(
            remove_flag_value(
                vec!["--tui-mode".into(), "regular".into(), "--offline".into()],
                "--tui-mode"
            ),
            vec!["--offline"]
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn opencode_agent_selection_failure_prevents_prompt_submission() {
        let _lock = crate::utils::wardian_test_env_lock_async().await;
        let temp = tempfile::tempdir().expect("native broker tempdir");
        wardian_core::db::init_db_at_path(&temp.path().join("state.db"))
            .expect("initialize native broker db");
        let script = temp.path().join("opencode-provider.cjs");
        let log = temp.path().join("opencode-bootstrap.log");
        std::fs::write(
            &script,
            r#"const fs = require('node:fs');
const readline = require('node:readline');
const input = readline.createInterface({ input: process.stdin });
input.on('line', (line) => {
  fs.appendFileSync(process.env.WARDIAN_NATIVE_TEST_LOG, line + '\n');
  const request = JSON.parse(line);
  let response;
  if (request.method === 'initialize') {
    response = { jsonrpc: '2.0', id: request.id, result: { protocolVersion: 1 } };
  } else if (request.method === 'session/new') {
    response = { jsonrpc: '2.0', id: request.id, result: { sessionId: 'ses-test' } };
  } else if (request.method === 'session/set_mode') {
    response = { jsonrpc: '2.0', id: request.id,
      error: { code: -32000, message: 'configured agent is unavailable' } };
  } else if (request.method === 'session/prompt') {
    fs.appendFileSync(process.env.WARDIAN_NATIVE_TEST_LOG, 'PROMPT_SUBMITTED\n');
    process.exit(17);
  }
  if (response) console.log(JSON.stringify(response));
});
"#,
        )
        .expect("write OpenCode provider fixture");
        unsafe {
            std::env::set_var("WARDIAN_NATIVE_TEST_SCRIPT", &script);
            std::env::set_var("WARDIAN_NATIVE_TEST_LOG", &log);
        }
        let _script_guard = NativeTestScriptGuard;

        let broker = Arc::new(NativeDeliveryBroker::new());
        let mut admission = test_admission("opencode-agent", "opencode-agent-key", "review");
        admission.provider = "opencode".into();
        let record = broker.admit(admission).await.expect("admit turn");
        let config = AgentConfig {
            provider: "opencode".into(),
            session_id: "agent-native-test".into(),
            folder: temp.path().display().to_string(),
            provider_config: wardian_core::models::ProviderConfig::OpenCode(
                wardian_core::models::OpenCodeProviderConfig {
                    agent: Some("reviewer".into()),
                    ..Default::default()
                },
            ),
            ..Default::default()
        };
        let failure = broker
            .dispatch(
                NativeSessionSpec {
                    target_agent_id: "agent-native-test".into(),
                    provider: "opencode".into(),
                    generation: 1,
                    workspace: temp.path().to_path_buf(),
                    config,
                },
                record,
            )
            .await
            .expect_err("selection failure must stop bootstrap");

        assert_eq!(failure.code, NativeDeliveryErrorCode::TransportUnavailable);
        assert_eq!(
            broker
                .get("opencode-agent")
                .expect("failed delivery record")
                .phase,
            NativeDeliveryPhase::FailedBeforeSubmit
        );
        let requests = std::fs::read_to_string(&log)
            .expect("bootstrap request log")
            .lines()
            .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
            .collect::<Vec<_>>();
        assert!(requests.iter().any(|request| {
            request["method"] == "session/set_mode" && request["params"]["modeId"] == "reviewer"
        }));
        assert!(!requests
            .iter()
            .any(|request| request["method"] == "session/prompt"));
        broker
            .dispose_agent("agent-native-test")
            .await
            .expect("dispose failed test session");
    }

    #[test]
    fn claude_missing_resume_transcript_reuses_the_id_as_a_fresh_session() {
        let mut config = AgentConfig {
            provider: "claude".to_string(),
            resume_session: Some("claude-session-1".to_string()),
            ..AgentConfig::default()
        };

        reconcile_claude_native_session(&mut config, |_| false);

        assert_eq!(config.resume_session, None);
        assert_eq!(
            config.fresh_provider_session_id.as_deref(),
            Some("claude-session-1")
        );
    }

    #[test]
    fn claude_durable_resume_transcript_keeps_resume_mode() {
        let mut config = AgentConfig {
            provider: "claude".to_string(),
            resume_session: Some("claude-session-1".to_string()),
            ..AgentConfig::default()
        };

        reconcile_claude_native_session(&mut config, |_| true);

        assert_eq!(config.resume_session.as_deref(), Some("claude-session-1"));
        assert_eq!(config.fresh_provider_session_id, None);
    }

    #[test]
    fn pi_missing_resume_transcript_reuses_the_id_as_a_fresh_session() {
        let mut config = AgentConfig {
            provider: "pi".to_string(),
            resume_session: Some("pi-session-1".to_string()),
            ..AgentConfig::default()
        };

        reconcile_pi_native_session(&mut config, |_| false);

        assert_eq!(config.resume_session, None);
        assert_eq!(
            config.fresh_provider_session_id.as_deref(),
            Some("pi-session-1")
        );
    }

    #[test]
    fn pi_durable_resume_transcript_keeps_resume_mode() {
        let mut config = AgentConfig {
            provider: "pi".to_string(),
            resume_session: Some("pi-session-1".to_string()),
            ..AgentConfig::default()
        };

        reconcile_pi_native_session(&mut config, |_| true);

        assert_eq!(config.resume_session.as_deref(), Some("pi-session-1"));
        assert_eq!(config.fresh_provider_session_id, None);
    }

    #[test]
    fn codex_bootstrap_uses_the_agent_resume_thread_before_a_broker_binding_exists() {
        let config = AgentConfig {
            provider: "codex".to_string(),
            resume_session: Some(" codex-thread-1 ".to_string()),
            ..AgentConfig::default()
        };

        assert_eq!(
            configured_provider_session_id(NativeProviderProtocol::CodexAppServer, &config)
                .as_deref(),
            Some("codex-thread-1")
        );
        assert_eq!(
            configured_provider_session_id(NativeProviderProtocol::ClaudeStreamJson, &config),
            None
        );
    }

    #[test]
    fn native_bootstrap_diagnostics_are_single_line_and_bounded() {
        let noisy = format!("first\nsecond {}", "x".repeat(2_000));
        let diagnostic = bounded_diagnostic(&noisy);
        assert!(diagnostic.starts_with("first second "));
        assert!(!diagnostic.contains('\n'));
        assert_eq!(diagnostic.chars().count(), 1024);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn native_runtime_retains_memory_lease_until_runtime_drop() {
        let _lock = crate::utils::wardian_test_env_lock_async().await;
        let temp = tempfile::tempdir().expect("native memory tempdir");
        wardian_core::db::init_db_at_path(&temp.path().join("state.db"))
            .expect("initialize native broker db");
        std::fs::create_dir_all(temp.path().join("settings")).expect("settings directory");
        std::fs::write(
            temp.path().join("settings/app.json"),
            r#"{"schema_version":2,"overrides":{"memory_enabled":true}}"#,
        )
        .expect("enable memory");
        let _home = NativeHomeGuard::set(temp.path());
        let script = temp.path().join("native-provider.cjs");
        std::fs::write(
            &script,
            r#"const readline = require('node:readline');
const input = readline.createInterface({ input: process.stdin });
input.on('line', (line) => {
  const request = JSON.parse(line);
  if (request.type === 'get_state') {
    console.log(JSON.stringify({ id: request.id, type: 'response', command: 'get_state', success: true, data: { sessionId: 'fixture-session' } }));
  }
});
"#,
        )
        .expect("provider fixture");
        unsafe { std::env::set_var("WARDIAN_NATIVE_TEST_SCRIPT", &script) };
        let _script_guard = NativeTestScriptGuard;

        let config = AgentConfig {
            provider: "pi".to_string(),
            session_id: "agent-native-test".to_string(),
            ..AgentConfig::default()
        };
        let spec = NativeSessionSpec {
            target_agent_id: "agent-native-test".to_string(),
            provider: "pi".to_string(),
            generation: 1,
            workspace: temp.path().to_path_buf(),
            config,
        };
        let capabilities = NativeProviderProtocol::PiRpc.capabilities("fixture");
        let runtime = start_runtime(&spec, NativeProviderProtocol::PiRpc, &capabilities)
            .await
            .expect("start native runtime");
        let token = runtime
            ._memory_capability
            .as_ref()
            .expect("enabled native runtime capability")
            .token()
            .to_string();
        let store = wardian_core::memory::MemoryStore::from_default_home().unwrap();
        assert!(store
            .validate_capability("agent-native-test", &token)
            .unwrap());

        drop(runtime);
        assert!(!store
            .validate_capability("agent-native-test", &token)
            .unwrap());
    }

    #[test]
    fn disabled_native_command_does_not_issue_a_memory_lease() {
        let _lock = crate::utils::wardian_test_env_lock();
        let temp = tempfile::tempdir().expect("native memory tempdir");
        std::fs::create_dir_all(temp.path().join("settings")).expect("settings directory");
        std::fs::write(
            temp.path().join("settings/app.json"),
            r#"{"schema_version":2,"overrides":{}}"#,
        )
        .expect("disable memory");
        let _home = NativeHomeGuard::set(temp.path());
        let script = temp.path().join("native-provider.cjs");
        std::fs::write(&script, "").expect("provider fixture");
        unsafe { std::env::set_var("WARDIAN_NATIVE_TEST_SCRIPT", &script) };
        let _script_guard = NativeTestScriptGuard;

        let config = AgentConfig {
            provider: "claude".to_string(),
            session_id: "agent-native-test".to_string(),
            ..AgentConfig::default()
        };
        let spec = NativeSessionSpec {
            target_agent_id: "agent-native-test".to_string(),
            provider: "claude".to_string(),
            generation: 1,
            workspace: temp.path().to_path_buf(),
            config,
        };
        let (_command, lease) =
            native_command(&spec, NativeProviderProtocol::ClaudeStreamJson, None)
                .expect("construct native command");
        let token = lease.as_ref().map(|lease| lease.token().to_string());
        assert!(token.is_none(), "disabled launch must not issue capability");
    }

    #[tokio::test]
    async fn node_provider_version_comes_from_nearest_package_metadata() {
        let temp = tempfile::tempdir().expect("node package tempdir");
        let package = temp.path().join("node_modules").join("provider-package");
        let script = package.join("dist").join("cli.js");
        std::fs::create_dir_all(script.parent().expect("script parent"))
            .expect("create package tree");
        std::fs::write(&script, "console.log('provider');").expect("write provider script");
        std::fs::write(
            package.join("package.json"),
            r#"{"name":"provider-package","version":"1.2.3"}"#,
        )
        .expect("write package metadata");

        let version = node_package_version(
            if cfg!(windows) { "node.exe" } else { "node" },
            &[script.display().to_string()],
        )
        .await;

        assert_eq!(version.as_deref(), Some("1.2.3"));
    }

    #[test]
    fn habitat_resolution_skips_common_and_selects_the_agent_projection() {
        let temp = tempfile::tempdir().expect("habitat resolution tempdir");
        let common = temp.path().join("common");
        let agent_root = temp.path().join("agents").join("agent-native-test");
        let habitat = agent_root.join("habitat");
        std::fs::create_dir_all(&common).expect("create common include");
        std::fs::create_dir_all(&habitat).expect("create agent habitat");
        let config = AgentConfig {
            session_id: "agent-native-test-missing-from-real-home".to_string(),
            system_include_directories: Some(vec![
                common.display().to_string(),
                agent_root.display().to_string(),
            ]),
            ..AgentConfig::default()
        };

        assert_eq!(habitat_root(&config).as_deref(), Some(habitat.as_path()));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn persistent_actor_confirms_two_turns_and_reuses_idempotent_result() {
        let _lock = crate::utils::wardian_test_env_lock_async().await;
        let temp = tempfile::tempdir().expect("native broker tempdir");
        wardian_core::db::init_db_at_path(&temp.path().join("state.db"))
            .expect("initialize native broker db");
        let script = temp.path().join("native-provider.cjs");
        std::fs::write(
            &script,
            r#"const readline = require('node:readline');
const input = readline.createInterface({ input: process.stdin });
let pending = null;
input.on('line', (line) => {
  const request = JSON.parse(line);
  if (request.type === 'user') {
    const wardian = request.wardian || {};
    pending = { session_id: 'fixture-session', wardian };
    console.log(JSON.stringify({ ...pending, type: 'user', message: { role: 'user', content: [] } }));
    console.log(JSON.stringify({ ...pending, type: 'assistant', message: { role: 'assistant', content: [{ type: 'text', text: 'working' }] } }));
    console.log(JSON.stringify({ type: 'control_request', request_id: `permission:${wardian.interaction_id}`, request: { subtype: 'can_use_tool', tool_name: 'Bash', input: { command: 'git status' }, permission_suggestions: [], blocked_path: null, tool_use_id: 'tool-1' } }));
  } else if (request.type === 'control_response') {
    if (request.response.response.behavior !== 'deny') process.exit(17);
    console.log(JSON.stringify({ ...pending, type: 'result', is_error: false, result: 'done' }));
    pending = null;
  }
});
"#,
        )
        .expect("write native provider fixture");
        unsafe { std::env::set_var("WARDIAN_NATIVE_TEST_SCRIPT", &script) };
        let _script_guard = NativeTestScriptGuard;

        let broker = Arc::new(NativeDeliveryBroker::new());
        let config = AgentConfig {
            provider: "claude".to_string(),
            session_id: "agent-native-test".to_string(),
            folder: temp.path().display().to_string(),
            provider_config: wardian_core::models::ProviderConfig::Claude(
                wardian_core::models::ClaudeProviderConfig {
                    permission_mode: Some("manual".to_string()),
                    ..Default::default()
                },
            ),
            ..AgentConfig::default()
        };
        let spec = NativeSessionSpec {
            target_agent_id: "agent-native-test".to_string(),
            provider: "claude".to_string(),
            generation: 1,
            workspace: temp.path().to_path_buf(),
            config,
        };

        for (id, key) in [
            ("interaction-one", "key-one"),
            ("interaction-two", "key-two"),
        ] {
            let record = broker
                .admit(test_admission(id, key, "review"))
                .await
                .expect("admit turn");
            let receipt = broker
                .dispatch(spec.clone(), record)
                .await
                .expect("positive turn-start receipt");
            assert_eq!(receipt.record.phase, NativeDeliveryPhase::TurnStarted);
            assert_eq!(
                receipt.binding.provider_session_id.as_deref(),
                Some("fixture-session")
            );
        }

        for _ in 0..50 {
            if broker
                .get("interaction-two")
                .is_ok_and(|record| record.phase == NativeDeliveryPhase::Completed)
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert_eq!(
            broker.get("interaction-two").expect("second turn").phase,
            NativeDeliveryPhase::Completed
        );

        let replay = broker
            .admit(test_admission("different-server-id", "key-one", "review"))
            .await
            .expect("idempotent replay");
        assert_eq!(replay.envelope.interaction_id, "interaction-one");
        broker.dispose_agent("agent-native-test").await.unwrap();
    }

    #[tokio::test(flavor = "current_thread")]
    async fn malformed_post_submit_framing_stays_unconfirmed_without_retry() {
        let _lock = crate::utils::wardian_test_env_lock_async().await;
        let temp = tempfile::tempdir().expect("native broker tempdir");
        wardian_core::db::init_db_at_path(&temp.path().join("state.db"))
            .expect("initialize native broker db");
        let script = temp.path().join("malformed-provider.cjs");
        let log = temp.path().join("submissions.log");
        std::fs::write(
            &script,
            r#"const fs = require('node:fs');
const readline = require('node:readline');
const input = readline.createInterface({ input: process.stdin });
input.on('line', (line) => {
  fs.appendFileSync(process.env.WARDIAN_NATIVE_TEST_LOG, line + '\n');
  console.log('not-json');
});
"#,
        )
        .expect("write malformed provider fixture");
        unsafe {
            std::env::set_var("WARDIAN_NATIVE_TEST_SCRIPT", &script);
            std::env::set_var("WARDIAN_NATIVE_TEST_LOG", &log);
        }
        let _script_guard = NativeTestScriptGuard;

        let broker = Arc::new(NativeDeliveryBroker::new());
        let config = AgentConfig {
            provider: "claude".to_string(),
            session_id: "agent-native-test".to_string(),
            folder: temp.path().display().to_string(),
            ..AgentConfig::default()
        };
        let record = broker
            .admit(test_admission(
                "interaction-uncertain",
                "uncertain-key",
                "review",
            ))
            .await
            .expect("admit uncertain turn");
        let failure = broker
            .dispatch(
                NativeSessionSpec {
                    target_agent_id: "agent-native-test".to_string(),
                    provider: "claude".to_string(),
                    generation: 1,
                    workspace: temp.path().to_path_buf(),
                    config,
                },
                record,
            )
            .await
            .expect_err("malformed framing must fail closed");

        assert_eq!(failure.code, NativeDeliveryErrorCode::SubmittedUnconfirmed);
        assert!(failure.provider_boundary_crossed);
        assert_eq!(
            broker
                .get("interaction-uncertain")
                .expect("uncertain delivery")
                .phase,
            NativeDeliveryPhase::SubmittedUnconfirmed
        );
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        assert_eq!(
            std::fs::read_to_string(log)
                .expect("submission log")
                .lines()
                .count(),
            1,
            "the actor must not replay an uncertain submission"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn invalidate_premise_is_acknowledged_inside_the_active_pi_turn() {
        let _lock = crate::utils::wardian_test_env_lock_async().await;
        let temp = tempfile::tempdir().expect("native broker tempdir");
        wardian_core::db::init_db_at_path(&temp.path().join("state.db"))
            .expect("initialize native broker db");
        let script = temp.path().join("pi-provider.cjs");
        std::fs::write(
            &script,
            r#"const readline = require('node:readline');
const input = readline.createInterface({ input: process.stdin });
input.on('line', (line) => {
  const request = JSON.parse(line);
  if (request.type === 'get_state') {
    console.log(JSON.stringify({ id: request.id, type: 'response', command: 'get_state', success: true, data: { sessionId: 'pi-fixture-session' } }));
  } else if (request.type === 'prompt') {
    console.log(JSON.stringify({ id: request.id, type: 'response', command: 'prompt', success: true }));
    console.log(JSON.stringify({ type: 'agent_start' }));
    setTimeout(() => console.log(JSON.stringify({ type: 'agent_settled' })), 500);
  } else if (request.type === 'steer') {
    console.log(JSON.stringify({ id: request.id, type: 'response', command: 'steer', success: true }));
  }
});
"#,
        )
        .expect("write Pi provider fixture");
        unsafe { std::env::set_var("WARDIAN_NATIVE_TEST_SCRIPT", &script) };
        let _script_guard = NativeTestScriptGuard;

        let broker = Arc::new(NativeDeliveryBroker::new());
        let config = AgentConfig {
            provider: "pi".to_string(),
            session_id: "agent-native-test".to_string(),
            folder: temp.path().display().to_string(),
            ..AgentConfig::default()
        };
        let spec = NativeSessionSpec {
            target_agent_id: "agent-native-test".to_string(),
            provider: "pi".to_string(),
            generation: 1,
            workspace: temp.path().to_path_buf(),
            config,
        };
        let mut first = test_admission("interaction-active", "active-key", "start work");
        first.provider = "pi".to_string();
        let first = broker.admit(first).await.expect("admit active turn");
        broker
            .dispatch(spec.clone(), first)
            .await
            .expect("active turn started");

        let mut correction = test_admission(
            "interaction-correction",
            "correction-key",
            "premise changed",
        );
        correction.provider = "pi".to_string();
        correction.operation = NativeMessageOperation::InvalidatePremise;
        let correction = broker.admit(correction).await.expect("admit correction");
        let receipt = broker
            .dispatch(spec, correction)
            .await
            .expect("provider accepted correction");
        assert_eq!(receipt.record.phase, NativeDeliveryPhase::ProviderAccepted);

        for _ in 0..100 {
            if broker
                .get("interaction-correction")
                .is_ok_and(|record| record.phase == NativeDeliveryPhase::Completed)
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert_eq!(
            broker
                .get("interaction-correction")
                .expect("completed correction")
                .phase,
            NativeDeliveryPhase::Completed
        );
        broker.dispose_agent("agent-native-test").await.unwrap();
    }

    #[tokio::test(flavor = "current_thread")]
    async fn restart_recovery_requeues_only_never_submitted_work() {
        let _lock = crate::utils::wardian_test_env_lock_async().await;
        let temp = tempfile::tempdir().expect("native broker tempdir");
        wardian_core::db::init_db_at_path(&temp.path().join("state.db"))
            .expect("initialize native broker db");
        let broker = NativeDeliveryBroker::new();

        broker
            .admit(test_admission("interaction-queued", "queued-key", "queued"))
            .await
            .expect("admit queued delivery");
        let mut interrupted = broker
            .admit(test_admission(
                "interaction-dispatching",
                "dispatching-key",
                "uncertain",
            ))
            .await
            .expect("admit interrupted delivery");
        interrupted.phase = NativeDeliveryPhase::Dispatching;
        wardian_core::db::upsert_native_delivery(&interrupted).expect("persist interrupted phase");
        let mut expired = test_admission("interaction-expired", "expired-key", "expired");
        expired.deadline_at = Some("2020-01-01T00:00:00.000Z".to_string());
        assert_eq!(
            broker
                .admit(expired)
                .await
                .expect_err("expired admission")
                .code,
            NativeDeliveryErrorCode::DeadlineExpired
        );

        let recovered = broker
            .recover_after_restart(100)
            .await
            .expect("recover native queue");

        assert_eq!(recovered.len(), 1);
        assert_eq!(recovered[0].envelope.interaction_id, "interaction-queued");
        assert_eq!(
            broker
                .get("interaction-dispatching")
                .expect("interrupted delivery")
                .phase,
            NativeDeliveryPhase::SubmittedUnconfirmed
        );
    }

    #[tokio::test(flavor = "current_thread")]
    #[ignore = "opt-in real provider acceptance; set WARDIAN_E2E_REAL_NATIVE_PROVIDER"]
    async fn real_provider_completes_two_turns_on_one_native_session() {
        let provider = std::env::var("WARDIAN_E2E_REAL_NATIVE_PROVIDER")
            .expect("WARDIAN_E2E_REAL_NATIVE_PROVIDER is required");
        assert!(NativeProviderProtocol::for_provider(&provider).is_some());
        let _lock = crate::utils::wardian_test_env_lock_async().await;
        let temp = tempfile::tempdir().expect("native provider e2e tempdir");
        wardian_core::db::init_db_at_path(&temp.path().join("state.db"))
            .expect("initialize native provider e2e db");
        let agent_id = uuid::Uuid::new_v4().to_string();
        let broker = Arc::new(NativeDeliveryBroker::new());
        let config = AgentConfig {
            provider: provider.clone(),
            session_id: agent_id.clone(),
            session_name: format!("native-e2e-{provider}"),
            folder: std::env::current_dir()
                .expect("current workspace")
                .display()
                .to_string(),
            fresh_provider_session_id: Some(uuid::Uuid::new_v4().to_string()),
            ..AgentConfig::default()
        };
        let spec = NativeSessionSpec {
            target_agent_id: agent_id.clone(),
            provider: provider.clone(),
            generation: 1,
            workspace: std::env::current_dir().expect("current workspace"),
            config,
        };

        let mut binding_id = None;
        for turn in 1..=2 {
            let id = format!("real-{provider}-turn-{turn}");
            let mut admission = test_admission(
                &id,
                &format!("real-{provider}-key-{turn}"),
                &format!(
                    "This is native transport acceptance turn {turn}. Reply with exactly NATIVE_TURN_{turn}_OK and do not use tools."
                ),
            );
            admission.provider = provider.clone();
            admission.target_agent_id = agent_id.clone();
            let record = broker.admit(admission).await.expect("admit real turn");
            let receipt = tokio::time::timeout(
                std::time::Duration::from_secs(180),
                broker.dispatch(spec.clone(), record),
            )
            .await
            .expect("real provider turn-start timeout")
            .expect("real provider positive turn-start");
            assert_eq!(receipt.record.phase, NativeDeliveryPhase::TurnStarted);
            if let Some(previous) = binding_id.as_ref() {
                assert_eq!(receipt.binding.provider_session_id.as_ref(), Some(previous));
            } else {
                binding_id = Some(
                    receipt
                        .binding
                        .provider_session_id
                        .clone()
                        .expect("real provider must expose a session binding"),
                );
            }
            for _ in 0..1_800 {
                if broker
                    .get(&id)
                    .is_ok_and(|record| record.phase == NativeDeliveryPhase::Completed)
                {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
            assert_eq!(
                broker.get(&id).expect("real turn record").phase,
                NativeDeliveryPhase::Completed
            );
        }
        broker.dispose_agent(&agent_id).await.unwrap();
    }
}

#[cfg(test)]
mod codex_creation_revision_tests {
    use super::CodexCreationRegistry;
    use std::time::Duration;

    #[tokio::test]
    async fn exact_generation_disposal_cancels_existing_requests_only() {
        let registry = CodexCreationRegistry::default();
        let old = registry.register("agent", 7).unwrap();
        let same_generation = registry.register("agent", 7).unwrap();
        let newer = registry.register("agent", 8).unwrap();
        let other_agent = registry.register("other", 7).unwrap();
        registry.cancel("agent", Some(7)).unwrap();
        assert!(old.is_cancelled());
        assert!(same_generation.is_cancelled());
        assert!(!newer.is_cancelled());
        assert!(!other_agent.is_cancelled());
        // Subscribe only after disposal: the cancellation must not be lost.
        tokio::time::timeout(Duration::from_secs(1), old.cancelled())
            .await
            .expect("completed disposal must cancel an unpolled request");
        let later = registry.register("agent", 7).unwrap();
        assert!(!later.is_cancelled());
        registry.cancel("agent", Some(7)).unwrap();
        assert!(later.is_cancelled());
        assert!(!newer.is_cancelled());
    }

    #[test]
    fn agent_disposal_cancels_all_existing_generations_without_affecting_other_agents() {
        let registry = CodexCreationRegistry::default();
        let first = registry.register("agent", 7).unwrap();
        let second = registry.register("agent", 8).unwrap();
        let other = registry.register("other", 7).unwrap();
        registry.cancel("agent", None).unwrap();
        assert!(first.is_cancelled());
        assert!(second.is_cancelled());
        assert!(!other.is_cancelled());
        assert!(!registry.register("agent", 9).unwrap().is_cancelled());
    }

    #[test]
    fn completed_creation_revisions_are_pruned_without_generation_tombstones() {
        let registry = CodexCreationRegistry::default();
        for generation in 0..100 {
            let request = registry.register("agent", generation).unwrap();
            assert_eq!(registry.revisions.lock().unwrap().len(), 1);
            registry.cancel("agent", Some(generation)).unwrap();
            assert!(request.is_cancelled());
        }
        registry.cancel("agent", None).unwrap();
        assert!(registry.revisions.lock().unwrap().is_empty());
        assert!(!registry.register("agent", 0).unwrap().is_cancelled());
    }
}
