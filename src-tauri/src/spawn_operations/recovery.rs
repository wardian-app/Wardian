use super::persisted_roster;
use crate::commands::agent::{self, PreparedAgentSpawn, RecoveryBootstrap};
use crate::state::AppState;
use std::sync::atomic::Ordering;
use wardian_core::agent_spawn::recovery::{
    self, Admission, Blocker, Finalized, LaunchAccepted, RegistrationEvidence, Request,
};
use wardian_core::models::{AgentConfig, TerminalRuntimeState};

#[cfg(test)]
mod tests;

/// Admit through the current app owner so the exact registered incarnation
/// stays pinned through the metadata commit.
pub(crate) async fn recover(state: &AppState, request: Request) -> Result<Finalized, Blocker> {
    let expected = request.clone();
    let admission = tokio::task::spawn_blocking(move || recovery::admit_recovery(&expected))
        .await
        .map_err(|_| evidence_unavailable())??;
    let plan = match admission {
        Admission::Committed(receipt) => return Ok(receipt),
        Admission::NeedsLiveProof(plan) => plan,
    };
    let agent_id = &plan.operation.agent_id;
    let native_id = plan
        .provider_session_id
        .as_deref()
        .ok_or_else(native_unverified)?;
    let lifecycle = state.lock_agent_lifecycle(agent_id).await;
    let expected = request.clone();
    if let Some(receipt) =
        tokio::task::spawn_blocking(move || recovery::committed_receipt(&expected))
            .await
            .map_err(|_| evidence_unavailable())??
    {
        return Ok(receipt);
    }
    if !state.agent_roster_restored.load(Ordering::Acquire) {
        return Err(Blocker::new(
            "spawn_recovery_roster_unavailable",
            "The current app has not completed roster restoration.",
        ));
    }
    let roster = crate::manager::roster_io::acquire_roster_barrier()
        .await
        .map_err(|_| evidence_unavailable())?;
    let checked_agent_id = agent_id.clone();
    let (roster, saved) = tokio::task::spawn_blocking(move || {
        let pending = roster
            .has_pending_replacement_for(&checked_agent_id)
            .map_err(|_| evidence_unavailable())?;
        if pending {
            return Err(Blocker::new(
                "spawn_recovery_replacement_pending",
                "A replacement journal still owns this agent.",
            ));
        }
        let saved = persisted_roster().map_err(|_| saved_unverified())?;
        Ok((roster, saved))
    })
    .await
    .map_err(|_| evidence_unavailable())??;
    let (config, runtime) = live_registration(state, agent_id, native_id).await?;
    if config.model.as_deref() != Some(&request.expected_model) {
        return Err(Blocker::new(
            "spawn_recovery_model_mismatch",
            "The live configuration does not have the expected model.",
        ));
    }
    verify_saved_registration(&saved, &config, native_id)?;
    let terminal = state
        .terminal_sessions
        .broker_state(agent_id)
        .await
        .map_err(|_| native_unverified())?;
    if terminal.session_id != *agent_id
        || terminal.runtime_generation != runtime
        || terminal.runtime_state != TerminalRuntimeState::Live
    {
        return Err(native_unverified());
    }
    let (owner, client, provider_generation) = state
        .native_delivery
        .lock_codex_registration_for_recovery(agent_id, runtime, native_id, &request.expected_model)
        .await
        .ok_or_else(native_unverified)?;
    let (current, current_runtime) = live_registration(state, agent_id, native_id).await?;
    if serialized_config(&current)? != serialized_config(&config)? || current_runtime != runtime {
        return Err(native_unverified());
    }
    let native_id = native_id.to_owned();
    tokio::task::spawn_blocking(move || {
        let (_lifecycle, _roster, _owner) = (lifecycle, roster, owner);
        recovery::finalize_registered(
            &request,
            &plan,
            &RegistrationEvidence {
                config,
                terminal_generation: runtime,
                provider_generation,
            },
            || client.recovery_thread_is_idle(&native_id),
        )
    })
    .await
    .map_err(|_| evidence_unavailable())?
}

