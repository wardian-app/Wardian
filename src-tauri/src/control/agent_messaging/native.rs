//! Canonical messaging over the single generation-bound Codex owner.
use super::*;
use crate::delivery::native_broker::{NativeBrokerError, NativeSessionSpec};
use wardian_core::conversation_lease::{ConversationLeaseOwner, PersistedConversationLeaseGuard};

pub(super) fn spawn_information(app: &AppHandle, recipient: &str) {
    let app = app.clone();
    let recipient = recipient.to_owned();
    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        if let Err(error) = push_pending_information(&state, &recipient).await {
            manager::log_debug(&format!("[WARDIAN] information delivery: {error}"));
        }
    });
}

async fn settle(
    state: &AppState,
    claim: &store::TaskClaim,
    result: Result<&str, bool>,
) -> Result<(), ControlError> {
    match result {
        Ok(delivery_state) => {
            state
                .interactions
                .finish_agent_task(claim, delivery_state)
                .await
        }
        Err(false) => {
            state
                .interactions
                .release_agent_message_before_write(claim)
                .await
        }
        Err(true) => {
            state
                .interactions
                .finish_agent_task(claim, "uncertain")
                .await
        }
    }
    .map_err(control_error)
}

/// No owner creation: unsupported legacy/embedded sessions retain receive.
pub(super) async fn push_pending_information(
    state: &AppState,
    recipient: &str,
) -> Result<(), ControlError> {
    let info = delivery_target_info(state, recipient).await?;
    if info.provider != "codex" {
        return Ok(());
    }
    let generation = state
        .interactions
        .current_provider_input_generation(recipient)
        .await
        .unwrap_or(0);
    if state
        .native_delivery
        .codex_binding(recipient, generation)
        .await
        .is_err()
    {
        return Ok(());
    }
    let highwater = store::with_db(|conn| store::information_highwater(conn, recipient))
        .map_err(control_error)?;
    loop {
        let ids = store::with_db(|conn| store::pending_information(conn, recipient, highwater))
            .map_err(control_error)?;
        if ids.is_empty() {
            return Ok(());
        }
        for id in ids {
            let Some(claim) = state
                .interactions
                .claim_agent_information(recipient, &id, generation)
                .await
                .map_err(control_error)?
            else {
                continue;
            };
            let context = prepare_claim_context(state, &claim).await?;
            let result = state
                .native_delivery
                .codex_push(recipient, generation, &id, &context)
                .await;
            settle(
                state,
                &claim,
                result
                    .as_ref()
                    .map(|receipt| receipt.delivery_state.as_str())
                    .map_err(|error| error.provider_boundary_crossed),
            )
            .await?;
            if let Err(error) = result {
                return Err(native_error(error, "native_push_unavailable"));
            }
        }
    }
}

pub(super) async fn interrupt(state: &AppState, target: &str) -> Result<Response, ControlError> {
    let recipient = resolve_exact(state, target).await?;
    let info = delivery_target_info(state, &recipient).await?;
    if info.provider != "codex" {
        return Err(ControlError::coded(
            "unsupported_interrupt",
            "This provider has no verified interrupt bridge.",
        ));
    }
    let generation = state
        .interactions
        .current_provider_input_generation(&recipient)
        .await
        .unwrap_or(0);
    let receipt = state
        .native_delivery
        .codex_interrupt(&recipient, generation)
        .await
        .map_err(|error| native_error(error, "unsupported_interrupt"))?;
    Ok(Response::InterruptAgent {
        target_agent_id: receipt.wardian_agent_id,
        generation: receipt.generation,
        delivery_state: receipt.delivery_state,
        interruption_confirmed: receipt.interruption_confirmed,
        provider_session_id: receipt.provider_session_id,
        provider_turn_id: receipt.provider_turn_id,
    })
}

pub(super) async fn dispatch_attached_task(
    state: &AppState,
    info: &DeliveryTargetInfo,
) -> Result<(), ControlError> {
    let Some(lifecycle) = state.try_lock_agent_lifecycle(&info.uuid).await else {
        return Ok(());
    };
    let current = delivery_target_info(state, &info.uuid).await?;
    if !same_delivery_target_incarnation(info, &current)
        || active_conversation_lease_for_delivery(&current)
    {
        return Ok(());
    }
    let generation = state
        .interactions
        .current_provider_input_generation(&info.uuid)
        .await
        .unwrap_or(0);
    if state
        .native_delivery
        .codex_binding(&info.uuid, generation)
        .await
        .is_err()
    {
        return Ok(());
    }
    let Some(claim) = state
        .interactions
        .claim_agent_task(&info.uuid, generation)
        .await
        .map_err(control_error)?
    else {
        return Ok(());
    };
    let context = prepare_claim_context(state, &claim).await?;
    drop(lifecycle);
    // The actor revalidates this generation before writing. No lifecycle lock
    // is held while waiting for native protocol acknowledgement.
    let result = state
        .native_delivery
        .codex_followup(&info.uuid, generation, &claim.record.id, &context)
        .await;
    settle(
        state,
        &claim,
        result
            .as_ref()
            .map(|receipt| receipt.delivery_state.as_str())
            .map_err(|error| error.provider_boundary_crossed),
    )
    .await?;
    result
        .map(|_| ())
        .map_err(|error| native_error(error, "native_followup_unavailable"))
}

fn native_error(error: NativeBrokerError, prewrite_code: &'static str) -> ControlError {
    ControlError::coded(
        if error.provider_boundary_crossed {
            "submitted_unconfirmed"
        } else {
            prewrite_code
        },
        error.message,
    )
}

