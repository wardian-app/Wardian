//! Owned durable removal, joined process shutdown and private-home cleanup.
//!
//! Removal joins the native owner before deleting durable state. If later
//! persistence fails, metadata remains but the owner is stopped; never
//! automatically restart or replay its work.
use super::{
    acquire_agent_lifecycle_transition_lease_for_session,
    agent_lifecycle::{lock_agent_lifecycle, stop_native_owner},
    clone_remove_existing_path, detach_agent_for_kill, mark_agent_paused_off,
    take_agent_runtime_for_termination, validate_agent_removal, DeletedAgentReferenceCleanup,
    LifecycleLeaseHeartbeat,
};
use crate::{
    manager,
    state::{ActiveAgent, AppState},
};
use std::{
    collections::{BTreeSet, HashMap},
    path::Path,
    sync::Arc,
    time::Duration,
};
use tauri::{AppHandle, Emitter, Manager, State};
use wardian_core::models::AgentConfig;

#[path = "removal_processes.rs"]
mod processes;
use processes::RemovalProcesses;
#[path = "bound_removal.rs"]
mod bound;
pub(crate) use bound::recover_bound_removals;

#[cfg(test)]
#[path = "bound_removal_tests.rs"]
mod bound_tests;
#[cfg(test)]
#[path = "removal_tests.rs"]
mod tests;

/// Retains lifecycle and durable exclusion through publication even if the
/// requesting command is cancelled during filesystem or database completion.
pub(super) async fn remove_agent<R: tauri::Runtime>(
    session_id: String,
    expected_name: Option<&str>,
    state: State<'_, AppState>,
    app: AppHandle<R>,
    require_stopped: bool,
) -> Result<(), String> {
    manager::log_debug(&format!(
        "[WARDIAN] {} agent for session: {}",
        if require_stopped { "delete" } else { "kill" },
        session_id,
    ));
    let target_config = {
        let agents = state.agents.lock().await;
        agents.get(&session_id).map(|agent| agent.config.clone())
    };
    let Some(target_config) = target_config else {
        return bound::retry(session_id, expected_name, app).await;
    };
    let _lifecycle_lease =
        acquire_agent_lifecycle_transition_lease_for_session(&state, &session_id, "remove").await?;
    let lifecycle_heartbeat = LifecycleLeaseHeartbeat::start(_lifecycle_lease.owner().clone());
    let _lifecycle_guard = lock_agent_lifecycle(&state, &session_id).await;
    lifecycle_heartbeat.ensure_active("remove")?;
    {
        let agents = state.agents.lock().await;
        let agent = validate_removal_incarnation(&agents, &session_id, &target_config)?;
        validate_agent_removal(agent, expected_name, require_stopped)?;
    }
    stop_native_owner(&state, &session_id, true).await?;
    let barrier = manager::roster_io::acquire_roster_barrier().await?;
    let home = crate::utils::fs::get_wardian_home().ok_or("Could not locate Wardian home")?;
    let expected_name = expected_name.map(str::to_string);
    #[cfg(test)]
    let probe = manager::ROSTER_IO_PROBE
        .try_with(|probe| probe.borrow_mut().take())
        .ok()
        .flatten();
    #[cfg(test)]
    let delete_probe = crate::state::interactions::DELETE_IO_PROBE
        .try_with(|probe| probe.borrow_mut().take())
        .ok()
        .flatten();
    let ownership = AgentRemovalOwnership {
        admission: RemovalAdmission {
            authority: Arc::new(RemovalAuthority {
                _lifecycle_lease,
                _lifecycle_guard,
                lifecycle_heartbeat,
            }),
            barrier: Arc::new(barrier),
        },
        target_config,
        home,
    };
    // Dropping the requesting command's JoinHandle cannot drop admission or
    // lifecycle authority during a physical write, DB deletion or publication.
    let operation =
        finish_agent_removal(session_id, expected_name, app, require_stopped, ownership);
    #[cfg(test)]
    let operation = crate::state::interactions::DELETE_IO_PROBE
        .scope(std::cell::RefCell::new(delete_probe), operation);
    #[cfg(test)]
    let operation = manager::ROSTER_IO_PROBE.scope(std::cell::RefCell::new(probe), operation);
    tokio::spawn(operation)
        .await
        .map_err(|error| format!("Agent removal continuation failed: {error}"))?
}

