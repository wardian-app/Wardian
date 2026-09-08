//! Joined process shutdown and owned-home cleanup for agent removal.
//!
//! Removal joins the native owner before deleting durable state. If later
//! persistence fails, metadata remains but the owner is stopped; never
//! automatically restart or replay its work.
use super::clone_remove_existing_path;
use crate::{manager, state::ActiveAgent};
use std::{path::Path, time::Duration};

#[cfg(test)]
#[path = "removal_tests.rs"]
mod tests;

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
