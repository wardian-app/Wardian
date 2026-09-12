//! Codex's shared owner lives in the existing native sessions registry.
use super::*;
use crate::delivery::codex_shared::{
    CodexSharedError, CodexSharedOwner, CodexSharedReceipt, CodexTuiAttachment,
};
use std::time::Duration;

impl NativeDeliveryBroker {
    pub(super) async fn owner_gate(&self, agent_id: &str) -> Arc<Mutex<()>> {
        self.owner_gates
            .lock()
            .await
            .entry(agent_id.to_owned())
            .or_default()
            .clone()
    }

    /// Invalidate already-requested creations before joining the owner gate,
    /// including detached workers not yet registered. Also signal starting slots
    /// published while waiting; retain exclusion until the owned child exits.
    pub(super) async fn lock_owner_for_stop(
        &self,
        agent_id: &str,
        generation: Option<u64>,
    ) -> Result<tokio::sync::OwnedMutexGuard<()>, NativeBrokerError> {
        self.codex_creations
            .cancel(agent_id, generation)
            .map_err(|message| shared_error(CodexSharedError::unsupported(message)))?;
        let gate = self.owner_gate(agent_id).await;
        loop {
            let changed = self.owner_changes.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            let handle = self.sessions.lock().await.get(agent_id).cloned();
            if let Some(handle) = handle.filter(|handle| {
                handle.provider == "codex"
                    && handle.shared_codex.is_none()
                    && generation.is_none_or(|value| value == handle.generation)
            }) {
                // A full starting channel already contains its shutdown signal.
                let _ = handle.tx.try_send(SessionCommand::Shutdown);
            }
            tokio::select! {
                guard = gate.clone().lock_owned() => return Ok(guard),
                _ = &mut changed => {}
            }
        }
    }

    /// Keep the registry's stopping slot until the owned process has exited.
    pub(super) async fn stop_registered_owner(
        &self,
        agent_id: &str,
        generation: Option<u64>,
    ) -> Result<(), NativeBrokerError> {
        let handle = self.sessions.lock().await.get(agent_id).cloned();
        let Some(handle) = handle else { return Ok(()) };
        if generation.is_some_and(|generation| generation != handle.generation) {
            return Ok(());
        }
        if let Some(owner) = &handle.shared_codex {
            // Withdraw advertised capabilities before joining transport exit.
            let binding = NativeSessionBinding {
                target_agent_id: agent_id.to_owned(),
                generation: handle.generation,
                provider: "codex".into(),
                transport: "codex_app_server_ws".into(),
                provider_session_id: None,
                capabilities: NativeTransportCapabilities::degraded("codex", "codex_app_server_ws"),
                observed_at: now(),
            };
            let _ = wardian_core::db::upsert_native_session_binding(&binding);
            owner.shutdown().await.map_err(shared_error)?;
        } else if let Some(mut stopped) = handle.stopped.clone() {
            let _ = handle.tx.send(SessionCommand::Shutdown).await;
            tokio::time::timeout(Duration::from_secs(30), async {
                loop {
                    if *stopped.borrow_and_update() {
                        return Ok(());
                    }
                    stopped.changed().await.map_err(|_| {
                        error(
                            NativeDeliveryErrorCode::TransportUnavailable,
                            "native owner ended without an exit acknowledgement",
                            false,
                        )
                    })?;
                }
            })
            .await
            .map_err(|_| {
                error(
                    NativeDeliveryErrorCode::TransportUnavailable,
                    "native owner shutdown is still pending; replacement blocked",
                    false,
                )
            })??;
        }
        self.sessions.lock().await.remove(agent_id);
        Ok(())
    }

    /// Used by captured PTY incarnations; an old exit cannot dispose a replacement.
    pub async fn dispose_codex_generation(
        &self,
        agent_id: &str,
        generation: u64,
    ) -> Result<(), NativeBrokerError> {
        let _gate = self.lock_owner_for_stop(agent_id, Some(generation)).await?;
        self.stop_registered_owner(agent_id, Some(generation)).await
    }

