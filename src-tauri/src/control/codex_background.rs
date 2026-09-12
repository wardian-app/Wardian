//! Native execution for ordinary messages to retained Off Codex agents.
//!
//! The caller owns the workspace execution guard. This module keeps the
//! conversation lease until the exact shared owner exits, on every outcome.
use super::*;
use crate::delivery::native_broker::{NativeBrokerError, NativeSessionSpec};
use wardian_core::conversation_lease::{ConversationLeaseOwner, PersistedConversationLeaseGuard};
use wardian_core::native_transport::{NativeDeliveryErrorCode, NativeDeliveryPhase};

fn failure(message: impl Into<String>, crossed: bool) -> NativeBrokerError {
    NativeBrokerError {
        code: if crossed {
            NativeDeliveryErrorCode::SubmittedUnconfirmed
        } else {
            NativeDeliveryErrorCode::CapabilityUnavailable
        },
        message: message.into(),
        provider_boundary_crossed: crossed,
    }
}

fn renew(owner: &ConversationLeaseOwner) -> Result<(), NativeBrokerError> {
    let now = chrono::Utc::now();
    let renewed = wardian_core::conversation_lease::renew_lease_owner_persisted(
        owner,
        &now.to_rfc3339(),
        &(now + chrono::Duration::minutes(20)).to_rfc3339(),
    )
    .map_err(|error| failure(error, true))?;
    if renewed {
        Ok(())
    } else {
        Err(failure("Background conversation lease was lost", true))
    }
}