struct RemovalAuthority {
    _lifecycle_lease: wardian_core::conversation_lease::PersistedConversationLeaseGuard,
    _lifecycle_guard: tokio::sync::OwnedMutexGuard<()>,
    lifecycle_heartbeat: LifecycleLeaseHeartbeat,
}

#[derive(Clone)]
struct RemovalAdmission {
    authority: Arc<RemovalAuthority>,
    barrier: Arc<wardian_core::agent_replacement::AgentRosterBarrier>,
}

#[derive(Clone)]
struct AgentRemovalOwnership {
    admission: RemovalAdmission,
    target_config: Arc<std::sync::Mutex<AgentConfig>>,
    home: std::path::PathBuf,
}

/// One admitted deletion retains the exact incarnation and lease through both
/// durable stores and live publication. Compensation always recaptures live state.
async fn finish_agent_removal<R: tauri::Runtime>(
    session_id: String,
    expected_name: Option<String>,
    app: AppHandle<R>,
    require_stopped: bool,
    ownership: AgentRemovalOwnership,
) -> Result<(), String> {
    let state = app.state::<AppState>();
    let (config, generation) = {
        let agents = state.agents.lock().await;
        let agent = validate_removal_incarnation(&agents, &session_id, &ownership.target_config)?;
        validate_agent_removal(agent, expected_name.as_deref(), require_stopped)?;
        let config = agent
            .config
            .lock()
            .map_err(|error| error.to_string())?
            .clone();
        (config, agent.runtime_generation)
    };
    ownership
        .admission
        .authority
        .lifecycle_heartbeat
        .ensure_active("remove")?;
    let ticket = bound::admit(&ownership, &config, generation).await?;
    if let Err(error) = write_owned_snapshot(
        &ownership,
        &app,
        &session_id,
        generation,
        expected_name,
        require_stopped,
        true,
    )
    .await
    {
        return Err(match ticket {
            Some(ticket) => format!(
                "[agent-name-ownership-pending] Agent {} removal intent retained (attempt {}); inspect actual roster before retry: {error}",
                ticket.agent_id, ticket.attempt_id,
            ),
            None => {
                compensate_failed_agent_removal(
                    &ownership,
                    &app,
                    &session_id,
                    generation,
                    format!("Failed to persist agent deletion: {error}"),
                )
                .await
            }
        });
    }
    let (agent, ticket) = match ticket {
        Some(ticket) => {
            let agent = finish_bound_after_json(&ownership, &app, ticket.clone())
                .await
                .map_err(|error| bound::pending(&ticket, error))?;
            (agent, Some(ticket))
        }
        None => (
            finish_ordinary_after_json(&ownership, &app, &session_id).await?,
            None,
        ),
    };
    state.remove_agent_delivery_state(&session_id).await;
    state
        .terminal_sessions
        .forget_deferred_geometry(&session_id)
        .await;
    let remaining = state
        .agents
        .lock()
        .await
        .keys()
        .cloned()
        .collect::<BTreeSet<_>>();
    let workspace = agent
        .config
        .lock()
        .map_err(|error| error.to_string())?
        .folder
        .clone();
    let _ = app.emit("agents-updated", ());
    // Every physical callback has finished. Keep the lifecycle authority while
    // releasing unrelated roster writers before reference/profile cleanup.
    drop(ownership.admission.barrier);
    let authority = ownership.admission.authority;
    if authority
        .lifecycle_heartbeat
        .ensure_active("clean removed agent")
        .is_ok()
    {
        cleanup_committed_removal(
            &app,
            &session_id,
            &ownership.home,
            &workspace,
            &remaining,
            ticket.as_ref().map(|ticket| ticket.profile()),
        )
        .await;
    }
    Ok(())
}

