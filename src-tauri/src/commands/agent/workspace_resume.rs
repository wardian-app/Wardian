//! Session-preserving worktree handover reuses the ordinary resume replacement pipeline.
//! The published roster retains the old folder until the replacement commits.
use super::*;

/// Shared exclusion protects both the old published folder and unassigned launch target.
pub(super) fn protect_worktree_assignment(
) -> Result<wardian_core::automation_execution_lock::HeadlessExecutionGuard, String> {
    wardian_core::automation_execution_lock::acquire_headless_execution_guard()
        .map_err(|error| format!("Cannot assign a worktree while deletion is in progress: {error}"))
}

/// Prepare the exact-ID launch contract before stopping any existing runtime.
pub(super) fn prepare_workspace_resume_config(
    config: &AgentConfig,
    mutation: &WorkspaceMutation,
) -> Result<AgentConfig, String> {
    if config
        .resume_session
        .as_deref()
        .is_none_or(|id| id.trim().is_empty())
    {
        return Err("Cannot move a running provider without its exact conversation identity; wait for provider initialization or pause it first".into());
    }
    let mut prepared = config.clone();
    mutation.apply(&mut prepared)?;
    let persistence = prepared.session_persistence;
    prepared.session_persistence = AgentSessionPersistenceOverride::Resume;
    prepare_resume_config_for_runtime(&mut prepared, 0)?;
    if !matches!(prepared.provider.as_str(), "codex" | "mock") {
        return Err(format!("Active worktree handover is not qualified for provider {}; turn the agent Off before assigning a different workspace", prepared.provider));
    }
    prepared.session_persistence = persistence;
    Ok(prepared)
}

pub(super) struct WorktreeLifecycle {
    pub(super) guard: tokio::sync::OwnedMutexGuard<()>,
    pub(super) lease: wardian_core::conversation_lease::PersistedConversationLeaseGuard,
    pub(super) heartbeat: LifecycleLeaseHeartbeat,
}

pub(super) enum WorkspaceMutation {
    Enable(std::path::PathBuf),
    Join(String),
    Disable,
}

impl WorkspaceMutation {
    pub(super) fn apply(&self, config: &mut AgentConfig) -> Result<(), String> {
        match self {
            Self::Enable(path) => {
                enable_worktree_config(config, path);
                Ok(())
            }
            Self::Join(folder) => assign_worktree_config(config, folder),
            Self::Disable => disable_worktree_config(config),
        }
    }
}

pub(super) async fn finish_worktree_mutation(
    session_id: String,
    state: State<'_, AppState>,
    app: AppHandle,
    mutation: WorkspaceMutation,
    context: WorktreeLifecycle,
) -> Result<(), String> {
    context.heartbeat.ensure_active("worktree assignment")?;
    let is_stopped = {
        let agents = state.agents.lock().await;
        let agent = agents
            .get(&session_id)
            .ok_or_else(|| format!("Agent {session_id} not found"))?;
        !agent_has_running_process(agent) && agent.config.lock().unwrap().is_off
    };
    if !is_stopped {
        return resume_agent_inner(session_id, state, app, Some((context, mutation))).await;
    }
    let mut roster = lock_agent_roster_for_save(&state).await?;
    let agent = roster
        .agents
        .get_mut(&session_id)
        .ok_or_else(|| format!("Agent {session_id} not found"))?;
    context.heartbeat.ensure_active("worktree assignment")?;
    mutation.apply(&mut agent.config.lock().unwrap())?;
    let _context = roster.save(context).await?;
    let _ = app.emit("agents-updated", ());
    Ok(())
}

