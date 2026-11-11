//! Pending legacy deletion uses its existing exact attempt, never name adoption.

use super::*;
use wardian_core::agent_spawn::{
    self as ledger, BoundRemovalTicket, RemovalAgentIdentity, RemovalClosureReceipt,
    RemovalProfileIdentity,
};

pub(super) fn pending(ticket: &BoundRemovalTicket, error: impl std::fmt::Display) -> String {
    format!("[agent-name-ownership-pending] Agent {} roster removal committed with durable ownership pending (attempt {}): {error}",
        ticket.agent_id, ticket.attempt_id)
}

pub(super) fn roster(home: &Path) -> Result<Vec<AgentConfig>, String> {
    match std::fs::read(home.join("settings/state.json")) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map_err(|_| "Could not decode actual removal roster".into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(_) => Err("Could not read actual removal roster".into()),
    }
}

pub(super) fn profile(home: &Path, agent: &str) -> Result<RemovalProfileIdentity, String> {
    let mut components = Path::new(agent).components();
    if !matches!(components.next(), Some(std::path::Component::Normal(_)))
        || components.next().is_some()
    {
        return Err("Removal profile is not a single agent identity".into());
    }
    let directory = home.join("agents").join(agent);
    match std::fs::symlink_metadata(&directory) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(RemovalProfileIdentity {
                directory_id: None,
                ownership_records: std::collections::BTreeMap::new(),
            })
        }
        Err(_) => return Err("Could not inspect removal profile".into()),
        Ok(_) => {}
    }
    let directory_id = crate::utils::codex_home::directory_identity(&directory)?;
    let mut ownership_records = std::collections::BTreeMap::new();
    for name in [
        ".wardian-codex-home.json",
        ".wardian-codex-home-cleanup.json",
        crate::utils::codex_home::HABITAT_ALIAS_RECORD,
    ] {
        ownership_records.insert(
            name.into(),
            crate::utils::codex_home::ownership_record_digest(&directory.join(name))?,
        );
    }
    Ok(RemovalProfileIdentity {
        directory_id: Some(directory_id),
        ownership_records,
    })
}

pub(super) async fn admit(
    ownership: &AgentRemovalOwnership,
    config: &AgentConfig,
    generation: Option<u64>,
) -> Result<Option<BoundRemovalTicket>, String> {
    let home = ownership.home.clone();
    let admission = ownership.admission.clone();
    let identity = RemovalAgentIdentity::from(config);
    tokio::task::spawn_blocking(move || {
        let _barrier = admission.barrier;
        admission
            .authority
            .lifecycle_heartbeat
            .ensure_active("classify removal")?;
        if !ledger::requires_bound_removal(&identity.agent_id)? {
            return Ok(None);
        }
        let prior = ledger::bound_removal_ticket(&identity.agent_id)?;
        let saved = roster(&home)?;
        let matches = saved
            .iter()
            .filter(|config| config.session_id == identity.agent_id)
            .collect::<Vec<_>>();
        if matches.len() > 1
            || matches
                .first()
                .is_some_and(|config| RemovalAgentIdentity::from(*config) != identity)
            || (matches.is_empty()
                && prior.as_ref().is_none_or(|ticket| {
                    ticket.state != "intent" || ticket.identity() != &identity
                }))
        {
            return Err("Bound removal actual roster identity is missing or incompatible".into());
        }
        let _preparation =
            crate::utils::codex_home::acquire_preparation(&home, &identity.agent_id)?;
        let profile = profile(&home, &identity.agent_id)?;
        ledger::admit_bound_removal(&identity, generation, &profile, || {
            admission
                .authority
                .lifecycle_heartbeat
                .ensure_active("admit bound removal")
        })?
        .map(Some)
        .ok_or_else(|| "Removal classification changed before admission".into())
    })
    .await
    .map_err(|error| error.to_string())?
}

pub(super) fn seal(
    ticket: &BoundRemovalTicket,
    receipt: &RemovalClosureReceipt,
    home: &Path,
    pre_sql: impl FnOnce() -> Result<(), String>,
    sql_lease: impl FnOnce() -> Result<(), String>,
) -> Result<BoundRemovalTicket, String> {
    let _preparation = crate::utils::codex_home::acquire_preparation(home, &ticket.agent_id)?;
    let profile = profile(home, &ticket.agent_id)?;
    if roster(home)?
        .iter()
        .any(|config| config.session_id == ticket.agent_id)
    {
        return Err("Bound closure roster contains a replacement".into());
    }
    pre_sql()?;
    ledger::seal_bound_removal(ticket, receipt, &profile, sql_lease)
}

pub(super) fn complete(
    ticket: &BoundRemovalTicket,
    home: &Path,
    pre_sql: impl FnOnce() -> Result<(), String>,
    sql_lease: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    let _preparation = crate::utils::codex_home::acquire_preparation(home, &ticket.agent_id)?;
    let profile = profile(home, &ticket.agent_id)?;
    let saved = roster(home)?;
    pre_sql()?;
    ledger::complete_bound_removal(ticket, &saved, &profile, sql_lease)
}

