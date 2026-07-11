use super::replay::{ReplayRing, MAX_BATCH_BYTES};
use super::snapshot::build_snapshot;
use std::collections::HashMap;
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{broadcast, mpsc, oneshot, Mutex as AsyncMutex, Notify, RwLock};
use wardian_core::models::*;

pub const TERMINAL_SESSION_ACTOR_CAPACITY: usize = 256;
pub const MAX_DESKTOP_PRESENTATIONS_PER_SESSION: usize = 64;
pub const MAX_REMOTE_PRESENTATIONS_PER_SESSION: usize = 3;
pub const MAX_DEFERRED_TERMINAL_SESSIONS: usize = 256;
const ACTIVATION_TIMEOUT: Duration = Duration::from_secs(5);
const EVENT_WAKE_COALESCE: Duration = Duration::from_millis(16);
const ACTOR_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);
const RUNTIME_INPUT_SEND_TIMEOUT: Duration = Duration::from_secs(2);

type ResizeHandler = dyn Fn(TerminalGeometry) -> Result<(), String> + Send + Sync + 'static;
type BrokerReply<T> = oneshot::Sender<Result<T, TerminalBrokerError>>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalBrokerError {
    SessionNotFound,
    RuntimeTerminated,
    RuntimeUnavailable,
    ActorClosed,
    InvalidIdentity,
    InvalidRequest(&'static str),
    PresentationNotFound,
    ConsumerNotFound,
    DesktopConsumerAlreadyRegistered,
    PresentationLimit {
        client_kind: TerminalClientKind,
        limit: usize,
    },
    StaleRuntimeGeneration {
        expected: u64,
        received: u64,
    },
    RuntimeIo(String),
}

impl fmt::Display for TerminalBrokerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{self:?}")
    }
}

impl std::error::Error for TerminalBrokerError {}

#[derive(Clone)]
pub struct TerminalRuntimeHandles {
    input_tx: mpsc::Sender<Vec<u8>>,
    resize: Arc<ResizeHandler>,
}

