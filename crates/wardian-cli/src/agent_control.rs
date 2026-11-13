//! Render live agent lifecycle mutations after the desktop operation succeeds.
use crate::{control_error, errors::CliError, live};

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