/// Authorize only a vacant witnessed identity. The worker inherits the same
/// lease and both local barriers through its first materialization.
pub(super) async fn admit_launch(
    state: &AppState,
    request: Request,
) -> Result<(LaunchAccepted, PreparedAgentSpawn, RecoveryBootstrap), Blocker> {
    let expected = request.clone();
    let plan = tokio::task::spawn_blocking(move || recovery::plan_launch(&expected))
        .await
        .map_err(|_| evidence_unavailable())??;
    let agent_id = &plan.plan.operation.agent_id;
    let name = &plan.plan.operation.agent_name;
    if !state.agent_roster_restored.load(Ordering::Acquire) {
        return Err(Blocker::new(
            "spawn_recovery_roster_unavailable",
            "The current app has not completed roster restoration.",
        ));
    }
    let lifecycle = state.lock_agent_lifecycle(agent_id).await;
    let roster = crate::manager::roster_io::acquire_roster_barrier()
        .await
        .map_err(|_| evidence_unavailable())?;
    {
        let agents = state.agents.lock().await;
        if agents.contains_key(agent_id) || state.agent_order.lock().await.contains(agent_id) {
            return Err(agent_occupied());
        }
        for other in agents.values() {
            let config = other.config.lock().map_err(|_| evidence_unavailable())?;
            if config.session_id == *agent_id || config.session_name.eq_ignore_ascii_case(name) {
                return Err(agent_occupied());
            }
        }
    }
    if state
        .native_delivery
        .has_agent_owner(agent_id)
        .await
        .map_err(|_| evidence_unavailable())?
    {
        return Err(agent_occupied());
    }
    match state.terminal_sessions.broker_state(agent_id).await {
        Err(crate::state::terminal_session::TerminalBrokerError::SessionNotFound) => {}
        Ok(_) => return Err(agent_occupied()),
        Err(_) => return Err(evidence_unavailable()),
    }
    let (reply, prepared, lease, roster, lifecycle) = tokio::task::spawn_blocking(move || {
        let operation = &plan.plan.operation;
        let agent_id = &operation.agent_id;
        if roster
            .has_pending_replacement_for(agent_id)
            .map_err(|_| evidence_unavailable())?
        {
            return Err(Blocker::new(
                "spawn_recovery_replacement_pending",
                "A replacement journal still owns this agent.",
            ));
        }
        let saved = persisted_roster().map_err(|_| saved_unverified())?;
        if saved.iter().any(|config| {
            config.session_id == *agent_id
                || config
                    .session_name
                    .eq_ignore_ascii_case(&operation.agent_name)
        }) {
            return Err(agent_occupied());
        }
        let home = crate::utils::fs::get_wardian_home().ok_or_else(evidence_unavailable)?;
        match std::fs::symlink_metadata(home.join("agents").join(agent_id)) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Ok(_) => return Err(agent_occupied()),
            Err(_) => return Err(evidence_unavailable()),
        }
        let lease = agent::acquire_agent_lifecycle_transition_lease_for_identity(
            agent_id,
            &operation.agent_name,
            "codex",
            "",
            "spawn-recovery",
        )
        .map_err(|_| {
            Blocker::new(
                "spawn_recovery_lease_conflict",
                "The vacant-agent conversation lease could not be acquired.",
            )
        })?;
        if !crate::utils::process::find_wardian_provider_process_candidates(
            agent_id,
            "codex",
            Some(std::process::id()),
        )
        .is_empty()
        {
            return Err(agent_occupied());
        }
        let prepared =
            agent::prepare_recovery_launch(&plan, &request.expected_model).map_err(|_| {
                Blocker::new(
                    "spawn_recovery_preparation_changed",
                    "The captured Codex launch inputs could not be verified.",
                )
            })?;
        let reply = recovery::claim_recovery_launch(&request, &plan, &lease)?;
        Ok((reply, prepared, lease, roster, lifecycle))
    })
    .await
    .map_err(|_| evidence_unavailable())??;
    let heartbeat = std::sync::Arc::new(agent::LifecycleLeaseHeartbeat::start_codex_recovery(
        lease.owner().clone(),
    ));
    Ok((
        reply,
        prepared,
        RecoveryBootstrap {
            lease,
            roster,
            lifecycle,
            heartbeat,
        },
    ))
}