pub(super) async fn resume_agent_inner(
    session_id: String,
    state: State<'_, AppState>,
    app: AppHandle,
    transition: Option<(WorktreeLifecycle, WorkspaceMutation)>,
) -> Result<(), String> {
    manager::log_debug(&format!(
        "[WARDIAN] resume_agent called for session: {}",
        session_id
    ));
    let (mut lifecycle_lease, mut lifecycle_heartbeat, _lifecycle_guard, workspace_change) =
        match transition {
            Some((context, mutation)) => (
                context.lease,
                context.heartbeat,
                context.guard,
                Some(mutation),
            ),
            None => {
                let lease = acquire_agent_lifecycle_transition_lease_for_session(
                    &state,
                    &session_id,
                    "resume",
                )
                .await?;
                let heartbeat = LifecycleLeaseHeartbeat::start(lease.owner().clone());
                let guard = lock_agent_lifecycle(&state, &session_id).await;
                (lease, heartbeat, guard, None)
            }
        };
    lifecycle_heartbeat.ensure_active("resume")?;
    let initial_config = lifecycle_config_for_session(&state, &session_id).await?;
    let workspace_handover = workspace_change.is_some();
    let mut workspace_reservation = if let Some(mutation) = workspace_change.as_ref() {
        let prepared = prepare_workspace_resume_config(&initial_config, mutation)?;
        {
            let agents = state.agents.lock().await;
            let agent = agents
                .get(&session_id)
                .ok_or("Agent disappeared before worktree preflight")?;
            super::workspace_stop::validate_stop_authority(agent)?;
        }
        let home = crate::utils::fs::get_wardian_home().ok_or("Unable to resolve Wardian home")?;
        Some(super::workspace_reservation::WorkspaceReservation::create(
            &home,
            &session_id,
            Path::new(&prepared.folder),
        )?)
    } else {
        None
    };
    let workspace_stop_hold = if workspace_handover {
        let agents = state.agents.lock().await;
        let agent = agents
            .get(&session_id)
            .ok_or("Agent disappeared before worktree shutdown")?;
        // The lifecycle lease has a TTL. This exact prior-provider hold persists
        // across cancellation, expiry, and restart until owned exit is confirmed.
        hold_previous_provider_before_rotation(agent, &lifecycle_lease)?
    } else {
        None
    };
    if workspace_handover {
        // Shutdown can itself be cancelled or fail without an exit receipt.
        // Retain cross-process exclusion and the durable target before stopping.
        lifecycle_lease.retain_on_drop();
        if let Some(reservation) = workspace_reservation.as_mut() {
            reservation.arm();
        }
    }
    let starts_fresh = workspace_change.is_none()
        && resolved_session_persistence(&initial_config) == AgentSessionPersistence::Fresh;
    let codex_stops_before_preflight = starts_fresh && initial_config.provider == "codex";
    if workspace_handover {
        // Dispose the native owner, but keep the TUI and its retained job in
        // the roster until the handover obtains its own runtime-tree receipt.
        state
            .native_delivery
            .dispose_agent(&session_id)
            .await
            .map_err(|error| error.to_string())?;
    } else if codex_stops_before_preflight {
        stop_native_owner_with_before_capture(&state, &session_id, false, |agent| {
            hold_previous_provider_before_rotation(agent, &lifecycle_lease).map(|_| ())
        })
        .await?;
    } else {
        stop_native_owner(&state, &session_id, false).await?;
    }
    let snapshot = {
        let agents = state.agents.lock().await;
        let agent = agents
            .get(&session_id)
            .ok_or_else(|| format!("Agent {} not found", session_id))?;
        capture_resume_runtime_snapshot(agent)
    };
    lifecycle_heartbeat.ensure_active("resume")?;

    // Detach the paused runtime's status incarnation before a new provider is
    // spawned. A late event from the old PTY must not become current while the
    // replacement is bootstrapping.
    let staged_status_arc = {
        let mut agents = state.agents.lock().await;
        let agent = agents
            .get_mut(&session_id)
            .ok_or_else(|| format!("Agent {} not found", session_id))?;
        lifecycle_heartbeat.ensure_active("resume")?;
        replace_agent_status_incarnation(agent, "Processing...")
    };
    manager::publish_agent_status(&app, &session_id, &staged_status_arc);

    let status_before_resume = snapshot.current_status.clone();
    let mut config = snapshot.config.clone();
    if let Some(mutation) = workspace_change {
        mutation.apply(&mut config)?;
    }
    let fresh_pending_boundary =
        if starts_fresh {
            crate::commands::chat::archive_agent_chat_events_until_stable_for_lifecycle(
                &state,
                &session_id,
            )
            .await
            .map_err(|error| {
                format!("Failed to acquire the closing provider log before fresh resume: {error}")
            })?;
            let snapshot =
                match crate::commands::chat::agent_archive_capture_snapshot(&state, &session_id)
                    .await
                {
                    Ok(snapshot) => snapshot,
                    Err(error) => {
                        restore_agent_status_after_failed_runtime_start(
                            &state,
                            &app,
                            &session_id,
                            &status_before_resume,
                        )
                        .await;
                        return Err(format!(
                    "Failed to capture the closing conversation before fresh resume: {error}"
                ));
                    }
                };
            let pending = match prepare_conversation_boundary(snapshot) {
                Ok(pending) => pending,
                Err(error) => {
                    restore_agent_status_after_failed_runtime_start(
                        &state,
                        &app,
                        &session_id,
                        &status_before_resume,
                    )
                    .await;
                    return Err(format!(
                        "Failed to prepare the closing conversation before fresh resume: {error}"
                    ));
                }
            };
            if let Err(error) = stage_conversation_boundary(&state, &pending) {
                restore_agent_status_after_failed_runtime_start(
                    &state,
                    &app,
                    &session_id,
                    &status_before_resume,
                )
                .await;
                return Err(format!(
                    "Failed to stage the closing conversation before fresh resume: {error}"
                ));
            }
            Some(pending)
        } else {
            None
        };
    let persistence_override = config.session_persistence;
    config.session_persistence = if starts_fresh {
        AgentSessionPersistenceOverride::Fresh
    } else {
        AgentSessionPersistenceOverride::Resume
    };
    if let Err(error) = prepare_resume_config_for_runtime(&mut config, snapshot.query_count) {
        restore_agent_status_after_failed_runtime_start(
            &state,
            &app,
            &session_id,
            &status_before_resume,
        )
        .await;
        return Err(error);
    }
    config.session_persistence = persistence_override;
    if let Err(error) = prepare_provider_owned_fresh_identity(&mut config).await {
        restore_agent_status_after_failed_runtime_start(
            &state,
            &app,
            &session_id,
            &status_before_resume,
        )
        .await;
        return Err(error);
    }
    let pending = PendingRuntime::prepare(&config, &state.terminal_sessions)?;
    lifecycle_heartbeat.ensure_active("resume")?;
    let (mut old_runtime, previous_hold_owner) = {
        let mut agents = state.agents.lock().await;
        let agent = agents
            .get_mut(&session_id)
            .ok_or_else(|| format!("Agent {} not found", session_id))?;
        let previous_hold_owner = if starts_fresh && !codex_stops_before_preflight {
            hold_previous_provider_before_rotation(agent, &lifecycle_lease)?
        } else {
            None
        };
        (
            take_agent_runtime_for_termination(agent),
            previous_hold_owner,
        )
    };
    if let Some(runtime_generation) = old_runtime.runtime_generation {
        if let Err(error) = state
            .terminal_sessions
            .terminate_and_remove_runtime(&session_id, runtime_generation)
            .await
        {
            manager::log_debug(&format!(
                "[WARDIAN] old terminal runtime already unavailable during resume {session_id}: {error}"
            ));
        }
    }
    if workspace_handover {
        if let Err(error) =
            super::workspace_stop::stop_with_receipt(&mut old_runtime, workspace_stop_hold.as_ref())
                .await
        {
            lifecycle_heartbeat.stop().await;
            restore_agent_status_after_failed_runtime_start(&state, &app, &session_id, "Error")
                .await;
            return Err(format!("Workspace replacement withheld: {error}; shutdown or exact hold cleanup was not confirmed, so conversation and target protection remain for inspection"));
        }
    } else {
        manager::terminate_rotated_agent_process(&mut old_runtime, previous_hold_owner.as_ref())
            .await?;
    }
    if workspace_handover {
        // The old writer has stopped, but its log and published configuration
        // still exist. Drain the closing turn before replacement can rotate
        // that log; preserve the existing logging policy and conversation.
        if let Err(error) =
            crate::commands::chat::archive_agent_chat_events_until_stable_for_lifecycle(
                &state,
                &session_id,
            )
            .await
        {
            lifecycle_heartbeat.stop().await;
            let release_error = lifecycle_lease.release().err();
            if release_error.is_none() {
                if let Some(reservation) = workspace_reservation.as_mut() {
                    reservation.disarm_after_confirmed_stop();
                }
            }
            restore_agent_status_after_failed_runtime_start(&state, &app, &session_id, "Error")
                .await;
            return Err(format!("Failed to retain the closing worktree conversation: {error}; fence cleanup: {release_error:?}"));
        }
    }
    let mut publication_disposition = manager::SpawnPublicationDisposition::new();
    let unpublished_failure = publication_disposition.failure_signal();
    if let Some(reservation) = workspace_reservation.as_mut() {
        reservation.arm();
    }
    let mut new_active = match spawn_replacement_under_lifecycle_lease(
        app.clone(),
        config.clone(),
        !starts_fresh,
        (!starts_fresh)
            .then_some(snapshot.init_timestamp.clone())
            .flatten(),
        ReplacementSpawnLease {
            lease: lifecycle_lease,
            unpublished_failure: unpublished_failure.clone(),
        },
        &mut lifecycle_heartbeat,
        "resume",
    )
    .await
    {
        Ok(active) => pending.attach(active),
        Err(error) => {
            restore_agent_status_after_failed_runtime_start(&state, &app, &session_id, "Error")
                .await;
            return Err(error);
        }
    };
    if let Err(error) = lifecycle_heartbeat.ensure_active("resume") {
        let error = new_active.stop_after_failure(error).await;
        let error = retain_uncertain_replacement_until_expiry(&unpublished_failure, error);
        restore_agent_status_after_failed_runtime_start(&state, &app, &session_id, "Error").await;
        return Err(error);
    }
    restore_runtime_state_after_resume(&mut new_active, &snapshot, starts_fresh);
    promote_fresh_provider_session_after_resume(&config.provider, &mut new_active);
    let new_config = new_active.config.lock().unwrap().clone();
    let new_created_at = new_active.init_timestamp.lock().unwrap().clone();
    let new_status_arc = new_active.current_status.clone();
    let mut pending_new_active = new_active;
    if let Err(error) = lifecycle_heartbeat.ensure_active("resume") {
        let error = pending_new_active.stop_after_failure(error).await;
        let error = retain_uncertain_replacement_until_expiry(&unpublished_failure, error);
        restore_agent_status_after_failed_runtime_start(&state, &app, &session_id, "Error").await;
        return Err(error);
    }
    let commit_result = {
        let mut agents = state.agents.lock().await;
        let order = state.agent_order.lock().await;
        lifecycle_heartbeat.ensure_active("resume").and_then(|_| {
            commit_resume_replacement(
                &state,
                &mut agents,
                &order,
                &session_id,
                pending_new_active.slot(),
                &snapshot.config,
                snapshot.init_timestamp.as_deref(),
                &new_config,
                new_created_at.as_deref(),
                starts_fresh,
                fresh_pending_boundary.as_ref(),
            )
        })
    };
    let (mut old_agent, session_close_context, replacement_journal) = match commit_result {
        Ok(result) => result,
        Err(error) => {
            let error = pending_new_active.stop_after_failure(error).await;
            let error = retain_uncertain_replacement_until_expiry(&unpublished_failure, error);
            restore_agent_status_after_failed_runtime_start(&state, &app, &session_id, "Error")
                .await;
            return Err(error);
        }
    };
    publication_disposition.commit();
    if let Some(reservation) = workspace_reservation.take() {
        reservation.committed();
    }
    manager::publish_agent_status(&app, &session_id, &new_status_arc);

    manager::terminate_active_agent_process(&mut old_agent);

    let _ = app.emit(
        "agent-terminal-cleared",
        terminal_cleared_payload(&session_id),
    );
    let _ = app.emit("agents-updated", ());
    let _ = app.emit(
        "agent-pty-output-ready",
        terminal_cleared_payload(&session_id),
    );
    if let Some(context) = session_close_context {
        if let Err(error) =
            crate::automation::session_close::invoke_matching(app.clone(), context).await
        {
            return Err(format!(
                "The agent resumed, but its session-close automation was not durably accepted: {error}"
            ));
        }
    }
    if let Err(error) = replacement_journal.complete() {
        manager::log_debug(&format!(
            "[WARDIAN] resume journal cleanup deferred for {session_id}: {error}"
        ));
    }
    Ok(())
}
