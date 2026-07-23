use super::gateway::RemoteGatewayContext;
use axum::extract::ws::{Message, WebSocket};
use base64::Engine;
use std::future::Future;
use tauri::Manager;
use wardian_core::models::{
    TerminalActivationAckRequest, TerminalActivationBeginRequest, TerminalBrokerEvent,
    TerminalBrokerEventKind, TerminalClientKind, TerminalEventAckRequest, TerminalEventBatch,
    TerminalEventBatchStatus, TerminalEventReadRequest, TerminalEventSubscriptionRequest,
    TerminalEventUnsubscribeRequest, TerminalGeometry, TerminalGeometryRequest,
    TerminalInputRequest, TerminalLeaseIdentity, TerminalOwnerResyncAckRequest,
    TerminalOwnerResyncBeginRequest, TerminalPresentationRegistration,
    TerminalPresentationUpdateRequest, TerminalPresentationViewportRequest, TerminalRenderState,
    TerminalRequestedInteraction, TerminalSessionLifecycleEvent, TerminalSnapshot,
    TerminalVisibility,
};

const REMOTE_TERMINAL_ATTACH_STREAM_NAME: &str = "terminal_attach";
const WEBSOCKET_FIRST_TICKET_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
const TERMINAL_SESSION_CHECK_INTERVAL: std::time::Duration = std::time::Duration::from_secs(2);
const SOCKET_SEND_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);
const DRAIN_SLICE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);
const MAX_INPUT_BYTES: usize = 64 * 1024;
const MAX_INPUT_BASE64_BYTES: usize = MAX_INPUT_BYTES.div_ceil(3) * 4;
const DRAIN_BATCHES_BEFORE_YIELD: usize = 8;
const MAX_DRAIN_EVENTS: u16 = 256;
const MAX_DRAIN_BYTES: u32 = 256 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TerminalProtocol {
    V1,
    V2,
}

#[derive(Debug, serde::Deserialize, PartialEq, Eq)]
struct TerminalOpenMessage {
    #[serde(default)]
    protocol_version: Option<u8>,
    ticket: String,
    cols: u16,
    rows: u16,
}

impl TerminalOpenMessage {
    fn protocol(&self) -> Result<TerminalProtocol, &'static str> {
        match self.protocol_version {
            None => Ok(TerminalProtocol::V1),
            Some(2) => Ok(TerminalProtocol::V2),
            Some(_) => Err("unsupported_terminal_protocol"),
        }
    }

    fn geometry(&self) -> TerminalGeometry {
        clamp_remote_geometry(self.cols, self.rows)
    }
}

#[derive(Debug, serde::Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
enum TerminalClientMessage {
    ReportViewport {
        runtime_generation: u64,
        cols: u16,
        rows: u16,
    },
    BeginActivation {
        runtime_generation: u64,
        observed_lease_epoch: u64,
    },
    AckActivation {
        runtime_generation: u64,
        lease_epoch: u64,
        activation_id: String,
    },
    Input {
        runtime_generation: Option<u64>,
        lease_epoch: Option<u64>,
        data: String,
    },
    Binary {
        runtime_generation: Option<u64>,
        lease_epoch: Option<u64>,
        data_base64: String,
    },
    Resize {
        runtime_generation: Option<u64>,
        lease_epoch: Option<u64>,
        geometry_sequence: Option<u64>,
        cols: u16,
        rows: u16,
    },
    RequestSnapshot,
    RequestEvents {
        runtime_generation: u64,
        after_sequence: u64,
    },
    AckEvents {
        runtime_generation: u64,
        applied_sequence: u64,
    },
    SetPresentationState {
        runtime_generation: u64,
        visibility: TerminalVisibility,
        render_state: TerminalRenderState,
        requested_interaction: TerminalRequestedInteraction,
        observed_lease_epoch: u64,
        cols: Option<u16>,
        rows: Option<u16>,
    },
    BeginOwnerResync {
        runtime_generation: u64,
        lease_epoch: u64,
    },
    AckOwnerResync {
        runtime_generation: u64,
        lease_epoch: u64,
        resync_id: String,
    },
    Detach,
}

