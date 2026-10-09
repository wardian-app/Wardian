//! Render live agent lifecycle mutations after the desktop operation succeeds.
use crate::{control_error, errors::CliError, live};

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