fn heartbeat(owner: &ConversationLeaseOwner) -> Result<(), ControlError> {
    let now = chrono::Utc::now();
    let renewed = wardian_core::conversation_lease::renew_lease_owner_persisted(
        owner,
        &now.to_rfc3339(),
        &(now + chrono::Duration::minutes(20)).to_rfc3339(),
    )
    .map_err(ControlError::request_failed)?;
    if renewed {
        Ok(())
    } else {
        Err(ControlError::coded(
            "lease_lost",
            "Background conversation acquisition is no longer current.",
        ))
    }
}

/// Explicit task acquisition only. Information cannot reach this function.
/// A second background task stays pending until this owner has exited and its
/// lease is released; it never joins an owner scheduled for first-turn shutdown.
pub(super) async fn dispatch_background(
    app: Option<&AppHandle>,
    state: &AppState,
    info: &DeliveryTargetInfo,
    task: store::TaskClaim,
    mut lease: PersistedConversationLeaseGuard,
) -> Result<(), ControlError> {
    let context = prepare_claim_context(state, &task).await?;
    let owner = lease.owner().clone();
    let spec = NativeSessionSpec {
        target_agent_id: info.uuid.clone(),
        provider: info.provider.clone(),
        generation: task.generation,
        workspace: info.cwd.clone(),
        config: info.config.clone(),
    };
    let lease_lost = std::sync::atomic::AtomicBool::new(false);
    let result = {
        let run = async {
            state
                .native_delivery
                .prepare_codex_background(spec.clone(), &owner, state)
                .await
                .map_err(|error| crate::delivery::codex_shared::CodexSharedError {
                    code: error.code,
                    message: error.message,
                    provider_boundary_crossed: false,
                })?;
            if lease_lost.load(std::sync::atomic::Ordering::Acquire) {
                return Err(crate::delivery::codex_shared::CodexSharedError {
                    code: "lease_lost".into(),
                    message: "Background lease was lost during owner preparation.".into(),
                    provider_boundary_crossed: false,
                });
            }
            // The owner is ready now. Drain a finite snapshot using only bounded
            // reference pages and one canonical body at a time. Each information
            // claim is settled independently before advancing; no payload vector.
            push_pending_information(state, &info.uuid)
                .await
                .map_err(|error| crate::delivery::codex_shared::CodexSharedError {
                    code: "startup_information_failed".into(),
                    message: error.message,
                    // This task has not been submitted. Any information failure
                    // was already settled against its own separate claim.
                    provider_boundary_crossed: false,
                })?;
            state
                .interactions
                .validate_agent_message_claim(&task)
                .await
                .map_err(|error| crate::delivery::codex_shared::CodexSharedError {
                    code: error.code,
                    message: error.message,
                    provider_boundary_crossed: false,
                })?;
            if lease_lost.load(std::sync::atomic::Ordering::Acquire) {
                return Err(crate::delivery::codex_shared::CodexSharedError {
                    code: "lease_lost".into(),
                    message: "Background lease was lost before task admission.".into(),
                    provider_boundary_crossed: false,
                });
            }
            state
                .native_delivery
                .run_codex_background(
                    spec,
                    &owner,
                    Vec::new(),
                    &task.record.id,
                    &context,
                    bounded_headless_delivery_timeout(None),
                )
                .await
        };
        tokio::pin!(run);
        let mut renew = tokio::time::interval(Duration::from_secs(10));
        loop {
            tokio::select! {
                result = &mut run => break result,
                _ = renew.tick() => {
                    if let Err(error) = heartbeat(&owner) {
                        manager::log_debug(&format!("[WARDIAN] background lease: {error}"));
                        lease_lost.store(true, std::sync::atomic::Ordering::Release);
                        // Disposal signals even a not-yet-ready startup and
                        // joins its owned child before releasing the lease.
                        let _ = state.native_delivery.dispose_codex_generation(&info.uuid, task.generation).await;
                    }
                }
            }
        }
    };
    // Orchestration has ended before cleanup. Keep the acquisition renewed until
    // exact owner exit joins; a lease failure never abandons a startup child.
    loop {
        match state
            .native_delivery
            .dispose_codex_generation(&info.uuid, task.generation)
            .await
        {
            Ok(()) => break,
            Err(error) => {
                manager::log_debug(&format!("[WARDIAN] owner cleanup pending: {error}"));
                let _ = heartbeat(&owner);
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }
    }
    let publication = if result
        .as_ref()
        .is_err_and(|error| !error.provider_boundary_crossed)
    {
        match state.interactions.fail_agent_startup(&task).await {
            Ok(replied) => {
                if let Some(app) = app {
                    spawn_information(app, &replied.record.target_session_ids[0]);
                }
                Ok(())
            }
            Err(error) => Err(control_error(error)),
        }
    } else {
        settle(
            state,
            &task,
            result
                .as_ref()
                .map(|(receipt, _)| receipt.delivery_state.as_str())
                .map_err(|error| error.provider_boundary_crossed),
        )
        .await
    };
    if let Ok((_, answer)) = &result {
        record_headless_message_response(state, info, &task.record.id, answer).await;
    }
    lease.release().map_err(ControlError::request_failed)?;
    record_headless_status_observation(app, state, info).await;
    publication?;
    result.map(|_| ()).map_err(|error| {
        ControlError::coded(
            if error.provider_boundary_crossed {
                "submitted_unconfirmed"
            } else {
                "native_followup_unavailable"
            },
            error.message,
        )
    })
}