impl TerminalRuntimeHandles {
    pub fn new<F>(input_tx: mpsc::Sender<Vec<u8>>, resize: F) -> Self
    where
        F: Fn(TerminalGeometry) -> Result<(), String> + Send + Sync + 'static,
    {
        Self {
            input_tx,
            resize: Arc::new(resize),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalClientIdentity {
    client_kind: TerminalClientKind,
    subject_id: Option<String>,
    authenticated: bool,
    interaction_allowed: bool,
}

impl TerminalClientIdentity {
    pub fn trusted_desktop() -> Self {
        Self {
            client_kind: TerminalClientKind::Desktop,
            subject_id: None,
            authenticated: true,
            interaction_allowed: true,
        }
    }

    pub fn authenticated_remote(
        attachment_id: impl Into<String>,
        interaction_allowed: bool,
    ) -> Self {
        Self {
            client_kind: TerminalClientKind::Remote,
            subject_id: Some(attachment_id.into()),
            authenticated: true,
            interaction_allowed,
        }
    }

    fn validate(
        &self,
        client_kind: TerminalClientKind,
        presentation_id: &str,
    ) -> Result<(), TerminalBrokerError> {
        if !self.authenticated || self.client_kind != client_kind {
            return Err(TerminalBrokerError::InvalidIdentity);
        }
        if client_kind == TerminalClientKind::Remote
            && self.subject_id.as_deref() != Some(presentation_id)
        {
            return Err(TerminalBrokerError::InvalidIdentity);
        }
        Ok(())
    }

    fn effective_capability(
        &self,
        requested: TerminalRequestedInteraction,
    ) -> TerminalInteractionCapability {
        if self.interaction_allowed && requested == TerminalRequestedInteraction::Interactive {
            TerminalInteractionCapability::Interactive
        } else {
            TerminalInteractionCapability::ReadOnly
        }
    }

    fn initially_fallback_promotion_eligible(&self) -> bool {
        self.client_kind == TerminalClientKind::Desktop
    }
}

pub trait TerminalTimer: Send + Sync {
    fn sleep(&self, duration: Duration) -> Pin<Box<dyn Future<Output = ()> + Send>>;
}

struct TokioTerminalTimer;

impl TerminalTimer for TokioTerminalTimer {
    fn sleep(&self, duration: Duration) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        Box::pin(tokio::time::sleep(duration))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ActivationKey {
    presentation_id: String,
    runtime_generation: u64,
    lease_epoch: u64,
    activation_id: String,
}

impl ActivationKey {
    fn from_ack(request: &TerminalActivationAckRequest) -> Self {
        Self {
            presentation_id: request.presentation_id.clone(),
            runtime_generation: request.runtime_generation,
            lease_epoch: request.lease_epoch,
            activation_id: request.activation_id.clone(),
        }
    }

    fn from_pending(pending: &PendingActivation) -> Self {
        Self {
            presentation_id: pending.state.presentation_id.clone(),
            runtime_generation: pending.state.runtime_generation,
            lease_epoch: pending.state.lease_epoch,
            activation_id: pending.state.activation_id.clone(),
        }
    }
}

struct ActivationControlState {
    key: ActivationKey,
    ack_reservations: usize,
    deadline_elapsed: bool,
    timeout_due: bool,
}

#[derive(Default)]
struct TerminalControlState {
    activation: Option<ActivationControlState>,
    wake_due: bool,
}

#[cfg(test)]
struct AckClassificationGate {
    classified: oneshot::Sender<()>,
    release: oneshot::Receiver<()>,
}

/// Constant-space priority state shared by broker callers, timer tasks, and the actor.
///
/// The synchronous mutex is held only for state transitions, never across an await. It
/// linearizes activation acknowledgements with deadline expiry while `Notify` lets the
/// actor cut ahead of a saturated external command queue without accumulating messages.
#[derive(Default)]
struct TerminalControlPlane {
    state: Mutex<TerminalControlState>,
    notify: Notify,
    #[cfg(test)]
    ack_classification_gate: Mutex<Option<AckClassificationGate>>,
}

struct DueControlSignals {
    activation_timeout: Option<ActivationKey>,
    flush_wake: bool,
}

impl TerminalControlPlane {
    fn start_activation(&self, key: ActivationKey) {
        self.state.lock().expect("terminal control lock").activation =
            Some(ActivationControlState {
                key,
                ack_reservations: 0,
                deadline_elapsed: false,
                timeout_due: false,
            });
    }

    fn clear_activation(&self, key: &ActivationKey) {
        let mut state = self.state.lock().expect("terminal control lock");
        if state
            .activation
            .as_ref()
            .is_some_and(|activation| activation.key == *key)
        {
            state.activation = None;
        }
    }

    fn classify_activation_ack(
        self: &Arc<Self>,
        request: &TerminalActivationAckRequest,
    ) -> ActivationAckArrival {
        let key = ActivationKey::from_ack(request);
        let mut state = self.state.lock().expect("terminal control lock");
        let Some(activation) = state
            .activation
            .as_mut()
            .filter(|activation| activation.key == key)
        else {
            return ActivationAckArrival::NonMatching;
        };
        if activation.deadline_elapsed {
            return ActivationAckArrival::MatchingAfterDeadline;
        }
        // Each matching arrival owns one counted reservation while it waits for the
        // bounded external FIFO. Releasing one caller cannot expose another live caller
        // to timeout rollback.
        activation.ack_reservations = activation
            .ack_reservations
            .checked_add(1)
            .expect("activation acknowledgement reservation overflow");
        ActivationAckArrival::MatchingBeforeDeadline {
            _reservation: ActivationAckReservation {
                control: self.clone(),
                key,
            },
        }
    }

    fn release_activation_ack(&self, key: &ActivationKey) {
        let mut state = self.state.lock().expect("terminal control lock");
        let Some(activation) = state
            .activation
            .as_mut()
            .filter(|activation| activation.key == *key)
        else {
            return;
        };
        activation.ack_reservations = activation
            .ack_reservations
            .checked_sub(1)
            .expect("activation acknowledgement reservation underflow");
        if activation.ack_reservations == 0
            && activation.deadline_elapsed
            && !activation.timeout_due
        {
            activation.timeout_due = true;
            // Publish the due bit and its notification under the same mutex. An ack
            // classified after this point cannot overtake an unpublished wakeup.
            self.notify.notify_one();
        }
    }

    fn activation_deadline_elapsed(&self, key: &ActivationKey) {
        let mut state = self.state.lock().expect("terminal control lock");
        let Some(activation) = state
            .activation
            .as_mut()
            .filter(|activation| activation.key == *key)
        else {
            return;
        };
        activation.deadline_elapsed = true;
        if activation.ack_reservations == 0 && !activation.timeout_due {
            activation.timeout_due = true;
            // Keep publication and notification atomic with respect to ack arrival
            // classification; the actor-side classification check is the second line
            // of defense if the notification is consumed or delayed.
            self.notify.notify_one();
        }
    }

    fn mark_wake_due(&self) {
        let mut state = self.state.lock().expect("terminal control lock");
        if !state.wake_due {
            state.wake_due = true;
            self.notify.notify_one();
        }
    }

    fn take_due(&self) -> DueControlSignals {
        let mut state = self.state.lock().expect("terminal control lock");
        let activation_timeout = state.activation.as_mut().and_then(|activation| {
            if activation.timeout_due {
                activation.timeout_due = false;
                Some(activation.key.clone())
            } else {
                None
            }
        });
        DueControlSignals {
            activation_timeout,
            flush_wake: std::mem::take(&mut state.wake_due),
        }
    }

    fn claim_unreserved_activation_timeout(&self, key: &ActivationKey) -> bool {
        let mut state = self.state.lock().expect("terminal control lock");
        let Some(activation) = state
            .activation
            .as_mut()
            .filter(|activation| activation.key == *key)
        else {
            return false;
        };
        if !activation.deadline_elapsed
            || activation.ack_reservations != 0
            || !activation.timeout_due
        {
            return false;
        }
        // The late ack is taking responsibility for the already-due rollback.
        // Clear the coalesced due bit so a delayed notification becomes a no-op.
        activation.timeout_due = false;
        true
    }

    fn clear(&self) {
        *self.state.lock().expect("terminal control lock") = TerminalControlState::default();
    }

    #[cfg(test)]
    fn activation_ack_reserved(&self, activation_id: &str) -> bool {
        self.activation_ack_reservation_count(activation_id) > 0
    }

    #[cfg(test)]
    fn activation_ack_reservation_count(&self, activation_id: &str) -> usize {
        self.state
            .lock()
            .expect("terminal control lock")
            .activation
            .as_ref()
            .map_or(0, |activation| {
                if activation.key.activation_id == activation_id {
                    activation.ack_reservations
                } else {
                    0
                }
            })
    }

    #[cfg(test)]
    fn activation_slots(&self) -> usize {
        usize::from(
            self.state
                .lock()
                .expect("terminal control lock")
                .activation
                .is_some(),
        )
    }

    #[cfg(test)]
    fn activation_deadline_has_elapsed(&self, activation_id: &str) -> bool {
        self.state
            .lock()
            .expect("terminal control lock")
            .activation
            .as_ref()
            .is_some_and(|activation| {
                activation.key.activation_id == activation_id && activation.deadline_elapsed
            })
    }

    #[cfg(test)]
    fn gate_next_ack_after_classification(&self) -> (oneshot::Receiver<()>, oneshot::Sender<()>) {
        let (classified_tx, classified_rx) = oneshot::channel();
        let (release_tx, release_rx) = oneshot::channel();
        *self
            .ack_classification_gate
            .lock()
            .expect("ack classification gate lock") = Some(AckClassificationGate {
            classified: classified_tx,
            release: release_rx,
        });
        (classified_rx, release_tx)
    }

    #[cfg(test)]
    async fn wait_after_ack_classification(&self, arrival: &ActivationAckArrival) {
        if !arrival.matching_before_deadline() {
            return;
        }
        let gate = self
            .ack_classification_gate
            .lock()
            .expect("ack classification gate lock")
            .take();
        if let Some(gate) = gate {
            let _ = gate.classified.send(());
            let _ = gate.release.await;
        }
    }
}

struct ActivationAckReservation {
    control: Arc<TerminalControlPlane>,
    key: ActivationKey,
}

enum ActivationAckArrival {
    MatchingBeforeDeadline {
        _reservation: ActivationAckReservation,
    },
    MatchingAfterDeadline,
    NonMatching,
}

impl ActivationAckArrival {
    fn matching_before_deadline(&self) -> bool {
        matches!(self, Self::MatchingBeforeDeadline { .. })
    }

    fn matching_after_deadline(&self) -> bool {
        matches!(self, Self::MatchingAfterDeadline)
    }
}

impl Drop for ActivationAckReservation {
    fn drop(&mut self) {
        self.control.release_activation_ack(&self.key);
    }
}

struct AbortOnDropTask(tokio::task::JoinHandle<()>);

impl Drop for AbortOnDropTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}

#[derive(Clone)]
pub struct TerminalSessionHandle {
    tx: mpsc::Sender<TerminalSessionMessage>,
    control: Arc<TerminalControlPlane>,
    runtime_generation: u64,
    lease_epoch: Arc<AtomicU64>,
    latest_sequence: Arc<AtomicU64>,
    terminated: Arc<AtomicBool>,
    abort_handle: tokio::task::AbortHandle,
}

pub struct TerminalSessionBroker {
    sessions: RwLock<HashMap<String, TerminalSessionHandle>>,
    deferred_geometries: AsyncMutex<DeferredGeometryState>,
    wake_tx: broadcast::Sender<TerminalEventsReady>,
    lifecycle_tx: broadcast::Sender<TerminalSessionLifecycleNotification>,
    timer: Arc<dyn TerminalTimer>,
}

#[derive(Default)]
struct DeferredGeometryState {
    sessions: HashMap<String, HashMap<String, DeferredPresentationGeometry>>,
    next_sequence: u64,
}

#[derive(Clone, Copy)]
struct DeferredPresentationGeometry {
    geometry: TerminalGeometry,
    sequence: u64,
}

impl Default for TerminalSessionBroker {
    fn default() -> Self {
        Self::with_timer(Arc::new(TokioTerminalTimer))
    }
}

impl TerminalSessionBroker {
    pub fn with_timer<T>(timer: Arc<T>) -> Self
    where
        T: TerminalTimer + 'static,
    {
        let (wake_tx, _) = broadcast::channel(256);
        let (lifecycle_tx, _) = broadcast::channel(64);
        Self {
            sessions: RwLock::new(HashMap::new()),
            deferred_geometries: AsyncMutex::new(DeferredGeometryState::default()),
            wake_tx,
            lifecycle_tx,
            timer,
        }
    }

    pub fn subscribe_wakeups(&self) -> broadcast::Receiver<TerminalEventsReady> {
        self.wake_tx.subscribe()
    }

    pub fn subscribe_lifecycle(&self) -> broadcast::Receiver<TerminalSessionLifecycleNotification> {
        self.lifecycle_tx.subscribe()
    }

    pub async fn start_or_replace_runtime(
        &self,
        session_id: &str,
        runtime: TerminalRuntimeHandles,
        geometry: TerminalGeometry,
    ) -> Result<u64, TerminalBrokerError> {
        validate_id(session_id, "session_id")?;
        let geometry = clamp_geometry(geometry, TerminalClientKind::Desktop);
        let (replaced, runtime_generation) = {
            let mut sessions = self.sessions.write().await;
            let previous = sessions.get(session_id).cloned();
            let runtime_generation = previous
                .as_ref()
                .map_or(1, |handle| handle.runtime_generation.saturating_add(1));
            let initial_lease_epoch = previous.as_ref().map_or(0, |handle| {
                handle.lease_epoch.load(Ordering::SeqCst).saturating_add(1)
            });
            let handle = self.spawn_actor(
                session_id.to_string(),
                runtime_generation,
                initial_lease_epoch,
                runtime,
                geometry,
            );
            (
                sessions.insert(session_id.to_string(), handle),
                runtime_generation,
            )
        };
        let lifecycle = if replaced.is_some() {
            TerminalSessionLifecycleEvent::RuntimeReplaced
        } else {
            TerminalSessionLifecycleEvent::RuntimeStarted
        };
        if let Some(old_handle) = replaced {
            self.shutdown_handle(
                session_id,
                old_handle,
                TerminalSessionLifecycleEvent::RuntimeReplaced,
            )
            .await;
        }
        let _ = self
            .lifecycle_tx
            .send(TerminalSessionLifecycleNotification {
                session_id: session_id.to_string(),
                runtime_generation,
                lifecycle,
            });
        self.deferred_geometries
            .lock()
            .await
            .sessions
            .remove(session_id);
        Ok(runtime_generation)
    }

    /// Remembers presentation geometry while no native runtime exists yet.
    ///
    /// This is broker-owned pre-runtime state rather than a second PTY-size
    /// authority. Once a runtime starts, its actor's canonical geometry wins
    /// and this deferred value is discarded.
    pub async fn remember_deferred_geometry(
        &self,
        session_id: &str,
        presentation_id: &str,
        geometry: TerminalGeometry,
    ) -> Result<TerminalGeometry, TerminalBrokerError> {
        validate_id(session_id, "session_id")?;
        validate_id(presentation_id, "presentation_id")?;
        let geometry = clamp_geometry(geometry, TerminalClientKind::Desktop);
        let mut deferred = self.deferred_geometries.lock().await;
        if !deferred.sessions.contains_key(session_id)
            && deferred.sessions.len() >= MAX_DEFERRED_TERMINAL_SESSIONS
        {
            return Err(TerminalBrokerError::InvalidRequest(
                "deferred_session_limit",
            ));
        }
        let is_new = deferred
            .sessions
            .get(session_id)
            .is_none_or(|presentations| !presentations.contains_key(presentation_id));
        if is_new
            && deferred
                .sessions
                .get(session_id)
                .is_some_and(|presentations| {
                    presentations.len() >= MAX_DESKTOP_PRESENTATIONS_PER_SESSION
                })
        {
            return Err(TerminalBrokerError::PresentationLimit {
                client_kind: TerminalClientKind::Desktop,
                limit: MAX_DESKTOP_PRESENTATIONS_PER_SESSION,
            });
        }
        deferred.next_sequence = deferred.next_sequence.saturating_add(1);
        let sequence = deferred.next_sequence;
        deferred
            .sessions
            .entry(session_id.to_string())
            .or_default()
            .insert(
                presentation_id.to_string(),
                DeferredPresentationGeometry { geometry, sequence },
            );
        Ok(geometry)
    }

    /// Returns the live actor's canonical geometry, or the latest pre-runtime
    /// viewport report when the session has not started yet.
    pub async fn spawn_geometry(
        &self,
        session_id: &str,
    ) -> Result<Option<TerminalGeometry>, TerminalBrokerError> {
        validate_id(session_id, "session_id")?;
        match self.broker_state(session_id).await {
            Ok(state) => Ok(Some(state.geometry)),
            Err(TerminalBrokerError::SessionNotFound)
            | Err(TerminalBrokerError::RuntimeTerminated)
            | Err(TerminalBrokerError::ActorClosed) => Ok(self
                .deferred_geometries
                .lock()
                .await
                .sessions
                .get(session_id)
                .and_then(|presentations| {
                    presentations
                        .values()
                        .max_by_key(|presentation| presentation.sequence)
                        .map(|presentation| presentation.geometry)
                })),
            Err(error) => Err(error),
        }
    }

    pub async fn forget_deferred_geometry(&self, session_id: &str) {
        self.deferred_geometries
            .lock()
            .await
            .sessions
            .remove(session_id);
    }

    pub async fn forget_deferred_presentation(&self, session_id: &str, presentation_id: &str) {
        let mut deferred = self.deferred_geometries.lock().await;
        if let Some(presentations) = deferred.sessions.get_mut(session_id) {
            presentations.remove(presentation_id);
            if presentations.is_empty() {
                deferred.sessions.remove(session_id);
            }
        }
    }

    pub fn process_output_blocking(
        &self,
        session_id: &str,
        runtime_generation: u64,
        bytes: Vec<u8>,
    ) -> Result<(), TerminalBrokerError> {
        let handle = self.session_handle_blocking(session_id)?;
        ensure_generation(&handle, runtime_generation)?;
        if handle.terminated.load(Ordering::SeqCst) {
            return Err(TerminalBrokerError::RuntimeTerminated);
        }
        let (reply_tx, reply_rx) = oneshot::channel();
        handle
            .tx
            .blocking_send(TerminalSessionMessage::Output {
                runtime_generation,
                bytes,
                reply: reply_tx,
            })
            .map_err(|_| TerminalBrokerError::RuntimeTerminated)?;
        reply_rx
            .blocking_recv()
            .map_err(|_| TerminalBrokerError::RuntimeTerminated)?
    }

    pub fn send_privileged_input_blocking(
        &self,
        session_id: &str,
        runtime_generation: u64,
        bytes: Vec<u8>,
    ) -> Result<(), TerminalBrokerError> {
        let handle = self.session_handle_blocking(session_id)?;
        ensure_generation(&handle, runtime_generation)?;
        if handle.terminated.load(Ordering::SeqCst) {
            return Err(TerminalBrokerError::RuntimeTerminated);
        }
        let (reply_tx, reply_rx) = oneshot::channel();
        handle
            .tx
            .blocking_send(TerminalSessionMessage::PrivilegedInput {
                bytes,
                reply: reply_tx,
            })
            .map_err(|_| TerminalBrokerError::RuntimeTerminated)?;
        reply_rx
            .blocking_recv()
            .map_err(|_| TerminalBrokerError::RuntimeTerminated)?
    }

    pub async fn register_presentation(
        &self,
        request: TerminalPresentationRegistration,
        identity: TerminalClientIdentity,
    ) -> Result<TerminalPresentationRegistrationResult, TerminalBrokerError> {
        let session_id = request.session_id.clone();
        self.request(&session_id, move |reply| TerminalSessionMessage::Register {
            request,
            identity,
            reply,
        })
        .await
    }

    pub async fn update_presentation(
        &self,
        request: TerminalPresentationUpdateRequest,
        identity: TerminalClientIdentity,
    ) -> Result<TerminalPresentationUpdateResult, TerminalBrokerError> {
        let session_id = request.session_id.clone();
        self.request(&session_id, move |reply| {
            TerminalSessionMessage::UpdatePresentation {
                request,
                identity,
                reply,
            }
        })
        .await
    }

    pub async fn unregister_presentation(
        &self,
        session_id: &str,
        presentation_id: &str,
        runtime_generation: u64,
    ) -> Result<TerminalBrokerState, TerminalBrokerError> {
        let owned_session = session_id.to_string();
        let owned_presentation = presentation_id.to_string();
        self.request(session_id, move |reply| {
            TerminalSessionMessage::UnregisterPresentation {
                session_id: owned_session,
                presentation_id: owned_presentation,
                runtime_generation,
                reply,
            }
        })
        .await
    }

    pub async fn report_presentation_viewport(
        &self,
        request: TerminalPresentationViewportRequest,
    ) -> Result<TerminalPresentationState, TerminalBrokerError> {
        let session_id = request.session_id.clone();
        self.request(&session_id, move |reply| {
            TerminalSessionMessage::ReportViewport { request, reply }
        })
        .await
    }

    pub async fn begin_activation(
        &self,
        request: TerminalActivationBeginRequest,
    ) -> Result<TerminalActivationBeginResult, TerminalBrokerError> {
        let session_id = request.session_id.clone();
        self.request(&session_id, move |reply| {
            TerminalSessionMessage::BeginActivation { request, reply }
        })
        .await
    }

    pub async fn ack_activation(
        &self,
        request: TerminalActivationAckRequest,
    ) -> Result<TerminalActivationAckResult, TerminalBrokerError> {
        let session_id = request.session_id.clone();
        let handle = self.session_handle(&session_id).await?;
        if handle.terminated.load(Ordering::SeqCst) {
            return Err(TerminalBrokerError::RuntimeTerminated);
        }
        let arrival = handle.control.classify_activation_ack(&request);
        #[cfg(test)]
        handle.control.wait_after_ack_classification(&arrival).await;
        let (reply_tx, reply_rx) = oneshot::channel();
        handle
            .tx
            .send(TerminalSessionMessage::AckActivation {
                request,
                arrival,
                reply: reply_tx,
            })
            .await
            .map_err(|_| TerminalBrokerError::RuntimeTerminated)?;
        reply_rx
            .await
            .map_err(|_| TerminalBrokerError::RuntimeTerminated)?
    }

    pub async fn begin_owner_resync(
        &self,
        request: TerminalOwnerResyncBeginRequest,
    ) -> Result<TerminalOwnerResyncBeginResult, TerminalBrokerError> {
        let session_id = request.session_id.clone();
        self.request(&session_id, move |reply| {
            TerminalSessionMessage::BeginOwnerResync { request, reply }
        })
        .await
    }

    pub async fn ack_owner_resync(
        &self,
        request: TerminalOwnerResyncAckRequest,
    ) -> Result<TerminalOwnerResyncAckResult, TerminalBrokerError> {
        let session_id = request.session_id.clone();
        self.request(&session_id, move |reply| {
            TerminalSessionMessage::AckOwnerResync { request, reply }
        })
        .await
    }

    pub async fn send_input(
        &self,
        request: TerminalInputRequest,
    ) -> Result<TerminalLeaseDecision, TerminalBrokerError> {
        let session_id = request.lease.session_id.clone();
        self.request(&session_id, move |reply| TerminalSessionMessage::Input {
            request,
            reply,
        })
        .await
    }

    pub async fn resize(
        &self,
        request: TerminalGeometryRequest,
    ) -> Result<TerminalGeometryCommitResult, TerminalBrokerError> {
        let session_id = request.lease.session_id.clone();
        self.request(&session_id, move |reply| TerminalSessionMessage::Resize {
            request,
            reply,
        })
        .await
    }

    /// One-release adapter for legacy desktop and not-yet-migrated remote
    /// callers. It is still serialized by the actor and cannot bypass a
    /// committed/pending presentation lease.
    pub async fn send_legacy_input(
        &self,
        session_id: &str,
        bytes: Vec<u8>,
    ) -> Result<TerminalLeaseDecision, TerminalBrokerError> {
        self.request(session_id, move |reply| {
            TerminalSessionMessage::CompatibilityInput { bytes, reply }
        })
        .await
    }

    pub async fn send_privileged_input(
        &self,
        session_id: &str,
        bytes: Vec<u8>,
    ) -> Result<(), TerminalBrokerError> {
        self.request(session_id, move |reply| {
            TerminalSessionMessage::PrivilegedInput { bytes, reply }
        })
        .await
    }

    pub async fn read_legacy_output(
        &self,
        session_id: &str,
        max_bytes: Option<usize>,
        peek: bool,
    ) -> Result<Option<String>, TerminalBrokerError> {
        self.request(session_id, move |reply| {
            TerminalSessionMessage::ReadCompatibilityOutput {
                max_bytes,
                peek,
                reply,
            }
        })
        .await
    }

    /// One-release native resize adapter. The broker remains the only native
    /// writer and rejects compatibility calls once presentation ownership is
    /// active or pending.
    pub async fn resize_legacy(
        &self,
        session_id: &str,
        geometry: TerminalGeometry,
    ) -> Result<TerminalGeometryCommitResult, TerminalBrokerError> {
        self.request(session_id, move |reply| {
            TerminalSessionMessage::CompatibilityResize { geometry, reply }
        })
        .await
    }

    pub async fn subscribe(
        &self,
        request: TerminalEventSubscriptionRequest,
    ) -> Result<TerminalEventSubscriptionResult, TerminalBrokerError> {
        let session_id = request.session_id.clone();
        self.request(&session_id, move |reply| {
            TerminalSessionMessage::Subscribe { request, reply }
        })
        .await
    }

    pub async fn read_events(
        &self,
        request: TerminalEventReadRequest,
    ) -> Result<TerminalEventBatch, TerminalBrokerError> {
        let handle = self.session_handle(&request.session_id).await?;
        if handle.terminated.load(Ordering::SeqCst) {
            return Ok(terminated_batch(&handle, request.after_sequence));
        }
        let (reply_tx, reply_rx) = oneshot::channel();
        if handle
            .tx
            .send(TerminalSessionMessage::ReadEvents {
                request: request.clone(),
                reply: reply_tx,
            })
            .await
            .is_err()
        {
            return Ok(terminated_batch(&handle, request.after_sequence));
        }
        match reply_rx.await {
            Ok(result) => result,
            Err(_) => Ok(terminated_batch(&handle, request.after_sequence)),
        }
    }

    pub async fn ack_events(
        &self,
        request: TerminalEventAckRequest,
    ) -> Result<TerminalEventAckResult, TerminalBrokerError> {
        let session_id = request.session_id.clone();
        self.request(&session_id, move |reply| {
            TerminalSessionMessage::AckEvents { request, reply }
        })
        .await
    }

    pub async fn unsubscribe(
        &self,
        request: TerminalEventUnsubscribeRequest,
    ) -> Result<(), TerminalBrokerError> {
        let session_id = request.session_id.clone();
        self.request(&session_id, move |reply| {
            TerminalSessionMessage::Unsubscribe { request, reply }
        })
        .await
    }

    pub async fn consumer_acknowledgements(
        &self,
        session_id: &str,
    ) -> Result<HashMap<String, u64>, TerminalBrokerError> {
        self.request(session_id, TerminalSessionMessage::ConsumerAcknowledgements)
            .await
    }

    pub async fn broker_state(
        &self,
        session_id: &str,
    ) -> Result<TerminalBrokerState, TerminalBrokerError> {
        self.request(session_id, TerminalSessionMessage::BrokerState)
            .await
    }

    pub async fn snapshot(
        &self,
        session_id: &str,
    ) -> Result<TerminalSnapshot, TerminalBrokerError> {
        self.request(session_id, TerminalSessionMessage::Snapshot)
            .await
    }

    pub async fn pause_runtime(
        &self,
        session_id: &str,
        runtime_generation: u64,
    ) -> Result<TerminalBrokerState, TerminalBrokerError> {
        let owned_session = session_id.to_string();
        self.request(session_id, move |reply| TerminalSessionMessage::Pause {
            session_id: owned_session,
            runtime_generation,
            reply,
        })
        .await
    }

    pub async fn terminate_runtime(
        &self,
        session_id: &str,
        runtime_generation: u64,
    ) -> Result<(), TerminalBrokerError> {
        let handle = self.session_handle(session_id).await?;
        ensure_generation(&handle, runtime_generation)?;
        self.shutdown_handle(
            session_id,
            handle,
            TerminalSessionLifecycleEvent::RuntimeTerminated,
        )
        .await;
        Ok(())
    }

    /// Terminates one exact runtime generation and removes its broker entry.
    /// A concurrent replacement cannot be removed by an older kill request.
    pub async fn terminate_and_remove_runtime(
        &self,
        session_id: &str,
        runtime_generation: u64,
    ) -> Result<(), TerminalBrokerError> {
        let handle = {
            let mut sessions = self.sessions.write().await;
            let handle = sessions
                .get(session_id)
                .cloned()
                .ok_or(TerminalBrokerError::SessionNotFound)?;
            ensure_generation(&handle, runtime_generation)?;
            sessions.remove(session_id);
            handle
        };
        self.shutdown_handle(
            session_id,
            handle,
            TerminalSessionLifecycleEvent::RuntimeTerminated,
        )
        .await;
        self.deferred_geometries
            .lock()
            .await
            .sessions
            .remove(session_id);
        Ok(())
    }

    #[cfg(test)]
    pub(super) async fn external_command_capacity_for_test(
        &self,
        session_id: &str,
    ) -> Result<usize, TerminalBrokerError> {
        Ok(self.session_handle(session_id).await?.tx.capacity())
    }

    #[cfg(test)]
    pub(super) async fn activation_ack_reserved_for_test(
        &self,
        session_id: &str,
        activation_id: &str,
    ) -> Result<bool, TerminalBrokerError> {
        Ok(self
            .session_handle(session_id)
            .await?
            .control
            .activation_ack_reserved(activation_id))
    }

    #[cfg(test)]
    pub(super) async fn activation_control_slots_for_test(
        &self,
        session_id: &str,
    ) -> Result<usize, TerminalBrokerError> {
        Ok(self
            .session_handle(session_id)
            .await?
            .control
            .activation_slots())
    }

    #[cfg(test)]
    pub(super) async fn activation_ack_reservation_count_for_test(
        &self,
        session_id: &str,
        activation_id: &str,
    ) -> Result<usize, TerminalBrokerError> {
        Ok(self
            .session_handle(session_id)
            .await?
            .control
            .activation_ack_reservation_count(activation_id))
    }

    #[cfg(test)]
    pub(super) async fn consume_control_notification_for_test(
        &self,
        session_id: &str,
    ) -> Result<(), TerminalBrokerError> {
        let handle = self.session_handle(session_id).await?;
        handle.control.notify.notified().await;
        Ok(())
    }

    #[cfg(test)]
    pub(super) async fn activation_deadline_has_elapsed_for_test(
        &self,
        session_id: &str,
        activation_id: &str,
    ) -> Result<bool, TerminalBrokerError> {
        Ok(self
            .session_handle(session_id)
            .await?
            .control
            .activation_deadline_has_elapsed(activation_id))
    }

    #[cfg(test)]
    pub(super) async fn gate_next_ack_after_classification_for_test(
        &self,
        session_id: &str,
    ) -> Result<(oneshot::Receiver<()>, oneshot::Sender<()>), TerminalBrokerError> {
        Ok(self
            .session_handle(session_id)
            .await?
            .control
            .gate_next_ack_after_classification())
    }

    #[cfg(test)]
    pub(super) async fn block_actor_for_test(
        &self,
        session_id: &str,
    ) -> Result<std::sync::mpsc::Sender<()>, TerminalBrokerError> {
        let handle = self.session_handle(session_id).await?;
        let (entered_tx, entered_rx) = oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        handle
            .tx
            .send(TerminalSessionMessage::TestBlock {
                entered: entered_tx,
                release: release_rx,
            })
            .await
            .map_err(|_| TerminalBrokerError::RuntimeTerminated)?;
        entered_rx
            .await
            .map_err(|_| TerminalBrokerError::RuntimeTerminated)?;
        Ok(release_tx)
    }

    fn spawn_actor(
        &self,
        session_id: String,
        runtime_generation: u64,
        lease_epoch_value: u64,
        runtime: TerminalRuntimeHandles,
        geometry: TerminalGeometry,
    ) -> TerminalSessionHandle {
        let (tx, rx) = mpsc::channel(TERMINAL_SESSION_ACTOR_CAPACITY);
        let control = Arc::new(TerminalControlPlane::default());
        let lease_epoch = Arc::new(AtomicU64::new(lease_epoch_value));
        let latest_sequence = Arc::new(AtomicU64::new(0));
        let terminated = Arc::new(AtomicBool::new(false));
        let actor = TerminalSessionActor::new(
            session_id,
            runtime_generation,
            lease_epoch.clone(),
            latest_sequence.clone(),
            terminated.clone(),
            runtime,
            geometry,
            control.clone(),
            self.wake_tx.clone(),
            self.lifecycle_tx.clone(),
            self.timer.clone(),
        );
        let task = tokio::spawn(actor.run(rx));
        let abort_handle = task.abort_handle();
        drop(task);
        TerminalSessionHandle {
            tx,
            control,
            runtime_generation,
            lease_epoch,
            latest_sequence,
            terminated,
            abort_handle,
        }
    }

    async fn shutdown_handle(
        &self,
        session_id: &str,
        handle: TerminalSessionHandle,
        lifecycle: TerminalSessionLifecycleEvent,
    ) {
        if handle.terminated.load(Ordering::SeqCst) {
            return;
        }
        let timeout = self.timer.sleep(ACTOR_SHUTDOWN_TIMEOUT);
        let shutdown_tx = handle.tx.clone();
        let shutdown = async move {
            let (reply_tx, reply_rx) = oneshot::channel();
            shutdown_tx
                .send(TerminalSessionMessage::Shutdown {
                    lifecycle,
                    reply: reply_tx,
                })
                .await
                .map_err(|_| ())?;
            reply_rx.await.map_err(|_| ())
        };
        tokio::pin!(timeout);
        tokio::pin!(shutdown);
        tokio::select! {
            result = &mut shutdown => {
                if result.is_err() {
                    handle.terminated.store(true, Ordering::SeqCst);
                }
            }
            _ = &mut timeout => {
                eprintln!(
                    "[Wardian] terminal session actor {session_id} did not stop within two seconds; aborting"
                );
                handle.abort_handle.abort();
                handle.terminated.store(true, Ordering::SeqCst);
            }
        }
    }

    async fn request<T, F>(&self, session_id: &str, message: F) -> Result<T, TerminalBrokerError>
    where
        T: Send + 'static,
        F: FnOnce(BrokerReply<T>) -> TerminalSessionMessage,
    {
        let handle = self.session_handle(session_id).await?;
        if handle.terminated.load(Ordering::SeqCst) {
            return Err(TerminalBrokerError::RuntimeTerminated);
        }
        let (reply_tx, reply_rx) = oneshot::channel();
        handle
            .tx
            .send(message(reply_tx))
            .await
            .map_err(|_| TerminalBrokerError::RuntimeTerminated)?;
        reply_rx
            .await
            .map_err(|_| TerminalBrokerError::RuntimeTerminated)?
    }

    async fn session_handle(
        &self,
        session_id: &str,
    ) -> Result<TerminalSessionHandle, TerminalBrokerError> {
        self.sessions
            .read()
            .await
            .get(session_id)
            .cloned()
            .ok_or(TerminalBrokerError::SessionNotFound)
    }

    fn session_handle_blocking(
        &self,
        session_id: &str,
    ) -> Result<TerminalSessionHandle, TerminalBrokerError> {
        loop {
            if let Ok(sessions) = self.sessions.try_read() {
                return sessions
                    .get(session_id)
                    .cloned()
                    .ok_or(TerminalBrokerError::SessionNotFound);
            }
            std::thread::yield_now();
        }
    }
}

fn ensure_generation(
    handle: &TerminalSessionHandle,
    received: u64,
) -> Result<(), TerminalBrokerError> {
    if received == handle.runtime_generation {
        Ok(())
    } else {
        Err(TerminalBrokerError::StaleRuntimeGeneration {
            expected: handle.runtime_generation,
            received,
        })
    }
}

fn terminated_batch(handle: &TerminalSessionHandle, after_sequence: u64) -> TerminalEventBatch {
    let latest_sequence = handle.latest_sequence.load(Ordering::SeqCst);
    TerminalEventBatch {
        status: TerminalEventBatchStatus::Terminated,
        runtime_generation: handle.runtime_generation,
        events: Vec::new(),
        next_sequence: after_sequence,
        available_from_sequence: latest_sequence.saturating_add(1),
        latest_sequence,
        recovery_snapshot: None,
    }
}

enum TerminalSessionMessage {
    Output {
        runtime_generation: u64,
        bytes: Vec<u8>,
        reply: BrokerReply<()>,
    },
    Register {
        request: TerminalPresentationRegistration,
        identity: TerminalClientIdentity,
        reply: BrokerReply<TerminalPresentationRegistrationResult>,
    },
    UpdatePresentation {
        request: TerminalPresentationUpdateRequest,
        identity: TerminalClientIdentity,
        reply: BrokerReply<TerminalPresentationUpdateResult>,
    },
    UnregisterPresentation {
        session_id: String,
        presentation_id: String,
        runtime_generation: u64,
        reply: BrokerReply<TerminalBrokerState>,
    },
    ReportViewport {
        request: TerminalPresentationViewportRequest,
        reply: BrokerReply<TerminalPresentationState>,
    },
    BeginActivation {
        request: TerminalActivationBeginRequest,
        reply: BrokerReply<TerminalActivationBeginResult>,
    },
    AckActivation {
        request: TerminalActivationAckRequest,
        arrival: ActivationAckArrival,
        reply: BrokerReply<TerminalActivationAckResult>,
    },
    Input {
        request: TerminalInputRequest,
        reply: BrokerReply<TerminalLeaseDecision>,
    },
    Resize {
        request: TerminalGeometryRequest,
        reply: BrokerReply<TerminalGeometryCommitResult>,
    },
    CompatibilityInput {
        bytes: Vec<u8>,
        reply: BrokerReply<TerminalLeaseDecision>,
    },
    CompatibilityResize {
        geometry: TerminalGeometry,
        reply: BrokerReply<TerminalGeometryCommitResult>,
    },
    PrivilegedInput {
        bytes: Vec<u8>,
        reply: BrokerReply<()>,
    },
    ReadCompatibilityOutput {
        max_bytes: Option<usize>,
        peek: bool,
        reply: BrokerReply<Option<String>>,
    },
    Subscribe {
        request: TerminalEventSubscriptionRequest,
        reply: BrokerReply<TerminalEventSubscriptionResult>,
    },
    ReadEvents {
        request: TerminalEventReadRequest,
        reply: BrokerReply<TerminalEventBatch>,
    },
    AckEvents {
        request: TerminalEventAckRequest,
        reply: BrokerReply<TerminalEventAckResult>,
    },
    Unsubscribe {
        request: TerminalEventUnsubscribeRequest,
        reply: BrokerReply<()>,
    },
    ConsumerAcknowledgements(BrokerReply<HashMap<String, u64>>),
    BrokerState(BrokerReply<TerminalBrokerState>),
    Snapshot(BrokerReply<TerminalSnapshot>),
    Pause {
        session_id: String,
        runtime_generation: u64,
        reply: BrokerReply<TerminalBrokerState>,
    },
    BeginOwnerResync {
        request: TerminalOwnerResyncBeginRequest,
        reply: BrokerReply<TerminalOwnerResyncBeginResult>,
    },
    AckOwnerResync {
        request: TerminalOwnerResyncAckRequest,
        reply: BrokerReply<TerminalOwnerResyncAckResult>,
    },
    #[cfg(test)]
    TestBlock {
        entered: oneshot::Sender<()>,
        release: std::sync::mpsc::Receiver<()>,
    },
    Shutdown {
        lifecycle: TerminalSessionLifecycleEvent,
        reply: oneshot::Sender<()>,
    },
}

struct PresentationRecord {
    state: TerminalPresentationState,
    last_geometry_sequence: u64,
    fallback_promotion_eligible: bool,
}

struct ConsumerRecord {
    client_kind: TerminalClientKind,
    acknowledged_sequence: u64,
}

struct PendingActivation {
    state: TerminalPendingActivationState,
    observed_lease_epoch: u64,
    begin_result: TerminalActivationBeginResult,
}

struct CompletedActivation {
    request: TerminalActivationAckRequest,
    result: TerminalActivationAckResult,
}

struct PendingOwnerResync {
    presentation_id: String,
    runtime_generation: u64,
    lease_epoch: u64,
    resync_id: String,
    begin_result: TerminalOwnerResyncBeginResult,
}

struct TerminalSessionActor {
    session_id: String,
    runtime_generation: u64,
    lease_epoch_shared: Arc<AtomicU64>,
    latest_sequence_shared: Arc<AtomicU64>,
    terminated: Arc<AtomicBool>,
    runtime: Option<TerminalRuntimeHandles>,
    runtime_state: TerminalRuntimeState,
    parser: vt100::Parser,
    replay: ReplayRing,
    presentations: HashMap<String, PresentationRecord>,
    consumers: HashMap<String, ConsumerRecord>,
    owner_presentation_id: Option<String>,
    pending_activation: Option<PendingActivation>,
    pending_owner_resync: Option<PendingOwnerResync>,
    completed_activation: Option<CompletedActivation>,
    lease_epoch: u64,
    stream_sequence: u64,
    interaction_sequence: u64,
    activation_sequence: u64,
    resync_sequence: u64,
    snapshot_sequence: u64,
    geometry: TerminalGeometry,
    compatibility_geometry_sequence: u64,
    compatibility_read_sequence: u64,
    wake_pending: bool,
    activation_timer: Option<AbortOnDropTask>,
    wake_timer: Option<AbortOnDropTask>,
    control: Arc<TerminalControlPlane>,
    wake_tx: broadcast::Sender<TerminalEventsReady>,
    lifecycle_tx: broadcast::Sender<TerminalSessionLifecycleNotification>,
    timer: Arc<dyn TerminalTimer>,
}

impl TerminalSessionActor {
    #[allow(clippy::too_many_arguments)]
    fn new(
        session_id: String,
        runtime_generation: u64,
        lease_epoch: Arc<AtomicU64>,
        latest_sequence: Arc<AtomicU64>,
        terminated: Arc<AtomicBool>,
        runtime: TerminalRuntimeHandles,
        geometry: TerminalGeometry,
        control: Arc<TerminalControlPlane>,
        wake_tx: broadcast::Sender<TerminalEventsReady>,
        lifecycle_tx: broadcast::Sender<TerminalSessionLifecycleNotification>,
        timer: Arc<dyn TerminalTimer>,
    ) -> Self {
        let initial_lease_epoch = lease_epoch.load(Ordering::SeqCst);
        Self {
            session_id,
            runtime_generation,
            lease_epoch_shared: lease_epoch,
            latest_sequence_shared: latest_sequence,
            terminated,
            runtime: Some(runtime),
            runtime_state: TerminalRuntimeState::Live,
            parser: vt100::Parser::new(geometry.rows, geometry.cols, 1_000),
            replay: ReplayRing::new(),
            presentations: HashMap::new(),
            consumers: HashMap::new(),
            owner_presentation_id: None,
            pending_activation: None,
            pending_owner_resync: None,
            completed_activation: None,
            lease_epoch: initial_lease_epoch,
            stream_sequence: 0,
            interaction_sequence: 0,
            activation_sequence: 0,
            resync_sequence: 0,
            snapshot_sequence: 0,
            geometry,
            compatibility_geometry_sequence: 0,
            compatibility_read_sequence: 0,
            wake_pending: false,
            activation_timer: None,
            wake_timer: None,
            control,
            wake_tx,
            lifecycle_tx,
            timer,
        }
    }

    async fn run(mut self, mut rx: mpsc::Receiver<TerminalSessionMessage>) {
        let control = self.control.clone();
        let mut shutdown = None;
        loop {
            tokio::select! {
                biased;
                _ = control.notify.notified() => {
                    self.handle_control().await;
                }
                message = rx.recv() => {
                    let Some(message) = message else {
                        break;
                    };
                    if let TerminalSessionMessage::Shutdown { lifecycle, reply } = message {
                        shutdown = Some((lifecycle, reply));
                        break;
                    }
                    self.handle_message(message).await;
                }
            }
        }

        let lifecycle = shutdown
            .as_ref()
            .map_or(TerminalSessionLifecycleEvent::RuntimeTerminated, |value| {
                value.0
            });
        self.runtime_state = TerminalRuntimeState::Terminated;
        self.activation_timer = None;
        self.wake_timer = None;
        self.control.clear();
        self.pending_activation = None;
        self.pending_owner_resync = None;
        self.consumers.clear();
        self.terminated.store(true, Ordering::SeqCst);
        let _ = self.wake_tx.send(TerminalEventsReady {
            session_id: self.session_id.clone(),
            runtime_generation: self.runtime_generation,
            latest_sequence: self.stream_sequence,
        });
        let _ = self
            .lifecycle_tx
            .send(TerminalSessionLifecycleNotification {
                session_id: self.session_id.clone(),
                runtime_generation: self.runtime_generation,
                lifecycle,
            });
        if let Some((_, reply)) = shutdown {
            let _ = reply.send(());
        }
    }

    async fn handle_message(&mut self, message: TerminalSessionMessage) {
        match message {
            TerminalSessionMessage::Output {
                runtime_generation,
                bytes,
                reply,
            } => {
                let _ = reply.send(self.process_output(runtime_generation, bytes));
            }
            TerminalSessionMessage::Register {
                request,
                identity,
                reply,
            } => {
                let result = self.register(request, identity);
                let _ = reply.send(result);
            }
            TerminalSessionMessage::UpdatePresentation {
                request,
                identity,
                reply,
            } => {
                let result = self.update_presentation(request, identity).await;
                let _ = reply.send(result);
            }
            TerminalSessionMessage::UnregisterPresentation {
                session_id,
                presentation_id,
                runtime_generation,
                reply,
            } => {
                let result = self
                    .unregister(&session_id, &presentation_id, runtime_generation)
                    .await;
                let _ = reply.send(result);
            }
            TerminalSessionMessage::ReportViewport { request, reply } => {
                let result = self.report_viewport(request);
                let _ = reply.send(result);
            }
            TerminalSessionMessage::BeginActivation { request, reply } => {
                let result = self.begin_activation(request);
                let _ = reply.send(result);
            }
            TerminalSessionMessage::AckActivation {
                request,
                arrival,
                reply,
            } => {
                let result = self.ack_activation(request, &arrival).await;
                let _ = reply.send(result);
            }
            TerminalSessionMessage::Input { request, reply } => {
                let result = self.send_input(request).await;
                let _ = reply.send(result);
            }
            TerminalSessionMessage::Resize { request, reply } => {
                let result = self.resize(request).await;
                let _ = reply.send(result);
            }
            TerminalSessionMessage::CompatibilityInput { bytes, reply } => {
                let result = self.send_compatibility_input(bytes).await;
                let _ = reply.send(result);
            }
            TerminalSessionMessage::CompatibilityResize { geometry, reply } => {
                let result = self.resize_compatibility(geometry).await;
                let _ = reply.send(result);
            }
            TerminalSessionMessage::PrivilegedInput { bytes, reply } => {
                let result = self.send_privileged_input(bytes).await;
                let _ = reply.send(result);
            }
            TerminalSessionMessage::ReadCompatibilityOutput {
                max_bytes,
                peek,
                reply,
            } => {
                let result = self.read_compatibility_output(max_bytes, peek);
                let _ = reply.send(result);
            }
            TerminalSessionMessage::Subscribe { request, reply } => {
                let result = self.subscribe(request);
                let _ = reply.send(result);
            }
            TerminalSessionMessage::ReadEvents { request, reply } => {
                let result = self.read_events(request);
                let _ = reply.send(result);
            }
            TerminalSessionMessage::AckEvents { request, reply } => {
                let result = self.ack_events(request);
                let _ = reply.send(result);
            }
            TerminalSessionMessage::Unsubscribe { request, reply } => {
                let result = self.unsubscribe(request);
                let _ = reply.send(result);
            }
            TerminalSessionMessage::ConsumerAcknowledgements(reply) => {
                let acknowledgements = self
                    .consumers
                    .iter()
                    .map(|(id, consumer)| (id.clone(), consumer.acknowledged_sequence))
                    .collect();
                let _ = reply.send(Ok(acknowledgements));
            }
            TerminalSessionMessage::BrokerState(reply) => {
                let _ = reply.send(Ok(self.broker_state()));
            }
            TerminalSessionMessage::Snapshot(reply) => {
                let snapshot = self.snapshot();
                let _ = reply.send(Ok(snapshot));
            }
            TerminalSessionMessage::Pause {
                session_id,
                runtime_generation,
                reply,
            } => {
                let result = self.pause(&session_id, runtime_generation);
                let _ = reply.send(result);
            }
            TerminalSessionMessage::BeginOwnerResync { request, reply } => {
                let result = self.begin_owner_resync(request);
                let _ = reply.send(result);
            }
            TerminalSessionMessage::AckOwnerResync { request, reply } => {
                let result = self.ack_owner_resync(request);
                let _ = reply.send(result);
            }
            #[cfg(test)]
            TerminalSessionMessage::TestBlock { entered, release } => {
                let _ = entered.send(());
                let _ = tokio::task::spawn_blocking(move || release.recv()).await;
            }
            TerminalSessionMessage::Shutdown { .. } => unreachable!("handled by actor loop"),
        }
    }

    async fn handle_control(&mut self) {
        let due = self.control.take_due();
        if let Some(key) = due.activation_timeout {
            self.activation_timeout(&key).await;
        }
        if due.flush_wake {
            self.wake_pending = false;
            self.wake_timer = None;
            let _ = self.wake_tx.send(TerminalEventsReady {
                session_id: self.session_id.clone(),
                runtime_generation: self.runtime_generation,
                latest_sequence: self.stream_sequence,
            });
        }
    }

    fn process_output(
        &mut self,
        runtime_generation: u64,
        bytes: Vec<u8>,
    ) -> Result<(), TerminalBrokerError> {
        self.ensure_generation(runtime_generation)?;
        if self.runtime_state != TerminalRuntimeState::Live || self.runtime.is_none() {
            return Err(TerminalBrokerError::RuntimeUnavailable);
        }
        if bytes.is_empty() {
            return Ok(());
        }
        for chunk in bytes.chunks(MAX_BATCH_BYTES as usize) {
            self.parser.process(chunk);
            self.emit_event(TerminalBrokerEventKind::Output {
                bytes: chunk.to_vec(),
            });
        }
        Ok(())
    }

    fn register(
        &mut self,
        request: TerminalPresentationRegistration,
        identity: TerminalClientIdentity,
    ) -> Result<TerminalPresentationRegistrationResult, TerminalBrokerError> {
        self.ensure_session(&request.session_id)?;
        validate_id(&request.presentation_id, "presentation_id")?;
        identity.validate(request.client_kind, &request.presentation_id)?;
        let is_new = !self.presentations.contains_key(&request.presentation_id);
        if is_new {
            let limit = presentation_limit(request.client_kind);
            let count = self
                .presentations
                .values()
                .filter(|presentation| presentation.state.client_kind == request.client_kind)
                .count();
            if count >= limit {
                return Err(TerminalBrokerError::PresentationLimit {
                    client_kind: request.client_kind,
                    limit,
                });
            }
        } else if self
            .presentations
            .get(&request.presentation_id)
            .is_some_and(|record| record.state.client_kind != request.client_kind)
        {
            return Err(TerminalBrokerError::InvalidIdentity);
        }

        let existing = self.presentations.get(&request.presentation_id);
        let presentation_interaction_sequence = if let Some(record) = existing {
            record.state.interaction_sequence
        } else {
            self.interaction_sequence = self.interaction_sequence.saturating_add(1);
            self.interaction_sequence
        };
        let requires_resync = existing.is_some_and(|record| record.state.requires_resync)
            || request.render_state == TerminalRenderState::Suspended;
        let state = TerminalPresentationState {
            presentation_id: request.presentation_id.clone(),
            client_kind: request.client_kind,
            desired_geometry: request
                .desired_geometry
                .map(|geometry| clamp_geometry(geometry, request.client_kind)),
            visibility: request.visibility,
            render_state: request.render_state,
            interaction_capability: identity.effective_capability(request.requested_interaction),
            interaction_sequence: presentation_interaction_sequence,
            requires_resync,
        };
        let last_geometry_sequence = self
            .presentations
            .get(&request.presentation_id)
            .map_or(0, |record| record.last_geometry_sequence);
        let fallback_promotion_eligible = self
            .presentations
            .get(&request.presentation_id)
            .map_or_else(
                || identity.initially_fallback_promotion_eligible(),
                |record| record.fallback_promotion_eligible,
            );
        self.presentations.insert(
            request.presentation_id,
            PresentationRecord {
                state: state.clone(),
                last_geometry_sequence,
                fallback_promotion_eligible,
            },
        );
        if self
            .pending_owner_resync
            .as_ref()
            .is_some_and(|pending| pending.presentation_id == state.presentation_id)
        {
            self.pending_owner_resync = None;
        }
        let initial_snapshot = self.snapshot();
        Ok(TerminalPresentationRegistrationResult {
            presentation: state,
            broker_state: self.broker_state(),
            initial_snapshot,
        })
    }

    async fn update_presentation(
        &mut self,
        request: TerminalPresentationUpdateRequest,
        identity: TerminalClientIdentity,
    ) -> Result<TerminalPresentationUpdateResult, TerminalBrokerError> {
        self.ensure_session(&request.session_id)?;
        self.ensure_generation(request.runtime_generation)?;
        let client_kind = self
            .presentations
            .get(&request.presentation_id)
            .ok_or(TerminalBrokerError::PresentationNotFound)?
            .state
            .client_kind;
        identity.validate(client_kind, &request.presentation_id)?;
        let presentation = {
            let record = self
                .presentations
                .get_mut(&request.presentation_id)
                .ok_or(TerminalBrokerError::PresentationNotFound)?;
            record.state.desired_geometry = request
                .desired_geometry
                .map(|geometry| clamp_geometry(geometry, client_kind));
            record.state.visibility = request.visibility;
            record.state.render_state = request.render_state;
            if request.render_state == TerminalRenderState::Suspended {
                record.state.requires_resync = true;
            }
            record.state.interaction_capability =
                identity.effective_capability(request.requested_interaction);
            record.state.clone()
        };
        let pending_became_ineligible = self.pending_activation.as_ref().is_some_and(|pending| {
            pending.state.presentation_id == request.presentation_id
                && !self.presentation_is_eligible(&request.presentation_id)
        });
        if pending_became_ineligible {
            if let Some(pending) = self.pending_activation.take() {
                self.rollback_pending(pending).await?;
            }
        }
        if self
            .pending_owner_resync
            .as_ref()
            .is_some_and(|pending| pending.presentation_id == request.presentation_id)
        {
            self.pending_owner_resync = None;
        }
        Ok(TerminalPresentationUpdateResult {
            presentation,
            broker_state: self.broker_state(),
        })
    }

    async fn unregister(
        &mut self,
        session_id: &str,
        presentation_id: &str,
        runtime_generation: u64,
    ) -> Result<TerminalBrokerState, TerminalBrokerError> {
        self.ensure_session(session_id)?;
        self.ensure_generation(runtime_generation)?;
        self.presentations
            .remove(presentation_id)
            .ok_or(TerminalBrokerError::PresentationNotFound)?;
        if self
            .pending_owner_resync
            .as_ref()
            .is_some_and(|pending| pending.presentation_id == presentation_id)
        {
            self.pending_owner_resync = None;
        }

        if self
            .pending_activation
            .as_ref()
            .is_some_and(|pending| pending.state.presentation_id == presentation_id)
        {
            if let Some(pending) = self.pending_activation.take() {
                self.rollback_pending(pending).await?;
            }
        } else if let Some(pending) = self.pending_activation.as_mut() {
            if pending.state.previous_owner_presentation_id.as_deref() == Some(presentation_id) {
                pending.state.previous_owner_presentation_id = None;
            }
        }

        if self.owner_presentation_id.as_deref() == Some(presentation_id) {
            self.owner_presentation_id = None;
            self.advance_lease_epoch();
            self.promote_latest_eligible().await?;
        }
        Ok(self.broker_state())
    }

    fn report_viewport(
        &mut self,
        request: TerminalPresentationViewportRequest,
    ) -> Result<TerminalPresentationState, TerminalBrokerError> {
        self.ensure_session(&request.session_id)?;
        self.ensure_generation(request.runtime_generation)?;
        let record = self
            .presentations
            .get_mut(&request.presentation_id)
            .ok_or(TerminalBrokerError::PresentationNotFound)?;
        record.state.desired_geometry = Some(clamp_geometry(
            TerminalGeometry {
                cols: request.cols,
                rows: request.rows,
            },
            record.state.client_kind,
        ));
        Ok(record.state.clone())
    }

    fn begin_activation(
        &mut self,
        request: TerminalActivationBeginRequest,
    ) -> Result<TerminalActivationBeginResult, TerminalBrokerError> {
        self.ensure_session(&request.session_id)?;
        if request.runtime_generation != self.runtime_generation {
            return Ok(self.rejected_begin(TerminalLeaseRejectionReason::GenerationChanged));
        }
        if self.runtime_state != TerminalRuntimeState::Live {
            return Ok(self.rejected_begin(TerminalLeaseRejectionReason::RuntimeUnavailable));
        }
        if let Some(pending) = self.pending_activation.as_ref() {
            if pending.state.presentation_id == request.presentation_id
                && (request.observed_lease_epoch == pending.observed_lease_epoch
                    || request.observed_lease_epoch == pending.state.lease_epoch)
            {
                return Ok(pending.begin_result.clone());
            }
        }
        if !self.presentations.contains_key(&request.presentation_id) {
            return Ok(self.rejected_begin(TerminalLeaseRejectionReason::PresentationNotFound));
        }
        if !self.presentation_is_eligible(&request.presentation_id) {
            return Ok(self.rejected_begin(TerminalLeaseRejectionReason::PresentationIneligible));
        }
        if request.observed_lease_epoch != self.lease_epoch {
            return Ok(self.rejected_begin(TerminalLeaseRejectionReason::LeaseEpochChanged));
        }

        let previous_pending = self.pending_activation.take();
        if let Some(pending) = previous_pending.as_ref() {
            self.cancel_activation_timeout(&ActivationKey::from_pending(pending));
        }
        let previous_owner_presentation_id = previous_pending
            .and_then(|pending| pending.state.previous_owner_presentation_id)
            .or_else(|| self.owner_presentation_id.take());
        self.advance_lease_epoch();
        self.activation_sequence = self.activation_sequence.saturating_add(1);
        self.interaction_sequence = self.interaction_sequence.saturating_add(1);
        if let Some(record) = self.presentations.get_mut(&request.presentation_id) {
            record.state.interaction_sequence = self.interaction_sequence;
        }
        let activation_id = format!(
            "terminal-activation-{}-{}-{}",
            self.runtime_generation, self.lease_epoch, self.activation_sequence
        );
        let pending_state = TerminalPendingActivationState {
            presentation_id: request.presentation_id.clone(),
            previous_owner_presentation_id,
            runtime_generation: self.runtime_generation,
            lease_epoch: self.lease_epoch,
            activation_id: activation_id.clone(),
        };
        self.emit_event(TerminalBrokerEventKind::Ownership {
            owner_presentation_id: None,
            lease_epoch: self.lease_epoch,
            activation_id: Some(activation_id.clone()),
        });
        let snapshot = self.snapshot();
        let begin_result = TerminalActivationBeginResult {
            decision: self.accepted_decision(),
            activation_id: Some(activation_id.clone()),
            sequence_barrier: snapshot.sequence_barrier,
            snapshot: Some(snapshot),
        };
        self.pending_activation = Some(PendingActivation {
            state: pending_state,
            observed_lease_epoch: request.observed_lease_epoch,
            begin_result: begin_result.clone(),
        });
        self.schedule_activation_timeout(ActivationKey {
            presentation_id: request.presentation_id,
            runtime_generation: self.runtime_generation,
            lease_epoch: self.lease_epoch,
            activation_id,
        });
        Ok(begin_result)
    }

    async fn ack_activation(
        &mut self,
        request: TerminalActivationAckRequest,
        arrival: &ActivationAckArrival,
    ) -> Result<TerminalActivationAckResult, TerminalBrokerError> {
        self.ensure_session(&request.session_id)?;
        if arrival.matching_after_deadline() {
            let key = ActivationKey::from_ack(&request);
            if self.control.claim_unreserved_activation_timeout(&key) {
                self.activation_timeout(&key).await;
            }
            return Ok(self.rejected_ack(TerminalLeaseRejectionReason::StaleActivation));
        }
        if let Some(completed) = self.completed_activation.as_ref() {
            if completed.request == request {
                return Ok(completed.result.clone());
            }
        }
        if request.runtime_generation != self.runtime_generation {
            return Ok(self.rejected_ack(TerminalLeaseRejectionReason::GenerationChanged));
        }
        let matches_pending = self.pending_activation.as_ref().is_some_and(|pending| {
            pending.state.presentation_id == request.presentation_id
                && pending.state.runtime_generation == request.runtime_generation
                && pending.state.lease_epoch == request.lease_epoch
                && pending.state.activation_id == request.activation_id
        });
        if !matches_pending {
            return Ok(self.rejected_ack(TerminalLeaseRejectionReason::StaleActivation));
        }
        if !arrival.matching_before_deadline() {
            return Ok(self.rejected_ack(TerminalLeaseRejectionReason::StaleActivation));
        }
        if !self.presentation_is_eligible(&request.presentation_id) {
            if let Some(pending) = self.pending_activation.take() {
                self.rollback_pending(pending).await?;
            }
            return Ok(self.rejected_ack(TerminalLeaseRejectionReason::PresentationIneligible));
        }

        let desired = self
            .presentations
            .get(&request.presentation_id)
            .and_then(|record| record.state.desired_geometry);
        if let Some(geometry) = desired {
            if let Err(error) = self.commit_geometry(geometry, 0).await {
                if let Some(pending) = self.pending_activation.take() {
                    let _ = self.rollback_pending(pending).await;
                }
                return Err(error);
            }
        }
        if let Some(record) = self.presentations.get_mut(&request.presentation_id) {
            record.state.requires_resync = false;
            record.fallback_promotion_eligible = true;
        }
        self.cancel_activation_timeout(&ActivationKey::from_ack(&request));
        self.pending_activation = None;
        self.owner_presentation_id = Some(request.presentation_id.clone());
        self.emit_event(TerminalBrokerEventKind::Ownership {
            owner_presentation_id: self.owner_presentation_id.clone(),
            lease_epoch: self.lease_epoch,
            activation_id: Some(request.activation_id.clone()),
        });
        let snapshot = self.snapshot();
        let result = TerminalActivationAckResult {
            decision: self.accepted_decision(),
            broker_state: self.broker_state(),
            snapshot: Some(snapshot),
        };
        self.completed_activation = Some(CompletedActivation {
            request,
            result: result.clone(),
        });
        Ok(result)
    }

    fn begin_owner_resync(
        &mut self,
        request: TerminalOwnerResyncBeginRequest,
    ) -> Result<TerminalOwnerResyncBeginResult, TerminalBrokerError> {
        self.ensure_session(&request.session_id)?;
        if request.runtime_generation != self.runtime_generation {
            return Ok(
                self.rejected_owner_resync_begin(TerminalLeaseRejectionReason::GenerationChanged)
            );
        }
        if self.runtime_state != TerminalRuntimeState::Live {
            return Ok(
                self.rejected_owner_resync_begin(TerminalLeaseRejectionReason::RuntimeUnavailable)
            );
        }
        if self.pending_activation.is_some() {
            return Ok(
                self.rejected_owner_resync_begin(TerminalLeaseRejectionReason::PendingActivation)
            );
        }
        if let Some(pending) = self.pending_owner_resync.as_ref() {
            if pending.presentation_id == request.presentation_id
                && pending.runtime_generation == request.runtime_generation
                && pending.lease_epoch == request.lease_epoch
            {
                return Ok(pending.begin_result.clone());
            }
        }
        if self.owner_presentation_id.as_deref() != Some(&request.presentation_id) {
            return Ok(self.rejected_owner_resync_begin(TerminalLeaseRejectionReason::NotOwner));
        }
        if request.lease_epoch != self.lease_epoch {
            return Ok(
                self.rejected_owner_resync_begin(TerminalLeaseRejectionReason::LeaseEpochChanged)
            );
        }
        let Some(record) = self.presentations.get(&request.presentation_id) else {
            return Ok(self
                .rejected_owner_resync_begin(TerminalLeaseRejectionReason::PresentationNotFound));
        };
        if !record.state.requires_resync {
            return Ok(
                self.rejected_owner_resync_begin(TerminalLeaseRejectionReason::ResyncNotRequired)
            );
        }
        if !self.presentation_is_eligible(&request.presentation_id) {
            return Ok(self.rejected_owner_resync_begin(
                TerminalLeaseRejectionReason::PresentationIneligible,
            ));
        }

        self.resync_sequence = self.resync_sequence.saturating_add(1);
        let resync_id = format!(
            "terminal-owner-resync-{}-{}-{}",
            self.runtime_generation, self.lease_epoch, self.resync_sequence
        );
        let snapshot = self.snapshot();
        let begin_result = TerminalOwnerResyncBeginResult {
            decision: self.accepted_decision(),
            resync_id: Some(resync_id.clone()),
            sequence_barrier: snapshot.sequence_barrier,
            snapshot: Some(snapshot),
        };
        self.pending_owner_resync = Some(PendingOwnerResync {
            presentation_id: request.presentation_id,
            runtime_generation: self.runtime_generation,
            lease_epoch: self.lease_epoch,
            resync_id,
            begin_result: begin_result.clone(),
        });
        Ok(begin_result)
    }

    fn ack_owner_resync(
        &mut self,
        request: TerminalOwnerResyncAckRequest,
    ) -> Result<TerminalOwnerResyncAckResult, TerminalBrokerError> {
        self.ensure_session(&request.session_id)?;
        let matches_pending = self.pending_owner_resync.as_ref().is_some_and(|pending| {
            pending.presentation_id == request.presentation_id
                && pending.runtime_generation == request.runtime_generation
                && pending.lease_epoch == request.lease_epoch
                && pending.resync_id == request.resync_id
        });
        if !matches_pending {
            return Ok(
                self.rejected_owner_resync_ack(TerminalLeaseRejectionReason::StaleOwnerResync)
            );
        }
        if request.runtime_generation != self.runtime_generation {
            return Ok(
                self.rejected_owner_resync_ack(TerminalLeaseRejectionReason::GenerationChanged)
            );
        }
        if request.lease_epoch != self.lease_epoch {
            return Ok(
                self.rejected_owner_resync_ack(TerminalLeaseRejectionReason::LeaseEpochChanged)
            );
        }
        if self.owner_presentation_id.as_deref() != Some(&request.presentation_id) {
            return Ok(self.rejected_owner_resync_ack(TerminalLeaseRejectionReason::NotOwner));
        }
        if !self.presentation_is_eligible(&request.presentation_id) {
            return Ok(self
                .rejected_owner_resync_ack(TerminalLeaseRejectionReason::PresentationIneligible));
        }
        if let Some(record) = self.presentations.get_mut(&request.presentation_id) {
            record.state.requires_resync = false;
        }
        self.pending_owner_resync = None;
        Ok(TerminalOwnerResyncAckResult {
            decision: self.accepted_decision(),
            broker_state: self.broker_state(),
        })
    }

    async fn send_input(
        &mut self,
        request: TerminalInputRequest,
    ) -> Result<TerminalLeaseDecision, TerminalBrokerError> {
        self.ensure_session(&request.lease.session_id)?;
        if let Some(reason) = self.validate_active_lease(&request.lease) {
            return Ok(self.rejected_decision(reason));
        }
        if !request.bytes.is_empty() {
            self.send_runtime_input(request.bytes).await?;
        }
        Ok(self.accepted_decision())
    }

    async fn resize(
        &mut self,
        request: TerminalGeometryRequest,
    ) -> Result<TerminalGeometryCommitResult, TerminalBrokerError> {
        self.ensure_session(&request.lease.session_id)?;
        if let Some(reason) = self.validate_active_lease(&request.lease) {
            return Ok(self.rejected_resize(reason, request.geometry_sequence));
        }
        let client_kind = self
            .presentations
            .get(&request.lease.presentation_id)
            .ok_or(TerminalBrokerError::PresentationNotFound)?
            .state
            .client_kind;
        let last_geometry_sequence = self
            .presentations
            .get(&request.lease.presentation_id)
            .map_or(0, |record| record.last_geometry_sequence);
        if request.geometry_sequence <= last_geometry_sequence {
            return Ok(self.rejected_resize(
                TerminalLeaseRejectionReason::StaleGeometrySequence,
                request.geometry_sequence,
            ));
        }
        let geometry = clamp_geometry(request.geometry, client_kind);
        self.commit_geometry(geometry, request.geometry_sequence)
            .await?;
        if let Some(record) = self.presentations.get_mut(&request.lease.presentation_id) {
            record.last_geometry_sequence = request.geometry_sequence;
            record.state.desired_geometry = Some(geometry);
        }
        let snapshot = self.snapshot();
        Ok(TerminalGeometryCommitResult {
            decision: self.accepted_decision(),
            geometry_sequence: request.geometry_sequence,
            geometry: self.geometry,
            snapshot: Some(snapshot),
        })
    }

    fn compatibility_rejection(&self) -> Option<TerminalLeaseRejectionReason> {
        if self.runtime_state != TerminalRuntimeState::Live {
            return Some(TerminalLeaseRejectionReason::RuntimeUnavailable);
        }
        if self.pending_activation.is_some() {
            return Some(TerminalLeaseRejectionReason::PendingActivation);
        }
        if self.owner_presentation_id.is_some() {
            return Some(TerminalLeaseRejectionReason::NotOwner);
        }
        None
    }

    async fn send_compatibility_input(
        &mut self,
        bytes: Vec<u8>,
    ) -> Result<TerminalLeaseDecision, TerminalBrokerError> {
        if let Some(reason) = self.compatibility_rejection() {
            return Ok(self.rejected_decision(reason));
        }
        if !bytes.is_empty() {
            self.send_runtime_input(bytes).await?;
        }
        Ok(self.accepted_decision())
    }

    async fn send_privileged_input(&mut self, bytes: Vec<u8>) -> Result<(), TerminalBrokerError> {
        if self.runtime_state != TerminalRuntimeState::Live {
            return Err(TerminalBrokerError::RuntimeUnavailable);
        }
        if !bytes.is_empty() {
            self.send_runtime_input(bytes).await?;
        }
        Ok(())
    }

    async fn send_runtime_input(&self, bytes: Vec<u8>) -> Result<(), TerminalBrokerError> {
        let input_tx = self
            .runtime
            .as_ref()
            .ok_or(TerminalBrokerError::RuntimeUnavailable)?
            .input_tx
            .clone();
        tokio::time::timeout(RUNTIME_INPUT_SEND_TIMEOUT, input_tx.send(bytes))
            .await
            .map_err(|_| TerminalBrokerError::RuntimeIo("input_timeout".to_string()))?
            .map_err(|_| TerminalBrokerError::RuntimeIo("input_channel_closed".to_string()))
    }

    fn read_compatibility_output(
        &mut self,
        max_bytes: Option<usize>,
        peek: bool,
    ) -> Result<Option<String>, TerminalBrokerError> {
        let available_from = self.replay.available_from_sequence(self.stream_sequence);
        if self.compatibility_read_sequence.saturating_add(1) < available_from {
            self.compatibility_read_sequence = available_from.saturating_sub(1);
        }
        let (mut bytes, next_sequence) = self
            .replay
            .raw_output_after(self.compatibility_read_sequence);
        if !peek {
            self.compatibility_read_sequence = next_sequence;
        }
        if let Some(limit) = max_bytes {
            if bytes.len() > limit {
                bytes = bytes.split_off(bytes.len().saturating_sub(limit));
                while !bytes.is_empty() && std::str::from_utf8(&bytes).is_err() {
                    bytes.remove(0);
                }
            }
        }
        if bytes.is_empty() {
            Ok(None)
        } else {
            Ok(Some(String::from_utf8_lossy(&bytes).into_owned()))
        }
    }

    async fn resize_compatibility(
        &mut self,
        geometry: TerminalGeometry,
    ) -> Result<TerminalGeometryCommitResult, TerminalBrokerError> {
        self.compatibility_geometry_sequence =
            self.compatibility_geometry_sequence.saturating_add(1);
        let geometry_sequence = self.compatibility_geometry_sequence;
        if let Some(reason) = self.compatibility_rejection() {
            return Ok(self.rejected_resize(reason, geometry_sequence));
        }
        let geometry = clamp_geometry(geometry, TerminalClientKind::Desktop);
        self.commit_geometry(geometry, geometry_sequence).await?;
        let snapshot = self.snapshot();
        Ok(TerminalGeometryCommitResult {
            decision: self.accepted_decision(),
            geometry_sequence,
            geometry: self.geometry,
            snapshot: Some(snapshot),
        })
    }

    fn subscribe(
        &mut self,
        request: TerminalEventSubscriptionRequest,
    ) -> Result<TerminalEventSubscriptionResult, TerminalBrokerError> {
        self.ensure_session(&request.session_id)?;
        validate_id(&request.consumer_id, "consumer_id")?;
        if request.client_kind == TerminalClientKind::Desktop
            && self.consumers.iter().any(|(id, consumer)| {
                id != &request.consumer_id && consumer.client_kind == TerminalClientKind::Desktop
            })
        {
            return Err(TerminalBrokerError::DesktopConsumerAlreadyRegistered);
        }
        self.consumers.insert(
            request.consumer_id,
            ConsumerRecord {
                client_kind: request.client_kind,
                acknowledged_sequence: 0,
            },
        );
        let initial_snapshot = self.snapshot();
        Ok(TerminalEventSubscriptionResult {
            broker_state: self.broker_state(),
            initial_snapshot,
        })
    }

    fn read_events(
        &mut self,
        request: TerminalEventReadRequest,
    ) -> Result<TerminalEventBatch, TerminalBrokerError> {
        self.ensure_session(&request.session_id)?;
        if request.runtime_generation != self.runtime_generation {
            let snapshot = self.snapshot();
            return Ok(TerminalEventBatch {
                status: TerminalEventBatchStatus::GenerationChanged,
                runtime_generation: self.runtime_generation,
                events: Vec::new(),
                next_sequence: snapshot.sequence_barrier,
                available_from_sequence: self.replay.available_from_sequence(self.stream_sequence),
                latest_sequence: self.stream_sequence,
                recovery_snapshot: Some(snapshot),
            });
        }
        if !self.consumers.contains_key(&request.consumer_id) {
            return Err(TerminalBrokerError::ConsumerNotFound);
        }
        let available_from_sequence = self.replay.available_from_sequence(self.stream_sequence);
        let cursor_has_gap = request.after_sequence > self.stream_sequence
            || request.after_sequence < available_from_sequence.saturating_sub(1);
        if cursor_has_gap {
            let snapshot = self.snapshot();
            return Ok(TerminalEventBatch {
                status: TerminalEventBatchStatus::Gap,
                runtime_generation: self.runtime_generation,
                events: Vec::new(),
                next_sequence: snapshot.sequence_barrier,
                available_from_sequence,
                latest_sequence: self.stream_sequence,
                recovery_snapshot: Some(snapshot),
            });
        }
        let events = self.replay.read_after(
            request.after_sequence,
            request.max_events,
            request.max_bytes,
        );
        if events.is_empty() && request.after_sequence < self.stream_sequence {
            let snapshot = self.snapshot();
            return Ok(TerminalEventBatch {
                status: TerminalEventBatchStatus::Gap,
                runtime_generation: self.runtime_generation,
                events: Vec::new(),
                next_sequence: snapshot.sequence_barrier,
                available_from_sequence,
                latest_sequence: self.stream_sequence,
                recovery_snapshot: Some(snapshot),
            });
        }
        let next_sequence = events
            .last()
            .map_or(request.after_sequence, |event| event.sequence);
        Ok(TerminalEventBatch {
            status: TerminalEventBatchStatus::Events,
            runtime_generation: self.runtime_generation,
            events,
            next_sequence,
            available_from_sequence,
            latest_sequence: self.stream_sequence,
            recovery_snapshot: None,
        })
    }

    fn ack_events(
        &mut self,
        request: TerminalEventAckRequest,
    ) -> Result<TerminalEventAckResult, TerminalBrokerError> {
        self.ensure_session(&request.session_id)?;
        self.ensure_generation(request.runtime_generation)?;
        let consumer = self
            .consumers
            .get_mut(&request.consumer_id)
            .ok_or(TerminalBrokerError::ConsumerNotFound)?;
        consumer.acknowledged_sequence = consumer
            .acknowledged_sequence
            .max(request.applied_sequence.min(self.stream_sequence));
        Ok(TerminalEventAckResult {
            accepted_sequence: consumer.acknowledged_sequence,
            latest_sequence: self.stream_sequence,
        })
    }

    fn unsubscribe(
        &mut self,
        request: TerminalEventUnsubscribeRequest,
    ) -> Result<(), TerminalBrokerError> {
        self.ensure_session(&request.session_id)?;
        self.consumers
            .remove(&request.consumer_id)
            .ok_or(TerminalBrokerError::ConsumerNotFound)?;
        Ok(())
    }

    fn pause(
        &mut self,
        session_id: &str,
        runtime_generation: u64,
    ) -> Result<TerminalBrokerState, TerminalBrokerError> {
        self.ensure_session(session_id)?;
        self.ensure_generation(runtime_generation)?;
        if self.runtime_state == TerminalRuntimeState::Live {
            self.runtime_state = TerminalRuntimeState::Paused;
            self.runtime.take();
            self.pending_owner_resync = None;
            self.emit_event(TerminalBrokerEventKind::Lifecycle {
                lifecycle: TerminalSessionLifecycleEvent::RuntimePaused,
            });
            let _ = self
                .lifecycle_tx
                .send(TerminalSessionLifecycleNotification {
                    session_id: self.session_id.clone(),
                    runtime_generation: self.runtime_generation,
                    lifecycle: TerminalSessionLifecycleEvent::RuntimePaused,
                });
        }
        Ok(self.broker_state())
    }

    async fn activation_timeout(&mut self, key: &ActivationKey) {
        let matches = self.pending_activation.as_ref().is_some_and(|pending| {
            pending.state.presentation_id == key.presentation_id
                && pending.state.runtime_generation == key.runtime_generation
                && pending.state.lease_epoch == key.lease_epoch
                && pending.state.activation_id == key.activation_id
        });
        if matches {
            if let Some(pending) = self.pending_activation.take() {
                let _ = self.rollback_pending(pending).await;
            }
        }
    }

    async fn rollback_pending(
        &mut self,
        pending: PendingActivation,
    ) -> Result<(), TerminalBrokerError> {
        self.cancel_activation_timeout(&ActivationKey::from_pending(&pending));
        self.owner_presentation_id = None;
        if let Some(previous_owner) = pending.state.previous_owner_presentation_id {
            if self.presentation_has_active_renderer(&previous_owner) {
                let desired = self
                    .presentations
                    .get(&previous_owner)
                    .and_then(|record| record.state.desired_geometry);
                if let Some(geometry) = desired {
                    self.commit_geometry(geometry, 0).await?;
                }
                self.owner_presentation_id = Some(previous_owner);
            }
        }
        self.emit_event(TerminalBrokerEventKind::Ownership {
            owner_presentation_id: self.owner_presentation_id.clone(),
            lease_epoch: self.lease_epoch,
            activation_id: None,
        });
        Ok(())
    }

    async fn promote_latest_eligible(&mut self) -> Result<(), TerminalBrokerError> {
        let candidate = self
            .presentations
            .iter()
            .filter(|(id, record)| {
                record.fallback_promotion_eligible && self.presentation_has_active_renderer(id)
            })
            .max_by_key(|(_, record)| record.state.interaction_sequence)
            .map(|(id, record)| (id.clone(), record.state.desired_geometry));
        if let Some((presentation_id, desired_geometry)) = candidate {
            if let Some(geometry) = desired_geometry {
                self.commit_geometry(geometry, 0).await?;
            }
            self.owner_presentation_id = Some(presentation_id);
        }
        self.emit_event(TerminalBrokerEventKind::Ownership {
            owner_presentation_id: self.owner_presentation_id.clone(),
            lease_epoch: self.lease_epoch,
            activation_id: None,
        });
        Ok(())
    }

    async fn commit_geometry(
        &mut self,
        geometry: TerminalGeometry,
        geometry_sequence: u64,
    ) -> Result<(), TerminalBrokerError> {
        if geometry == self.geometry {
            return Ok(());
        }
        let resize = self
            .runtime
            .as_ref()
            .ok_or(TerminalBrokerError::RuntimeUnavailable)?
            .resize
            .clone();
        tokio::task::spawn_blocking(move || resize(geometry))
            .await
            .map_err(|error| TerminalBrokerError::RuntimeIo(error.to_string()))?
            .map_err(TerminalBrokerError::RuntimeIo)?;
        self.parser
            .screen_mut()
            .set_size(geometry.rows, geometry.cols);
        self.geometry = geometry;
        self.emit_event(TerminalBrokerEventKind::Geometry {
            geometry,
            geometry_sequence,
        });
        Ok(())
    }

    fn validate_active_lease(
        &self,
        lease: &TerminalLeaseIdentity,
    ) -> Option<TerminalLeaseRejectionReason> {
        if lease.runtime_generation != self.runtime_generation {
            return Some(TerminalLeaseRejectionReason::GenerationChanged);
        }
        if self.runtime_state != TerminalRuntimeState::Live {
            return Some(TerminalLeaseRejectionReason::RuntimeUnavailable);
        }
        if self.pending_activation.is_some() {
            return Some(TerminalLeaseRejectionReason::PendingActivation);
        }
        if lease.lease_epoch != self.lease_epoch {
            return Some(TerminalLeaseRejectionReason::LeaseEpochChanged);
        }
        if !self.presentations.contains_key(&lease.presentation_id) {
            return Some(TerminalLeaseRejectionReason::PresentationNotFound);
        }
        if self.owner_presentation_id.as_deref() != Some(&lease.presentation_id) {
            return Some(TerminalLeaseRejectionReason::NotOwner);
        }
        if !self.presentation_has_active_renderer(&lease.presentation_id) {
            return Some(TerminalLeaseRejectionReason::PresentationIneligible);
        }
        None
    }

    fn presentation_is_eligible(&self, presentation_id: &str) -> bool {
        self.runtime_state == TerminalRuntimeState::Live
            && self
                .presentations
                .get(presentation_id)
                .is_some_and(|record| {
                    record.state.visibility == TerminalVisibility::Visible
                        && record.state.render_state == TerminalRenderState::Mounted
                        && record.state.interaction_capability
                            == TerminalInteractionCapability::Interactive
                })
    }

    fn presentation_has_active_renderer(&self, presentation_id: &str) -> bool {
        self.presentation_is_eligible(presentation_id)
            && self
                .presentations
                .get(presentation_id)
                .is_some_and(|record| !record.state.requires_resync)
    }

    fn emit_event(&mut self, event: TerminalBrokerEventKind) {
        self.stream_sequence = self.stream_sequence.saturating_add(1);
        let event = TerminalBrokerEvent {
            sequence: self.stream_sequence,
            runtime_generation: self.runtime_generation,
            event,
        };
        self.replay.push(event);
        self.latest_sequence_shared
            .store(self.stream_sequence, Ordering::SeqCst);
        self.schedule_wake();
    }

    fn schedule_wake(&mut self) {
        if self.wake_pending {
            return;
        }
        self.wake_pending = true;
        let timer = self.timer.clone();
        let control = self.control.clone();
        let delay = timer.sleep(EVENT_WAKE_COALESCE);
        self.wake_timer = Some(AbortOnDropTask(tokio::spawn(async move {
            delay.await;
            control.mark_wake_due();
        })));
    }

    fn schedule_activation_timeout(&mut self, key: ActivationKey) {
        self.activation_timer = None;
        self.control.start_activation(key.clone());
        let timer = self.timer.clone();
        let control = self.control.clone();
        let delay = timer.sleep(ACTIVATION_TIMEOUT);
        self.activation_timer = Some(AbortOnDropTask(tokio::spawn(async move {
            delay.await;
            control.activation_deadline_elapsed(&key);
        })));
    }

    fn cancel_activation_timeout(&mut self, key: &ActivationKey) {
        self.activation_timer = None;
        self.control.clear_activation(key);
    }

    fn snapshot(&mut self) -> TerminalSnapshot {
        self.snapshot_sequence = self.snapshot_sequence.saturating_add(1);
        build_snapshot(
            &self.session_id,
            self.runtime_generation,
            self.stream_sequence,
            self.geometry,
            self.parser.screen(),
            self.snapshot_sequence,
        )
    }

    fn broker_state(&self) -> TerminalBrokerState {
        TerminalBrokerState {
            session_id: self.session_id.clone(),
            runtime_generation: self.runtime_generation,
            lease_epoch: self.lease_epoch,
            stream_sequence: self.stream_sequence,
            interaction_sequence: self.interaction_sequence,
            geometry: self.geometry,
            owner_presentation_id: self.owner_presentation_id.clone(),
            pending_activation: self
                .pending_activation
                .as_ref()
                .map(|pending| pending.state.clone()),
            runtime_state: self.runtime_state,
        }
    }

    fn advance_lease_epoch(&mut self) {
        self.lease_epoch = self.lease_epoch.saturating_add(1);
        self.lease_epoch_shared
            .store(self.lease_epoch, Ordering::SeqCst);
        self.completed_activation = None;
        self.pending_owner_resync = None;
    }

    fn accepted_decision(&self) -> TerminalLeaseDecision {
        TerminalLeaseDecision::accepted(
            self.runtime_generation,
            self.lease_epoch,
            self.owner_presentation_id.clone(),
        )
    }

    fn rejected_decision(&self, reason: TerminalLeaseRejectionReason) -> TerminalLeaseDecision {
        TerminalLeaseDecision::rejected(
            reason,
            self.runtime_generation,
            self.lease_epoch,
            self.owner_presentation_id.clone(),
        )
    }

    fn rejected_begin(
        &self,
        reason: TerminalLeaseRejectionReason,
    ) -> TerminalActivationBeginResult {
        TerminalActivationBeginResult {
            decision: self.rejected_decision(reason),
            activation_id: None,
            snapshot: None,
            sequence_barrier: self.stream_sequence,
        }
    }

    fn rejected_ack(&self, reason: TerminalLeaseRejectionReason) -> TerminalActivationAckResult {
        TerminalActivationAckResult {
            decision: self.rejected_decision(reason),
            broker_state: self.broker_state(),
            snapshot: None,
        }
    }

    fn rejected_owner_resync_begin(
        &self,
        reason: TerminalLeaseRejectionReason,
    ) -> TerminalOwnerResyncBeginResult {
        TerminalOwnerResyncBeginResult {
            decision: self.rejected_decision(reason),
            resync_id: None,
            snapshot: None,
            sequence_barrier: self.stream_sequence,
        }
    }

    fn rejected_owner_resync_ack(
        &self,
        reason: TerminalLeaseRejectionReason,
    ) -> TerminalOwnerResyncAckResult {
        TerminalOwnerResyncAckResult {
            decision: self.rejected_decision(reason),
            broker_state: self.broker_state(),
        }
    }

    fn rejected_resize(
        &self,
        reason: TerminalLeaseRejectionReason,
        geometry_sequence: u64,
    ) -> TerminalGeometryCommitResult {
        TerminalGeometryCommitResult {
            decision: self.rejected_decision(reason),
            geometry_sequence,
            geometry: self.geometry,
            snapshot: None,
        }
    }

    fn ensure_generation(&self, received: u64) -> Result<(), TerminalBrokerError> {
        if received == self.runtime_generation {
            Ok(())
        } else {
            Err(TerminalBrokerError::StaleRuntimeGeneration {
                expected: self.runtime_generation,
                received,
            })
        }
    }

    fn ensure_session(&self, session_id: &str) -> Result<(), TerminalBrokerError> {
        if session_id == self.session_id {
            Ok(())
        } else {
            Err(TerminalBrokerError::SessionNotFound)
        }
    }
}

fn presentation_limit(client_kind: TerminalClientKind) -> usize {
    match client_kind {
        TerminalClientKind::Desktop => MAX_DESKTOP_PRESENTATIONS_PER_SESSION,
        TerminalClientKind::Remote => MAX_REMOTE_PRESENTATIONS_PER_SESSION,
    }
}

fn clamp_geometry(geometry: TerminalGeometry, client_kind: TerminalClientKind) -> TerminalGeometry {
    let (max_cols, max_rows) = match client_kind {
        TerminalClientKind::Desktop => (500, 200),
        TerminalClientKind::Remote => (240, 80),
    };
    TerminalGeometry {
        cols: geometry.cols.clamp(20, max_cols),
        rows: geometry.rows.clamp(8, max_rows),
    }
}

fn validate_id(value: &str, field: &'static str) -> Result<(), TerminalBrokerError> {
    if value.trim().is_empty()
        || value.len() > MAX_TERMINAL_IDENTIFIER_BYTES
        || value.chars().any(char::is_whitespace)
    {
        Err(TerminalBrokerError::InvalidRequest(field))
    } else {
        Ok(())
    }
}