    pub async fn shutdown_all(&self) -> Result<(), NativeBrokerError> {
        // Fence queued Codex startups before taking the registry snapshot.
        self.shutting_down
            .store(true, std::sync::atomic::Ordering::Release);
        let agents: Vec<_> = self.sessions.lock().await.keys().cloned().collect();
        let mut first_error = None;
        for agent in agents {
            if let Err(error) = self.dispose_agent(&agent).await {
                first_error.get_or_insert(error);
            }
        }
        first_error.map_or(Ok(()), Err)
    }

    async fn create_shared_codex(
        self: &Arc<Self>,
        spec: NativeSessionSpec,
    ) -> Result<Arc<CodexSharedOwner>, CodexSharedError> {
        // Capture disposal revisions before the first await or detached spawn.
        let request = self
            .codex_creations
            .register(&spec.target_agent_id, spec.generation)
            .map_err(CodexSharedError::unsupported)?;
        // Initialization owns cleanup independently of the caller. Caller exit
        // or generation disposal signals cancellation; the gate remains held
        // until the same registered child has actually exited.
        let broker = self.clone();
        let (mut reply, result) = oneshot::channel();
        tokio::spawn(async move {
            let agent_id = spec.target_agent_id.clone();
            let generation = spec.generation;
            let started = broker
                .create_shared_codex_inner(spec, reply.closed(), request)
                .await;
            let created = started.as_ref().is_ok_and(|(_, created)| *created);
            if reply.send(started.map(|(owner, _)| owner)).is_err() && created {
                let _ = broker.dispose_codex_generation(&agent_id, generation).await;
            }
        });
        result.await.map_err(|_| {
            CodexSharedError::uncertain("owner initialization ended without its result")
        })?
    }

