use super::normalize_agent_description;
use crate::manager;
use crate::state::conversation_archive::effective_conversation_logging;
use crate::state::AppState;
use wardian_core::conversations::ConversationLoggingSetting;
use wardian_core::models::AgentConfig;

pub(super) async fn persist_agent_config_while_lifecycle_locked(
    mut new_config: AgentConfig,
    state: &AppState,
) -> Result<(), String> {
    new_config.validate_provider_config_matches_provider()?;
    new_config.description = normalize_agent_description(&new_config.description)?;
    new_config.mark_provider_config_nested_for_save();
    let capture_snapshot =
        crate::commands::chat::agent_archive_capture_snapshot(state, &new_config.session_id)
            .await?;
    let (config_handle, previous_config, previous_state_snapshot, created_at) = {
        let agents = state.agents.lock().await;
        let order = state.agent_order.lock().await;
        let agent = agents
            .get(&new_config.session_id)
            .ok_or_else(|| format!("Agent {} not found", new_config.session_id))?;
        let config_handle = agent.config.clone();
        let previous_config = agent.config.lock().unwrap().clone();
        let previous_state_snapshot = manager::state_configs_snapshot(&agents, &order);
        let created_at = agent.init_timestamp.lock().unwrap().clone();
        (
            config_handle,
            previous_config,
            previous_state_snapshot,
            created_at,
        )
    };

    // If class has changed, auto-update the system_include_directories.
    let current_class = previous_config.agent_class.clone();
    if current_class != new_config.agent_class {
        manager::log_debug(&format!(
            "[WARDIAN] Agent class changed from {} to {}. Updating system include directories.",
            current_class, new_config.agent_class
        ));
        new_config.system_include_directories =
            Some(crate::utils::fs::resolve_system_include_directories(
                &new_config.agent_class,
                &new_config.session_id,
            ));
    }

    let global_logging = crate::utils::shell::load_shell_settings()
        .unwrap_or_default()
        .conversation_logging;
    let previous_logging =
        effective_conversation_logging(global_logging, previous_config.conversation_logging);
    let next_logging =
        effective_conversation_logging(global_logging, new_config.conversation_logging);
    let _policy_guard = if previous_logging != next_logging {
        Some(state.conversation_capture_policy_lock.lock().await)
    } else {
        None
    };
    if previous_logging != ConversationLoggingSetting::Disabled
        && next_logging == ConversationLoggingSetting::Disabled
    {
        crate::commands::chat::record_provider_log_policy_for_snapshot(
            state,
            &capture_snapshot,
            false,
        )?;
    }

    let roster_barrier = wardian_core::agent_replacement::acquire_agent_roster_barrier(true)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "Agent roster barrier is unavailable".to_string())?;
    let mut state_snapshot = previous_state_snapshot.clone();
    let persisted_config = state_snapshot
        .iter_mut()
        .find(|config| config.session_id == new_config.session_id)
        .ok_or_else(|| {
            format!(
                "Agent {} is missing from persisted order",
                new_config.session_id
            )
        })?;
    *persisted_config = new_config.clone();
    manager::try_save_state_snapshot_unlocked(&state_snapshot)
        .map_err(|error| format!("Failed to persist agent configuration: {error}"))?;

    let workspace = crate::utils::fs::resolve_cwd(&new_config.folder, &new_config.session_id)
        .to_string_lossy()
        .to_string();
    let project = wardian_core::db::project_name_from_workspace(&workspace);
    wardian_core::db::upsert_agent(&wardian_core::db::AgentUpsert {
        session_id: &new_config.session_id,
        session_name: &new_config.session_name,
        description: &new_config.description,
        agent_class: &new_config.agent_class,
        provider: &new_config.provider,
        workspace: Some(&workspace),
        project: project.as_deref(),
        is_off: new_config.is_off,
        created_at: created_at.as_deref(),
    })
    .map_err(|error| {
        let rollback_error = manager::try_save_state_snapshot_unlocked(&previous_state_snapshot)
            .err()
            .map(|rollback| format!("; state rollback also failed: {rollback}"))
            .unwrap_or_default();
        format!("Failed to persist agent metadata: {error}{rollback_error}")
    })?;

    *config_handle.lock().unwrap() = new_config;
    // Never carry the global roster barrier into the per-agent archive gate.
    drop(roster_barrier);
    if previous_logging == ConversationLoggingSetting::Disabled
        && next_logging != ConversationLoggingSetting::Disabled
    {
        crate::commands::chat::record_provider_log_policy_for_snapshot(
            state,
            &capture_snapshot,
            true,
        )?;
    }
    Ok(())
}
