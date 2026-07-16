use crate::state::terminal_session::{
    TerminalBrokerError, TerminalClientIdentity, TerminalSessionBroker,
};
use crate::state::AppState;
use serde::Deserialize;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, State};
use wardian_core::models::*;

#[derive(Debug, Clone, Deserialize)]
pub struct TerminalPresentationUnregisterRequest {
    pub session_id: String,
    pub presentation_id: String,
    pub runtime_generation: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TerminalSnapshotRequest {
    pub session_id: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TerminalPresentationTextInputRequest {
    pub session_id: String,
    pub presentation_id: String,
    pub runtime_generation: u64,
    pub lease_epoch: u64,
    pub input: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TerminalPresentationBinaryInputRequest {
    pub session_id: String,
    pub presentation_id: String,
    pub runtime_generation: u64,
    pub lease_epoch: u64,
    pub input: Vec<u8>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TerminalPresentationResizeRequest {
    pub session_id: String,
    pub presentation_id: String,
    pub runtime_generation: u64,
    pub lease_epoch: u64,
    pub geometry_sequence: u64,
    pub cols: u16,
    pub rows: u16,
}

fn command_error(error: TerminalBrokerError) -> String {
    error.to_string()
}

pub(crate) async fn register_terminal_presentation_with_broker(
    broker: &TerminalSessionBroker,
    request: TerminalPresentationRegistration,
) -> Result<TerminalPresentationRegistrationResult, TerminalBrokerError> {
    let deferred = request.desired_geometry.map(|geometry| {
        (
            request.session_id.clone(),
            request.presentation_id.clone(),
            geometry,
        )
    });
    let result = broker
        .register_presentation(request, TerminalClientIdentity::trusted_desktop())
        .await;
    if matches!(result, Err(TerminalBrokerError::SessionNotFound)) {
        if let Some((session_id, presentation_id, geometry)) = deferred {
            broker
                .remember_deferred_geometry(&session_id, &presentation_id, geometry)
                .await?;
        }
    }
    result
}

pub(crate) async fn report_terminal_presentation_viewport_with_broker(
    broker: &TerminalSessionBroker,
    request: TerminalPresentationViewportRequest,
) -> Result<TerminalPresentationState, TerminalBrokerError> {
    broker.report_presentation_viewport(request).await
}

pub(crate) async fn begin_terminal_activation_with_broker(
    broker: &TerminalSessionBroker,
    request: TerminalActivationBeginRequest,
) -> Result<TerminalActivationBeginResult, TerminalBrokerError> {
    broker.begin_activation(request).await
}

pub(crate) async fn ack_terminal_activation_with_broker(
    broker: &TerminalSessionBroker,
    request: TerminalActivationAckRequest,
) -> Result<TerminalActivationAckResult, TerminalBrokerError> {
    broker.ack_activation(request).await
}

pub(crate) async fn send_terminal_presentation_input_with_broker(
    broker: &TerminalSessionBroker,
    request: TerminalPresentationTextInputRequest,
) -> Result<TerminalLeaseDecision, TerminalBrokerError> {
    broker
        .send_input(TerminalInputRequest {
            lease: TerminalLeaseIdentity {
                session_id: request.session_id,
                presentation_id: request.presentation_id,
                runtime_generation: request.runtime_generation,
                lease_epoch: request.lease_epoch,
            },
            bytes: request.input.into_bytes(),
        })
        .await
}

pub(crate) async fn unregister_terminal_presentation_with_broker(
    broker: &TerminalSessionBroker,
    session_id: &str,
    presentation_id: &str,
    runtime_generation: u64,
) -> Result<TerminalBrokerState, TerminalBrokerError> {
    let result = broker
        .unregister_presentation(session_id, presentation_id, runtime_generation)
        .await;
    if matches!(result, Err(TerminalBrokerError::SessionNotFound)) {
        broker
            .forget_deferred_presentation(session_id, presentation_id)
            .await;
    }
    result
}

#[tauri::command]
pub async fn register_terminal_presentation(
    request: TerminalPresentationRegistration,
    state: State<'_, AppState>,
) -> Result<TerminalPresentationRegistrationResult, String> {
    register_terminal_presentation_with_broker(&state.terminal_sessions, request)
        .await
        .map_err(command_error)
}

#[tauri::command]
pub async fn update_terminal_presentation(
    request: TerminalPresentationUpdateRequest,
    state: State<'_, AppState>,
) -> Result<TerminalPresentationUpdateResult, String> {
    state
        .terminal_sessions
        .update_presentation(request, TerminalClientIdentity::trusted_desktop())
        .await
        .map_err(command_error)
}

#[tauri::command]
pub async fn unregister_terminal_presentation(
    request: TerminalPresentationUnregisterRequest,
    state: State<'_, AppState>,
) -> Result<TerminalBrokerState, String> {
    unregister_terminal_presentation_with_broker(
        &state.terminal_sessions,
        &request.session_id,
        &request.presentation_id,
        request.runtime_generation,
    )
    .await
    .map_err(command_error)
}

#[tauri::command]
pub async fn report_terminal_presentation_viewport(
    request: TerminalPresentationViewportRequest,
    state: State<'_, AppState>,
) -> Result<TerminalPresentationState, String> {
    report_terminal_presentation_viewport_with_broker(&state.terminal_sessions, request)
        .await
        .map_err(command_error)
}

#[tauri::command]
pub async fn begin_terminal_activation(
    request: TerminalActivationBeginRequest,
    state: State<'_, AppState>,
) -> Result<TerminalActivationBeginResult, String> {
    begin_terminal_activation_with_broker(&state.terminal_sessions, request)
        .await
        .map_err(command_error)
}

#[tauri::command]
pub async fn ack_terminal_activation(
    request: TerminalActivationAckRequest,
    state: State<'_, AppState>,
) -> Result<TerminalActivationAckResult, String> {
    ack_terminal_activation_with_broker(&state.terminal_sessions, request)
        .await
        .map_err(command_error)
}

#[tauri::command]
pub async fn begin_terminal_owner_resync(
    request: TerminalOwnerResyncBeginRequest,
    state: State<'_, AppState>,
) -> Result<TerminalOwnerResyncBeginResult, String> {
    state
        .terminal_sessions
        .begin_owner_resync(request)
        .await
        .map_err(command_error)
}

#[tauri::command]
pub async fn ack_terminal_owner_resync(
    request: TerminalOwnerResyncAckRequest,
    state: State<'_, AppState>,
) -> Result<TerminalOwnerResyncAckResult, String> {
    state
        .terminal_sessions
        .ack_owner_resync(request)
        .await
        .map_err(command_error)
}

#[tauri::command]
pub async fn request_terminal_snapshot(
    request: TerminalSnapshotRequest,
    state: State<'_, AppState>,
) -> Result<TerminalSnapshot, String> {
    state
        .terminal_sessions
        .snapshot(&request.session_id)
        .await
        .map_err(command_error)
}

#[tauri::command]
pub async fn subscribe_terminal_events(
    mut request: TerminalEventSubscriptionRequest,
    state: State<'_, AppState>,
) -> Result<TerminalEventSubscriptionResult, String> {
    request.client_kind = TerminalClientKind::Desktop;
    state
        .terminal_sessions
        .subscribe(request)
        .await
        .map_err(command_error)
}

#[tauri::command]
pub async fn read_terminal_events(
    request: TerminalEventReadRequest,
    state: State<'_, AppState>,
) -> Result<TerminalEventBatch, String> {
    state
        .terminal_sessions
        .read_events(request)
        .await
        .map_err(command_error)
}

#[tauri::command]
pub async fn ack_terminal_events(
    request: TerminalEventAckRequest,
    state: State<'_, AppState>,
) -> Result<TerminalEventAckResult, String> {
    state
        .terminal_sessions
        .ack_events(request)
        .await
        .map_err(command_error)
}

#[tauri::command]
pub async fn unsubscribe_terminal_events(
    request: TerminalEventUnsubscribeRequest,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state
        .terminal_sessions
        .unsubscribe(request)
        .await
        .map_err(command_error)
}

#[tauri::command]
pub async fn send_terminal_presentation_input(
    request: TerminalPresentationTextInputRequest,
    state: State<'_, AppState>,
) -> Result<TerminalLeaseDecision, String> {
    send_terminal_presentation_input_with_broker(&state.terminal_sessions, request)
        .await
        .map_err(command_error)
}

#[tauri::command]
pub async fn send_terminal_presentation_binary(
    request: TerminalPresentationBinaryInputRequest,
    state: State<'_, AppState>,
) -> Result<TerminalLeaseDecision, String> {
    state
        .terminal_sessions
        .send_input(TerminalInputRequest {
            lease: TerminalLeaseIdentity {
                session_id: request.session_id,
                presentation_id: request.presentation_id,
                runtime_generation: request.runtime_generation,
                lease_epoch: request.lease_epoch,
            },
            bytes: request.input,
        })
        .await
        .map_err(command_error)
}

#[tauri::command]
pub async fn resize_terminal_presentation(
    request: TerminalPresentationResizeRequest,
    state: State<'_, AppState>,
) -> Result<TerminalGeometryCommitResult, String> {
    state
        .terminal_sessions
        .resize(TerminalGeometryRequest {
            lease: TerminalLeaseIdentity {
                session_id: request.session_id,
                presentation_id: request.presentation_id,
                runtime_generation: request.runtime_generation,
                lease_epoch: request.lease_epoch,
            },
            geometry_sequence: request.geometry_sequence,
            geometry: TerminalGeometry {
                cols: request.cols,
                rows: request.rows,
            },
        })
        .await
        .map_err(command_error)
}

/// Bridges broker wake/lifecycle broadcasts into coalesced Tauri events. Event
/// payloads are hints only; consumers still pull the bounded cursor protocol.
pub fn start_terminal_session_event_bridge(app: AppHandle, broker: Arc<TerminalSessionBroker>) {
    let mut wakeups = broker.subscribe_wakeups();
    let wake_app = app.clone();
    tauri::async_runtime::spawn(async move {
        loop {
            match wakeups.recv().await {
                Ok(event) => {
                    let _ = wake_app.emit("terminal-session-events-ready", event);
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    });

    let mut lifecycle = broker.subscribe_lifecycle();
    tauri::async_runtime::spawn(async move {
        loop {
            match lifecycle.recv().await {
                Ok(event) => {
                    let _ = app.emit("terminal-session-lifecycle", event);
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::terminal_session::{TerminalRuntimeHandles, TerminalSessionBroker};
    use std::sync::{Arc, Mutex};
    use tokio::sync::mpsc;

    fn geometry(cols: u16, rows: u16) -> TerminalGeometry {
        TerminalGeometry { cols, rows }
    }

    async fn runtime() -> (
        TerminalSessionBroker,
        u64,
        mpsc::Receiver<Vec<u8>>,
        Arc<Mutex<Vec<TerminalGeometry>>>,
    ) {
        let broker = TerminalSessionBroker::default();
        let (input_tx, input_rx) = mpsc::channel(8);
        let resizes = Arc::new(Mutex::new(Vec::new()));
        let observed = resizes.clone();
        let generation = broker
            .start_or_replace_runtime(
                "command-session",
                TerminalRuntimeHandles::new(input_tx, move |geometry| {
                    observed.lock().expect("resize log").push(geometry);
                    Ok(())
                }),
                geometry(80, 24),
            )
            .await
            .expect("runtime");
        (broker, generation, input_rx, resizes)
    }

    #[tokio::test]
    async fn terminal_session_desktop_commands_return_nonfatal_mirror_decisions() {
        let (broker, generation, mut input_rx, resizes) = runtime().await;
        for presentation_id in ["owner", "mirror"] {
            register_terminal_presentation_with_broker(
                &broker,
                TerminalPresentationRegistration {
                    presentation_id: presentation_id.to_string(),
                    session_id: "command-session".to_string(),
                    client_kind: TerminalClientKind::Desktop,
                    desired_geometry: Some(geometry(100, 30)),
                    visibility: TerminalVisibility::Visible,
                    render_state: TerminalRenderState::Mounted,
                    requested_interaction: TerminalRequestedInteraction::Interactive,
                    observed_lease_epoch: 0,
                },
            )
            .await
            .expect("desktop registration");
        }
        report_terminal_presentation_viewport_with_broker(
            &broker,
            TerminalPresentationViewportRequest {
                session_id: "command-session".to_string(),
                presentation_id: "mirror".to_string(),
                runtime_generation: generation,
                cols: 140,
                rows: 50,
            },
        )
        .await
        .expect("mirror viewport");
        assert!(resizes.lock().expect("resize log").is_empty());

        let begin = begin_terminal_activation_with_broker(
            &broker,
            TerminalActivationBeginRequest {
                session_id: "command-session".to_string(),
                presentation_id: "owner".to_string(),
                runtime_generation: generation,
                observed_lease_epoch: 0,
            },
        )
        .await
        .expect("begin");
        let lease_epoch = begin.decision.lease_epoch;
        ack_terminal_activation_with_broker(
            &broker,
            TerminalActivationAckRequest {
                session_id: "command-session".to_string(),
                presentation_id: "owner".to_string(),
                runtime_generation: generation,
                lease_epoch,
                activation_id: begin.activation_id.expect("activation id"),
            },
        )
        .await
        .expect("ack");
        assert_eq!(
            resizes.lock().expect("resize log").as_slice(),
            &[geometry(100, 30)]
        );

        let mirror = send_terminal_presentation_input_with_broker(
            &broker,
            TerminalPresentationTextInputRequest {
                session_id: "command-session".to_string(),
                presentation_id: "mirror".to_string(),
                runtime_generation: generation,
                lease_epoch,
                input: "blocked".to_string(),
            },
        )
        .await
        .expect("structured mirror input");
        assert_eq!(mirror.status, TerminalLeaseDecisionStatus::Rejected);
        assert!(input_rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn terminal_session_unregistering_last_presentation_keeps_runtime_live() {
        let (broker, generation, _input_rx, _resizes) = runtime().await;
        register_terminal_presentation_with_broker(
            &broker,
            TerminalPresentationRegistration {
                presentation_id: "only".to_string(),
                session_id: "command-session".to_string(),
                client_kind: TerminalClientKind::Desktop,
                desired_geometry: None,
                visibility: TerminalVisibility::Hidden,
                render_state: TerminalRenderState::Suspended,
                requested_interaction: TerminalRequestedInteraction::ReadOnly,
                observed_lease_epoch: 0,
            },
        )
        .await
        .expect("register");
        let state = unregister_terminal_presentation_with_broker(
            &broker,
            "command-session",
            "only",
            generation,
        )
        .await
        .expect("unregister");
        assert_eq!(state.runtime_state, TerminalRuntimeState::Live);
        assert_eq!(
            broker
                .broker_state("command-session")
                .await
                .expect("runtime")
                .runtime_generation,
            generation
        );
    }

    #[tokio::test]
    async fn terminal_session_legacy_adapters_cannot_bypass_a_committed_owner() {
        let (broker, generation, mut input_rx, resizes) = runtime().await;
        let legacy_input = broker
            .send_legacy_input("command-session", b"legacy-before-owner".to_vec())
            .await
            .expect("legacy input decision");
        assert_eq!(legacy_input.status, TerminalLeaseDecisionStatus::Accepted);
        assert_eq!(
            input_rx.recv().await.as_deref(),
            Some(b"legacy-before-owner".as_slice())
        );
        let legacy_resize = broker
            .resize_legacy("command-session", geometry(90, 28))
            .await
            .expect("legacy resize decision");
        assert_eq!(
            legacy_resize.decision.status,
            TerminalLeaseDecisionStatus::Accepted
        );
        assert_eq!(
            resizes.lock().expect("resizes").as_slice(),
            &[geometry(90, 28)]
        );

        register_terminal_presentation_with_broker(
            &broker,
            TerminalPresentationRegistration {
                presentation_id: "owner".to_string(),
                session_id: "command-session".to_string(),
                client_kind: TerminalClientKind::Desktop,
                desired_geometry: Some(geometry(100, 30)),
                visibility: TerminalVisibility::Visible,
                render_state: TerminalRenderState::Mounted,
                requested_interaction: TerminalRequestedInteraction::Interactive,
                observed_lease_epoch: 0,
            },
        )
        .await
        .expect("register owner");
        let begin = broker
            .begin_activation(TerminalActivationBeginRequest {
                session_id: "command-session".to_string(),
                presentation_id: "owner".to_string(),
                runtime_generation: generation,
                observed_lease_epoch: 0,
            })
            .await
            .expect("begin owner");
        broker
            .ack_activation(TerminalActivationAckRequest {
                session_id: "command-session".to_string(),
                presentation_id: "owner".to_string(),
                runtime_generation: generation,
                lease_epoch: begin.decision.lease_epoch,
                activation_id: begin.activation_id.expect("activation id"),
            })
            .await
            .expect("ack owner");

        let blocked_input = broker
            .send_legacy_input("command-session", b"blocked".to_vec())
            .await
            .expect("structured legacy input rejection");
        let blocked_resize = broker
            .resize_legacy("command-session", geometry(120, 40))
            .await
            .expect("structured legacy resize rejection");
        assert_eq!(blocked_input.status, TerminalLeaseDecisionStatus::Rejected);
        assert_eq!(
            blocked_resize.decision.status,
            TerminalLeaseDecisionStatus::Rejected
        );
        assert!(input_rx.try_recv().is_err());
        assert_eq!(
            resizes.lock().expect("resizes").as_slice(),
            &[geometry(90, 28), geometry(100, 30)]
        );
    }

    #[tokio::test]
    async fn terminal_session_registration_seeds_deferred_geometry_before_runtime_spawn() {
        let broker = TerminalSessionBroker::default();
        let result = register_terminal_presentation_with_broker(
            &broker,
            TerminalPresentationRegistration {
                presentation_id: "restoring-surface".to_string(),
                session_id: "restoring-session".to_string(),
                client_kind: TerminalClientKind::Desktop,
                desired_geometry: Some(geometry(143, 47)),
                visibility: TerminalVisibility::Visible,
                render_state: TerminalRenderState::Mounted,
                requested_interaction: TerminalRequestedInteraction::Interactive,
                observed_lease_epoch: 0,
            },
        )
        .await;

        assert_eq!(result, Err(TerminalBrokerError::SessionNotFound));
        assert_eq!(
            broker
                .spawn_geometry("restoring-session")
                .await
                .expect("deferred geometry"),
            Some(geometry(143, 47))
        );
        broker.forget_deferred_geometry("restoring-session").await;
        assert_eq!(
            broker
                .spawn_geometry("restoring-session")
                .await
                .expect("forgotten geometry"),
            None
        );
    }

    #[tokio::test]
    async fn terminal_session_close_before_spawn_prunes_deferred_presentation_geometry() {
        let broker = TerminalSessionBroker::default();
        let registration = TerminalPresentationRegistration {
            presentation_id: "closed-before-spawn".to_string(),
            session_id: "restoring-session".to_string(),
            client_kind: TerminalClientKind::Desktop,
            desired_geometry: Some(geometry(150, 48)),
            visibility: TerminalVisibility::Visible,
            render_state: TerminalRenderState::Mounted,
            requested_interaction: TerminalRequestedInteraction::Interactive,
            observed_lease_epoch: 0,
        };
        assert_eq!(
            register_terminal_presentation_with_broker(&broker, registration).await,
            Err(TerminalBrokerError::SessionNotFound)
        );
        assert_eq!(
            unregister_terminal_presentation_with_broker(
                &broker,
                "restoring-session",
                "closed-before-spawn",
                0,
            )
            .await,
            Err(TerminalBrokerError::SessionNotFound)
        );
        assert_eq!(
            broker
                .spawn_geometry("restoring-session")
                .await
                .expect("pruned geometry"),
            None
        );
    }
}