    async fn create_shared_codex_inner(
        &self,
        spec: NativeSessionSpec,
        cancelled: impl std::future::Future<Output = ()>,
        request: CodexCreationRequest,
    ) -> Result<(Arc<CodexSharedOwner>, bool), CodexSharedError> {
        #[cfg(test)]
        {
            let barrier = self
                .codex_creation_test
                .before_gate
                .lock()
                .unwrap()
                .get(&spec.generation)
                .cloned();
            if let Some(barrier) = barrier {
                barrier.reached.notify_one();
                barrier.release.notified().await;
            }
        }
        let cancelled = async {
            tokio::select! {
                _ = cancelled => {},
                _ = request.cancelled() => {},
            }
        };
        tokio::pin!(cancelled);
        let gate = self.owner_gate(&spec.target_agent_id).await;
        let _gate = tokio::select! {
            biased;
            _ = &mut cancelled => return Err(CodexSharedError::unsupported("owner preparation caller ended or generation disposed")),
            guard = gate.lock_owned() => guard,
        };
        let mut sessions = self.sessions.lock().await;
        if request.is_cancelled() {
            return Err(CodexSharedError::unsupported(
                "owner creation cancelled by disposal",
            ));
        }
        if self
            .shutting_down
            .load(std::sync::atomic::Ordering::Acquire)
        {
            return Err(CodexSharedError::unsupported(
                "native broker is shutting down; no owner started",
            ));
        }
        if let Some(handle) = sessions.get(&spec.target_agent_id).cloned() {
            if handle.generation != spec.generation {
                return Err(CodexSharedError::unsupported(
                    "old owner must exit before starting another generation",
                ));
            }
            let owner = handle.shared_codex.ok_or_else(|| {
                CodexSharedError::unsupported(
                    "existing embedded/stdio session requires explicit restart",
                )
            })?;
            if !spec.config.is_off {
                return Err(CodexSharedError::unsupported(
                    "interactive attachment requires the previous generation to exit; loaded owners cannot prove TUI attachment",
                ));
            }
            owner.client.receipt("owner_ready")?;
            return Ok((owner, false));
        }
        let capabilities = NativeTransportCapabilities::degraded("codex", "codex_app_server_ws");
        let (starting_tx, mut starting_rx) = mpsc::channel(1);
        let (starting_stopped, starting_exit) = tokio::sync::watch::channel(false);
        sessions.insert(
            spec.target_agent_id.clone(),
            NativeSessionHandle {
                generation: spec.generation,
                provider: "codex".into(),
                capabilities: capabilities.clone(),
                tx: starting_tx,
                shared_codex: None,
                stopped: Some(starting_exit),
            },
        );
        drop(sessions);
        self.owner_changes.notify_waiters();
        let cancellation = async {
            tokio::select! {
                _ = &mut cancelled => {},
                _ = starting_rx.recv() => {},
            }
        };
        let start = async {
            #[cfg(test)]
            if self
                .codex_creation_test
                .reject_start
                .load(std::sync::atomic::Ordering::Acquire)
            {
                self.codex_creation_test
                    .start_attempts
                    .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
                return Err(CodexSharedError::unsupported("test owner-start sentinel"));
            }
            CodexSharedOwner::start(&spec, cancellation).await
        };
        let owner = match start.await {
            Ok(owner) => owner,
            Err(error) => {
                // start returns only after its child (if any) has exited. A
                // panic drops the false acknowledgement and leaves the slot
                // fenced instead of authorizing a second provider process.
                starting_stopped.send_replace(true);
                self.sessions.lock().await.remove(&spec.target_agent_id);
                return Err(error);
            }
        };
        let ready = spec.config.is_off;
        let capabilities = if ready {
            NativeProviderProtocol::CodexAppServer.capabilities(&owner.observed_version)
        } else {
            capabilities
        };
        let binding = NativeSessionBinding {
            target_agent_id: spec.target_agent_id.clone(),
            generation: spec.generation,
            provider: "codex".into(),
            transport: "codex_app_server_ws".into(),
            provider_session_id: if ready {
                Some(owner.client.receipt("owner_ready")?.provider_session_id)
            } else {
                None
            },
            capabilities: capabilities.clone(),
            observed_at: now(),
        };
        let (tx, _rx) = mpsc::channel(1);
        self.sessions.lock().await.insert(
            spec.target_agent_id.clone(),
            NativeSessionHandle {
                generation: spec.generation,
                provider: "codex".into(),
                capabilities,
                tx,
                shared_codex: Some(owner.clone()),
                stopped: None,
            },
        );
        if request.is_cancelled() {
            // Keep the registered owner reachable if shutdown fails. A cancelled
            // creation cannot publish success or abandon an unjoined child.
            owner.shutdown().await?;
            self.sessions.lock().await.remove(&spec.target_agent_id);
            return Err(CodexSharedError::unsupported(
                "owner creation cancelled by disposal",
            ));
        }
        if wardian_core::db::upsert_native_session_binding(&binding).is_err() {
            // Even a persistence failure retains the owner slot until exit.
            owner.shutdown().await?;
            self.sessions.lock().await.remove(&spec.target_agent_id);
            return Err(CodexSharedError::unsupported(
                "cannot persist native owner binding",
            ));
        }
        Ok((owner, true))
    }

    /// Lifecycle-only launch; manager must have terminated the prior incarnation.
    pub async fn prepare_codex_tui(
        self: &Arc<Self>,
        spec: NativeSessionSpec,
    ) -> Result<CodexTuiAttachment, CodexSharedError> {
        Ok(self.create_shared_codex(spec).await?.attachment.clone())
    }

