//! Canonical v2 CLI entrypoints. One request, one receipt, no legacy fallback.
use crate::{
    args::{
        MessageArgs, MessageCommand, MessageCommandArgs, ReceiveMessagesArgs, ReplyArgs,
        ReplyStatusArg,
    },
    errors::{CliError, ExitCode},
    live, read_message_input,
};
use wardian_core::{
    agent_messaging::{AgentMessagingRequest, MAX_MESSAGE_BYTES},
    control::ReplyStatus,
};

fn invalid(message: &str) -> CliError {
    CliError::backend(ExitCode::Generic, "invalid_arguments", message)
}

fn nonempty(value: &str) -> Result<(), CliError> {
    if value.trim().is_empty() {
        Err(invalid("Text must not be empty"))
    } else {
        Ok(())
    }
}

fn body(message: Option<&str>, stdin: bool, file: Option<&str>) -> Result<String, CliError> {
    let message = read_message_input(message, stdin, file)?;
    nonempty(&message)?;
    if message.len() > MAX_MESSAGE_BYTES {
        return Err(invalid("Message exceeds the 64 KiB limit"));
    }
    Ok(message)
}

fn message_request(
    target: String,
    message: String,
    task: bool,
    idempotency_key: Option<String>,
) -> Result<AgentMessagingRequest, CliError> {
    let lower = target.to_ascii_lowercase();
    if target.trim().is_empty()
        || target != target.trim()
        || matches!(lower.as_str(), "all" | "*" | "broadcast")
        || lower.starts_with("class:")
    {
        return Err(invalid(
            "Use one exact agent name or UUID; broadcast selectors are unsupported",
        ));
    }
    if let Some(key) = &idempotency_key {
        nonempty(key)?;
    }
    Ok(if task {
        AgentMessagingRequest::FollowupTask {
            target,
            message,
            idempotency_key,
        }
    } else {
        AgentMessagingRequest::SendMessage {
            target,
            message,
            idempotency_key,
        }
    })
}

fn send(args: &MessageArgs, task: bool) -> Result<String, CliError> {
    let message = body(args.message.as_deref(), args.stdin, args.file.as_deref())?;
    invoke(message_request(
        args.target.clone(),
        message,
        task,
        args.idempotency_key.clone(),
    )?)
}

fn receive(args: &ReceiveMessagesArgs) -> Result<String, CliError> {
    for cursor in [&args.cursor, &args.ack_cursor].into_iter().flatten() {
        nonempty(cursor)?;
    }
    invoke(AgentMessagingRequest::ReceiveMessages {
        cursor: args.cursor.clone(),
        ack_cursor: args.ack_cursor.clone(),
        limit: Some(args.limit),
        timeout_ms: Some(args.timeout_ms),
    })
}

fn reply(args: &ReplyArgs) -> Result<String, CliError> {
    nonempty(&args.request_id)?;
    let message = body(args.message.as_deref(), args.stdin, args.file.as_deref())?;
    let status = match args.status {
        ReplyStatusArg::Done => ReplyStatus::Done,
        ReplyStatusArg::Blocked => ReplyStatus::Blocked,
        ReplyStatusArg::Failed => ReplyStatus::Failed,
    };
    invoke(AgentMessagingRequest::Reply {
        request_id: args.request_id.clone(),
        status,
        message,
    })
}

fn invoke(request: AgentMessagingRequest) -> Result<String, CliError> {
    live::require_current_message_origin().map_err(|_| {
        CliError::backend(
            ExitCode::NotInSession,
            "missing_managed_sender",
            "Messaging requires an existing managed Wardian sender. No request was sent.",
        )
    })?;
    let value = live::messaging::request(request).map_err(|error| {
        let mut error = crate::control_error(error);
        error
            .message
            .push_str(" Delivery may be uncertain; do not replay automatically.");
        error
    })?;
    serde_json::to_string(&value)
        .map(|value| format!("{value}\n"))
        .map_err(|error| CliError::generic(error.to_string()))
}

/// Dispatch only canonical operations, before any CLI-local storage migration.
pub(crate) fn handle(args: &MessageCommandArgs) -> Result<String, CliError> {
    match &args.command {
        MessageCommand::List => invoke(AgentMessagingRequest::ListAgents),
        MessageCommand::Send(args) => send(args, false),
        MessageCommand::Followup(args) => send(args, true),
        MessageCommand::Receive(args) => receive(args),
        MessageCommand::Reply(args) => reply(args),
        MessageCommand::Interrupt { target } => {
            // Reuse exact-recipient validation, without admitting a message.
            message_request(target.clone(), String::new(), false, None)?;
            invoke(AgentMessagingRequest::InterruptAgent {
                target: target.clone(),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn info_and_task_use_distinct_canonical_operations() {
        assert!(matches!(
            message_request("Peer".into(), "info".into(), false, None).unwrap(),
            AgentMessagingRequest::SendMessage { .. }
        ));
        assert!(matches!(
            message_request("Peer".into(), "task".into(), true, None).unwrap(),
            AgentMessagingRequest::FollowupTask { .. }
        ));
    }

    #[test]
    fn selectors_and_blank_targets_never_become_requests() {
        for target in [
            "",
            " ",
            "all",
            "ALL",
            "*",
            "broadcast",
            "class:Coder",
            "Peer ",
        ] {
            assert!(message_request(target.into(), "body".into(), true, None).is_err());
        }
    }
}
