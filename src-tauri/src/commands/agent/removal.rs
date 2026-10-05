//! Owned durable removal, joined process shutdown and private-home cleanup.
//!
//! Removal joins the native owner before deleting durable state. If later
//! persistence fails, metadata remains but the owner is stopped; never
//! automatically restart or replay its work.
use super::{
    acquire_agent_lifecycle_transition_lease_for_session,
    agent_lifecycle::{lock_agent_lifecycle, stop_native_owner},
    clone_remove_existing_path, detach_agent_for_kill, validate_agent_removal,
    DeletedAgentReferenceCleanup, LifecycleLeaseHeartbeat,
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
        Arc::clone(
            &agents
                .get(&session_id)
                .ok_or_else(|| format!("Agent {session_id} not found"))?
                .config,
        )
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
        _lifecycle_lease,
        _lifecycle_guard,
        target_config,
        barrier,
        home,
        lifecycle_heartbeat,
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

struct AgentRemovalOwnership {
    _lifecycle_lease: wardian_core::conversation_lease::PersistedConversationLeaseGuard,
    _lifecycle_guard: tokio::sync::OwnedMutexGuard<()>,
    target_config: Arc<std::sync::Mutex<AgentConfig>>,
    barrier: wardian_core::agent_replacement::AgentRosterBarrier,
    home: std::path::PathBuf,
    lifecycle_heartbeat: LifecycleLeaseHeartbeat,
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
    let AgentRemovalOwnership {
        _lifecycle_lease,
        _lifecycle_guard,
        target_config,
        barrier,
        home,
        lifecycle_heartbeat,
    } = ownership;
    let state = app.state::<AppState>();
    lifecycle_heartbeat.ensure_active("remove")?;
    let deletion_state_snapshot = {
        let agents = state.agents.lock().await;
        let order = state.agent_order.lock().await;
        let agent = validate_removal_incarnation(&agents, &session_id, &target_config)?;
        validate_agent_removal(agent, expected_name.as_deref(), require_stopped)?;
        manager::state_configs_snapshot(&agents, &order)
            .into_iter()
            .filter(|config| config.session_id != session_id)
            .collect::<Vec<_>>()
    };
    lifecycle_heartbeat.ensure_active("remove")?;
    if let Err(error) =
        manager::roster_io::write_snapshot_strict(&barrier, home.clone(), deletion_state_snapshot)
            .await
    {
        return compensate_failed_agent_removal(
            &state,
            &session_id,
            &target_config,
            &barrier,
            &home,
            &lifecycle_heartbeat,
            format!("Failed to persist agent deletion: {error}"),
        )
        .await;
    }
    lifecycle_heartbeat.ensure_active("delete durable agent state")?;
    if let Err(error) = state
        .interactions
        .delete_agent_durable_state(&session_id)
        .await
    {
        return compensate_failed_agent_removal(
            &state,
            &session_id,
            &target_config,
            &barrier,
            &home,
            &lifecycle_heartbeat,
            error,
        )
        .await;
    }
    // Both stores have committed. Finish cache/map publication under the local
    // lifecycle guard; a later heartbeat failure must not leave the live roster
    // advertising the deleted incarnation.
    let (agent, remaining_agent_ids) = {
        let mut agents = state.agents.lock().await;
        let mut order = state.agent_order.lock().await;
        validate_removal_incarnation(&agents, &session_id, &target_config)?;
        let agent = detach_agent_for_kill(&mut agents, &mut order, &session_id);
        let remaining_agent_ids = agent
            .is_some()
            .then(|| agents.keys().cloned().collect::<BTreeSet<_>>());
        (agent, remaining_agent_ids)
    };
    if agent.is_some() {
        state.remove_agent_delivery_state(&session_id).await;
    }
    // Both durable stores and cache/map publication are now complete. Slow
    // browser/process/reference cleanup need not exclude unrelated roster saves.
    drop(barrier);
    state
        .terminal_sessions
        .forget_deferred_geometry(&session_id)
        .await;
    // A browser this agent opened has no other owner, so it goes with the
    // agent rather than lingering as an orphaned headless process.
    for browser_id in state.browser_sessions.close_for_agent(&session_id).await {
        manager::log_debug(&format!(
            "[WARDIAN] closed browser session {browser_id} owned by {session_id}"
        ));
    }

    #[allow(unused_mut)]
    if let Some(mut agent) = agent {
        let terminal_cleanup = state
            .terminal_sessions
            .remove_agent_session(&session_id, agent.runtime_generation)
            .await
            .map_err(|error| format!("Terminal broker cleanup failed: {error}"));
        let agent_workspace = agent
            .config
            .lock()
            .ok()
            .map(|config| config.folder.clone())
            .filter(|folder| !folder.trim().is_empty());
        // Broker shutdown alone is not proof of TUI exit (it can time out).
        let process_join = join_agent_processes_for_removal(&mut agent).await;
        let process_join = terminal_cleanup.and(process_join);

        // Durable state was deleted before detaching the live agent. Post-commit
        // cleanup is best-effort so a lease heartbeat cannot leave the roster
        // diverging from the two durable stores after the commit.
        let _ = app.emit("agents-updated", ());

        // Cleanup: remove persisted references and the agent's private directory.
        // Snapshot refs go with the agent. They live in the operator's own object
        // store, so leaving them behind would keep superseded blobs reachable for
        // an agent that no longer exists.
        if let Some(workspace) = agent_workspace.as_deref() {
            if let Err(error) =
                crate::commands::change_snapshot::drop_agent_snapshots(workspace, &session_id)
            {
                manager::log_debug(&format!(
                    "[WARDIAN] Failed to drop change snapshots for {}: {}",
                    session_id, error
                ));
            }
        }
        if let Some(home) = crate::utils::fs::get_wardian_home() {
            if let Err(error) =
                crate::commands::change_review::remove_change_review_watermarks_for_agent(
                    &home,
                    &session_id,
                )
            {
                manager::log_debug(&format!(
                    "[WARDIAN] Failed to clean change review watermarks for {}: {}",
                    session_id, error
                ));
            }
            if let Some(remaining_agent_ids) = remaining_agent_ids.as_ref() {
                match DeletedAgentReferenceCleanup::run(&home, remaining_agent_ids) {
                    Ok(cleanup) => {
                        if cleanup.watchlists_changed {
                            let _ = app.emit("watchlists-updated", ());
                        }
                        if cleanup.topology_changed {
                            let _ = app.emit("topology-changed", ());
                        }
                    }
                    Err(error) => manager::log_debug(&format!(
                        "[WARDIAN] Failed to clean deleted agent references for {}: {}",
                        session_id, error
                    )),
                }
            }

            if let Err(error) = cleanup_removed_agent_directory(&home, &session_id, process_join) {
                manager::log_debug(&format!(
                    "[WARDIAN] Retaining agent directory and compact ownership records for {session_id}: {error}"
                ));
            }
        }

        Ok(())
    } else {
        let err_msg = format!("Agent with session ID {} not found", session_id);
        manager::log_debug(&format!("[WARDIAN] {}", err_msg));
        Err(err_msg)
    }
}

async fn compensate_failed_agent_removal(
    state: &AppState,
    session_id: &str,
    target_config: &Arc<std::sync::Mutex<AgentConfig>>,
    barrier: &wardian_core::agent_replacement::AgentRosterBarrier,
    home: &std::path::Path,
    lifecycle_heartbeat: &LifecycleLeaseHeartbeat,
    error: String,
) -> Result<(), String> {
    lifecycle_heartbeat.ensure_active("compensate removal")?;
    let live = {
        let agents = state.agents.lock().await;
        let order = state.agent_order.lock().await;
        validate_removal_incarnation(&agents, session_id, target_config)?;
        manager::state_configs_snapshot(&agents, &order)
    };
    let rollback_error =
        manager::roster_io::write_snapshot_strict(barrier, home.to_path_buf(), live)
            .await
            .err()
            .map(|rollback| format!("; state snapshot rollback failed: {rollback}"))
            .unwrap_or_default();
    Err(format!("{error}{rollback_error}"))
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
    let missing_tui_handle = (agent.process_id.is_some() || agent.runtime_generation.is_some())
        && agent.child_process.is_none();
    let mut tui = agent.child_process.take();
    let mut background = std::mem::take(&mut agent.background_processes);
    // Keep the actual handles while the existing helper kills the PTY tree and
    // closes the job object. It normally drops these handles without waiting.
    manager::terminate_active_agent_process(agent);
    if let Some(child) = tui.as_mut() {
        let _ = child.kill();
    }
    for child in &mut background {
        #[cfg(windows)]
        let _ = crate::utils::process::force_kill_process_tree(child.id());
        let _ = child.kill();
    }
    if missing_tui_handle {
        return Err("Cannot prove TUI exit: process handle is missing".into());
    }
    wait_for_removal_process_exit(Duration::from_secs(5), || {
        let mut exited = true;
        if let Some(child) = tui.as_mut() {
            exited &= child
                .try_wait()
                .map_err(|error| error.to_string())?
                .is_some();
        }
        for child in &mut background {
            exited &= child
                .try_wait()
                .map_err(|error| error.to_string())?
                .is_some();
        }
        Ok(exited)
    })
    .await
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
    process_join?;
    let agent_dir = home.join("agents").join(agent_id);
    match std::fs::symlink_metadata(&agent_dir) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.to_string()),
        Ok(_) => {}
    }
    let _preparation = crate::utils::codex_home::acquire_preparation(home, agent_id)?;
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