    /// Manager has started the captured PTY reader/responder. Keep this owner
    /// unavailable until exclusive loaded membership, identity and policy agree.
    pub(crate) async fn finalize_codex_tui(
        &self,
        agent_id: &str,
        generation: u64,
        tui_alive: impl FnMut() -> Result<(), CodexSharedError>,
        publish_identity: impl FnOnce(&str) -> Result<(), CodexSharedError>,
    ) -> Result<CodexSharedReceipt, CodexSharedError> {
        let gate = self.owner_gate(agent_id).await;
        let _gate = gate.lock().await;
        let owner = self.shared_codex(agent_id, generation).await?;
        let result = async {
            let response = owner.finalize_interactive(tui_alive).await?;
            let id = response["thread"]["id"]
                .as_str()
                .ok_or_else(|| CodexSharedError::unsupported("attachment identity absent"))?;
            let mut sessions = self.sessions.lock().await;
            let handle = sessions
                .get_mut(agent_id)
                .filter(|handle| {
                    handle.generation == generation
                        && handle
                            .shared_codex
                            .as_ref()
                            .is_some_and(|current| Arc::ptr_eq(current, &owner))
                })
                .ok_or_else(|| CodexSharedError::unsupported("attachment owner changed"))?;
            publish_identity(id)?;
            // No await between binding, persistence and capability publication.
            owner.client.bind(&response)?;
            let capabilities =
                NativeProviderProtocol::CodexAppServer.capabilities(&owner.observed_version);
            let binding = NativeSessionBinding {
                target_agent_id: agent_id.to_owned(),
                generation,
                provider: "codex".into(),
                transport: "codex_app_server_ws".into(),
                provider_session_id: Some(id.to_owned()),
                capabilities: capabilities.clone(),
                observed_at: now(),
            };
            wardian_core::db::upsert_native_session_binding(&binding).map_err(|_| {
                CodexSharedError::unsupported("cannot persist attached native owner binding")
            })?;
            handle.capabilities = capabilities;
            owner.client.receipt("owner_ready")
        }
        .await;
        if result.is_err() {
            // Keep the registry slot fenced. Manager joins its captured TUI
            // before disposing this daemon; no replacement can start early.
            owner.client.close().await;
        }
        result
    }

    async fn shared_codex(
        &self,
        agent_id: &str,
        generation: u64,
    ) -> Result<Arc<CodexSharedOwner>, CodexSharedError> {
        let handle = self
            .sessions
            .lock()
            .await
            .get(agent_id)
            .cloned()
            .ok_or_else(|| {
                CodexSharedError::unsupported(
                    "no shared Codex owner; information does not start one",
                )
            })?;
        if handle.generation != generation {
            return Err(CodexSharedError::unsupported(
                "stale Codex owner generation",
            ));
        }
        handle.shared_codex.ok_or_else(|| {
            CodexSharedError::unsupported(
                "embedded/stdio Codex is unsupported until explicit restart",
            )
        })
    }

    pub async fn codex_binding(
        &self,
        agent_id: &str,
        generation: u64,
    ) -> Result<CodexSharedReceipt, NativeBrokerError> {
        self.shared_codex(agent_id, generation)
            .await
            .map_err(shared_error)?
            .client
            .receipt("owner_ready")
            .map_err(shared_error)
    }

    /// Subscribe without creating an owner or changing its thread/generation.
    pub(crate) async fn codex_observations(
        &self,
        agent_id: &str,
        generation: u64,
    ) -> Result<
        tokio::sync::watch::Receiver<crate::delivery::codex_shared::Observation>,
        NativeBrokerError,
    > {
        Ok(self
            .shared_codex(agent_id, generation)
            .await
            .map_err(shared_error)?
            .client
            .observations())
    }