pub(super) async fn deliver(
    state: &AppState,
    request: HeadlessMessageDeliveryRequest<'_>,
    mut lease: PersistedConversationLeaseGuard,
    generation: u64,
) -> HeadlessMessageDelivery {
    let info = request.info;
    let owner = lease.owner().clone();
    let lost = std::sync::atomic::AtomicBool::new(false);
    let run = async {
        if request.input_mode != MessageInputMode::Message {
            return Err(failure(
                "Off Codex native delivery requires message input",
                false,
            ));
        }
        if request
            .orchestration
            .and_then(|options| options.expected_generation)
            .is_some_and(|expected| expected != generation)
        {
            return Err(NativeBrokerError {
                code: NativeDeliveryErrorCode::StaleGeneration,
                message: "Expected provider generation is stale".into(),
                provider_boundary_crossed: false,
            });
        }
        let spec = NativeSessionSpec {
            target_agent_id: info.uuid.clone(),
            provider: "codex".into(),
            generation,
            workspace: info.cwd.clone(),
            config: info.config.clone(),
        };
        state
            .native_delivery
            .prepare_codex_background(spec, &owner, state)
            .await
            .map_err(|error| failure(error.message, error.provider_boundary_crossed))?;
        if lost.load(std::sync::atomic::Ordering::Acquire) {
            return Err(failure(
                "Lease lost during native startup; no message submitted",
                false,
            ));
        }
        agent_messaging::push_native_information(state, &info.uuid)
            .await
            .map_err(|error| failure(error.message, false))?;
        if lost.load(std::sync::atomic::Ordering::Acquire) {
            return Err(failure(
                "Lease lost before native admission; no message submitted",
                false,
            ));
        }
        let mut detail = deliver_native_message(
            state,
            info,
            request.interaction_id,
            request.prompt,
            request.input_mode,
            request.queue_policy,
            request.origin,
            request.orchestration,
            request.parent_interaction_id,
        )
        .await?;
        let deadline = tokio::time::Instant::now() + request.timeout;
        loop {
            let record = state.native_delivery.get(request.interaction_id)?;
            if record.phase == NativeDeliveryPhase::Completed {
                detail.delivery_state = "provider_applied".into();
                detail.delivery_phase = Some("completed".into());
                detail.observed_state = Some("matching_provider_completion".into());
                detail.reason = Some("Shared Codex background turn completed".into());
                return Ok((detail, record.detail.unwrap_or_default()));
            }
            if record.phase.is_terminal() {
                return Err(failure(
                    format!("Native background delivery ended with {:?}", record.phase),
                    true,
                ));
            }
            if lost.load(std::sync::atomic::Ordering::Acquire)
                || tokio::time::Instant::now() >= deadline
            {
                return Err(failure("Native background completion was not confirmed before lease loss or timeout; message was not replayed", true));
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    };
    let result = {
        tokio::pin!(run);
        let mut heartbeat = tokio::time::interval(Duration::from_secs(10));
        loop {
            tokio::select! {
                result = &mut run => break result,
                _ = heartbeat.tick() => {
                    if renew(&owner).is_err() {
                        lost.store(true, std::sync::atomic::Ordering::Release);
                        // Signal startup cancellation and join the owned child
                        // before releasing the conversation lease.
                        let _ = state.native_delivery.dispose_codex_generation(&info.uuid, generation).await;
                    }
                }
            }
        }
    };
    // No lease release or competing process until owner exit is confirmed.
    loop {
        match state
            .native_delivery
            .dispose_codex_generation(&info.uuid, generation)
            .await
        {
            Ok(()) => break,
            Err(error) => {
                manager::log_debug(&format!(
                    "[WARDIAN] native background cleanup pending: {error}"
                ));
                let _ = renew(&owner);
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }
    }
    let mut detail = match result {
        Ok((detail, answer)) => {
            record_headless_message_response(state, info, request.interaction_id, &answer).await;
            detail
        }
        Err(error) => native_delivery_failure_detail(
            info,
            request.interaction_id,
            request.input_mode,
            request.queue_policy,
            &error,
        ),
    };
    persist_interaction_delivery_attempt(
        state,
        request.interaction_id,
        &info.uuid,
        DeliveryTransportKind::NativeProvider,
        &detail,
    )
    .await;
    record_delivery_attempt(state, &detail).await;
    if let Err(error) = lease.release() {
        detail.reason = Some(format!(
            "Native owner exited; conversation lease cleanup pending: {error}"
        ));
    } else {
        record_headless_status_observation(request.app, state, info).await;
    }
    HeadlessMessageDelivery::Completed(Box::new(detail))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn ordinary_off_send_prepares_native_owner_and_never_falls_back_on_startup_failure() {
        let fixture = super::super::test_support::TestWardianHome::new_async().await;
        let state = AppState::new();
        let id = "11111111-1111-4111-8111-111111111121";
        // Exercise the real owner's first startup guard, before it can read a
        // provider home or launch a process. This value is a synthetic fixture.
        const KEY: &str = "WARDIAN_CODEX_STARTUP_TEST_TOKEN";
        struct Restore(Option<std::ffi::OsString>);
        impl Drop for Restore {
            fn drop(&mut self) {
                if let Some(value) = self.0.take() {
                    std::env::set_var(KEY, value);
                } else {
                    std::env::remove_var(KEY);
                }
            }
        }
        let _restore = Restore(std::env::var_os(KEY));
        std::env::set_var(KEY, id);
        let agent = super::super::tests::test_agent(id, "NativeBackground", "Test");
        {
            let mut config = agent.config.lock().unwrap();
            config.provider = "codex".into();
            config.is_off = true;
            config.folder = fixture.path().to_string_lossy().into_owned();
        }
        *agent.current_status.lock().unwrap() = "Off".into();
        state.agents.lock().await.insert(id.into(), agent);
        let error = deliver_prompt_to_agent(
            None,
            &state,
            id,
            "Do not launch a model",
            MessageInputMode::Message,
        )
        .await
        .expect_err("the actual native startup guard must reject the fixture");
        let detail = &error.details().unwrap()["delivery"][0];
        assert_eq!(detail["runtime_state"], "native_provider_session");
        assert_eq!(detail["delivery_phase"], "failed_before_submit");
        assert_eq!(detail["error"]["code"], "capability_unavailable");
        assert!(detail["error"]["message"]
            .as_str()
            .unwrap()
            .contains("session identifier matches a credential environment value"));
        assert!(wardian_core::conversation_lease::load_leases().is_empty());
        let generation = state
            .interactions
            .current_provider_input_generation(id)
            .await
            .unwrap();
        assert!(state
            .native_delivery
            .codex_binding(id, generation)
            .await
            .is_err());
        assert!(state
            .native_delivery
            .get(detail["message_id"].as_str().unwrap())
            .is_err());
    }
}
