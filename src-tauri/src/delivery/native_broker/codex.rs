//! Codex's shared owner lives in the existing native sessions registry.
use super::*;
use crate::delivery::codex_shared::{
    CodexSettingsUpdate, CodexSharedError, CodexSharedOwner, CodexSharedReceipt, CodexThreadExit,
    CodexTuiAttachment,
};
use std::time::Duration;

fn publish_codex_tui_binding(
    client: &super::super::codex_shared::CodexSharedClient,
    observed_version: &str,
    capabilities: &mut NativeTransportCapabilities,
    response: &serde_json::Value,
    publish_identity: impl FnOnce(&str) -> Result<(), CodexSharedError>,
    publication: Option<&crate::manager::RegistrationPublicationState>,
) -> Result<CodexSharedReceipt, CodexSharedError> {
    let id = response["thread"]["id"]
        .as_str()
        .ok_or_else(|| CodexSharedError::unsupported("attachment identity absent"))?;
    let (agent_id, generation) = client.identity();
    let publish = || {
        publish_identity(id)?;
        // Hold the exact startup lease across this synchronous boundary.
        let recovery = publication.is_some_and(|state| state.is_recovery());
        client.bind_with_input_gate(response, recovery)?;
        let negotiated = NativeProviderProtocol::CodexAppServer.capabilities(observed_version);
        let binding = NativeSessionBinding {
            target_agent_id: agent_id.to_owned(),
            generation,
            provider: "codex".into(),
            transport: "codex_app_server_ws".into(),
            provider_session_id: Some(id.to_owned()),
            capabilities: negotiated.clone(),
            observed_at: now(),
        };
        wardian_core::db::upsert_native_session_binding(&binding).map_err(|_| {
            CodexSharedError::unsupported("cannot persist attached native owner binding")
        })?;
        *capabilities = negotiated;
        client.attached_receipt()
    };
    match publication {
        Some(publication) => publication
            .with_recovery_attachment(agent_id, id, publish)
            .map_err(CodexSharedError::unsupported)?,
        None => publish(),
    }
}

/// Retain the exact owner and its creation gate through captured TUI shutdown.
/// Dropping before runtime capture merely releases the gate; the registry still
/// owns the live process. After capture the retained stop worker owns this token.
pub(crate) struct CapturedCodexStop {
    broker: Arc<NativeDeliveryBroker>,
    agent_id: String,
    owner_generation: Option<u64>,
    owner: Option<Arc<CodexSharedOwner>>,
    _gate: tokio::sync::OwnedMutexGuard<()>,
    _admission: CodexStopAdmission,
}

impl CapturedCodexStop {
    pub(crate) async fn finish(
        &self,
        thread_id: Option<&str>,
        tui_tree_exited: bool,
    ) -> Result<Option<Arc<CodexThreadExit>>, NativeBrokerError> {
        self.broker
            .dispose_pi_generation(&self.agent_id, None)
            .await?;
        self.broker
            .dispose_opencode_http(&self.agent_id, None)
            .await?;
        let current = self
            .broker
            .sessions
            .lock()
            .await
            .get(&self.agent_id)
            .cloned();
        if let Some(owner) = &self.owner {
            if !current
                .as_ref()
                .and_then(|handle| handle.shared_codex.as_ref())
                .is_some_and(|current| Arc::ptr_eq(current, owner))
            {
                return Err(shared_error(CodexSharedError::uncertain(
                    "captured Codex owner changed",
                )));
            }
        }
        let receipt = self
            .broker
            .stop_registered_owner_with_evidence(
                &self.agent_id,
                None,
                self.owner_generation,
                thread_id,
                tui_tree_exited,
            )
            .await?;
        Ok(receipt)
    }

    /// Validate writer evidence against the captured owner, whose generation
    /// is independent of the TUI generation retained by the stop worker.
    pub(crate) fn release_hold(
        &self,
        receipt: &CodexThreadExit,
        hold: &wardian_core::conversation_lease::ConversationLease,
    ) -> Result<(), String> {
        receipt.release_hold(
            hold,
            self.owner_generation
                .ok_or("Captured Codex owner generation is absent")?,
        )
    }
}

