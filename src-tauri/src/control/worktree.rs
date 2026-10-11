//! Worktree control operations; preparation never needs live agent state.
use super::*;

/// Preparation has no AppHandle or AppState dependency and cannot assign an agent.
pub(super) async fn handle_agent_worktree_prepare(
    source: String,
    name: String,
    reference: String,
) -> Result<String, ControlError> {
    let response = tokio::task::spawn_blocking(move || {
        crate::commands::git::prepare_worktree(&source, &name, &reference)
    })
    .await
    .map_err(|error| ControlError::request_failed(error.to_string()))?
    .map_err(ControlError::request_failed)?;
    ok_json(&response)
}

pub(super) async fn handle_agent_worktree_enable(
    app: &AppHandle,
    target: &str,
    name: Option<String>,
) -> Result<String, ControlError> {
    let uuid = resolve_target_uuid(app, target)
        .await
        .ok_or_else(|| ControlError::not_found(format!("agent not found: {target}")))?;
    let previous_workspace = agent_workspace(app, &uuid).await;
    let branch_name = agent_worktree_branch_name(app, &uuid, name.as_deref()).await?;

    let state = app.state::<AppState>();
    crate::commands::agent::enable_agent_worktree(uuid.clone(), name, state, app.clone())
        .await
        .map_err(ControlError::request_failed)?;

    let worktrees = list_agent_worktree_summaries(app.state::<AppState>()).await?;
    let worktree = worktree_for_member(&worktrees, &uuid);
    let agent = live_agent_identity(app, &uuid).await?;
    let response = AgentWorktreeMutationResponse {
        schema: wardian_core::control::CONTROL_SCHEMA,
        ok: true,
        action: "enable".to_string(),
        previous_workspace,
        current_workspace: agent.workspace.clone(),
        agent,
        worktree,
        previous_worktree: None,
        branch_name: Some(branch_name),
        cleared_session: true,
    };
    ok_json(&response)
}

pub(super) async fn handle_agent_worktree_join(
    app: &AppHandle,
    target: &str,
    worktree: &str,
) -> Result<String, ControlError> {
    let uuid = resolve_target_uuid(app, target)
        .await
        .ok_or_else(|| ControlError::not_found(format!("agent not found: {target}")))?;
    let previous_workspace = agent_workspace(app, &uuid).await;
    let state = app.state::<AppState>();
    let before = list_agent_worktree_summaries(app.state::<AppState>()).await?;
    let target_worktree = worktree_by_folder(&before, worktree).ok_or_else(|| {
        ControlError::coded(
            "not_managed_worktree",
            format!("worktree is not managed by Wardian: {worktree}"),
        )
    })?;

    crate::commands::agent::assign_agent_worktree(
        uuid.clone(),
        target_worktree.worktree_folder.clone(),
        state,
        app.clone(),
    )
    .await
    .map_err(ControlError::request_failed)?;

    let worktrees = list_agent_worktree_summaries(app.state::<AppState>()).await?;
    let agent = live_agent_identity(app, &uuid).await?;
    let response = AgentWorktreeMutationResponse {
        schema: wardian_core::control::CONTROL_SCHEMA,
        ok: true,
        action: "join".to_string(),
        previous_workspace,
        current_workspace: agent.workspace.clone(),
        agent,
        worktree: worktree_for_member(&worktrees, &uuid).or(Some(target_worktree)),
        previous_worktree: None,
        branch_name: None,
        cleared_session: true,
    };
    ok_json(&response)
}

pub(super) async fn handle_agent_worktree_disable(
    app: &AppHandle,
    target: &str,
) -> Result<String, ControlError> {
    let uuid = resolve_target_uuid(app, target)
        .await
        .ok_or_else(|| ControlError::not_found(format!("agent not found: {target}")))?;
    let previous_workspace = agent_workspace(app, &uuid).await;
    let before = list_agent_worktree_summaries(app.state::<AppState>()).await?;
    let previous_worktree = worktree_for_member(&before, &uuid);

    let state = app.state::<AppState>();
    crate::commands::agent::disable_agent_worktree(uuid.clone(), state, app.clone())
        .await
        .map_err(ControlError::request_failed)?;

    let agent = live_agent_identity(app, &uuid).await?;
    let response = AgentWorktreeMutationResponse {
        schema: wardian_core::control::CONTROL_SCHEMA,
        ok: true,
        action: "disable".to_string(),
        previous_workspace,
        current_workspace: agent.workspace.clone(),
        agent,
        worktree: None,
        previous_worktree,
        branch_name: None,
        cleared_session: true,
    };
    ok_json(&response)
}
