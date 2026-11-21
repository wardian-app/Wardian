//! Delete real persisted identities without manufacturing live runtime/config state.
use super::*;
use wardian_core::db::{self, AgentRow};

#[derive(Clone)]
struct Ownership {
    admission: RemovalAdmission,
    row: AgentRow,
    saved: Option<AgentConfig>,
    profile: wardian_core::agent_spawn::RemovalProfileIdentity,
    home: std::path::PathBuf,
}

async fn lookup(id: &str) -> Result<Option<AgentRow>, String> {
    let id = id.to_string();
    tokio::task::spawn_blocking(move || {
        db::get_db_conn(|conn| Ok(db::get_agent_by_session_id_with_conn(conn, &id)?))
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

pub(super) async fn remove<R: tauri::Runtime>(
    id: String,
    expected_name: Option<&str>,
    app: AppHandle<R>,
    require_stopped: bool,
) -> Result<(), String> {
    let ticket_id = id.clone();
    let ticket = tokio::task::spawn_blocking(move || {
        wardian_core::agent_spawn::bound_removal_ticket(&ticket_id)
    })
    .await
    .map_err(|error| error.to_string())??;
    // A persisted row cannot weaken an existing exact, sealed-ticket protocol.
    if ticket.is_some() {
        return bound::retry(id, expected_name, app).await;
    }
    let Some(row) = lookup(&id).await? else {
        return bound::retry(id, expected_name, app).await;
    };
    validate_confirmation(&row, expected_name, require_stopped)?;
    let home = crate::utils::fs::get_wardian_home().ok_or("Could not locate Wardian home")?;
    let saved = saved_target(&bound::roster(&home)?, &id)?;
    if saved
        .as_ref()
        .is_some_and(|config| config.session_name != row.session_name)
    {
        return Err(
            "Persisted removal confirmation does not match the saved agent identity".into(),
        );
    }
    let provider = saved
        .as_ref()
        .map(|config| config.provider.as_str())
        .or(row.provider.as_deref())
        .unwrap_or("claude");
    let resume = saved
        .as_ref()
        .and_then(|config| config.resume_session.as_deref())
        .unwrap_or_default();
    let lease = super::super::acquire_agent_lifecycle_transition_lease_for_identity(
        &id,
        &row.session_name,
        provider,
        resume,
        "remove persisted agent",
    )?;
    let heartbeat = LifecycleLeaseHeartbeat::start(lease.owner().clone());
    let state = app.state::<AppState>();
    let guard = lock_agent_lifecycle(&state, &id).await;
    let barrier = manager::roster_io::acquire_roster_barrier().await?;
    let profile = bound::profile(&home, &id)?;
    let ownership = Ownership {
        admission: RemovalAdmission {
            authority: Arc::new(RemovalAuthority {
                _lifecycle_lease: lease,
                _lifecycle_guard: guard,
                lifecycle_heartbeat: heartbeat,
            }),
            barrier: Arc::new(barrier),
        },
        row,
        saved,
        profile,
        home,
    };
    validate_absence(&ownership, &app).await?;
    if lookup(&id).await?.as_ref() != Some(&ownership.row) {
        return Err("Persisted agent identity changed before removal".into());
    }
    // Keep all authority through physical I/O even if the caller disappears.
    #[cfg(test)]
    let roster_probe = manager::ROSTER_IO_PROBE
        .try_with(|probe| probe.borrow_mut().take())
        .ok()
        .flatten();
    #[cfg(test)]
    let delete_probe = crate::state::interactions::DELETE_IO_PROBE
        .try_with(|probe| probe.borrow_mut().take())
        .ok()
        .flatten();
    let operation = finish(ownership, app);
    #[cfg(test)]
    let operation =
        manager::ROSTER_IO_PROBE.scope(std::cell::RefCell::new(roster_probe), operation);
    #[cfg(test)]
    let operation = crate::state::interactions::DELETE_IO_PROBE
        .scope(std::cell::RefCell::new(delete_probe), operation);
    tokio::spawn(operation)
        .await
        .map_err(|error| format!("Persisted removal continuation failed: {error}"))?
}

fn validate_confirmation(
    row: &AgentRow,
    expected_name: Option<&str>,
    require_stopped: bool,
) -> Result<(), String> {
    if expected_name.is_some_and(|name| name != row.session_name) {
        return Err(format!(
            "delete confirmation must exactly match the agent name `{}`",
            row.session_name
        ));
    }
    // A historical PID is not an owned process handle, even with --force.
    if row.last_pid.is_some() {
        return Err("Cannot remove an unloaded agent with an unverified recorded process; no owned runtime is available for termination".into());
    }
    if require_stopped
        && !row.is_off
        && !row.last_status.as_deref().is_some_and(|status| {
            status.eq_ignore_ascii_case("off") || status.eq_ignore_ascii_case("error")
        })
    {
        return Err("Cannot delete an unloaded agent whose persisted status is active; verify provider ownership before using --force".into());
    }
    Ok(())
}

fn saved_target(configs: &[AgentConfig], id: &str) -> Result<Option<AgentConfig>, String> {
    let mut matching = configs.iter().filter(|config| config.session_id == id);
    let target = matching.next().cloned();
    if matching.next().is_some() {
        return Err("Persisted removal has ambiguous saved configurations".into());
    }
    Ok(target)
}

async fn validate_absence<R: tauri::Runtime>(
    owner: &Ownership,
    app: &AppHandle<R>,
) -> Result<(), String> {
    owner
        .admission
        .authority
        .lifecycle_heartbeat
        .ensure_active("remove persisted agent")?;
    let state = app.state::<AppState>();
    let id = &owner.row.session_id;
    if state.agents.lock().await.contains_key(id) {
        return Err("Persisted removal has a replacement live agent".into());
    }
    match state.terminal_sessions.broker_state(id).await {
        Err(crate::state::terminal_session::TerminalBrokerError::SessionNotFound) => {}
        Ok(_) => return Err("Persisted removal has an unverified terminal owner".into()),
        Err(error) => return Err(format!("Could not prove terminal-owner absence: {error}")),
    }
    crate::manager::codex_stop::await_quiescent(&owner.home, id).await?;
    if state.native_delivery.has_agent_owner(id).await? {
        return Err("Persisted removal has an unverified native owner or preparation".into());
    }
    let leases = wardian_core::conversation_lease::load_leases_checked()?;
    if leases
        .iter()
        .any(|lease| lease.agent_id == *id && lease.mode == "uncertain_previous_provider")
    {
        return Err("Persisted removal retains an uncertain previous-provider hold; verify exit before removal".into());
    }
    let lookup_id = id.clone();
    if tokio::task::spawn_blocking(move || {
        wardian_core::agent_spawn::restore_is_excluded(&lookup_id)
    })
    .await
    .map_err(|error| error.to_string())??
    {
        return Err("Cannot remove an unloaded agent with unresolved spawn ownership".into());
    }
    if bound::profile(&owner.home, id)? != owner.profile {
        return Err("Persisted removal profile ownership changed".into());
    }
    owner
        .admission
        .authority
        .lifecycle_heartbeat
        .ensure_active("remove persisted metadata")
}

async fn write_roster<R: tauri::Runtime>(
    owner: &Ownership,
    app: &AppHandle<R>,
    compensate: bool,
) -> Result<(), String> {
    let owner = owner.clone();
    let app = app.clone();
    #[cfg(test)]
    let probe = manager::ROSTER_IO_PROBE
        .try_with(|probe| probe.borrow_mut().take())
        .ok()
        .flatten();
    let operation = move || {
        manager::try_save_state_snapshot_captured_for_home(
            &owner.home,
            Some(&owner.row.session_id),
            || {
                tauri::async_runtime::block_on(validate_absence(&owner, &app))?;
                let mut configs = bound::roster(&owner.home)?;
                let current = saved_target(&configs, &owner.row.session_id)?;
                if compensate {
                    if current.is_none()
                        && lookup_sync(&owner.row.session_id)?.as_ref() == Some(&owner.row)
                    {
                        if let Some(saved) = &owner.saved {
                            configs.push(saved.clone());
                        }
                    }
                } else {
                    if serde_json::to_value(&current).map_err(|error| error.to_string())?
                        != serde_json::to_value(&owner.saved).map_err(|error| error.to_string())?
                    {
                        return Err("Persisted removal saved identity changed".into());
                    }
                    configs.retain(|config| config.session_id != owner.row.session_id);
                }
                Ok(configs)
            },
            || {
                owner
                    .admission
                    .authority
                    .lifecycle_heartbeat
                    .ensure_active("write persisted removal roster")
            },
        )
    };
    #[cfg(test)]
    let operation =
        move || manager::ROSTER_IO_PROBE.sync_scope(std::cell::RefCell::new(probe), operation);
    tokio::task::spawn_blocking(operation)
        .await
        .map_err(|error| error.to_string())?
}

fn lookup_sync(id: &str) -> Result<Option<AgentRow>, String> {
    db::get_db_conn(|conn| Ok(db::get_agent_by_session_id_with_conn(conn, id)?))
        .map_err(|error| error.to_string())
}

async fn finish<R: tauri::Runtime>(owner: Ownership, app: AppHandle<R>) -> Result<(), String> {
    let id = owner.row.session_id.clone();
    write_roster(&owner, &app, false).await?;
    let state = app.state::<AppState>();
    let delete_owner = owner.clone();
    let delete_app = app.clone();
    let result = state
        .interactions
        .delete_agent_durable_state_fenced(&id, move || {
            tauri::async_runtime::block_on(validate_absence(&delete_owner, &delete_app))?;
            if bound::roster(&delete_owner.home)?
                .iter()
                .any(|config| config.session_id == delete_owner.row.session_id)
            {
                return Err("Persisted removal roster no longer proves absence".into());
            }
            db::delete_unloaded_agent_fenced(&delete_owner.row, || {
                delete_owner
                    .admission
                    .authority
                    .lifecycle_heartbeat
                    .ensure_active("delete persisted agent state")
            })
            .map_err(|error| format!("Failed to delete persisted agent state: {error}"))
        })
        .await;
    if let Err(error) = result {
        return Err(match write_roster(&owner, &app, true).await {
            Ok(()) => error,
            Err(restore) => format!("{error}; persisted roster compensation failed: {restore}"),
        });
    }
    state.remove_agent_delivery_state(&id).await;
    state.terminal_sessions.forget_deferred_geometry(&id).await;
    state.agent_order.lock().await.retain(|agent| agent != &id);
    // Persisted peers are real identities even when they have no live runtime.
    let mut remaining = db::get_all_agents()
        .map_err(|error| error.to_string())?
        .into_iter()
        .map(|row| row.session_id)
        .collect::<BTreeSet<_>>();
    remaining.extend(state.agents.lock().await.keys().cloned());
    let workspace = owner
        .saved
        .as_ref()
        .map(|config| config.folder.as_str())
        .or(owner.row.workspace.as_deref())
        .unwrap_or_default()
        .to_string();
    let _ = app.emit("agents-updated", ());
    drop(owner.admission.barrier);
    let authority = owner.admission.authority;
    if authority
        .lifecycle_heartbeat
        .ensure_active("clean persisted agent profile")
        .is_ok()
    {
        cleanup_committed_removal(
            &app,
            &id,
            &owner.home,
            &workspace,
            &remaining,
            Some(&owner.profile),
        )
        .await;
    }
    Ok(())
}