impl NativeDeliveryBroker {
    #[cfg(test)]
    pub(crate) async fn install_recovery_cleanup_fixture(&self, id: &str, generation: u64) {
        let (tx, mut incoming) = mpsc::channel(1);
        let (stopped, observed) = tokio::sync::watch::channel(false);
        self.sessions.lock().await.insert(
            id.to_owned(),
            NativeSessionHandle {
                generation,
                provider: "codex".into(),
                capabilities: NativeTransportCapabilities::degraded("codex", "codex_app_server_ws"),
                tx,
                shared_codex: None,
                codex_tui_runtime_generation: None,
                stopped: Some(observed),
            },
        );
        tokio::spawn(async move {
            assert!(matches!(
                incoming.recv().await,
                Some(SessionCommand::Shutdown)
            ));
            stopped.send_replace(true);
        });
    }

    /// Bind a newly registered terminal to its exact prepared owner. Provider
    /// input can advance without a terminal, so these counters are not equal.
    pub(crate) async fn bind_codex_tui_runtime(
        &self,
        agent_id: &str,
        provider_generation: u64,
        runtime_generation: u64,
    ) -> Result<(), CodexSharedError> {
        let gate = self.owner_gate(agent_id).await;
        let _gate = gate.lock().await;
        let mut sessions = self.sessions.lock().await;
        let handle = sessions
            .get_mut(agent_id)
            .filter(|handle| {
                handle.provider == "codex"
                    && handle.generation == provider_generation
                    && handle.shared_codex.is_some()
            })
            .ok_or_else(|| CodexSharedError::uncertain("Codex TUI owner changed before binding"))?;
        if runtime_generation == 0
            || handle
                .codex_tui_runtime_generation
                .is_some_and(|bound| bound != runtime_generation)
        {
            return Err(CodexSharedError::uncertain(
                "Codex owner is already bound to another TUI or the TUI generation is invalid",
            ));
        }
        handle.codex_tui_runtime_generation = Some(runtime_generation);
        Ok(())
    }

