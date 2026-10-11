//! Worktree CLI dispatch and receipt rendering.
use super::{args::AgentWorktreeCommand, control_error, errors::CliError, live};

pub(super) fn handle_agent_worktree(command: &AgentWorktreeCommand) -> Result<String, CliError> {
    match command {
        AgentWorktreeCommand::Prepare {
            source,
            name,
            reference,
        } => {
            let response =
                live::agent_worktree_prepare(source, name, reference).map_err(control_error)?;
            serde_json::to_string(&response)
                .map(|json| format!("{json}\n"))
                .map_err(|error| CliError::generic(error.to_string()))
        }
        AgentWorktreeCommand::List => {
            let worktrees = live::agent_worktree_list().map_err(control_error)?;
            render_worktree_list(&worktrees)
        }
        AgentWorktreeCommand::Enable { target, name } => {
            let response =
                live::agent_worktree_enable(target, name.as_deref()).map_err(control_error)?;
            render_worktree_mutation_response(&response)
        }
        AgentWorktreeCommand::Join { target, worktree } => {
            let response = live::agent_worktree_join(target, worktree).map_err(control_error)?;
            render_worktree_mutation_response(&response)
        }
        AgentWorktreeCommand::Disable { target } => {
            let response = live::agent_worktree_disable(target).map_err(control_error)?;
            render_worktree_mutation_response(&response)
        }
    }
}

pub(super) fn render_worktree_list(
    worktrees: &[wardian_core::control::AgentWorktreeSummary],
) -> Result<String, CliError> {
    serde_json::to_string(&serde_json::json!({
        "schema": 1,
        "worktrees": worktrees,
    }))
    .map(|json| format!("{json}\n"))
    .map_err(|e| CliError::generic(e.to_string()))
}

pub(super) fn render_worktree_mutation_response(
    response: &wardian_core::control::AgentWorktreeMutationResponse,
) -> Result<String, CliError> {
    serde_json::to_string(response)
        .map(|json| format!("{json}\n"))
        .map_err(|e| CliError::generic(e.to_string()))
}