async fn validate_live_removal<R: tauri::Runtime>(
    ownership: &AgentRemovalOwnership,
    app: &AppHandle<R>,
    session_id: &str,
    generation: Option<u64>,
) -> Result<(), String> {
    ownership
        .admission
        .authority
        .lifecycle_heartbeat
        .ensure_active("remove")?;
    let state = app.state::<AppState>();
    let agents = state.agents.lock().await;
    let agent = validate_removal_incarnation(&agents, session_id, &ownership.target_config)?;
    if agent.runtime_generation != generation {
        return Err(format!(
            "Agent {session_id} incarnation changed during removal"
        ));
    }
    Ok(())
}

async fn validate_detached_removal<R: tauri::Runtime>(
    ownership: &AgentRemovalOwnership,
    app: &AppHandle<R>,
    ticket: &wardian_core::agent_spawn::BoundRemovalTicket,
) -> Result<(), String> {
    bound::validate_no_replacement(&ownership.admission, app, ticket).await?;
    let config = ownership
        .target_config
        .lock()
        .map_err(|error| error.to_string())?
        .clone();
    if wardian_core::agent_spawn::RemovalAgentIdentity::from(&config) != *ticket.identity() {
        return Err("Detached removal identity changed".into());
    }
    Ok(())
}

async fn write_owned_snapshot<R: tauri::Runtime>(
    ownership: &AgentRemovalOwnership,
    app: &AppHandle<R>,
    session_id: &str,
    generation: Option<u64>,
    expected_name: Option<String>,
    require_stopped: bool,
    remove_target: bool,
) -> Result<(), String> {
    let owner = ownership.clone();
    let app = app.clone();
    let id = session_id.to_string();
    #[cfg(test)]
    let probe = manager::ROSTER_IO_PROBE
        .try_with(|probe| probe.borrow_mut().take())
        .ok()
        .flatten();
    let operation = move || {
        let removed_id = remove_target.then(|| id.clone());
        manager::try_save_state_snapshot_captured_for_home(
            &owner.home,
            removed_id.as_deref(),
            || {
                tauri::async_runtime::block_on(async {
                    validate_live_removal(&owner, &app, &id, generation).await?;
                    let state = app.state::<AppState>();
                    let agents = state.agents.lock().await;
                    let order = state.agent_order.lock().await;
                    let agent = validate_removal_incarnation(&agents, &id, &owner.target_config)?;
                    validate_agent_removal(agent, expected_name.as_deref(), require_stopped)?;
                    Ok(manager::state_configs_snapshot(&agents, &order)
                        .into_iter()
                        .filter(|config| !remove_target || config.session_id != id)
                        .collect())
                })
            },
            || {
                owner
                    .admission
                    .authority
                    .lifecycle_heartbeat
                    .ensure_active("write removal roster")
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

async fn compensate_failed_agent_removal<R: tauri::Runtime>(
    ownership: &AgentRemovalOwnership,
    app: &AppHandle<R>,
    session_id: &str,
    generation: Option<u64>,
    error: String,
) -> String {
    if let Err(owner_error) = validate_live_removal(ownership, app, session_id, generation).await {
        return owner_error;
    }
    let written =
        write_owned_snapshot(ownership, app, session_id, generation, None, false, false).await;
    if let Err(owner_error) = validate_live_removal(ownership, app, session_id, generation).await {
        return owner_error;
    }
    match written {
        Ok(()) => error,
        Err(rollback) => format!("{error}; state snapshot rollback failed: {rollback}"),
    }
}

async fn finish_ordinary_after_json<R: tauri::Runtime>(
    ownership: &AgentRemovalOwnership,
    app: &AppHandle<R>,
    session_id: &str,
) -> Result<ActiveAgent, String> {
    let state = app.state::<AppState>();
    let mut runtime = {
        let mut agents = state.agents.lock().await;
        let agent = agents
            .get_mut(session_id)
            .ok_or("Removal target disappeared")?;
        if !Arc::ptr_eq(&agent.config, &ownership.target_config) {
            return Err(format!(
                "Agent {session_id} incarnation changed during removal"
            ));
        }
        take_agent_runtime_for_termination(agent)
    };
    let generation = runtime.runtime_generation;
    let broker = state
        .terminal_sessions
        .remove_agent_session(session_id, generation)
        .await
        .map_err(|error| format!("Terminal broker cleanup failed: {error}"));
    let joined = join_agent_processes_for_removal(&mut runtime).await;
    if joined.is_err() {
        let mut agents = state.agents.lock().await;
        if let Some(agent) = agents.get_mut(session_id).filter(|agent| {
            Arc::ptr_eq(&agent.config, &ownership.target_config)
                && agent.runtime_generation.is_none()
        }) {
            agent.child_process = runtime.child_process.take();
            agent.background_processes = std::mem::take(&mut runtime.background_processes);
            agent.process_id = runtime.process_id.take();
            agent.runtime_generation = runtime.runtime_generation.take();
            #[cfg(windows)]
            {
                agent.job_object = runtime.job_object.take();
            }
        }
    }
    let expected_generation = if joined.is_err() { generation } else { None };
    validate_live_removal(ownership, app, session_id, expected_generation).await?;
    if joined.is_ok() {
        let mut agents = state.agents.lock().await;
        let agent = agents
            .get_mut(session_id)
            .ok_or("Removal target disappeared")?;
        if !Arc::ptr_eq(&agent.config, &ownership.target_config) {
            return Err(format!(
                "Agent {session_id} incarnation changed during removal"
            ));
        }
        mark_agent_paused_off(agent);
    }
    if let Err(error) = broker.and(joined) {
        return Err(compensate_failed_agent_removal(
            ownership,
            app,
            session_id,
            expected_generation,
            error,
        )
        .await);
    }
    let owner = ownership.clone();
    let delete_app = app.clone();
    let id = session_id.to_string();
    let deletion = state
        .interactions
        .delete_agent_durable_state_fenced(session_id, move || {
            tauri::async_runtime::block_on(validate_live_removal(&owner, &delete_app, &id, None))?;
            if bound::roster(&owner.home)?
                .iter()
                .any(|config| config.session_id == id)
            {
                return Err("Removal roster no longer proves absence".into());
            }
            tauri::async_runtime::block_on(validate_live_removal(&owner, &delete_app, &id, None))?;
            wardian_core::db::delete_agent_after_owned_removal_fenced(&id, true, || {
                owner
                    .admission
                    .authority
                    .lifecycle_heartbeat
                    .ensure_active("delete durable agent state")
            })
            .map_err(|error| format!("Failed to delete agent state: {error}"))
        })
        .await;
    if let Err(error) = deletion {
        return Err(compensate_failed_agent_removal(ownership, app, session_id, None, error).await);
    }
    // SQL committed. A later lease loss must not leave this deleted incarnation
    // advertised, while a replacement Arc must never be detached.
    let mut agents = state.agents.lock().await;
    let mut order = state.agent_order.lock().await;
    validate_removal_incarnation(&agents, session_id, &ownership.target_config)?;
    detach_agent_for_kill(&mut agents, &mut order, session_id)
        .ok_or("Removal target disappeared".into())
}

async fn finish_bound_after_json<R: tauri::Runtime>(
    ownership: &AgentRemovalOwnership,
    app: &AppHandle<R>,
    ticket: wardian_core::agent_spawn::BoundRemovalTicket,
) -> Result<ActiveAgent, String> {
    let state = app.state::<AppState>();
    validate_live_removal(
        ownership,
        app,
        &ticket.agent_id,
        ticket.runtime_generation(),
    )
    .await?;
    let mut agent = {
        let mut agents = state.agents.lock().await;
        let mut order = state.agent_order.lock().await;
        validate_removal_incarnation(&agents, &ticket.agent_id, &ownership.target_config)?;
        detach_agent_for_kill(&mut agents, &mut order, &ticket.agent_id)
            .ok_or("Bound removal target disappeared")?
    };
    let count = usize::from(agent.child_process.is_some()) + agent.background_processes.len();
    state
        .terminal_sessions
        .remove_agent_session(&ticket.agent_id, agent.runtime_generation)
        .await
        .map_err(|error| format!("Terminal broker cleanup failed: {error}"))?;
    join_agent_processes_for_removal(&mut agent).await?;
    validate_detached_removal(ownership, app, &ticket).await?;
    let receipt = wardian_core::agent_spawn::RemovalClosureReceipt {
        runtime_generation: ticket.runtime_generation(),
        owned_process_count: count,
        all_process_handles_joined: true,
        broker_closed: true,
    };
    let owner = ownership.clone();
    let seal_app = app.clone();
    let unsealed = ticket.clone();
    let sealed = tokio::task::spawn_blocking(move || {
        bound::seal(
            &unsealed,
            &receipt,
            &owner.home,
            || {
                tauri::async_runtime::block_on(validate_detached_removal(
                    &owner, &seal_app, &unsealed,
                ))
            },
            || {
                owner
                    .admission
                    .authority
                    .lifecycle_heartbeat
                    .ensure_active("seal bound closure")
            },
        )
    })
    .await
    .map_err(|error| error.to_string())??;
    let owner = ownership.clone();
    let delete_app = app.clone();
    state
        .interactions
        .delete_agent_durable_state_fenced(&ticket.agent_id, move || {
            bound::complete(
                &sealed,
                &owner.home,
                || {
                    tauri::async_runtime::block_on(validate_detached_removal(
                        &owner,
                        &delete_app,
                        &sealed,
                    ))
                },
                || {
                    owner
                        .admission
                        .authority
                        .lifecycle_heartbeat
                        .ensure_active("delete bound state")
                },
            )
        })
        .await?;
    Ok(agent)
}

async fn cleanup_committed_removal<R: tauri::Runtime>(
    app: &AppHandle<R>,
    session_id: &str,
    home: &Path,
    workspace: &str,
    remaining: &BTreeSet<String>,
    profile: Option<&wardian_core::agent_spawn::RemovalProfileIdentity>,
) {
    let state = app.state::<AppState>();
    for browser_id in state.browser_sessions.close_for_agent(session_id).await {
        manager::log_debug(&format!(
            "[WARDIAN] closed browser session {browser_id} owned by {session_id}"
        ));
    }
    if !workspace.trim().is_empty() {
        if let Err(error) =
            crate::commands::change_snapshot::drop_agent_snapshots(workspace, session_id)
        {
            manager::log_debug(&format!(
                "[WARDIAN] Failed to drop change snapshots: {error}"
            ));
        }
    }
    if let Err(error) =
        crate::commands::change_review::remove_change_review_watermarks_for_agent(home, session_id)
    {
        manager::log_debug(&format!(
            "[WARDIAN] Failed to clean change review watermarks: {error}"
        ));
    }
    match DeletedAgentReferenceCleanup::run(home, remaining) {
        Ok(cleanup) => {
            if cleanup.watchlists_changed {
                let _ = app.emit("watchlists-updated", ());
            }
            if cleanup.topology_changed {
                let _ = app.emit("topology-changed", ());
            }
        }
        Err(error) => manager::log_debug(&format!(
            "[WARDIAN] Failed to clean deleted references: {error}"
        )),
    }
    if !matches!(
        wardian_core::agent_spawn::restore_is_excluded(session_id),
        Ok(false)
    ) {
        manager::log_debug("[WARDIAN] Retaining removed profile with unresolved ownership");
        return;
    }
    let cleanup = match profile {
        Some(profile) => {
            cleanup_removed_agent_directory_with_profile(home, session_id, Ok(()), Some(profile))
        }
        None => cleanup_removed_agent_directory(home, session_id, Ok(())),
    };
    if let Err(error) = cleanup {
        manager::log_debug(&format!("[WARDIAN] Retaining removed profile: {error}"));
    }
}

fn validate_removal_incarnation<'a>(
    agents: &'a HashMap<String, ActiveAgent>,
    session_id: &str,
    expected: &Arc<std::sync::Mutex<AgentConfig>>,
) -> Result<&'a ActiveAgent, String> {
    let agent = agents
        .get(session_id)
        .ok_or_else(|| format!("Agent {session_id} no longer exists"))?;
    if !Arc::ptr_eq(&agent.config, expected) {
        return Err(format!(
            "Agent {session_id} incarnation changed during removal"
        ));
    }
    Ok(agent)
}

/// Observe every retained child handle after termination, without holding the
/// roster or preparation lock. Killing or closing a terminal actor is not a join.
pub(super) async fn join_agent_processes_for_removal(
    agent: &mut ActiveAgent,
) -> Result<(), String> {
    let mut processes = RemovalProcesses::take(agent);
    let joined = processes.join().await;
    if joined.is_err() {
        processes.restore(agent);
    }
    joined
}
async fn wait_for_removal_process_exit(
    timeout: Duration,
    mut poll: impl FnMut() -> Result<bool, String>,
) -> Result<(), String> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if poll()? {
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            return Err("Timed out joining agent processes; ownership records retained".into());
        }
        tokio::time::sleep_until(
            deadline.min(tokio::time::Instant::now() + Duration::from_millis(25)),
        )
        .await;
    }
}

