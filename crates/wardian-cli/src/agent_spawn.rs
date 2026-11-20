use super::{
    control_error, live, parse_timeout, render_options, render_show, AgentArgs, AgentCommand,
    CliError, ExitCode,
};
use std::time::Duration;

pub(super) fn handle_agent_spawn(
    command: &AgentCommand,
    args: &AgentArgs,
) -> Result<String, CliError> {
    let AgentCommand::Spawn {
        provider,
        class,
        name,
        workspace,
        model,
        reasoning_effort,
        async_mode,
        operation_id: requested_operation_id,
        wait_timeout,
    } = command
    else {
        unreachable!("spawn handler receives the spawn command")
    };
    let wait_timeout = parse_timeout(wait_timeout)?;
    let max_wait = Duration::from_millis(wardian_core::agent_spawn::MAX_OBSERVER_WAIT_MS);
    if wait_timeout > max_wait {
        return Err(CliError::backend(
            ExitCode::Generic,
            "invalid_timeout",
            format!("spawn wait is limited to {} seconds", max_wait.as_secs()),
        ));
    }
    let (operation_id, generated) = match requested_operation_id.as_deref() {
        Some(operation_id) => (
            wardian_core::agent_spawn::canonical_operation_id(operation_id)
                .map_err(CliError::generic)?,
            false,
        ),
        None => (wardian_core::agent_spawn::new_operation_id(), true),
    };
    if generated {
        live::persist_spawn_submission_receipt(&operation_id).map_err(|error| {
            CliError::backend(
                ExitCode::Generic,
                "spawn_receipt_failed",
                format!("could not persist the spawn request key before sending: {error}"),
            )
        })?;
    }
    let request = wardian_core::agent_spawn::AgentSpawnRequest {
        provider: provider.to_string(),
        class: class.to_string(),
        name: name.clone(),
        workspace: workspace.clone(),
        model: model.clone(),
        reasoning_effort: reasoning_effort.clone(),
    };
    let admission = live::agent_spawn_admit(&operation_id, request)
        .map_err(|error| spawn_operation_error(error, &operation_id))?;
    if *async_mode {
        return serde_json::to_string(&serde_json::json!({
            "schema": 1,
            "ok": true,
            "operation_id": operation_id,
            "operation": admission.operation,
            "lookup": format!("wardian agent spawn-status {operation_id}"),
        }))
        .map(|json| format!("{json}\n"))
        .map_err(|error| CliError::generic(error.to_string()));
    }

    let result = live::agent_spawn_wait(&operation_id, wait_timeout)
        .map_err(|error| spawn_operation_error(error, &operation_id))?;
    match result.operation.state {
        wardian_core::agent_spawn::AgentSpawnState::Registered if !result.timed_out => {
            let agent = result
                .current_agent
                .filter(|agent| {
                    result.availability == wardian_core::control::AgentSpawnAvailability::Current
                        && agent.uuid == result.operation.agent_id
                        && agent.name == result.operation.agent_name
                        && agent.provider == result.operation.provider
                        && agent.class == result.operation.class
                })
                .ok_or_else(|| {
                    spawn_outcome_error(
                        "historical registration has no compatible current roster identity",
                        &operation_id,
                    )
                })?;
            render_show(&agent, &render_options(args))
        }
        wardian_core::agent_spawn::AgentSpawnState::Failed => Err(spawn_outcome_error(
            result
                .operation
                .error_message
                .as_deref()
                .unwrap_or("spawn validation failed"),
            &operation_id,
        )),
        wardian_core::agent_spawn::AgentSpawnState::OutcomeUnknown => Err(spawn_outcome_error(
            result
                .operation
                .error_message
                .as_deref()
                .unwrap_or("launch outcome is unknown"),
            &operation_id,
        )),
        _ => Err(spawn_outcome_error(
            "spawn is still in progress",
            &operation_id,
        )),
    }
}

pub(super) fn handle_agent_spawn_status(operation_id: &str) -> Result<String, CliError> {
    let operation_id = canonical_spawn_operation_id(operation_id)?;
    let response = live::agent_spawn_operation(&operation_id)
        .map_err(|error| spawn_operation_error(error, &operation_id))?;
    serde_json::to_string(&response)
        .map(|json| format!("{json}\n"))
        .map_err(|error| CliError::generic(error.to_string()))
}

pub(super) fn handle_agent_spawn_wait(
    operation_id: &str,
    timeout: &str,
) -> Result<String, CliError> {
    let operation_id = canonical_spawn_operation_id(operation_id)?;
    let timeout = parse_timeout(timeout)?;
    if timeout.as_millis() > u128::from(wardian_core::agent_spawn::MAX_OBSERVER_WAIT_MS) {
        return Err(CliError::backend(
            ExitCode::Generic,
            "invalid_timeout",
            "spawn wait is limited to 60 seconds",
        ));
    }
    let response = live::agent_spawn_wait(&operation_id, timeout)
        .map_err(|error| spawn_operation_error(error, &operation_id))?;
    serde_json::to_string(&response)
        .map(|json| format!("{json}\n"))
        .map_err(|error| CliError::generic(error.to_string()))
}

fn canonical_spawn_operation_id(operation_id: &str) -> Result<String, CliError> {
    wardian_core::agent_spawn::canonical_operation_id(operation_id).map_err(CliError::generic)
}

fn spawn_operation_error(error: std::io::Error, operation_id: &str) -> CliError {
    let mut mapped = control_error(error);
    let lower = mapped.message.to_ascii_lowercase();
    if mapped.code == "bad_request"
        && lower.contains("unknown variant")
        && (lower.contains("agent_spawn_async") || lower.contains("agentspawnasync"))
    {
        mapped.code = "unsupported_version".to_string();
        mapped.message = format!(
            "the running Wardian app does not support durable spawn requests; update the app before retrying. Operation key: {operation_id}. Check it with: wardian agent spawn-status {operation_id}"
        );
        return mapped;
    }
    mapped.message = format!(
        "{}. Operation key: {operation_id}. Check it with: wardian agent spawn-status {operation_id}",
        mapped.message
    );
    mapped
}

fn spawn_outcome_error(message: &str, operation_id: &str) -> CliError {
    CliError::backend(
        ExitCode::Generic,
        "spawn_operation_incomplete",
        format!(
            "{message}. Operation key: {operation_id}. Check it with: wardian agent spawn-status {operation_id}"
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durable_spawn_errors_keep_the_operation_key_and_flag_old_apps() {
        let operation_id = "5d4ebea1-1ce4-44fa-a67e-410f74aa1e0c";
        let old_app_error = std::io::Error::other(live::ControlEndpointError::new(
            "bad_request",
            "malformed control request JSON: unknown variant `agent_spawn_async`",
        ));
        let mapped = spawn_operation_error(old_app_error, operation_id);
        assert_eq!(mapped.code, "unsupported_version");
        assert!(mapped.message.contains(operation_id));
        assert!(mapped.message.contains("wardian agent spawn-status"));

        let admission_error = std::io::Error::other(live::ControlEndpointError::new(
            "queue_full",
            "agent spawn queue is full",
        ));
        let mapped = spawn_operation_error(admission_error, operation_id);
        assert_eq!(mapped.code, "queue_full");
        assert!(mapped.message.contains(operation_id));
        assert!(mapped.message.contains("wardian agent spawn-status"));
    }
}