    /// Caller owns the durable message claim. Does not consume receive_messages cursors.
    pub async fn codex_push(
        &self,
        agent_id: &str,
        generation: u64,
        message_id: &str,
        context: &str,
    ) -> Result<CodexSharedReceipt, NativeBrokerError> {
        let frame: serde_json::Value = serde_json::from_str(context).map_err(|_| {
            shared_error(CodexSharedError::unsupported(
                "native push requires a serialized canonical Wardian frame",
            ))
        })?;
        let owner = self
            .shared_codex(agent_id, generation)
            .await
            .map_err(shared_error)?;
        let mut receipt = owner.client.push(frame).await.map_err(shared_error)?;
        receipt.message_id = Some(message_id.to_owned());
        Ok(receipt)
    }

    /// Caller owns task admission and correlation; active Codex work uses native admission.
    pub async fn codex_followup(
        &self,
        agent_id: &str,
        generation: u64,
        message_id: &str,
        context: &str,
    ) -> Result<CodexSharedReceipt, NativeBrokerError> {
        self.shared_codex(agent_id, generation)
            .await
            .map_err(shared_error)?
            .client
            .followup(message_id, context)
            .await
            .map_err(shared_error)
    }

    pub async fn codex_interrupt(
        &self,
        agent_id: &str,
        generation: u64,
    ) -> Result<CodexSharedReceipt, NativeBrokerError> {
        self.shared_codex(agent_id, generation)
            .await
            .map_err(shared_error)?
            .client
            .interrupt()
            .await
            .map_err(shared_error)
    }

    /// Explicit background policy only. Lease precedes lifecycle gate at the caller.
    /// Only an already-claimed task authorizes this preparation. The caller then
    /// drains bounded claimed Message/Reply pages with codex_push, admits the task
    /// once, and joins dispose_codex_generation on every outcome before releasing
    /// its lease. Information alone never calls this launch seam.
    pub async fn prepare_codex_background(
        self: &Arc<Self>,
        spec: NativeSessionSpec,
        lease_owner: &wardian_core::conversation_lease::ConversationLeaseOwner,
        state: &crate::state::AppState,
    ) -> Result<CodexSharedReceipt, CodexSharedError> {
        validate_background_lease(&spec, lease_owner)?;
        let starts_fresh = crate::manager::codex_shared::background_starts_fresh(&spec.config);
        let mut launch_spec = spec.clone();
        if starts_fresh {
            launch_spec.config.resume_session = None;
        }
        let owner = self.create_shared_codex(launch_spec).await?;
        let receipt = owner.client.receipt("owner_ready")?;
        // Recheck the acquisition after potentially long initialization, before
        // publishing current identity or accepting any task/provider context.
        validate_background_lease(&spec, lease_owner)?;
        crate::manager::codex_shared::publish_background_identity(
            state,
            &spec,
            &receipt.provider_session_id,
            starts_fresh,
        )
        .await
        .map_err(CodexSharedError::unsupported)?;
        Ok(receipt)
    }

    /// Execute only on the exact owner prepared by the claimed-task startup path.
    /// Run-stage lookup never creates an owner. Validate the lease before each
    /// provider operation and join exact-generation disposal on every outcome.
    pub async fn run_codex_background(
        self: &Arc<Self>,
        spec: NativeSessionSpec,
        lease_owner: &wardian_core::conversation_lease::ConversationLeaseOwner,
        pending_context: Vec<serde_json::Value>,
        message_id: &str,
        context: &str,
        timeout: Duration,
    ) -> Result<(CodexSharedReceipt, String), CodexSharedError> {
        let agent_id = spec.target_agent_id.clone();
        let generation = spec.generation;
        let result = async {
            validate_background_lease(&spec, lease_owner)?;
            let owner = self.shared_codex(&agent_id, generation).await?;
            Self::run_prepared_codex_background(
                &owner.client,
                &spec,
                lease_owner,
                pending_context,
                message_id,
                context,
                timeout,
            )
            .await
        }
        .await;
        self.dispose_codex_generation(&agent_id, generation)
            .await
            .map_err(|failure| CodexSharedError {
                code: "owner_shutdown_failed".into(),
                message: failure.message,
                provider_boundary_crossed: true,
            })?;
        result
    }