    /// Reserve the current Codex owner without stopping or discarding it before
    /// the roster's runtime and prior-session hold transfer into the stop fence.
    /// The supplied generation names the TUI, not the provider-input counter.
    pub(crate) async fn capture_codex_stop(
        self: &Arc<Self>,
        agent_id: &str,
        runtime_generation: Option<u64>,
    ) -> Result<CapturedCodexStop, NativeBrokerError> {
        let admission = self
            .codex_creations
            .pause_for_stop(agent_id)
            .map_err(|message| shared_error(CodexSharedError::unsupported(message)))?;
        let gate = self.lock_owner_for_stop(agent_id, None).await?;
        let handle = self.sessions.lock().await.get(agent_id).cloned();
        if handle
            .as_ref()
            .is_some_and(|handle| handle.codex_tui_runtime_generation != runtime_generation)
        {
            return Err(shared_error(CodexSharedError::uncertain(
                "Codex owner is not bound to captured TUI",
            )));
        }
        Ok(CapturedCodexStop {
            broker: self.clone(),
            agent_id: agent_id.to_owned(),
            owner_generation: handle.as_ref().map(|handle| handle.generation),
            owner: handle.and_then(|handle| handle.shared_codex),
            _gate: gate,
            _admission: admission,
        })
    }
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
        self.stop_registered_owner_with_evidence(agent_id, generation, None, None, false)
            .await
            .map(|_| ())
    }

    async fn stop_registered_owner_with_evidence(
        &self,
        agent_id: &str,
        generation: Option<u64>,
        captured_generation: Option<u64>,
        thread_id: Option<&str>,
        tui_tree_exited: bool,
    ) -> Result<Option<Arc<CodexThreadExit>>, NativeBrokerError> {
        let handle = self.sessions.lock().await.get(agent_id).cloned();
        let Some(handle) = handle else {
            return Ok(None);
        };
        if generation.is_some_and(|generation| generation != handle.generation) {
            return Ok(None);
        }
        let mut receipt = None;
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
            receipt = owner
                .stop_after_tui(
                    agent_id,
                    captured_generation.unwrap_or(0),
                    thread_id,
                    tui_tree_exited,
                )
                .await
                .map_err(shared_error)?;
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
        Ok(receipt)
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
        let mut agents: Vec<_> = self.sessions.lock().await.keys().cloned().collect();
        for agent_id in self.pi_bridges.lock().await.keys() {
            if !agents.iter().any(|known| known == agent_id) {
                agents.push(agent_id.clone());
            }
        }
        for agent_id in self.opencode_http.lock().await.keys() {
            if !agents.iter().any(|known| known == agent_id) {
                agents.push(agent_id.clone());
            }
        }
        for agent_id in self.opencode_http_states.lock().await.keys() {
            if !agents.iter().any(|known| known == agent_id) {
                agents.push(agent_id.clone());
            }
        }
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
        self.create_shared_codex_with_context(spec, None, None)
            .await
    }

    async fn create_shared_codex_with_context(
        self: &Arc<Self>,
        spec: NativeSessionSpec,
        codex_launch_context: Option<Arc<crate::utils::codex_home::CodexLaunchContext>>,
        recovery_heartbeat: Option<Arc<crate::commands::agent::LifecycleLeaseHeartbeat>>,
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
                .create_shared_codex_inner(
                    spec,
                    codex_launch_context,
                    recovery_heartbeat.as_deref(),
                    reply.closed(),
                    request,
                )
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
        codex_launch_context: Option<Arc<crate::utils::codex_home::CodexLaunchContext>>,
        recovery_heartbeat: Option<&crate::commands::agent::LifecycleLeaseHeartbeat>,
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
                codex_tui_runtime_generation: None,
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
                return crate::delivery::codex_shared::spawn_codex_owner_at_lease_boundary(
                    &spec,
                    recovery_heartbeat,
                    || {
                        self.codex_creation_test
                            .start_attempts
                            .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
                        Err("test owner-start sentinel".into())
                    },
                )
                .map_err(CodexSharedError::unsupported);
            }
            CodexSharedOwner::start(
                &spec,
                codex_launch_context,
                recovery_heartbeat,
                cancellation,
            )
            .await
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
                codex_tui_runtime_generation: None,
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
    pub(crate) async fn prepare_codex_tui(
        self: &Arc<Self>,
        spec: NativeSessionSpec,
        codex_launch_context: Option<Arc<crate::utils::codex_home::CodexLaunchContext>>,
        recovery_heartbeat: Option<Arc<crate::commands::agent::LifecycleLeaseHeartbeat>>,
    ) -> Result<CodexTuiAttachment, CodexSharedError> {
        Ok(self
            .create_shared_codex_with_context(spec, codex_launch_context, recovery_heartbeat)
            .await?
            .attachment
            .clone())
    }

    /// Manager has started the captured PTY reader/responder. Keep this owner
    /// unavailable until exclusive loaded membership, identity and policy agree.
    pub(crate) async fn finalize_codex_tui<F, Fut>(
        &self,
        agent_id: &str,
        generation: u64,
        tui_alive: F,
        publish_identity: impl FnOnce(&str) -> Result<(), CodexSharedError>,
        publication: Option<&crate::manager::RegistrationPublicationState>,
    ) -> Result<CodexSharedReceipt, CodexSharedError>
    where
        F: FnMut() -> Fut,
        Fut: std::future::Future<Output = Result<(), CodexSharedError>>,
    {
        let gate = self.owner_gate(agent_id).await;
        let _gate = gate.lock().await;
        let owner = self.shared_codex(agent_id, generation).await?;
        let result = async {
            if publication.is_some_and(|state| state.is_recovery())
                && owner.client.attached_receipt().is_ok()
            {
                return Err(CodexSharedError::unsupported(
                    "interactive attachment requires an unbound fresh owner",
                ));
            }
            let response = owner.finalize_interactive(tui_alive).await?;
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
            publish_codex_tui_binding(
                &owner.client,
                &owner.observed_version,
                &mut handle.capabilities,
                &response,
                publish_identity,
                publication,
            )
        }
        .await;
        if result.is_err() {
            // Keep the registry slot fenced. Manager joins its captured TUI
            // before disposing this daemon; no replacement can start early.
            owner.client.close().await;
        }
        result
    }

    /// Pin the still-current native binding while the captured TUI and input
    /// generation publish readiness after durable recovery registration.
    pub(crate) async fn lock_recovery_codex_input(
        &self,
        agent_id: &str,
        provider_generation: u64,
        runtime_generation: u64,
        native_id: &str,
    ) -> Result<
        (
            tokio::sync::OwnedMutexGuard<()>,
            Arc<crate::delivery::codex_shared::CodexSharedClient>,
        ),
        String,
    > {
        let gate = self.owner_gate(agent_id).await.lock_owned().await;
        let sessions = self.sessions.lock().await;
        let client = sessions
            .get(agent_id)
            .filter(|handle| {
                handle.generation == provider_generation
                    && handle.provider == "codex"
                    && handle.codex_tui_runtime_generation == Some(runtime_generation)
            })
            .and_then(|handle| handle.shared_codex.as_ref())
            .map(|owner| owner.client.clone())
            .filter(|client| {
                client.attached_receipt().is_ok_and(|binding| {
                    binding.wardian_agent_id == agent_id
                        && binding.generation == provider_generation
                        && binding.provider_session_id == native_id
                })
            })
            .ok_or("Captured Codex native input owner changed after registration")?;
        Ok((gate, client))
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

    /// Pins the exact already-bound idle owner for metadata finalization. Uses
    /// retained observations only and sends no command to Codex.
    pub(crate) async fn lock_codex_registration_for_recovery(
        &self,
        agent_id: &str,
        terminal_generation: u64,
        session: &str,
        model: &str,
    ) -> Option<(
        tokio::sync::OwnedMutexGuard<()>,
        Arc<crate::delivery::codex_shared::CodexSharedClient>,
        u64,
    )> {
        // Recovery already owns the roster barrier. A busy native owner may
        // be publishing into that roster, so fail closed without waiting.
        let gate = self.owner_gate(agent_id).await.try_lock_owned().ok()?;
        let handle = self.sessions.lock().await.get(agent_id)?.clone();
        let owner = handle.shared_codex?;
        let receipt = owner.client.receipt("owner_ready").ok()?;
        if handle.provider != "codex"
            || handle.codex_tui_runtime_generation != Some(terminal_generation)
            || receipt.wardian_agent_id != agent_id
            || receipt.generation != handle.generation
            || receipt.provider_session_id != session
            || owner.attachment.model_override.as_deref() != Some(model)
            || !owner.client.recovery_thread_is_idle(session)
        {
            return None;
        }
        Some((gate, owner.client.clone(), handle.generation))
    }

    /// Clone the current generation's exact client without retaining AppState or
    /// a registry lock while waiting for the admitted turn's final result.
    /// This never creates an owner or follows a replacement generation.
    pub async fn codex_completion_client(
        &self,
        agent_id: &str,
        generation: u64,
    ) -> Result<Arc<crate::delivery::codex_shared::CodexSharedClient>, NativeBrokerError> {
        Ok(self
            .shared_codex(agent_id, generation)
            .await
            .map_err(shared_error)?
            .client
            .clone())
    }

    /// Apply one model/effort pair through the already-owned Codex client.
    /// Recheck the exact owner after every awaited operation before exposing
    /// either acceptance or rejection to the caller.
    pub async fn codex_update_thread_settings(
        &self,
        agent_id: &str,
        generation: u64,
        model: &str,
        effort: &str,
    ) -> Result<CodexSettingsUpdate, CodexSharedError> {
        let owner = self.shared_codex(agent_id, generation).await?;
        let binding = owner.client.settings_binding()?;
        let result = owner.update_thread_settings(&binding, model, effort).await;
        let current = self.shared_codex(agent_id, generation).await;
        if !current
            .as_ref()
            .is_ok_and(|current| Arc::ptr_eq(current, &owner))
        {
            return Err(CodexSharedError::uncertain(
                "Codex owner changed after settings submission",
            ));
        }
        result
    }

    /// Read-only selection predicate for routing. A current-generation session
    /// slot or active creation request owns the native boundary even before a
    /// ready receipt exists; dispatch then fails closed through the state
    /// machine instead of falling through to another transport.
    pub(crate) async fn codex_owner_selected(&self, agent_id: &str, generation: u64) -> bool {
        let session_selected = self
            .sessions
            .lock()
            .await
            .get(agent_id)
            .is_some_and(|handle| handle.provider == "codex" && handle.generation == generation);
        session_selected || self.codex_creations.has_active(agent_id, generation)
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
            let (status, answer) = client.wait_for_final_result(turn_id, timeout).await?;
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
        client_send_authority: Option<&crate::remote::client_messages::ClientSendAuthority>,
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
            .native_message_with_authority(
                &record.envelope.message_id,
                &record.envelope.body,
                client_send_authority,
            )
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
        client_send_authority: Option<&crate::remote::client_messages::ClientSendAuthority>,
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
            .submit_shared_codex_input(&owner.client, &record, client_send_authority)
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

pub(crate) fn shared_error(failure: CodexSharedError) -> NativeBrokerError {
    error(
        if failure.provider_boundary_crossed {
            NativeDeliveryErrorCode::SubmittedUnconfirmed
        } else if matches!(
            failure.code.as_str(),
            "stale_turn_rejected" | "task_activity_deferred"
        ) {
            NativeDeliveryErrorCode::FailedBeforeSubmit
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
    async fn codex_recovery_attachment_publication_requires_live_exact_lease() {
        exercise_recovery_attachment_publication(true, false).await;
    }

    #[tokio::test]
    async fn codex_recovery_attachment_keeps_native_input_closed_before_commit() {
        exercise_recovery_attachment_publication(false, false).await;
    }

    #[tokio::test]
    async fn codex_recovery_attachment_leaves_superseded_input_closed_after_commit() {
        exercise_recovery_attachment_publication(false, true).await;
    }

    async fn exercise_recovery_attachment_publication(expire: bool, supersede: bool) {
        use crate::manager::codex_onboarding::publish_recovery_readiness;
        use wardian_core::control::ProviderInputReadiness;
        use wardian_core::conversation_lease::{self, CodexRecoveryLeaseScope};

        let _home = crate::control::test_support::TestWardianHome::new_async().await;
        let agent_id = uuid::Uuid::new_v4().to_string();
        let native_id = uuid::Uuid::new_v4().to_string();
        let state = crate::state::AppState::new();
        let input = state
            .interactions
            .start_provider_input_generation(&agent_id, ProviderInputReadiness::Booting, None)
            .await;
        let lease = crate::commands::agent::acquire_agent_lifecycle_transition_lease_for_identity(
            &agent_id,
            "attachment",
            "codex",
            "",
            "fixture",
        )
        .unwrap();
        assert!(conversation_lease::refresh_codex_recovery_lease_persisted(
            lease.owner(),
            chrono::Duration::minutes(20),
            CodexRecoveryLeaseScope::Attach {
                agent_id: &agent_id,
                native_id: &native_id
            },
        )
        .unwrap());
        let publication = crate::manager::RegistrationPublicationState::for_recovery(
            lease.owner().clone(),
            agent_id.clone(),
            native_id.clone(),
        );
        let (client, _peer) =
            crate::delivery::codex_shared::CodexSharedClient::unbound_input_test_client(
                &agent_id,
                input.generation,
            )
            .await;
        let response = serde_json::json!({
            "thread": {"id": native_id, "canAcceptDirectInput": true, "status": {"type":"idle"}}
        });
        let mut capabilities =
            NativeTransportCapabilities::degraded("codex", "codex_app_server_ws");
        let before = capabilities.clone();
        let mut published_identity = None;
        if expire {
            let mut leases = conversation_lease::load_leases_checked().unwrap();
            let exact = leases
                .iter_mut()
                .find(|entry| entry.owner() == *lease.owner())
                .unwrap();
            exact.expires_at = (chrono::Utc::now() - chrono::Duration::seconds(1)).to_rfc3339();
            conversation_lease::save_leases(&leases).unwrap();
        }
        assert!(!publication.recovery_lease_lost());
        let result = publish_codex_tui_binding(
            &client,
            "0.154.0",
            &mut capabilities,
            &response,
            |id| {
                published_identity = Some(id.to_owned());
                Ok(())
            },
            Some(&publication),
        );
        let binding = wardian_core::db::latest_native_session_binding(&agent_id).unwrap();
        if expire {
            assert!(
                result.is_err(),
                "an unsignaled expiry must prevent binding publication"
            );
            assert!(publication.recovery_lease_lost());
            assert!(binding.is_none());
            assert!(published_identity.is_none());
            assert_eq!(capabilities, before);
        } else {
            assert_eq!(result.unwrap().provider_session_id, native_id);
            assert_eq!(
                binding.unwrap().provider_session_id.as_deref(),
                Some(native_id.as_str())
            );
            assert_eq!(published_identity.as_deref(), Some(native_id.as_str()));
            assert_ne!(capabilities, before);
        }
        assert!(
            client.receipt("owner_ready").is_err(),
            "native input must stay gated until the durable commit"
        );
        if expire {
            return;
        }
        assert!(client.settings_binding().is_err());
        let blocked = tokio::time::timeout(
            Duration::from_millis(500),
            client.push(serde_json::json!({"fixture":"blocked before send"})),
        )
        .await
        .expect("pending input must fail before the protocol boundary")
        .unwrap_err();
        assert!(!blocked.provider_boundary_crossed);
        let mut status = "Booting";
        assert!(publish_recovery_readiness(
            &state,
            &agent_id,
            input.generation,
            &client,
            &publication,
            || {
                status = "Idle";
                Ok(())
            },
        )
        .await
        .is_err());
        assert_eq!(status, "Booting");
        assert_eq!(
            state.interactions.provider_input_state(&agent_id).await,
            Some(input.clone())
        );

        // Durable commit itself is covered by the worker fixtures. Exercise
        // only the input step at that established publication state.
        publication.mark_recovery_committed_for_test();
        let expected = if supersede {
            state
                .interactions
                .start_provider_input_generation(&agent_id, ProviderInputReadiness::Booting, None)
                .await
        } else {
            input.clone()
        };
        let published = publish_recovery_readiness(
            &state,
            &agent_id,
            input.generation,
            &client,
            &publication,
            || {
                status = "Idle";
                Ok(())
            },
        )
        .await;
        if supersede {
            assert!(published.is_err());
            assert_eq!(status, "Booting");
            assert_eq!(
                state.interactions.provider_input_state(&agent_id).await,
                Some(expected)
            );
            assert!(client.receipt("owner_ready").is_err());
        } else {
            published.unwrap();
            assert_eq!(status, "Idle");
            let current = state
                .interactions
                .provider_input_state(&agent_id)
                .await
                .unwrap();
            assert_eq!(current.generation, input.generation);
            assert_eq!(current.state, ProviderInputReadiness::Ready);
            assert_eq!(
                client.receipt("owner_ready").unwrap().provider_session_id,
                native_id
            );
        }
    }

    #[test]
    fn proven_stale_steer_maps_to_failed_before_submit_only_without_crossed_boundary() {
        for (code, crossed, expected) in [
            (
                "stale_turn_rejected",
                false,
                NativeDeliveryErrorCode::FailedBeforeSubmit,
            ),
            (
                "stale_turn_rejected",
                true,
                NativeDeliveryErrorCode::SubmittedUnconfirmed,
            ),
            (
                "task_activity_deferred",
                false,
                NativeDeliveryErrorCode::FailedBeforeSubmit,
            ),
            (
                "task_activity_deferred",
                true,
                NativeDeliveryErrorCode::SubmittedUnconfirmed,
            ),
            (
                "provider_rejected",
                true,
                NativeDeliveryErrorCode::SubmittedUnconfirmed,
            ),
            (
                "unsupported",
                false,
                NativeDeliveryErrorCode::CapabilityUnavailable,
            ),
        ] {
            let mapped = shared_error(CodexSharedError {
                code: code.into(),
                message: "safe diagnostic".into(),
                provider_boundary_crossed: crossed,
            });
            assert_eq!(mapped.code, expected);
            assert_eq!(mapped.provider_boundary_crossed, crossed);
        }
    }

    #[tokio::test]
    async fn codex_recovery_lease_gates_parked_detached_first_start() {
        use std::sync::atomic::Ordering;
        use wardian_core::conversation_lease::{self, CodexRecoveryLeaseScope};

        let _home = crate::control::test_support::TestWardianHome::new_async().await;
        for phase in ["live", "expired", "replaced"] {
            let broker = Arc::new(NativeDeliveryBroker::new());
            let config = AgentConfig {
                session_id: uuid::Uuid::new_v4().to_string(),
                session_name: format!("parked-{phase}"),
                provider: "codex".into(),
                resume_session: Some(uuid::Uuid::new_v4().to_string()),
                ..Default::default()
            };
            let mut lease =
                crate::commands::agent::acquire_agent_lifecycle_transition_lease_for_identity(
                    &config.session_id,
                    &config.session_name,
                    "codex",
                    "",
                    "fixture",
                )
                .unwrap();
            let original_owner = lease.owner().clone();
            lease.retain_on_drop();
            let heartbeat = Arc::new(
                crate::commands::agent::LifecycleLeaseHeartbeat::start_codex_recovery(
                    original_owner.clone(),
                ),
            );
            heartbeat
                .ensure_codex_recovery_scope(
                    "retarget fixture",
                    CodexRecoveryLeaseScope::Attach {
                        agent_id: &config.session_id,
                        native_id: config.resume_session.as_deref().unwrap(),
                    },
                )
                .unwrap();
            let parked = Arc::new(CodexCreationTestBarrier::default());
            broker
                .codex_creation_test
                .before_gate
                .lock()
                .unwrap()
                .insert(1, parked.clone());
            broker
                .codex_creation_test
                .reject_start
                .store(true, Ordering::Release);
            let agent_id = config.session_id.clone();
            let weak = Arc::downgrade(&heartbeat);
            let pending = {
                let broker = broker.clone();
                tokio::spawn(async move {
                    broker
                        .prepare_codex_tui(
                            NativeSessionSpec {
                                target_agent_id: config.session_id.clone(),
                                provider: "codex".into(),
                                generation: 1,
                                workspace: PathBuf::new(),
                                config,
                            },
                            None,
                            Some(heartbeat),
                        )
                        .await
                })
            };
            tokio::time::timeout(std::time::Duration::from_secs(5), parked.reached.notified())
                .await
                .unwrap();
            assert!(weak.upgrade().is_some());
            let mut leases = conversation_lease::load_leases_checked().unwrap();
            let entry = leases
                .iter_mut()
                .find(|entry| entry.owner() == original_owner)
                .unwrap();
            if phase == "expired" {
                entry.expires_at = (chrono::Utc::now() - chrono::Duration::seconds(1)).to_rfc3339();
            } else if phase == "replaced" {
                entry.acquisition_id = uuid::Uuid::new_v4().to_string();
            }
            let expected = entry.clone();
            conversation_lease::save_leases(&leases).unwrap();
            parked.release.notify_one();
            let error = pending.await.unwrap().err().unwrap();
            assert!(!error.provider_boundary_crossed);
            assert!(!broker.has_agent_owner(&agent_id).await.unwrap());
            if phase == "live" {
                assert!(error.message.contains("owner-start sentinel"));
                assert_eq!(
                    broker
                        .codex_creation_test
                        .start_attempts
                        .load(Ordering::Acquire),
                    1
                );
                lease.release().unwrap();
            } else {
                assert!(error
                    .message
                    .contains("lifecycle conversation lease was lost"));
                assert_eq!(
                    broker
                        .codex_creation_test
                        .start_attempts
                        .load(Ordering::Acquire),
                    0
                );
                assert!(conversation_lease::load_leases_checked()
                    .unwrap()
                    .contains(&expected));
            }
        }
    }

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
                        None,
                        None,
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
                    codex_tui_runtime_generation: None,
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
                codex_tui_runtime_generation: None,
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
                    codex_tui_runtime_generation: None,
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