/// Caller holds lifecycle exclusion and has already joined the native owner.
/// Keep the preparation lock through both compact and agent-directory cleanup.
pub(super) fn cleanup_removed_agent_directory(
    home: &Path,
    agent_id: &str,
    process_join: Result<(), String>,
) -> Result<(), String> {
    cleanup_removed_agent_directory_with_profile(home, agent_id, process_join, None)
}

fn cleanup_removed_agent_directory_with_profile(
    home: &Path,
    agent_id: &str,
    process_join: Result<(), String>,
    expected_profile: Option<&wardian_core::agent_spawn::RemovalProfileIdentity>,
) -> Result<(), String> {
    process_join?;
    let agent_dir = home.join("agents").join(agent_id);
    match std::fs::symlink_metadata(&agent_dir) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.to_string()),
        Ok(_) => {}
    }
    let _preparation = crate::utils::codex_home::acquire_preparation(home, agent_id)?;
    if let Some(expected) = expected_profile {
        if &bound::profile(home, agent_id)? != expected {
            return Err("Removed profile ownership changed before cleanup".into());
        }
    }
    // On failure, leave the mapping and backup reachable for recovery. Never
    // remove the agent directory after a rejected or incomplete owned cleanup.
    // Off agents and other providers may never have materialized a habitat.
    // Inspect without following links; any ownership record requires validation.
    let has_habitat_alias = match std::fs::symlink_metadata(
        agent_dir.join(crate::utils::codex_home::HABITAT_ALIAS_RECORD),
    ) {
        Ok(_) => true,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => return Err(error.to_string()),
    };
    if has_habitat_alias {
        crate::utils::codex_home::cleanup_habitat_alias(home, agent_id)?;
    }

    let mut has_codex_state = false;
    for path in [
        agent_dir.join(".wardian-codex-home.json"),
        agent_dir.join(".wardian-codex-home-cleanup.json"),
        agent_dir.join("habitat").join(".codex"),
    ] {
        match std::fs::symlink_metadata(path) {
            Ok(_) => has_codex_state = true,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.to_string()),
        }
    }
    if has_codex_state {
        crate::utils::codex_home::cleanup_managed_home(home, agent_id)?;
    }
    std::fs::remove_dir_all(&agent_dir).map_err(|error| error.to_string())
}

pub(super) fn cleanup_failed_clone_profile_dir(profile_dir: &Path) {
    // Registration can start a compact owner before returning an error.
    // Its asynchronous failed-spawn cleanup has no joined-TUI receipt here.
    // Retain the ownership mapping and backup rather than orphan the slot.
    let no_compact_records = [
        ".wardian-codex-home.json",
        ".wardian-codex-home-cleanup.json",
        crate::utils::codex_home::HABITAT_ALIAS_RECORD,
    ]
    .iter()
    .all(|name| {
        matches!(
            std::fs::symlink_metadata(profile_dir.join(name)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound
        )
    });
    if no_compact_records {
        clone_remove_existing_path(profile_dir);
    } else {
        manager::log_debug(&format!(
            "[WARDIAN] Retaining failed clone directory {:?}: compact ownership may exist; process join is unconfirmed",
            profile_dir
        ));
    }
}