    async fn run_prepared_codex_background(
        client: &crate::delivery::codex_shared::CodexSharedClient,
        spec: &NativeSessionSpec,
        lease_owner: &wardian_core::conversation_lease::ConversationLeaseOwner,
        pending_context: Vec<serde_json::Value>,
        message_id: &str,
        context: &str,
        timeout: Duration,
    ) -> Result<(CodexSharedReceipt, String), CodexSharedError> {
        let mut crossed = false;
        let result = async {
            for context in pending_context {
                validate_background_lease(spec, lease_owner)?;
                client.push(context).await?;
                crossed = true;
            }
            validate_background_lease(spec, lease_owner)?;
            let mut receipt = client.followup(message_id, context).await?;
            crossed = true;
            let turn_id = receipt.provider_turn_id.as_deref().ok_or_else(|| {
                CodexSharedError::uncertain("native followup returned no turn identity")
            })?;
            let (status, answer) = client.wait_for_turn(turn_id, timeout).await?;
            if status != "completed" {
                return Err(CodexSharedError {
                    code: "provider_turn_failed".into(),
                    message: format!("native turn ended with {status}"),
                    provider_boundary_crossed: true,
                });
            }
            receipt.delivery_state = "provider_completed".into();
            Ok((receipt, answer))
        }
        .await;
        result.map_err(|mut error: CodexSharedError| {
            error.provider_boundary_crossed |= crossed;
            error
        })
    }

    // Keep admission and failure persistence together so callers cannot leave a
    // returned transport failure looking like an in-flight durable dispatch.
    async fn submit_shared_codex_input(
        &self,
        client: &crate::delivery::codex_shared::CodexSharedClient,
        record: &NativeDeliveryRecord,
    ) -> Result<CodexSharedReceipt, NativeBrokerError> {
        let id = &record.envelope.interaction_id;
        self.advance(
            id,
            NativeDeliveryPhase::Dispatching,
            NativeEvidenceSource::Caller,
            None,
            None,
            None,
            "shared_dispatching",
        )
        .await?;
        // NativeMessageEnvelope is ordinary input, including legacy agent-origin
        // messages. Canonical v2 tasks use codex_followup/run_codex_background.
        // Keep envelope identity in broker evidence, not a peer tool-output frame.
        match client
            .native_message(&record.envelope.message_id, &record.envelope.body)
            .await
        {
            Ok(receipt) => Ok(receipt),
            Err(failure) => {
                let failure = shared_error(failure);
                let phase = if failure.provider_boundary_crossed {
                    NativeDeliveryPhase::SubmittedUnconfirmed
                } else {
                    NativeDeliveryPhase::FailedBeforeSubmit
                };
                self.advance(
                    id,
                    phase,
                    NativeEvidenceSource::Caller,
                    None,
                    None,
                    Some(failure.message.clone()),
                    "shared_submission_failed",
                )
                .await
                .map_err(|mut persistence| {
                    // A failed database write cannot make an uncertain provider
                    // submission safe to replay. Retain both failure diagnostics.
                    persistence.provider_boundary_crossed |= failure.provider_boundary_crossed;
                    persistence.message = format!("{}; {}", failure.message, persistence.message);
                    persistence
                })?;
                Err(failure)
            }
        }
    }