pub(super) async fn validate_no_replacement<R: tauri::Runtime>(
    admission: &RemovalAdmission,
    app: &AppHandle<R>,
    ticket: &BoundRemovalTicket,
) -> Result<(), String> {
    admission
        .authority
        .lifecycle_heartbeat
        .ensure_active("retry bound removal")?;
    let state = app.state::<AppState>();
    if state.agents.lock().await.contains_key(&ticket.agent_id) {
        return Err(format!(
            "Agent {} incarnation changed during removal",
            ticket.agent_id
        ));
    }
    match state.terminal_sessions.broker_state(&ticket.agent_id).await {
        Err(crate::state::terminal_session::TerminalBrokerError::SessionNotFound) => {}
        Ok(_) => return Err("Bound removal has a replacement terminal owner".into()),
        Err(error) => return Err(format!("Could not prove terminal-owner absence: {error}")),
    }
    admission
        .authority
        .lifecycle_heartbeat
        .ensure_active("delete bound metadata")
}

/// Lost replies retry only the sealed metadata disposition, using new existing
/// lifecycle ownership. This path never creates a config or starts a provider.
pub(super) async fn retry<R: tauri::Runtime>(
    agent_id: String,
    expected_name: Option<&str>,
    app: AppHandle<R>,
) -> Result<(), String> {
    let lookup_id = agent_id.clone();
    let ticket = tokio::task::spawn_blocking(move || ledger::bound_removal_ticket(&lookup_id))
        .await
        .map_err(|error| error.to_string())??
        .ok_or_else(|| format!("Agent {agent_id} not found"))?;
    if expected_name.is_some_and(|name| name != ticket.identity().name) {
        return Err("Bound removal confirmation name does not match".into());
    }
    if !matches!(ticket.state.as_str(), "closure_proved" | "done") {
        return Err(pending(
            &ticket,
            "Owned closure remains unproved; metadata retry is not authorized",
        ));
    }
    let identity = ticket.identity();
    let lease = super::super::acquire_agent_lifecycle_transition_lease_for_identity(
        &agent_id,
        &identity.name,
        &identity.provider,
        identity.resume_session.as_deref().unwrap_or_default(),
        "retry removal",
    )?;
    let lifecycle_heartbeat = LifecycleLeaseHeartbeat::start(lease.owner().clone());
    let state = app.state::<AppState>();
    let guard = lock_agent_lifecycle(&state, &agent_id).await;
    let barrier = manager::roster_io::acquire_roster_barrier().await?;
    let home = crate::utils::fs::get_wardian_home().ok_or("Could not locate Wardian home")?;
    let admission = RemovalAdmission {
        authority: Arc::new(RemovalAuthority {
            _lifecycle_lease: lease,
            _lifecycle_guard: guard,
            lifecycle_heartbeat,
        }),
        barrier: Arc::new(barrier),
    };
    validate_no_replacement(&admission, &app, &ticket).await?;
    tokio::spawn(async move {
        let state = app.state::<AppState>();
        let delete_app = app.clone();
        let delete_admission = admission.clone();
        let delete_ticket = ticket.clone();
        let delete_home = home.clone();
        state
            .interactions
            .delete_agent_durable_state_fenced(&agent_id, move || {
                complete(
                    &delete_ticket,
                    &delete_home,
                    || {
                        tauri::async_runtime::block_on(validate_no_replacement(
                            &delete_admission,
                            &delete_app,
                            &delete_ticket,
                        ))
                    },
                    || {
                        delete_admission
                            .authority
                            .lifecycle_heartbeat
                            .ensure_active("retry bound SQL")
                    },
                )
            })
            .await
            .map_err(|error| pending(&ticket, error))?;
        validate_no_replacement(&admission, &app, &ticket).await?;
        state.remove_agent_delivery_state(&agent_id).await;
        state
            .terminal_sessions
            .forget_deferred_geometry(&agent_id)
            .await;
        let remaining = state
            .agents
            .lock()
            .await
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>();
        let _ = app.emit("agents-updated", ());
        drop(admission.barrier);
        let authority = admission.authority;
        if authority
            .lifecycle_heartbeat
            .ensure_active("clean bound retry")
            .is_ok()
        {
            super::cleanup_committed_removal(
                &app,
                &agent_id,
                &home,
                &ticket.identity().workspace,
                &remaining,
                Some(ticket.profile()),
            )
            .await;
        }
        Ok(())
    })
    .await
    .map_err(|error| format!("Bound removal continuation failed: {error}"))?
}

/// Called only after winning exclusive-home ownership and before any admission
/// or restoration. The new owner uses sealed proof, never an expired old lease.
pub(crate) fn recover_bound_removals() -> Result<(), String> {
    let home = crate::utils::fs::get_wardian_home().ok_or("Could not locate Wardian home")?;
    let _barrier = wardian_core::agent_replacement::acquire_agent_roster_barrier(false)
        .map_err(|error| error.to_string())?
        .ok_or("Bound deletion recovery roster is busy")?;
    for ticket in ledger::pending_bound_removals()? {
        if ticket.state != "closure_proved" {
            continue;
        }
        match complete(&ticket, &home, || Ok(()), || Ok(())) {
            Ok(()) => {
                // The durable done receipt remains even if physical cleanup is
                // busy or detects a changed profile. No roster is recreated.
                if let Err(error) = super::cleanup_removed_agent_directory_with_profile(
                    &home,
                    &ticket.agent_id,
                    Ok(()),
                    Some(ticket.profile()),
                ) {
                    manager::log_debug(&format!(
                        "[WARDIAN] Bound recovery retained profile: {error}"
                    ));
                }
            }
            Err(error) => manager::log_debug(&pending(&ticket, error)),
        }
    }
    Ok(())
}