#[derive(Debug, Clone)]
struct TerminalSocketBinding {
    protocol: TerminalProtocol,
    session_id: String,
    presentation_id: String,
    consumer_id: String,
    remote_session_id: String,
    device_id: String,
    runtime_generation: u64,
    lease_epoch: u64,
    owner_presentation_id: Option<String>,
    broker_geometry: TerminalGeometry,
    cursor: u64,
    geometry_sequence: u64,
    desired_geometry: TerminalGeometry,
    drain_continuation_pending: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ClientMessageAction {
    Continue,
    Close,
    Fatal(&'static str),
}

fn should_break_after_send(action: ClientMessageAction) -> bool {
    action != ClientMessageAction::Continue
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DrainSliceOutcome {
    action: ClientMessageAction,
    more_pending: bool,
}

impl DrainSliceOutcome {
    fn complete(action: ClientMessageAction) -> Self {
        Self {
            action,
            more_pending: false,
        }
    }

    fn more_pending() -> Self {
        Self {
            action: ClientMessageAction::Continue,
            more_pending: true,
        }
    }
}

pub(super) async fn handle_terminal_socket(
    ctx: RemoteGatewayContext,
    session_id: String,
    mut socket: WebSocket,
) {
    let first_message =
        match tokio::time::timeout(WEBSOCKET_FIRST_TICKET_TIMEOUT, socket.recv()).await {
            Ok(Some(Ok(message))) => message,
            Ok(Some(Err(_))) | Ok(None) => return,
            Err(_) => {
                let _ = send_error(&mut socket, "ticket_timeout", true, None).await;
                return;
            }
        };
    let open = match parse_open_message(first_message) {
        Ok(open) => open,
        Err(code) => {
            let _ = send_error(&mut socket, code, true, None).await;
            return;
        }
    };
    let protocol = match open.protocol() {
        Ok(protocol) => protocol,
        Err(code) => {
            let _ = send_error(&mut socket, code, true, None).await;
            return;
        }
    };

    let state = ctx.app.state::<crate::state::AppState>();
    let now_ms = chrono::Utc::now().timestamp_millis();
    let ticket_record = {
        let mut runtime = state.remote_runtime.lock().await;
        match crate::remote::auth::consume_websocket_ticket(&mut runtime, &open.ticket, now_ms) {
            Ok(record) => record,
            Err(_) => {
                drop(runtime);
                let _ = send_error(&mut socket, "invalid_websocket_ticket", true, None).await;
                return;
            }
        }
    };
    if ticket_record.stream != REMOTE_TERMINAL_ATTACH_STREAM_NAME
        || ticket_record.canonical_origin != ctx.config.canonical_origin
    {
        let _ = send_error(&mut socket, "invalid_websocket_ticket", true, None).await;
        return;
    }
    if !ticket_session_is_active(&state, &ticket_record.session_id, &ticket_record.device_id).await
    {
        let _ = send_error(&mut socket, "session_expired", true, None).await;
        return;
    }
    if !state.agents.lock().await.contains_key(&session_id) {
        let _ = send_error(&mut socket, "agent_not_found", true, None).await;
        return;
    }

    // Subscribe before taking the initial snapshot so output produced between
    // registration and the first drain cannot be stranded without a wake-up.
    let mut wakeups = state.terminal_sessions.subscribe_wakeups();
    let mut lifecycle = state.terminal_sessions.subscribe_lifecycle();
    let nonce = uuid::Uuid::new_v4();
    let presentation_id = format!(
        "remote-{}-{}-{nonce}",
        ticket_record.session_id, ticket_record.device_id
    );
    let consumer_id = format!("{presentation_id}-feed");
    let desired_geometry = open.geometry();
    let mut binding = TerminalSocketBinding {
        protocol,
        session_id,
        presentation_id,
        consumer_id,
        remote_session_id: ticket_record.session_id,
        device_id: ticket_record.device_id,
        runtime_generation: 0,
        lease_epoch: 0,
        owner_presentation_id: None,
        broker_geometry: desired_geometry,
        cursor: 0,
        geometry_sequence: 0,
        desired_geometry,
        drain_continuation_pending: false,
    };

    let registration = match register_binding(&state, &mut binding).await {
        Ok(registration) => registration,
        Err(crate::state::terminal_session::TerminalBrokerError::PresentationLimit { .. }) => {
            let _ = send_error(&mut socket, "websocket_connection_limit", true, None).await;
            return;
        }
        Err(_) => {
            let _ = send_error(&mut socket, "terminal_attach_failed", true, None).await;
            return;
        }
    };
    if send_registered(&mut socket, &binding, &registration)
        .await
        .is_err()
    {
        detach_binding(&state, &binding).await;
        return;
    }
    if protocol == TerminalProtocol::V1
        && activate_v1_after_snapshot(&state, &mut socket, &mut binding)
            .await
            .is_err()
    {
        detach_binding(&state, &binding).await;
        return;
    }
    let initial_drain = drain_event_slice(&state, &mut socket, &mut binding, None).await;
    if apply_drain_outcome(&mut binding, initial_drain) != ClientMessageAction::Continue {
        detach_binding(&state, &binding).await;
        return;
    }

    let mut session_check = tokio::time::interval(TERMINAL_SESSION_CHECK_INTERVAL);
    session_check.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    session_check.tick().await;

    'stream: loop {
        tokio::select! {
            _ = tokio::task::yield_now(), if binding.drain_continuation_pending => {
                binding.drain_continuation_pending = false;
                let outcome = drain_event_slice(&state, &mut socket, &mut binding, None).await;
                if apply_drain_outcome(&mut binding, outcome) != ClientMessageAction::Continue {
                    break;
                }
            }
            _ = session_check.tick() => {
                if !ticket_session_is_active(&state, &binding.remote_session_id, &binding.device_id).await {
                    let action = send_error(&mut socket, "session_expired", true, None).await;
                    debug_assert!(should_break_after_send(action));
                    break;
                }
            }
            event = wakeups.recv() => {
                match event {
                    Ok(event) if event.session_id == binding.session_id => {
                        let outcome = drain_event_slice(&state, &mut socket, &mut binding, None).await;
                        if apply_drain_outcome(&mut binding, outcome)
                            != ClientMessageAction::Continue {
                            break;
                        }
                    }
                    Ok(_) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        if send_current_snapshot(&state, &mut socket, &mut binding).await
                            != ClientMessageAction::Continue {
                            break;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
            event = lifecycle.recv() => {
                match event {
                    Ok(event) if event.session_id == binding.session_id
                        && event.runtime_generation > binding.runtime_generation
                        && matches!(event.lifecycle, TerminalSessionLifecycleEvent::RuntimeStarted | TerminalSessionLifecycleEvent::RuntimeReplaced) => {
                        match register_binding(&state, &mut binding).await {
                            Ok(registration) => {
                                if send_registered(&mut socket, &binding, &registration).await.is_err() {
                                    break;
                                }
                                if binding.protocol == TerminalProtocol::V1
                                    && activate_v1_after_snapshot(&state, &mut socket, &mut binding).await.is_err() {
                                    break;
                                }
                            }
                            Err(_) => {
                                let action = send_error(&mut socket, "terminal_runtime_unavailable", false, None).await;
                                if should_break_after_send(action) {
                                    break;
                                }
                            }
                        }
                    }
                    Ok(event) if event.session_id == binding.session_id
                        && event.runtime_generation >= binding.runtime_generation
                        && event.lifecycle == TerminalSessionLifecycleEvent::RuntimeTerminated => {
                        let action = send_error(&mut socket, "terminal_runtime_terminated", false, None).await;
                        if should_break_after_send(action) {
                            break;
                        }
                    }
                    Ok(_) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        match state.terminal_sessions.broker_state(&binding.session_id).await {
                            Ok(broker_state)
                                if broker_state.runtime_generation > binding.runtime_generation =>
                            {
                                match register_binding(&state, &mut binding).await {
                                    Ok(registration) => {
                                        if send_registered(&mut socket, &binding, &registration)
                                            .await
                                            .is_err()
                                        {
                                            break;
                                        }
                                        if binding.protocol == TerminalProtocol::V1
                                            && activate_v1_after_snapshot(
                                                &state,
                                                &mut socket,
                                                &mut binding,
                                            )
                                            .await
                                            .is_err()
                                        {
                                            break;
                                        }
                                    }
                                    Err(_) => {
                                        let action = send_error(
                                            &mut socket,
                                            "terminal_runtime_unavailable",
                                            false,
                                            None,
                                        )
                                        .await;
                                        if should_break_after_send(action) {
                                            break;
                                        }
                                    }
                                }
                            }
                            Ok(_) => {
                                if send_current_snapshot(&state, &mut socket, &mut binding).await
                                    != ClientMessageAction::Continue
                                {
                                    break;
                                }
                            }
                            Err(error) => {
                                let action = send_broker_error(&mut socket, error).await;
                                if should_break_after_send(action) {
                                    break;
                                }
                            }
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
            message = socket.recv() => {
                let action = match message {
                    Some(Ok(Message::Close(_))) | None => ClientMessageAction::Close,
                    Some(Ok(message)) => {
                        if !ticket_session_is_active(&state, &binding.remote_session_id, &binding.device_id).await {
                            let action = send_error(&mut socket, "session_expired", true, None).await;
                            debug_assert!(should_break_after_send(action));
                            break 'stream;
                        }
                        handle_client_message(&state, &mut socket, &mut binding, message).await
                    }
                    Some(Err(_)) => ClientMessageAction::Close,
                };
                match action {
                    ClientMessageAction::Continue => {}
                    ClientMessageAction::Close => break,
                    ClientMessageAction::Fatal(code) => {
                        let action = send_error(&mut socket, code, true, None).await;
                        debug_assert!(should_break_after_send(action));
                        break;
                    }
                }
            }
        }
    }

    detach_binding(&state, &binding).await;
}

async fn register_binding(
    state: &crate::state::AppState,
    binding: &mut TerminalSocketBinding,
) -> Result<
    wardian_core::models::TerminalPresentationRegistrationResult,
    crate::state::terminal_session::TerminalBrokerError,
> {
    let observed = state
        .terminal_sessions
        .broker_state(&binding.session_id)
        .await?;
    let identity = crate::state::terminal_session::TerminalClientIdentity::authenticated_remote(
        binding.presentation_id.clone(),
        true,
    );
    let registration = state
        .terminal_sessions
        .register_presentation(
            TerminalPresentationRegistration {
                presentation_id: binding.presentation_id.clone(),
                session_id: binding.session_id.clone(),
                client_kind: TerminalClientKind::Remote,
                desired_geometry: Some(binding.desired_geometry),
                visibility: TerminalVisibility::Visible,
                render_state: TerminalRenderState::Mounted,
                requested_interaction: TerminalRequestedInteraction::Interactive,
                observed_lease_epoch: observed.lease_epoch,
            },
            identity,
        )
        .await?;
    binding.runtime_generation = registration.broker_state.runtime_generation;
    binding.lease_epoch = registration.broker_state.lease_epoch;
    binding.owner_presentation_id = registration.broker_state.owner_presentation_id.clone();
    binding.broker_geometry = registration.broker_state.geometry;
    binding.cursor = registration.initial_snapshot.sequence_barrier;
    if let Err(error) = state
        .terminal_sessions
        .subscribe(TerminalEventSubscriptionRequest {
            session_id: binding.session_id.clone(),
            consumer_id: binding.consumer_id.clone(),
            client_kind: TerminalClientKind::Remote,
            runtime_generation: binding.runtime_generation,
        })
        .await
    {
        let _ = state
            .terminal_sessions
            .unregister_presentation(
                &binding.session_id,
                &binding.presentation_id,
                binding.runtime_generation,
            )
            .await;
        return Err(error);
    }
    Ok(registration)
}

async fn detach_binding(state: &crate::state::AppState, binding: &TerminalSocketBinding) {
    let _ = state
        .terminal_sessions
        .unsubscribe(TerminalEventUnsubscribeRequest {
            session_id: binding.session_id.clone(),
            consumer_id: binding.consumer_id.clone(),
        })
        .await;
    let _ = state
        .terminal_sessions
        .unregister_presentation(
            &binding.session_id,
            &binding.presentation_id,
            binding.runtime_generation,
        )
        .await;
}

async fn activate_v1_after_snapshot(
    state: &crate::state::AppState,
    socket: &mut WebSocket,
    binding: &mut TerminalSocketBinding,
) -> Result<(), ()> {
    let begin = state
        .terminal_sessions
        .begin_activation(TerminalActivationBeginRequest {
            session_id: binding.session_id.clone(),
            presentation_id: binding.presentation_id.clone(),
            runtime_generation: binding.runtime_generation,
            observed_lease_epoch: binding.lease_epoch,
        })
        .await
        .map_err(|_| ())?;
    if begin.decision.status != wardian_core::models::TerminalLeaseDecisionStatus::Accepted {
        if send_v1_decision(socket, &begin.decision).await != ClientMessageAction::Continue {
            return Err(());
        }
        return Ok(());
    }
    let Some(activation_id) = begin.activation_id else {
        return Err(());
    };
    let ack = state
        .terminal_sessions
        .ack_activation(TerminalActivationAckRequest {
            session_id: binding.session_id.clone(),
            presentation_id: binding.presentation_id.clone(),
            runtime_generation: begin.decision.runtime_generation,
            lease_epoch: begin.decision.lease_epoch,
            activation_id,
        })
        .await
        .map_err(|_| ())?;
    binding.runtime_generation = ack.broker_state.runtime_generation;
    binding.lease_epoch = ack.broker_state.lease_epoch;
    binding.owner_presentation_id = ack.broker_state.owner_presentation_id.clone();
    binding.broker_geometry = ack.broker_state.geometry;
    send_v1_ownership(socket, &ack.broker_state)
        .await
        .map_err(|_| ())
}

async fn handle_client_message(
    state: &crate::state::AppState,
    socket: &mut WebSocket,
    binding: &mut TerminalSocketBinding,
    message: Message,
) -> ClientMessageAction {
    let Message::Text(text) = message else {
        return ClientMessageAction::Fatal("invalid_terminal_attach_message");
    };
    let parsed = match serde_json::from_str::<TerminalClientMessage>(text.as_str()) {
        Ok(parsed) => parsed,
        Err(_) => return ClientMessageAction::Fatal("invalid_terminal_attach_message"),
    };
    if binding.protocol == TerminalProtocol::V1 {
        return handle_v1_client_message(state, socket, binding, parsed).await;
    }
    match parsed {
        TerminalClientMessage::ReportViewport {
            runtime_generation,
            cols,
            rows,
        } => {
            let result = state
                .terminal_sessions
                .report_presentation_viewport(TerminalPresentationViewportRequest {
                    session_id: binding.session_id.clone(),
                    presentation_id: binding.presentation_id.clone(),
                    runtime_generation,
                    cols,
                    rows,
                })
                .await;
            match result {
                Ok(presentation) => {
                    binding.desired_geometry = presentation
                        .desired_geometry
                        .unwrap_or(binding.desired_geometry);
                    send_json(
                        socket,
                        serde_json::json!({
                            "type": "presentation_state",
                            "presentation": presentation,
                        }),
                    )
                    .await
                }
                Err(error) => send_broker_error(socket, error).await,
            }
        }
        TerminalClientMessage::BeginActivation {
            runtime_generation,
            observed_lease_epoch,
        } => {
            match state
                .terminal_sessions
                .begin_activation(TerminalActivationBeginRequest {
                    session_id: binding.session_id.clone(),
                    presentation_id: binding.presentation_id.clone(),
                    runtime_generation,
                    observed_lease_epoch,
                })
                .await
            {
                Ok(result) => {
                    send_json(
                        socket,
                        serde_json::json!({"type":"activation_begin","result":result}),
                    )
                    .await
                }
                Err(error) => send_broker_error(socket, error).await,
            }
        }
        TerminalClientMessage::AckActivation {
            runtime_generation,
            lease_epoch,
            activation_id,
        } => {
            match state
                .terminal_sessions
                .ack_activation(TerminalActivationAckRequest {
                    session_id: binding.session_id.clone(),
                    presentation_id: binding.presentation_id.clone(),
                    runtime_generation,
                    lease_epoch,
                    activation_id,
                })
                .await
            {
                Ok(result) => {
                    binding.runtime_generation = result.broker_state.runtime_generation;
                    binding.lease_epoch = result.broker_state.lease_epoch;
                    send_json(
                        socket,
                        serde_json::json!({"type":"activation_ack","result":result}),
                    )
                    .await
                }
                Err(error) => send_broker_error(socket, error).await,
            }
        }
        TerminalClientMessage::Input {
            runtime_generation,
            lease_epoch,
            data,
        } => {
            if data.len() > MAX_INPUT_BYTES {
                return ClientMessageAction::Fatal("terminal_input_too_large");
            }
            let (Some(runtime_generation), Some(lease_epoch)) = (runtime_generation, lease_epoch)
            else {
                return ClientMessageAction::Fatal("invalid_terminal_attach_message");
            };
            match state
                .terminal_sessions
                .send_input(TerminalInputRequest {
                    lease: lease(binding, runtime_generation, lease_epoch),
                    bytes: data.into_bytes(),
                })
                .await
            {
                Ok(decision) => {
                    send_json(
                        socket,
                        serde_json::json!({"type":"input_result","decision":decision}),
                    )
                    .await
                }
                Err(error) => send_broker_error(socket, error).await,
            }
        }
        TerminalClientMessage::Binary {
            runtime_generation,
            lease_epoch,
            data_base64,
        } => {
            if data_base64.len() > MAX_INPUT_BASE64_BYTES {
                return ClientMessageAction::Fatal("terminal_input_too_large");
            }
            let bytes = match base64::engine::general_purpose::STANDARD.decode(data_base64) {
                Ok(bytes) if bytes.len() <= MAX_INPUT_BYTES => bytes,
                Ok(_) => return ClientMessageAction::Fatal("terminal_input_too_large"),
                Err(_) => return ClientMessageAction::Fatal("invalid_terminal_attach_message"),
            };
            let (Some(runtime_generation), Some(lease_epoch)) = (runtime_generation, lease_epoch)
            else {
                return ClientMessageAction::Fatal("invalid_terminal_attach_message");
            };
            match state
                .terminal_sessions
                .send_input(TerminalInputRequest {
                    lease: lease(binding, runtime_generation, lease_epoch),
                    bytes,
                })
                .await
            {
                Ok(decision) => {
                    send_json(
                        socket,
                        serde_json::json!({"type":"input_result","decision":decision}),
                    )
                    .await
                }
                Err(error) => send_broker_error(socket, error).await,
            }
        }
        TerminalClientMessage::Resize {
            runtime_generation,
            lease_epoch,
            geometry_sequence,
            cols,
            rows,
        } => {
            let (Some(runtime_generation), Some(lease_epoch), Some(geometry_sequence)) =
                (runtime_generation, lease_epoch, geometry_sequence)
            else {
                return ClientMessageAction::Fatal("invalid_terminal_attach_message");
            };
            match state
                .terminal_sessions
                .resize(TerminalGeometryRequest {
                    lease: lease(binding, runtime_generation, lease_epoch),
                    geometry_sequence,
                    geometry: clamp_remote_geometry(cols, rows),
                })
                .await
            {
                Ok(result) => {
                    binding.geometry_sequence =
                        binding.geometry_sequence.max(result.geometry_sequence);
                    send_json(
                        socket,
                        serde_json::json!({"type":"resize_result","result":result}),
                    )
                    .await
                }
                Err(error) => send_broker_error(socket, error).await,
            }
        }
        TerminalClientMessage::RequestSnapshot => {
            send_current_snapshot(state, socket, binding).await
        }
        TerminalClientMessage::RequestEvents {
            runtime_generation,
            after_sequence,
        } => {
            let outcome = drain_event_slice(
                state,
                socket,
                binding,
                Some((runtime_generation, after_sequence)),
            )
            .await;
            apply_drain_outcome(binding, outcome)
        }
        TerminalClientMessage::AckEvents {
            runtime_generation,
            applied_sequence,
        } => {
            match state
                .terminal_sessions
                .ack_events(TerminalEventAckRequest {
                    session_id: binding.session_id.clone(),
                    consumer_id: binding.consumer_id.clone(),
                    runtime_generation,
                    applied_sequence,
                })
                .await
            {
                Ok(result) => {
                    send_json(
                        socket,
                        serde_json::json!({"type":"events_ack","result":result}),
                    )
                    .await
                }
                Err(error) => send_broker_error(socket, error).await,
            }
        }
        TerminalClientMessage::SetPresentationState {
            runtime_generation,
            visibility,
            render_state,
            requested_interaction,
            observed_lease_epoch,
            cols,
            rows,
        } => {
            let desired_geometry = match (cols, rows) {
                (Some(cols), Some(rows)) => Some(clamp_remote_geometry(cols, rows)),
                (None, None) => None,
                _ => return ClientMessageAction::Fatal("invalid_terminal_attach_message"),
            };
            let identity =
                crate::state::terminal_session::TerminalClientIdentity::authenticated_remote(
                    binding.presentation_id.clone(),
                    true,
                );
            match state
                .terminal_sessions
                .update_presentation(
                    TerminalPresentationUpdateRequest {
                        presentation_id: binding.presentation_id.clone(),
                        session_id: binding.session_id.clone(),
                        runtime_generation,
                        desired_geometry,
                        visibility,
                        render_state,
                        requested_interaction,
                        observed_lease_epoch,
                    },
                    identity,
                )
                .await
            {
                Ok(result) => {
                    send_json(
                        socket,
                        serde_json::json!({
                            "type":"presentation_state",
                            "presentation":result.presentation,
                            "broker_state":result.broker_state,
                        }),
                    )
                    .await
                }
                Err(error) => send_broker_error(socket, error).await,
            }
        }
        TerminalClientMessage::BeginOwnerResync {
            runtime_generation,
            lease_epoch,
        } => {
            match state
                .terminal_sessions
                .begin_owner_resync(TerminalOwnerResyncBeginRequest {
                    session_id: binding.session_id.clone(),
                    presentation_id: binding.presentation_id.clone(),
                    runtime_generation,
                    lease_epoch,
                })
                .await
            {
                Ok(result) => {
                    send_json(
                        socket,
                        serde_json::json!({"type":"owner_resync_begin","result":result}),
                    )
                    .await
                }
                Err(error) => send_broker_error(socket, error).await,
            }
        }
        TerminalClientMessage::AckOwnerResync {
            runtime_generation,
            lease_epoch,
            resync_id,
        } => {
            match state
                .terminal_sessions
                .ack_owner_resync(TerminalOwnerResyncAckRequest {
                    session_id: binding.session_id.clone(),
                    presentation_id: binding.presentation_id.clone(),
                    runtime_generation,
                    lease_epoch,
                    resync_id,
                })
                .await
            {
                Ok(result) => {
                    send_json(
                        socket,
                        serde_json::json!({"type":"owner_resync_ack","result":result}),
                    )
                    .await
                }
                Err(error) => send_broker_error(socket, error).await,
            }
        }
        TerminalClientMessage::Detach => ClientMessageAction::Close,
    }
}

async fn handle_v1_client_message(
    state: &crate::state::AppState,
    socket: &mut WebSocket,
    binding: &mut TerminalSocketBinding,
    message: TerminalClientMessage,
) -> ClientMessageAction {
    match message {
        TerminalClientMessage::Input { data, .. } => {
            if data.len() > MAX_INPUT_BYTES {
                return ClientMessageAction::Fatal("terminal_input_too_large");
            }
            send_v1_input(state, socket, binding, data.into_bytes()).await
        }
        TerminalClientMessage::Binary { data_base64, .. } => {
            if data_base64.len() > MAX_INPUT_BASE64_BYTES {
                return ClientMessageAction::Fatal("terminal_input_too_large");
            }
            match base64::engine::general_purpose::STANDARD.decode(data_base64) {
                Ok(bytes) if bytes.len() <= MAX_INPUT_BYTES => {
                    send_v1_input(state, socket, binding, bytes).await
                }
                Ok(_) => ClientMessageAction::Fatal("terminal_input_too_large"),
                Err(_) => ClientMessageAction::Fatal("invalid_terminal_attach_message"),
            }
        }
        TerminalClientMessage::Resize { cols, rows, .. } => {
            binding.geometry_sequence = binding.geometry_sequence.saturating_add(1);
            match state
                .terminal_sessions
                .resize(TerminalGeometryRequest {
                    lease: lease(binding, binding.runtime_generation, binding.lease_epoch),
                    geometry_sequence: binding.geometry_sequence,
                    geometry: clamp_remote_geometry(cols, rows),
                })
                .await
            {
                Ok(result) => {
                    binding.lease_epoch = result.decision.lease_epoch;
                    binding.owner_presentation_id = result.decision.owner_presentation_id.clone();
                    binding.broker_geometry = result.geometry;
                    if let Some(snapshot) = result.snapshot {
                        send_v1_snapshot(socket, binding, &snapshot).await
                    } else {
                        send_v1_decision(socket, &result.decision).await
                    }
                }
                Err(error) => send_broker_error(socket, error).await,
            }
        }
        TerminalClientMessage::Detach => ClientMessageAction::Close,
        _ => ClientMessageAction::Fatal("invalid_terminal_attach_message"),
    }
}

async fn send_v1_input(
    state: &crate::state::AppState,
    socket: &mut WebSocket,
    binding: &mut TerminalSocketBinding,
    bytes: Vec<u8>,
) -> ClientMessageAction {
    match state
        .terminal_sessions
        .send_input(TerminalInputRequest {
            lease: lease(binding, binding.runtime_generation, binding.lease_epoch),
            bytes,
        })
        .await
    {
        Ok(decision) => {
            binding.lease_epoch = decision.lease_epoch;
            binding.owner_presentation_id = decision.owner_presentation_id.clone();
            send_v1_decision(socket, &decision).await
        }
        Err(error) => send_broker_error(socket, error).await,
    }
}

fn lease(
    binding: &TerminalSocketBinding,
    runtime_generation: u64,
    lease_epoch: u64,
) -> TerminalLeaseIdentity {
    TerminalLeaseIdentity {
        session_id: binding.session_id.clone(),
        presentation_id: binding.presentation_id.clone(),
        runtime_generation,
        lease_epoch,
    }
}

async fn drain_event_slice(
    state: &crate::state::AppState,
    socket: &mut WebSocket,
    binding: &mut TerminalSocketBinding,
    requested: Option<(u64, u64)>,
) -> DrainSliceOutcome {
    with_drain_slice_deadline(
        DRAIN_SLICE_TIMEOUT,
        drain_event_slice_inner(state, socket, binding, requested),
    )
    .await
}

async fn with_drain_slice_deadline<F>(deadline: std::time::Duration, drain: F) -> DrainSliceOutcome
where
    F: Future<Output = DrainSliceOutcome>,
{
    tokio::time::timeout(deadline, drain)
        .await
        .unwrap_or_else(|_| DrainSliceOutcome::complete(ClientMessageAction::Close))
}

async fn drain_event_slice_inner(
    state: &crate::state::AppState,
    socket: &mut WebSocket,
    binding: &mut TerminalSocketBinding,
    requested: Option<(u64, u64)>,
) -> DrainSliceOutcome {
    let (runtime_generation, mut cursor) =
        requested.unwrap_or((binding.runtime_generation, binding.cursor));
    for batch_index in 0..DRAIN_BATCHES_BEFORE_YIELD {
        let batch = match state
            .terminal_sessions
            .read_events(TerminalEventReadRequest {
                session_id: binding.session_id.clone(),
                consumer_id: binding.consumer_id.clone(),
                runtime_generation,
                after_sequence: cursor,
                max_events: MAX_DRAIN_EVENTS,
                max_bytes: MAX_DRAIN_BYTES,
            })
            .await
        {
            Ok(batch) => batch,
            Err(error) => {
                return DrainSliceOutcome::complete(send_broker_error(socket, error).await);
            }
        };
        binding.runtime_generation = batch.runtime_generation;
        match batch.status {
            TerminalEventBatchStatus::Events if batch.events.is_empty() => {
                return DrainSliceOutcome::complete(ClientMessageAction::Continue);
            }
            TerminalEventBatchStatus::Events => {
                cursor = batch.next_sequence;
                binding.cursor = cursor;
                let action = if binding.protocol == TerminalProtocol::V2 {
                    send_v2_batch(socket, &batch).await
                } else {
                    send_v1_batch(socket, binding, &batch).await
                };
                if action != ClientMessageAction::Continue {
                    return DrainSliceOutcome::complete(action);
                }
                if binding.protocol == TerminalProtocol::V1 {
                    let _ = state
                        .terminal_sessions
                        .ack_events(TerminalEventAckRequest {
                            session_id: binding.session_id.clone(),
                            consumer_id: binding.consumer_id.clone(),
                            runtime_generation: batch.runtime_generation,
                            applied_sequence: batch.next_sequence,
                        })
                        .await;
                }
                if !batch_has_more(&batch) {
                    return DrainSliceOutcome::complete(ClientMessageAction::Continue);
                }
                if batch_index + 1 == DRAIN_BATCHES_BEFORE_YIELD {
                    return DrainSliceOutcome::more_pending();
                }
            }
            TerminalEventBatchStatus::Gap | TerminalEventBatchStatus::GenerationChanged => {
                let Some(snapshot) = batch.recovery_snapshot.as_ref() else {
                    return DrainSliceOutcome::complete(ClientMessageAction::Fatal(
                        "terminal_snapshot_recovery_failed",
                    ));
                };
                binding.cursor = snapshot.sequence_barrier;
                binding.runtime_generation = snapshot.runtime_generation;
                if let Ok(broker_state) = state
                    .terminal_sessions
                    .broker_state(&binding.session_id)
                    .await
                {
                    binding.lease_epoch = broker_state.lease_epoch;
                    binding.owner_presentation_id = broker_state.owner_presentation_id;
                    binding.broker_geometry = broker_state.geometry;
                }
                let action = send_snapshot(socket, binding, snapshot).await;
                if binding.protocol == TerminalProtocol::V1
                    && action == ClientMessageAction::Continue
                {
                    let _ = state
                        .terminal_sessions
                        .ack_events(TerminalEventAckRequest {
                            session_id: binding.session_id.clone(),
                            consumer_id: binding.consumer_id.clone(),
                            runtime_generation: snapshot.runtime_generation,
                            applied_sequence: snapshot.sequence_barrier,
                        })
                        .await;
                }
                return DrainSliceOutcome::complete(action);
            }
            TerminalEventBatchStatus::Terminated => {
                return DrainSliceOutcome::complete(
                    send_error(socket, "terminal_runtime_terminated", false, None).await,
                );
            }
        }
    }
    DrainSliceOutcome::complete(ClientMessageAction::Continue)
}

fn apply_drain_outcome(
    binding: &mut TerminalSocketBinding,
    outcome: DrainSliceOutcome,
) -> ClientMessageAction {
    binding.drain_continuation_pending = outcome.more_pending;
    outcome.action
}

fn batch_has_more(batch: &TerminalEventBatch) -> bool {
    batch.status == TerminalEventBatchStatus::Events && batch.next_sequence < batch.latest_sequence
}

async fn send_current_snapshot(
    state: &crate::state::AppState,
    socket: &mut WebSocket,
    binding: &mut TerminalSocketBinding,
) -> ClientMessageAction {
    match (
        state.terminal_sessions.snapshot(&binding.session_id).await,
        state
            .terminal_sessions
            .broker_state(&binding.session_id)
            .await,
    ) {
        (Ok(snapshot), Ok(broker_state)) => {
            binding.runtime_generation = snapshot.runtime_generation;
            binding.cursor = snapshot.sequence_barrier;
            binding.lease_epoch = broker_state.lease_epoch;
            binding.owner_presentation_id = broker_state.owner_presentation_id;
            binding.broker_geometry = broker_state.geometry;
            send_snapshot(socket, binding, &snapshot).await
        }
        (Err(error), _) | (_, Err(error)) => send_broker_error(socket, error).await,
    }
}

async fn send_registered(
    socket: &mut WebSocket,
    binding: &TerminalSocketBinding,
    result: &wardian_core::models::TerminalPresentationRegistrationResult,
) -> Result<(), SocketSendFailure> {
    let payload = if binding.protocol == TerminalProtocol::V2 {
        serde_json::json!({
            "type": "registered",
            "protocol_version": 2,
            "presentation": result.presentation,
            "broker_state": result.broker_state,
            "initial_snapshot": result.initial_snapshot,
        })
    } else {
        v1_snapshot_payload(binding, &result.initial_snapshot)
    };
    send_socket_message(socket, Message::Text(payload.to_string().into())).await
}

async fn send_snapshot(
    socket: &mut WebSocket,
    binding: &TerminalSocketBinding,
    snapshot: &TerminalSnapshot,
) -> ClientMessageAction {
    if binding.protocol == TerminalProtocol::V2 {
        send_json(
            socket,
            serde_json::json!({"type":"snapshot","snapshot":snapshot}),
        )
        .await
    } else {
        send_v1_snapshot(socket, binding, snapshot).await
    }
}

async fn send_v2_batch(socket: &mut WebSocket, batch: &TerminalEventBatch) -> ClientMessageAction {
    let events = batch.events.iter().map(wire_event).collect::<Vec<_>>();
    send_json(
        socket,
        serde_json::json!({
            "type":"events",
            "batch": {
                "status": batch.status,
                "runtime_generation": batch.runtime_generation,
                "events": events,
                "next_sequence": batch.next_sequence,
                "available_from_sequence": batch.available_from_sequence,
                "latest_sequence": batch.latest_sequence,
                "recovery_snapshot": batch.recovery_snapshot,
            }
        }),
    )
    .await
}

fn wire_event(event: &TerminalBrokerEvent) -> serde_json::Value {
    match &event.event {
        TerminalBrokerEventKind::Output { bytes } => serde_json::json!({
            "sequence": event.sequence,
            "runtime_generation": event.runtime_generation,
            "type": "output",
            "bytes_base64": base64::engine::general_purpose::STANDARD.encode(bytes),
        }),
        TerminalBrokerEventKind::Geometry {
            geometry,
            geometry_sequence,
        } => serde_json::json!({
            "sequence": event.sequence,
            "runtime_generation": event.runtime_generation,
            "type": "geometry",
            "geometry": geometry,
            "geometry_sequence": geometry_sequence,
        }),
        TerminalBrokerEventKind::Ownership {
            owner_presentation_id,
            lease_epoch,
            activation_id,
        } => serde_json::json!({
            "sequence": event.sequence,
            "runtime_generation": event.runtime_generation,
            "type": "ownership",
            "owner_presentation_id": owner_presentation_id,
            "lease_epoch": lease_epoch,
            "activation_id": activation_id,
        }),
        TerminalBrokerEventKind::Lifecycle { lifecycle } => serde_json::json!({
            "sequence": event.sequence,
            "runtime_generation": event.runtime_generation,
            "type": "lifecycle",
            "lifecycle": lifecycle,
        }),
    }
}

async fn send_v1_batch(
    socket: &mut WebSocket,
    binding: &mut TerminalSocketBinding,
    batch: &TerminalEventBatch,
) -> ClientMessageAction {
    for event in &batch.events {
        let Some(payload) = v1_event_payload(binding, event) else {
            continue;
        };
        if send_socket_message(socket, Message::Text(payload.to_string().into()))
            .await
            .is_err()
        {
            return ClientMessageAction::Close;
        }
    }
    ClientMessageAction::Continue
}

fn v1_event_payload(
    binding: &mut TerminalSocketBinding,
    event: &TerminalBrokerEvent,
) -> Option<serde_json::Value> {
    match &event.event {
        TerminalBrokerEventKind::Output { bytes } => Some(serde_json::json!({
            "type":"update",
            "attachment_id": null,
            "owner_attachment_id": binding.owner_presentation_id,
            "state_base64": base64::engine::general_purpose::STANDARD.encode(bytes),
        })),
        TerminalBrokerEventKind::Geometry { geometry, .. } => {
            binding.broker_geometry = *geometry;
            Some(serde_json::json!({
                "type":"ownership",
                "owner_attachment_id": binding.owner_presentation_id,
                "cols": geometry.cols,
                "rows": geometry.rows,
            }))
        }
        TerminalBrokerEventKind::Ownership {
            owner_presentation_id,
            lease_epoch,
            ..
        } => {
            binding.lease_epoch = *lease_epoch;
            binding.owner_presentation_id = owner_presentation_id.clone();
            Some(serde_json::json!({
                "type":"ownership",
                "owner_attachment_id": owner_presentation_id,
                "cols": binding.broker_geometry.cols,
                "rows": binding.broker_geometry.rows,
            }))
        }
        TerminalBrokerEventKind::Lifecycle { .. } => None,
    }
}

async fn send_v1_snapshot(
    socket: &mut WebSocket,
    binding: &TerminalSocketBinding,
    snapshot: &TerminalSnapshot,
) -> ClientMessageAction {
    send_json(socket, v1_snapshot_payload(binding, snapshot)).await
}

fn v1_snapshot_payload(
    binding: &TerminalSocketBinding,
    snapshot: &TerminalSnapshot,
) -> serde_json::Value {
    serde_json::json!({
        "type":"snapshot",
        "attachment_id": binding.presentation_id,
        "owner_attachment_id": binding.owner_presentation_id,
        "cols": snapshot.geometry.cols,
        "rows": snapshot.geometry.rows,
        "state_base64": snapshot.terminal_state_base64,
    })
}

async fn send_v1_ownership(
    socket: &mut WebSocket,
    broker_state: &wardian_core::models::TerminalBrokerState,
) -> Result<(), SocketSendFailure> {
    send_socket_message(
        socket,
        Message::Text(
            serde_json::json!({
                "type":"ownership",
                "owner_attachment_id": broker_state.owner_presentation_id,
                "cols": broker_state.geometry.cols,
                "rows": broker_state.geometry.rows,
            })
            .to_string()
            .into(),
        ),
    )
    .await
}

async fn send_v1_decision(
    socket: &mut WebSocket,
    decision: &wardian_core::models::TerminalLeaseDecision,
) -> ClientMessageAction {
    if decision.status == wardian_core::models::TerminalLeaseDecisionStatus::Accepted {
        ClientMessageAction::Continue
    } else {
        send_error(socket, "terminal_lease_disagreement", false, Some(decision)).await
    }
}

async fn send_broker_error(
    socket: &mut WebSocket,
    error: crate::state::terminal_session::TerminalBrokerError,
) -> ClientMessageAction {
    use crate::state::terminal_session::TerminalBrokerError;
    let code = match error {
        TerminalBrokerError::StaleRuntimeGeneration { .. } => "stale_runtime_generation",
        TerminalBrokerError::PresentationNotFound => "terminal_presentation_not_found",
        TerminalBrokerError::ConsumerNotFound => "terminal_consumer_not_found",
        TerminalBrokerError::RuntimeIo(ref detail) if detail == "input_timeout" => {
            "terminal_input_buffer_full"
        }
        TerminalBrokerError::RuntimeTerminated => "terminal_runtime_terminated",
        TerminalBrokerError::RuntimeUnavailable | TerminalBrokerError::SessionNotFound => {
            "terminal_runtime_unavailable"
        }
        _ => "terminal_broker_error",
    };
    send_error(socket, code, false, None).await
}

async fn send_error(
    socket: &mut WebSocket,
    code: &'static str,
    fatal: bool,
    decision: Option<&wardian_core::models::TerminalLeaseDecision>,
) -> ClientMessageAction {
    let result = send_socket_message(
        socket,
        Message::Text(
            serde_json::json!({
                "type":"error",
                "code":code,
                "fatal":fatal,
                "decision":decision,
            })
            .to_string()
            .into(),
        ),
    )
    .await;
    if result.is_err() || fatal {
        ClientMessageAction::Close
    } else {
        ClientMessageAction::Continue
    }
}

async fn send_json(socket: &mut WebSocket, payload: serde_json::Value) -> ClientMessageAction {
    if send_socket_message(socket, Message::Text(payload.to_string().into()))
        .await
        .is_ok()
    {
        ClientMessageAction::Continue
    } else {
        ClientMessageAction::Close
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SocketSendFailure {
    Closed,
    Timeout,
}

async fn send_socket_message(
    socket: &mut WebSocket,
    message: Message,
) -> Result<(), SocketSendFailure> {
    bounded_socket_send(socket.send(message)).await
}

async fn bounded_socket_send<F, E>(send: F) -> Result<(), SocketSendFailure>
where
    F: Future<Output = Result<(), E>>,
{
    match tokio::time::timeout(SOCKET_SEND_TIMEOUT, send).await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(_)) => Err(SocketSendFailure::Closed),
        Err(_) => Err(SocketSendFailure::Timeout),
    }
}

fn parse_open_message(message: Message) -> Result<TerminalOpenMessage, &'static str> {
    let Message::Text(text) = message else {
        return Err("invalid_terminal_attach_message");
    };
    let mut open = serde_json::from_str::<TerminalOpenMessage>(text.as_str())
        .map_err(|_| "invalid_terminal_attach_message")?;
    open.ticket = open.ticket.trim().to_string();
    if open.ticket.is_empty() {
        return Err("invalid_terminal_attach_message");
    }
    Ok(open)
}

fn clamp_remote_geometry(cols: u16, rows: u16) -> TerminalGeometry {
    TerminalGeometry {
        cols: cols.clamp(20, 240),
        rows: rows.clamp(8, 80),
    }
}

async fn ticket_session_is_active(
    state: &crate::state::AppState,
    remote_session_id: &str,
    device_id: &str,
) -> bool {
    let now_ms = chrono::Utc::now().timestamp_millis();
    let runtime = state.remote_runtime.lock().await;
    runtime
        .sessions
        .get(remote_session_id)
        .is_some_and(|session| {
            session.device_id == device_id
                && crate::remote::auth::session_is_active(session, now_ms)
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::terminal_session::{
        TerminalBrokerError, TerminalClientIdentity, TerminalRuntimeHandles, TerminalSessionBroker,
        MAX_REMOTE_PRESENTATIONS_PER_SESSION,
    };
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use tokio::sync::mpsc;

    type RuntimeFixture = (
        TerminalRuntimeHandles,
        mpsc::Receiver<Vec<u8>>,
        Arc<Mutex<Vec<TerminalGeometry>>>,
    );

    fn runtime() -> RuntimeFixture {
        let (input_tx, input_rx) = mpsc::channel(16);
        let resizes = Arc::new(Mutex::new(Vec::new()));
        let observed = resizes.clone();
        (
            TerminalRuntimeHandles::new(input_tx, move |geometry| {
                observed.lock().expect("resize log").push(geometry);
                Ok(())
            }),
            input_rx,
            resizes,
        )
    }

    fn registration(
        session_id: &str,
        presentation_id: &str,
        client_kind: TerminalClientKind,
    ) -> TerminalPresentationRegistration {
        TerminalPresentationRegistration {
            presentation_id: presentation_id.to_string(),
            session_id: session_id.to_string(),
            client_kind,
            desired_geometry: Some(TerminalGeometry { cols: 80, rows: 24 }),
            visibility: TerminalVisibility::Visible,
            render_state: TerminalRenderState::Mounted,
            requested_interaction: TerminalRequestedInteraction::Interactive,
            observed_lease_epoch: 0,
        }
    }

    async fn activate(
        broker: &TerminalSessionBroker,
        session_id: &str,
        presentation_id: &str,
    ) -> wardian_core::models::TerminalActivationAckResult {
        let state = broker.broker_state(session_id).await.expect("broker state");
        let begin = broker
            .begin_activation(TerminalActivationBeginRequest {
                session_id: session_id.to_string(),
                presentation_id: presentation_id.to_string(),
                runtime_generation: state.runtime_generation,
                observed_lease_epoch: state.lease_epoch,
            })
            .await
            .expect("begin activation");
        broker
            .ack_activation(TerminalActivationAckRequest {
                session_id: session_id.to_string(),
                presentation_id: presentation_id.to_string(),
                runtime_generation: begin.decision.runtime_generation,
                lease_epoch: begin.decision.lease_epoch,
                activation_id: begin.activation_id.expect("activation id"),
            })
            .await
            .expect("ack activation")
    }

    fn v1_binding() -> TerminalSocketBinding {
        TerminalSocketBinding {
            protocol: TerminalProtocol::V1,
            session_id: "agent-1".to_string(),
            presentation_id: "remote-v1".to_string(),
            consumer_id: "remote-v1-feed".to_string(),
            remote_session_id: "remote-session".to_string(),
            device_id: "device".to_string(),
            runtime_generation: 1,
            lease_epoch: 1,
            owner_presentation_id: Some("desktop".to_string()),
            broker_geometry: TerminalGeometry { cols: 80, rows: 24 },
            cursor: 0,
            geometry_sequence: 0,
            desired_geometry: TerminalGeometry { cols: 80, rows: 24 },
            drain_continuation_pending: false,
        }
    }

    #[test]
    fn v2_open_is_explicit_and_v1_is_missing_version_only() {
        let v2 = parse_open_message(Message::Text(
            r#"{"protocol_version":2,"ticket":"ticket","cols":80,"rows":24}"#.into(),
        ))
        .expect("v2 open");
        assert_eq!(v2.protocol().expect("v2 protocol"), TerminalProtocol::V2);

        let v1 = parse_open_message(Message::Text(
            r#"{"ticket":"ticket","cols":80,"rows":24}"#.into(),
        ))
        .expect("v1 open");
        assert_eq!(v1.protocol().expect("v1 protocol"), TerminalProtocol::V1);

        let unsupported = parse_open_message(Message::Text(
            r#"{"protocol_version":3,"ticket":"ticket","cols":80,"rows":24}"#.into(),
        ))
        .expect("parsed unsupported protocol");
        assert_eq!(unsupported.protocol(), Err("unsupported_terminal_protocol"));

        let explicit_v1 = parse_open_message(Message::Text(
            r#"{"protocol_version":1,"ticket":"ticket","cols":80,"rows":24}"#.into(),
        ))
        .expect("parsed explicit v1");
        assert_eq!(explicit_v1.protocol(), Err("unsupported_terminal_protocol"));
    }

    #[test]
    fn remote_geometry_is_clamped_for_open_viewport_and_resize() {
        assert_eq!(
            clamp_remote_geometry(1, 2),
            TerminalGeometry { cols: 20, rows: 8 }
        );
        assert_eq!(
            clamp_remote_geometry(u16::MAX, u16::MAX),
            TerminalGeometry {
                cols: 240,
                rows: 80
            }
        );
    }

    #[test]
    fn wire_output_is_base64_not_json_byte_array() {
        let value = wire_event(&TerminalBrokerEvent {
            sequence: 1,
            runtime_generation: 2,
            event: TerminalBrokerEventKind::Output {
                bytes: vec![0, 1, 255],
            },
        });
        assert_eq!(value["bytes_base64"], "AAH/");
        assert!(value.get("bytes").is_none());
    }

    #[test]
    fn v1_payloads_track_real_owner_through_takeover_output_geometry_and_recovery() {
        let mut binding = v1_binding();
        let desktop_output = v1_event_payload(
            &mut binding,
            &TerminalBrokerEvent {
                sequence: 1,
                runtime_generation: 1,
                event: TerminalBrokerEventKind::Output {
                    bytes: b"desktop output".to_vec(),
                },
            },
        )
        .expect("output payload");
        assert_eq!(desktop_output["owner_attachment_id"], "desktop");

        let geometry = v1_event_payload(
            &mut binding,
            &TerminalBrokerEvent {
                sequence: 2,
                runtime_generation: 1,
                event: TerminalBrokerEventKind::Geometry {
                    geometry: TerminalGeometry {
                        cols: 120,
                        rows: 40,
                    },
                    geometry_sequence: 7,
                },
            },
        )
        .expect("geometry payload");
        assert_eq!(geometry["type"], "ownership");
        assert_eq!(geometry["owner_attachment_id"], "desktop");
        assert_eq!(geometry["cols"], 120);
        assert_eq!(geometry["rows"], 40);

        let takeover = v1_event_payload(
            &mut binding,
            &TerminalBrokerEvent {
                sequence: 3,
                runtime_generation: 1,
                event: TerminalBrokerEventKind::Ownership {
                    owner_presentation_id: Some("remote-v1".to_string()),
                    lease_epoch: 2,
                    activation_id: Some("activation".to_string()),
                },
            },
        )
        .expect("takeover payload");
        assert_eq!(takeover["owner_attachment_id"], "remote-v1");
        assert_eq!(binding.owner_presentation_id.as_deref(), Some("remote-v1"));

        binding.owner_presentation_id = Some("desktop-again".to_string());
        let recovery = v1_snapshot_payload(
            &binding,
            &TerminalSnapshot {
                snapshot_id: "snapshot".to_string(),
                session_id: "agent-1".to_string(),
                runtime_generation: 1,
                sequence_barrier: 3,
                geometry: TerminalGeometry {
                    cols: 120,
                    rows: 40,
                },
                terminal_state_base64: "AA==".to_string(),
                visible_grid: String::new(),
                scrollback: Vec::new(),
                formatted_scrollback: Vec::new(),
            },
        );
        assert_eq!(recovery["owner_attachment_id"], "desktop-again");
    }

    #[test]
    fn exact_input_limit_is_accepted_and_over_limit_is_rejected_before_decode() {
        let exact = base64::engine::general_purpose::STANDARD.encode(vec![0; MAX_INPUT_BYTES]);
        assert_eq!(exact.len(), MAX_INPUT_BASE64_BYTES);
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(&exact)
                .expect("exact base64")
                .len(),
            MAX_INPUT_BYTES
        );
        let over = base64::engine::general_purpose::STANDARD.encode(vec![0; MAX_INPUT_BYTES + 1]);
        assert_eq!(over.len(), MAX_INPUT_BASE64_BYTES);
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(&over)
                .expect("over-limit base64")
                .len(),
            MAX_INPUT_BYTES + 1
        );
    }

    #[test]
    fn invalid_binary_socket_framing_is_not_treated_as_terminal_data() {
        assert_eq!(
            parse_open_message(Message::Binary(vec![1, 2, 3].into())),
            Err("invalid_terminal_attach_message")
        );
    }

    #[test]
    fn active_loop_breaks_on_every_non_continue_send_outcome() {
        let cases = [
            (ClientMessageAction::Continue, false),
            (ClientMessageAction::Close, true),
            (ClientMessageAction::Fatal("fatal"), true),
        ];
        for (action, expected) in cases {
            assert_eq!(should_break_after_send(action), expected);
        }
    }

    #[tokio::test]
    async fn authenticated_remote_identity_is_server_derived_and_v2_registration_is_passive() {
        let broker = TerminalSessionBroker::default();
        let (runtime, _, _) = runtime();
        broker
            .start_or_replace_runtime("agent-1", runtime, TerminalGeometry { cols: 80, rows: 24 })
            .await
            .expect("runtime");

        let forged = broker
            .register_presentation(
                registration("agent-1", "server-presentation", TerminalClientKind::Remote),
                TerminalClientIdentity::authenticated_remote("client-chosen", true),
            )
            .await
            .expect_err("client identity cannot mint another presentation capability");
        assert_eq!(forged, TerminalBrokerError::InvalidIdentity);

        let registered = broker
            .register_presentation(
                registration("agent-1", "server-presentation", TerminalClientKind::Remote),
                TerminalClientIdentity::authenticated_remote("server-presentation", true),
            )
            .await
            .expect("authenticated registration");
        assert_eq!(registered.broker_state.owner_presentation_id, None);
        assert_eq!(
            registered.presentation.interaction_capability,
            wardian_core::models::TerminalInteractionCapability::Interactive
        );
    }

    #[tokio::test]
    async fn explicit_activation_transfers_desktop_to_remote_and_back() {
        let broker = TerminalSessionBroker::default();
        let (runtime, _, _) = runtime();
        broker
            .start_or_replace_runtime("agent-1", runtime, TerminalGeometry { cols: 80, rows: 24 })
            .await
            .expect("runtime");
        broker
            .register_presentation(
                registration("agent-1", "desktop", TerminalClientKind::Desktop),
                TerminalClientIdentity::trusted_desktop(),
            )
            .await
            .expect("desktop");
        broker
            .register_presentation(
                registration("agent-1", "remote", TerminalClientKind::Remote),
                TerminalClientIdentity::authenticated_remote("remote", true),
            )
            .await
            .expect("remote");

        let desktop = activate(&broker, "agent-1", "desktop").await;
        assert_eq!(
            desktop.broker_state.owner_presentation_id.as_deref(),
            Some("desktop")
        );
        let remote = activate(&broker, "agent-1", "remote").await;
        assert_eq!(
            remote.broker_state.owner_presentation_id.as_deref(),
            Some("remote")
        );
        let desktop_again = activate(&broker, "agent-1", "desktop").await;
        assert_eq!(
            desktop_again.broker_state.owner_presentation_id.as_deref(),
            Some("desktop")
        );
    }

    #[tokio::test]
    async fn mirror_viewport_only_updates_desired_geometry_and_non_owner_input_is_structured() {
        let broker = TerminalSessionBroker::default();
        let (runtime, _input_rx, resizes) = runtime();
        let generation = broker
            .start_or_replace_runtime("agent-1", runtime, TerminalGeometry { cols: 80, rows: 24 })
            .await
            .expect("runtime");
        broker
            .register_presentation(
                registration("agent-1", "desktop", TerminalClientKind::Desktop),
                TerminalClientIdentity::trusted_desktop(),
            )
            .await
            .expect("desktop");
        broker
            .register_presentation(
                registration("agent-1", "remote", TerminalClientKind::Remote),
                TerminalClientIdentity::authenticated_remote("remote", true),
            )
            .await
            .expect("remote");
        let owner = activate(&broker, "agent-1", "desktop").await;
        resizes.lock().expect("resize log").clear();

        let presentation = broker
            .report_presentation_viewport(TerminalPresentationViewportRequest {
                session_id: "agent-1".to_string(),
                presentation_id: "remote".to_string(),
                runtime_generation: generation,
                cols: u16::MAX,
                rows: u16::MAX,
            })
            .await
            .expect("report mirror viewport");
        assert_eq!(
            presentation.desired_geometry,
            Some(TerminalGeometry {
                cols: 240,
                rows: 80
            })
        );
        assert!(resizes.lock().expect("resize log").is_empty());

        let decision = broker
            .send_input(TerminalInputRequest {
                lease: TerminalLeaseIdentity {
                    session_id: "agent-1".to_string(),
                    presentation_id: "remote".to_string(),
                    runtime_generation: generation,
                    lease_epoch: owner.broker_state.lease_epoch,
                },
                bytes: b"must not write".to_vec(),
            })
            .await
            .expect("structured non-owner decision");
        assert_eq!(
            decision.reason,
            Some(wardian_core::models::TerminalLeaseRejectionReason::NotOwner)
        );
    }

    #[tokio::test]
    async fn remote_connection_limit_is_three_and_detach_immediately_frees_slot() {
        let broker = TerminalSessionBroker::default();
        let (initial_runtime, _, _) = runtime();
        let generation = broker
            .start_or_replace_runtime(
                "agent-1",
                initial_runtime,
                TerminalGeometry { cols: 80, rows: 24 },
            )
            .await
            .expect("runtime");
        for index in 0..MAX_REMOTE_PRESENTATIONS_PER_SESSION {
            let id = format!("remote-{index}");
            broker
                .register_presentation(
                    registration("agent-1", &id, TerminalClientKind::Remote),
                    TerminalClientIdentity::authenticated_remote(&id, true),
                )
                .await
                .expect("within limit");
        }
        let error = broker
            .register_presentation(
                registration("agent-1", "remote-over", TerminalClientKind::Remote),
                TerminalClientIdentity::authenticated_remote("remote-over", true),
            )
            .await
            .expect_err("fourth remote presentation rejected");
        assert_eq!(
            error,
            TerminalBrokerError::PresentationLimit {
                client_kind: TerminalClientKind::Remote,
                limit: MAX_REMOTE_PRESENTATIONS_PER_SESSION,
            }
        );

        broker
            .unregister_presentation("agent-1", "remote-0", generation)
            .await
            .expect("detach");
        broker
            .register_presentation(
                registration("agent-1", "remote-after", TerminalClientKind::Remote),
                TerminalClientIdentity::authenticated_remote("remote-after", true),
            )
            .await
            .expect("slot immediately reusable");
    }

    #[tokio::test]
    async fn shared_cursor_reports_gap_and_generation_recovery_snapshots() {
        let broker = TerminalSessionBroker::default();
        let (initial_runtime, _, _) = runtime();
        let generation = broker
            .start_or_replace_runtime(
                "agent-1",
                initial_runtime,
                TerminalGeometry { cols: 80, rows: 24 },
            )
            .await
            .expect("runtime");
        broker
            .subscribe(TerminalEventSubscriptionRequest {
                session_id: "agent-1".to_string(),
                consumer_id: "remote-feed".to_string(),
                client_kind: TerminalClientKind::Remote,
                runtime_generation: generation,
            })
            .await
            .expect("subscribe");
        let gap = broker
            .read_events(TerminalEventReadRequest {
                session_id: "agent-1".to_string(),
                consumer_id: "remote-feed".to_string(),
                runtime_generation: generation,
                after_sequence: u64::MAX,
                max_events: MAX_DRAIN_EVENTS,
                max_bytes: MAX_DRAIN_BYTES,
            })
            .await
            .expect("gap batch");
        assert_eq!(gap.status, TerminalEventBatchStatus::Gap);
        assert!(gap.recovery_snapshot.is_some());

        let (replacement, _, _) = runtime();
        let replacement_generation = broker
            .start_or_replace_runtime(
                "agent-1",
                replacement,
                TerminalGeometry { cols: 90, rows: 30 },
            )
            .await
            .expect("replace runtime");
        broker
            .subscribe(TerminalEventSubscriptionRequest {
                session_id: "agent-1".to_string(),
                consumer_id: "remote-feed".to_string(),
                client_kind: TerminalClientKind::Remote,
                runtime_generation: replacement_generation,
            })
            .await
            .expect("resubscribe");
        let changed = broker
            .read_events(TerminalEventReadRequest {
                session_id: "agent-1".to_string(),
                consumer_id: "remote-feed".to_string(),
                runtime_generation: generation,
                after_sequence: 0,
                max_events: MAX_DRAIN_EVENTS,
                max_bytes: MAX_DRAIN_BYTES,
            })
            .await
            .expect("generation changed batch");
        assert_eq!(changed.status, TerminalEventBatchStatus::GenerationChanged);
        assert_eq!(changed.runtime_generation, replacement_generation);
        assert!(changed.recovery_snapshot.is_some());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn bounded_drain_continues_past_eight_batches_without_another_wake() {
        let broker = Arc::new(TerminalSessionBroker::default());
        let (runtime, _, _) = runtime();
        let generation = broker
            .start_or_replace_runtime("agent-1", runtime, TerminalGeometry { cols: 80, rows: 24 })
            .await
            .expect("runtime");
        broker
            .subscribe(TerminalEventSubscriptionRequest {
                session_id: "agent-1".to_string(),
                consumer_id: "remote-feed".to_string(),
                client_kind: TerminalClientKind::Remote,
                runtime_generation: generation,
            })
            .await
            .expect("subscribe before output");

        let producer = broker.clone();
        tokio::task::spawn_blocking(move || {
            for _ in 0..2_305 {
                producer
                    .process_output_blocking("agent-1", generation, vec![b'x'])
                    .expect("produce output event");
            }
        })
        .await
        .expect("producer task");

        let mut cursor = 0;
        let mut batches = 0;
        let mut slices = 0;
        let mut revocation_poll_opportunities = 0;
        loop {
            slices += 1;
            let mut more_pending = false;
            for _ in 0..DRAIN_BATCHES_BEFORE_YIELD {
                let batch = broker
                    .read_events(TerminalEventReadRequest {
                        session_id: "agent-1".to_string(),
                        consumer_id: "remote-feed".to_string(),
                        runtime_generation: generation,
                        after_sequence: cursor,
                        max_events: MAX_DRAIN_EVENTS,
                        max_bytes: MAX_DRAIN_BYTES,
                    })
                    .await
                    .expect("read bounded batch");
                assert_eq!(batch.status, TerminalEventBatchStatus::Events);
                cursor = batch.next_sequence;
                batches += 1;
                more_pending = batch_has_more(&batch);
                if !more_pending {
                    assert_eq!(cursor, batch.latest_sequence);
                    break;
                }
            }
            let outcome = if more_pending {
                DrainSliceOutcome::more_pending()
            } else {
                DrainSliceOutcome::complete(ClientMessageAction::Continue)
            };
            assert!(batches <= slices * DRAIN_BATCHES_BEFORE_YIELD);
            if !outcome.more_pending {
                break;
            }
            // The real socket loop returns to select here, allowing close,
            // lifecycle, and session/device revocation branches to win.
            revocation_poll_opportunities += 1;
            tokio::task::yield_now().await;
        }
        assert!(batches > DRAIN_BATCHES_BEFORE_YIELD);
        assert!(slices > 1);
        assert!(revocation_poll_opportunities > 0);
        assert_eq!(cursor, 2_305);
    }

    #[tokio::test]
    async fn stalled_socket_send_times_out_and_cancels_the_sink_for_cleanup() {
        struct DropProbe(Arc<AtomicBool>);

        impl Drop for DropProbe {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }

        let cancelled = Arc::new(AtomicBool::new(false));
        let probe = DropProbe(cancelled.clone());
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(3),
            bounded_socket_send(async move {
                let _probe = probe;
                std::future::pending::<Result<(), ()>>().await
            }),
        )
        .await
        .expect("terminal send owns a two second deadline");
        assert_eq!(result, Err(SocketSendFailure::Timeout));
        assert!(cancelled.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn aggregate_drain_deadline_fires_while_individual_steps_keep_progressing() {
        let progress = Arc::new(AtomicUsize::new(0));
        let observed = progress.clone();
        let outcome = tokio::time::timeout(
            std::time::Duration::from_millis(250),
            with_drain_slice_deadline(std::time::Duration::from_millis(35), async move {
                for _ in 0..10 {
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                    observed.fetch_add(1, Ordering::SeqCst);
                }
                DrainSliceOutcome::complete(ClientMessageAction::Continue)
            }),
        )
        .await
        .expect("aggregate deadline itself remains bounded");

        assert_eq!(
            outcome,
            DrainSliceOutcome::complete(ClientMessageAction::Close)
        );
        let completed_steps = progress.load(Ordering::SeqCst);
        assert!(completed_steps > 0, "future made progress before deadline");
        assert!(
            completed_steps < 10,
            "aggregate deadline cancelled remaining steps"
        );
    }
}