    pub(super) async fn dispatch_shared_codex(
        self: &Arc<Self>,
        owner: Arc<CodexSharedOwner>,
        record: NativeDeliveryRecord,
        capabilities: NativeTransportCapabilities,
    ) -> Result<NativeDispatchReceipt, NativeBrokerError> {
        if record.envelope.operation == NativeMessageOperation::InvalidatePremise {
            return Err(error(
                NativeDeliveryErrorCode::CapabilityUnavailable,
                "legacy invalidate-premise is not ordinary followup admission on the shared owner",
                false,
            ));
        }
        let id = record.envelope.interaction_id.clone();
        let accepted = self
            .submit_shared_codex_input(&owner.client, &record)
            .await?;
        let updated = self
            .advance(
                &id,
                NativeDeliveryPhase::ProviderAccepted,
                NativeEvidenceSource::ProviderEvent,
                None,
                accepted.provider_turn_id.clone(),
                None,
                "shared_accepted",
            )
            .await
            .map_err(|mut failure| {
                failure.provider_boundary_crossed = true;
                failure
            })?;
        let binding = NativeSessionBinding {
            target_agent_id: accepted.wardian_agent_id,
            generation: accepted.generation,
            provider: "codex".into(),
            transport: "codex_app_server_ws".into(),
            provider_session_id: Some(accepted.provider_session_id),
            capabilities: capabilities.clone(),
            observed_at: now(),
        };
        let broker = self.clone();
        if let Some(turn_id) = accepted.provider_turn_id {
            tokio::spawn(async move {
                if let Ok((status, answer)) = owner
                    .client
                    .wait_for_turn(&turn_id, Duration::from_secs(900))
                    .await
                {
                    let phase = match status.as_str() {
                        "completed" => NativeDeliveryPhase::Completed,
                        "interrupted" => NativeDeliveryPhase::Cancelled,
                        _ => NativeDeliveryPhase::Failed,
                    };
                    let _ = broker
                        .advance(
                            &id,
                            phase,
                            NativeEvidenceSource::ProviderEvent,
                            None,
                            Some(turn_id),
                            Some(answer),
                            "shared_completed",
                        )
                        .await;
                }
            });
        }
        Ok(NativeDispatchReceipt {
            record: updated,
            binding,
            capabilities,
        })
    }
}

fn validate_background_lease(
    spec: &NativeSessionSpec,
    lease_owner: &wardian_core::conversation_lease::ConversationLeaseOwner,
) -> Result<(), CodexSharedError> {
    let now = chrono::Utc::now();
    let leased = wardian_core::conversation_lease::load_leases()
        .iter()
        .any(|lease| {
            lease.agent_id == spec.target_agent_id
                && lease.provider == "codex"
                && lease.owner() == *lease_owner
                && chrono::DateTime::parse_from_rfc3339(&lease.expires_at)
                    .is_ok_and(|end| end > now)
        });
    if !spec.config.is_off || !leased {
        return Err(CodexSharedError::unsupported(
            "background Codex requires off target and current explicit execution lease",
        ));
    }
    Ok(())
}

pub(super) fn shared_error(failure: CodexSharedError) -> NativeBrokerError {
    error(
        if failure.provider_boundary_crossed {
            NativeDeliveryErrorCode::SubmittedUnconfirmed
        } else {
            NativeDeliveryErrorCode::CapabilityUnavailable
        },
        failure.message,
        failure.provider_boundary_crossed,
    )
}

#[cfg(test)]
#[path = "codex_dispatch_tests.rs"]
mod dispatch_tests;

