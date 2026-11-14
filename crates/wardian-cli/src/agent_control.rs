//! Render live agent control responses after the desktop operation succeeds.
use crate::{control_error, errors::CliError, live};
use wardian_core::control::{ArchiveAdoptionNativeIdentity, ArchiveAdoptionRequest};

pub(crate) fn models(provider: &str, force_refresh: bool) -> Result<String, CliError> {
    let catalog = live::agent_models(provider, force_refresh).map_err(control_error)?;
    serde_json::to_string(&catalog)
        .map(|json| format!("{json}\n"))
        .map_err(|error| CliError::generic(error.to_string()))
}

pub(crate) fn rename(target: &str, new_name: &str) -> Result<String, CliError> {
    let response = live::agent_rename(target, new_name).map_err(control_error)?;
    serde_json::to_string(&response)
        .map(|json| format!("{json}\n"))
        .map_err(|error| CliError::generic(error.to_string()))
}

/// Only the live desktop may authorize and audit this mutation. Never fall back
/// to editing leases when the endpoint is unavailable or predates the command.
pub(crate) fn release_hold(args: &crate::args::ReleaseHoldArgs) -> Result<String, CliError> {
    let response = live::agent_release_hold(
        &args.target,
        &args.acquisition_id,
        args.operator_verified_exit,
    )
    .map_err(control_error)?;
    serde_json::to_string(&response)
        .map(|json| format!("{json}\n"))
        .map_err(|error| CliError::generic(error.to_string()))
}

/// Forward recorded approval to the desktop's archive authority without an offline fallback.
pub(crate) fn adopt_archive(args: &crate::args::AdoptArchiveArgs) -> Result<String, CliError> {
    let response = live::agent_adopt_archive(&args.target, archive_adoption_request(args))
        .map_err(control_error)?;
    serde_json::to_string(&response)
        .map(|json| format!("{json}\n"))
        .map_err(|error| CliError::generic(error.to_string()))
}

fn archive_adoption_request(args: &crate::args::AdoptArchiveArgs) -> ArchiveAdoptionRequest {
    ArchiveAdoptionRequest {
        authorization_id: args.authorization_id.clone(),
        conversation_id: args.conversation_id.clone(),
        provider_session: args.provider_session.clone(),
        source_path: args.source_path.clone(),
        start_offset: args.start_offset,
        end_offset: args.end_offset,
        native_identity: ArchiveAdoptionNativeIdentity {
            platform: args.native_platform.clone(),
            primary: args.native_primary,
            secondary: args.native_secondary,
        },
        approval_reference: args.approval_reference.clone(),
        approval_text: args.approval_text.clone(),
    }
}

pub(crate) fn restart(target: &str) -> Result<String, CliError> {
    live::agent_restart(target).map_err(control_error)?;
    Ok(format!(
        "{}\n",
        serde_json::to_string(&serde_json::json!({"schema":1,"ok":true,"target":target,"preserved":["agent","habitat","session_history"]}))
            .unwrap()
    ))
}

/// Start the desktop New Session lifecycle; do not substitute resume on failure.
pub(crate) fn new_session(target: &str) -> Result<String, CliError> {
    live::agent_new_session(target).map_err(control_error)?;
    Ok(format!(
        "{}\n",
        serde_json::to_string(&serde_json::json!({"schema":1,"ok":true,"target":target,"preserved":["agent","habitat","session_history"]}))
            .unwrap()
    ))
}

pub(crate) fn pause(target: &str) -> Result<String, CliError> {
    live::agent_pause(target).map_err(control_error)?;
    Ok(format!(
        "{}\n",
        serde_json::to_string(&serde_json::json!({"schema":1,"ok":true,"target":target})).unwrap()
    ))
}

pub(crate) fn resume(target: &str) -> Result<String, CliError> {
    live::agent_resume(target).map_err(control_error)?;
    Ok(format!(
        "{}\n",
        serde_json::to_string(&serde_json::json!({"schema":1,"ok":true,"target":target})).unwrap()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::args::{AgentCommand, Cli, Command};
    use clap::Parser;

    #[test]
    fn archive_adoption_arguments_preserve_exact_request_on_retry() {
        let cli = Cli::try_parse_from([
            "wardian",
            "agent",
            "adopt-archive",
            "coder-a1",
            "--authorization-id",
            "a5f6ad8a-0ac5-4ec2-99e0-b856663594a8",
            "--conversation-id",
            "conversation-1",
            "--provider-session",
            "89b85617-bcad-4d38-8329-67d3297d7c94",
            "--source-path",
            "/archive/session with spaces.jsonl",
            "--start-offset",
            "123",
            "--end-offset",
            "456",
            "--native-platform",
            "unix",
            "--native-primary",
            "18446744073709551615",
            "--native-secondary",
            "17",
            "--approval-reference",
            "recorded-user-approval-1",
            "--approval-text",
            "I approve exactly this range.\nKeep this text verbatim.",
        ])
        .unwrap();
        let Command::Agent(args) = cli.command else {
            panic!("expected agent command")
        };
        let Some(AgentCommand::AdoptArchive(args)) = args.command else {
            panic!("expected archive adoption")
        };
        let request = archive_adoption_request(&args);
        let expected = serde_json::json!({
            "authorization_id": "a5f6ad8a-0ac5-4ec2-99e0-b856663594a8",
            "conversation_id": "conversation-1",
            "provider_session": "89b85617-bcad-4d38-8329-67d3297d7c94",
            "source_path": "/archive/session with spaces.jsonl", "start_offset": 123, "end_offset": 456,
            "native_identity": {"platform": "unix", "primary": u64::MAX, "secondary": 17},
            "approval_reference": "recorded-user-approval-1",
            "approval_text": "I approve exactly this range.\nKeep this text verbatim."
        });
        assert_eq!(serde_json::to_value(&request).unwrap(), expected);
        assert_eq!(archive_adoption_request(&args), request);
        assert_eq!(
            serde_json::from_value::<ArchiveAdoptionRequest>(expected).unwrap(),
            request
        );
    }
}