fn agent_occupied() -> Blocker {
    Blocker::new(
        "spawn_recovery_agent_occupied",
        "The planned agent has existing roster, runtime, process, or profile evidence.",
    )
}

async fn live_registration(
    state: &AppState,
    agent_id: &str,
    native_id: &str,
) -> Result<(AgentConfig, u64), Blocker> {
    let mut agents = state.agents.lock().await;
    for (_, other) in agents.iter().filter(|(id, _)| id.as_str() != agent_id) {
        let config = other.config.lock().map_err(|_| evidence_unavailable())?;
        if config.session_id == agent_id || owns_native(&config, native_id) {
            return Err(Blocker::new(
                "spawn_recovery_competing_registration",
                "Another live agent has this identity or Codex session.",
            ));
        }
    }
    let agent = agents.get_mut(agent_id).ok_or_else(native_unverified)?;
    if state
        .agent_order
        .lock()
        .await
        .iter()
        .filter(|id| id.as_str() == agent_id)
        .count()
        != 1
    {
        return Err(native_unverified());
    }
    let config = agent
        .config
        .lock()
        .map_err(|_| evidence_unavailable())?
        .clone();
    let idle = agent
        .current_status
        .lock()
        .map_err(|_| evidence_unavailable())?
        .as_str()
        == "Idle";
    let ready = agent
        .watch_state
        .lock()
        .map_err(|_| evidence_unavailable())?
        .codex_attachment_ready();
    let generation = agent.runtime_generation.filter(|value| *value != 0);
    let child_alive = agent
        .child_process
        .as_mut()
        .is_some_and(|child| matches!(child.try_wait(), Ok(None)));
    if !idle || !ready || !child_alive || !owns_native(&config, native_id) {
        return Err(native_unverified());
    }
    Ok((config, generation.ok_or_else(native_unverified)?))
}

fn verify_saved_registration(
    saved: &[AgentConfig],
    config: &AgentConfig,
    native_id: &str,
) -> Result<(), Blocker> {
    let mut matches = saved
        .iter()
        .filter(|entry| entry.session_id == config.session_id);
    let exact = matches.next().ok_or_else(saved_unverified)?;
    if matches.next().is_some()
        || serialized_config(exact)? != serialized_config(config)?
        || saved.iter().any(|entry| {
            entry.session_id != config.session_id
                && (entry
                    .session_name
                    .eq_ignore_ascii_case(&config.session_name)
                    || owns_native(entry, native_id))
        })
    {
        return Err(saved_unverified());
    }
    Ok(())
}

fn owns_native(config: &AgentConfig, session: &str) -> bool {
    config.provider == "codex"
        && (config.resume_session.as_deref() == Some(session)
            || config.fresh_provider_session_id.as_deref() == Some(session))
}

fn serialized_config(config: &AgentConfig) -> Result<serde_json::Value, Blocker> {
    serde_json::to_value(config).map_err(|_| evidence_unavailable())
}

fn saved_unverified() -> Blocker {
    Blocker::new(
        "spawn_recovery_saved_registration_unverified",
        "The saved roster does not prove the same unique registration.",
    )
}

fn native_unverified() -> Blocker {
    Blocker::new(
        "spawn_recovery_native_unverified",
        "The exact live, idle Codex TUI and native owner could not be verified.",
    )
}

fn evidence_unavailable() -> Blocker {
    Blocker::new(
        "spawn_recovery_evidence_unavailable",
        "Registration evidence is unavailable.",
    )
}