#[cfg(test)]
#[path = "codex_lifecycle_tests.rs"]
mod lifecycle_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn app_shutdown_fences_startup_still_waiting_to_register() {
        let broker = Arc::new(NativeDeliveryBroker::new());
        let gate = broker.owner_gate("late").await.lock_owned().await;
        let starting = {
            let broker = broker.clone();
            tokio::spawn(async move {
                broker
                    .create_shared_codex_inner(
                        NativeSessionSpec {
                            target_agent_id: "late".into(),
                            provider: "codex".into(),
                            generation: 1,
                            workspace: PathBuf::new(),
                            config: AgentConfig {
                                session_id: "late".into(),
                                ..Default::default()
                            },
                        },
                        std::future::pending(),
                        broker.codex_creations.register("late", 1).unwrap(),
                    )
                    .await
            })
        };
        tokio::task::yield_now().await;
        broker.shutdown_all().await.unwrap();
        drop(gate);
        let error = tokio::time::timeout(Duration::from_secs(2), starting)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err();
        assert!(error.message.contains("shutting down"));
        assert!(!error.provider_boundary_crossed);
        assert!(broker.sessions.lock().await.is_empty());
    }

    #[tokio::test]
    async fn disposal_signals_a_startup_published_after_stop_begins() {
        tokio::time::timeout(Duration::from_secs(2), async {
            let broker = Arc::new(NativeDeliveryBroker::new());
            let gate = broker.owner_gate("starting").await.lock_owned().await;
            let stopping = {
                let broker = broker.clone();
                tokio::spawn(async move { broker.dispose_codex_generation("starting", 7).await })
            };
            tokio::task::yield_now().await;
            let (tx, mut rx) = mpsc::channel(1);
            let (stopped, observed) = tokio::sync::watch::channel(false);
            broker.sessions.lock().await.insert(
                "starting".into(),
                NativeSessionHandle {
                    generation: 7,
                    provider: "codex".into(),
                    capabilities: NativeTransportCapabilities::degraded(
                        "codex",
                        "codex_app_server_ws",
                    ),
                    tx,
                    shared_codex: None,
                    stopped: Some(observed),
                },
            );
            broker.owner_changes.notify_waiters();
            assert!(matches!(rx.recv().await, Some(SessionCommand::Shutdown)));
            assert!(
                !stopping.is_finished(),
                "disposal must join startup cleanup before returning"
            );
            stopped.send_replace(true);
            broker.sessions.lock().await.remove("starting");
            drop(gate);
            stopping.await.unwrap().unwrap();
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn stale_disposal_never_signals_a_new_startup_generation() {
        let broker = NativeDeliveryBroker::new();
        let (tx, mut rx) = mpsc::channel(1);
        let (_stopped, observed) = tokio::sync::watch::channel(false);
        broker.sessions.lock().await.insert(
            "new".into(),
            NativeSessionHandle {
                generation: 8,
                provider: "codex".into(),
                capabilities: NativeTransportCapabilities::degraded("codex", "codex_app_server_ws"),
                tx,
                shared_codex: None,
                stopped: Some(observed),
            },
        );
        broker.dispose_codex_generation("new", 7).await.unwrap();
        assert!(matches!(
            rx.try_recv(),
            Err(mpsc::error::TryRecvError::Empty)
        ));
        assert_eq!(broker.sessions.lock().await["new"].generation, 8);
    }

    #[tokio::test]
    async fn shutdown_visits_remaining_slots_after_first_failure_and_fences_old_generation() {
        let broker = NativeDeliveryBroker::new();
        for id in ["first", "second", "third"] {
            let (tx, _rx) = mpsc::channel(1);
            let (stopped, observed) = tokio::sync::watch::channel(true);
            broker.sessions.lock().await.insert(
                id.into(),
                NativeSessionHandle {
                    generation: 7,
                    provider: "codex".into(),
                    capabilities: NativeProviderProtocol::CodexAppServer.capabilities("0.153.4"),
                    tx,
                    shared_codex: None,
                    stopped: Some(observed),
                },
            );
            drop(stopped);
        }
        // Force failure in the actual first iteration position, independent of
        // HashMap's randomized order. There are no provider children in this test.
        let first = broker.sessions.lock().await.keys().next().unwrap().clone();
        let (failed, observed) = tokio::sync::watch::channel(false);
        broker
            .sessions
            .lock()
            .await
            .get_mut(&first)
            .unwrap()
            .stopped = Some(observed);
        drop(failed);
        assert!(broker.shutdown_all().await.is_err());
        assert_eq!(broker.sessions.lock().await.len(), 1);
        assert!(broker.sessions.lock().await.contains_key(&first));
        broker.dispose_codex_generation(&first, 6).await.unwrap();
        assert!(broker.sessions.lock().await.contains_key(&first));
    }
}
