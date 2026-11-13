//! Explicit operator fallback for legacy holds whose launch evidence was lost.
use super::{ok_json, resolve_target_uuid, ControlError};
use crate::state::AppState;
use serde::{Deserialize, Serialize};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
use tauri::{AppHandle, Manager};
use wardian_core::control::{
    AgentReleaseHoldResponse, ControlRequest, MessageOrigin, CONTROL_SCHEMA,
};
use wardian_core::conversation_lease::{
    load_leases_checked, release_previous_provider_hold_exact, ConversationLease,
};
use wardian_core::models::AgentConfig;

const MAX_RECORD_BYTES: usize = 64 * 1024;

#[derive(Debug, Serialize, Deserialize)]
struct Authorization {
    schema: u8,
    event: String,
    authorization_id: String,
    authorized_at: String,
    operator_os_identity: String,
    managed_actor: Option<MessageOrigin>,
    operator_verified_exit: bool,
    // This record precedes the lease write. It never asserts completed release.
    release_outcome: String,
    hold: ConversationLease,
}

pub(super) async fn dispatch(
    request: ControlRequest,
    app: &AppHandle,
) -> Result<String, ControlError> {
    let ControlRequest::AgentReleaseHold {
        target,
        acquisition_id,
        operator_verified_exit,
        origin,
    } = request
    else {
        return Err(ControlError::bad_request("expected agent_release_hold"));
    };
    let agent_id = resolve_target_uuid(app, &target)
        .await
        .ok_or_else(|| ControlError::not_found(format!("agent not found: {target}")))?;
    let result = release_for_state(
        &app.state::<AppState>(),
        &agent_id,
        acquisition_id,
        operator_verified_exit,
        origin,
        provider_candidates,
        operator_os_identity,
    )
    .await?;
    ok_json(&result)
}

/// Keep lifecycle exclusion inside the blocking owner: cancelling an IPC waiter
/// must not permit another lifecycle operation between audit and lease release.
async fn release_for_state(
    state: &AppState,
    agent_id: &str,
    acquisition_id: String,
    operator_verified_exit: bool,
    origin: Option<MessageOrigin>,
    scan: fn(&AgentConfig, &ConversationLease) -> Vec<u32>,
    identify: fn() -> Result<String, String>,
) -> Result<AgentReleaseHoldResponse, ControlError> {
    if !operator_verified_exit
        || acquisition_id.trim().is_empty()
        || acquisition_id.trim() != acquisition_id
    {
        return Err(ControlError::bad_request(
            "exact acquisition_id and operator_verified_exit=true are required",
        ));
    }
    let lifecycle = state.lock_agent_lifecycle(agent_id).await;
    let config = {
        let agents = state.agents.lock().await;
        let agent = agents
            .get(agent_id)
            .ok_or_else(|| ControlError::not_found("agent not found"))?;
        if agent.process_id.is_some()
            || agent.runtime_generation.is_some()
            || agent.child_process.is_some()
            || !agent.background_processes.is_empty()
        {
            return Err(ControlError::request_failed(
                "owned runtime still exists; hold retained",
            ));
        }
        agent.config.clone()
    };
    let config = config
        .lock()
        .map_err(|_| ControlError::request_failed("agent configuration lock poisoned"))?
        .clone();
    if config.session_id != agent_id {
        return Err(ControlError::request_failed(
            "agent configuration identity changed",
        ));
    }
    let home = crate::utils::fs::get_wardian_home()
        .ok_or_else(|| ControlError::request_failed("Could not locate Wardian home"))?;
    if crate::manager::codex_stop::has_retained_stop(&home, agent_id)
        .map_err(ControlError::request_failed)?
    {
        return Err(ControlError::request_failed(
            "retained stop still owns exact runtime evidence; use normal lifecycle recovery",
        ));
    }
    tokio::task::spawn_blocking(move || {
        let _lifecycle = lifecycle;
        release_for_config(&home, &config, &acquisition_id, origin, scan, identify)
    })
    .await
    .map_err(ControlError::request_failed)?
    .map_err(ControlError::request_failed)
}

fn release_for_config(
    home: &Path,
    config: &AgentConfig,
    acquisition_id: &str,
    origin: Option<MessageOrigin>,
    scan: fn(&AgentConfig, &ConversationLease) -> Vec<u32>,
    identify: fn() -> Result<String, String>,
) -> Result<AgentReleaseHoldResponse, String> {
    let holds: Vec<_> = load_leases_checked()?
        .into_iter()
        .filter(|hold| {
            hold.agent_id == config.session_id
                && hold.acquisition_id == acquisition_id
                && hold.owner_kind == "prior_provider_hold"
                && hold.mode == "uncertain_previous_provider"
        })
        .collect();
    if holds.len() != 1 {
        return Err("exact prior-provider hold acquisition for this agent was not found".into());
    }
    let hold = &holds[0];
    let authorization_id = uuid::Uuid::new_v4().to_string();
    release_previous_provider_hold_exact(hold, |exact| {
        if !scan(config, exact).is_empty() {
            return Err("a tied live-provider candidate exists; hold retained".into());
        }
        let operator_os_identity = identify()?;
        if operator_os_identity.trim().is_empty() {
            return Err("operator OS identity is unavailable; hold retained".into());
        }
        append_authorization(
            home,
            &Authorization {
                schema: 1,
                event: "prior_provider_hold_release_authorized".into(),
                authorization_id: authorization_id.clone(),
                authorized_at: chrono::Utc::now().to_rfc3339(),
                operator_os_identity,
                managed_actor: origin,
                operator_verified_exit: true,
                release_outcome: "authorization_only".into(),
                hold: exact.clone(),
            },
        )
    })?;
    Ok(AgentReleaseHoldResponse {
        schema: CONTROL_SCHEMA,
        ok: true,
        agent_id: config.session_id.clone(),
        provider: hold.provider.clone(),
        resume_session: hold.resume_session.clone(),
        acquisition_id: acquisition_id.to_string(),
        authorization_id,
    })
}

fn candidate_identities(config: &AgentConfig, hold: &ConversationLease) -> Vec<(String, String)> {
    let mut identities = vec![
        (hold.agent_id.clone(), hold.provider.clone()),
        (hold.resume_session.clone(), hold.provider.clone()),
        (config.session_id.clone(), config.provider.clone()),
    ];
    for session in [
        config.resume_session.as_ref(),
        config.fresh_provider_session_id.as_ref(),
    ]
    .into_iter()
    .flatten()
    {
        if session != &config.session_id {
            identities.push((session.clone(), config.provider.clone()));
        }
    }
    identities.sort();
    identities.dedup();
    identities
}

fn provider_candidates(config: &AgentConfig, hold: &ConversationLease) -> Vec<u32> {
    // Scan separately per provider: the existing batch API keys results only
    // by session identity, which may be shared by the old and current provider.
    candidate_identities(config, hold)
        .into_iter()
        .flat_map(|(identity, provider)| {
            crate::utils::process::find_wardian_provider_process_candidates(
                &identity,
                &provider,
                Some(std::process::id()),
            )
        })
        .collect()
}

/// The endpoint belongs to this OS user; query the inherited OS token rather
/// than accepting a username supplied by the client or USER/USERNAME variables.
fn operator_os_identity() -> Result<String, String> {
    #[cfg(windows)]
    {
        // EXTENDED_NAME_FORMAT (NameSamCompatible), LPWSTR, PULONG; BOOLEAN
        // return. This security API is not exposed by our enabled winapi
        // modules. Keep its published ABI and avoid PATH-resolved commands.
        #[link(name = "secur32")]
        extern "system" {
            fn GetUserNameExW(format: i32, buffer: *mut u16, length: *mut u32) -> u8;
        }
        let mut name = vec![0u16; 32768];
        let mut length = name.len() as u32;
        // SAFETY: the buffer owns `length` UTF-16 units; length is a live output
        // pointer. NameSamCompatible returns the OS DOMAIN\account principal.
        if unsafe { GetUserNameExW(2, name.as_mut_ptr(), &mut length) } == 0 {
            return Err(format!(
                "operator OS identity is unavailable; hold retained: {}",
                std::io::Error::last_os_error()
            ));
        }
        let end = name
            .iter()
            .position(|unit| *unit == 0)
            .ok_or("operator OS identity is not terminated")?;
        let account = String::from_utf16(&name[..end])
            .map_err(|_| "operator OS identity is invalid UTF-16")?;
        if account.trim().is_empty() {
            return Err("operator OS identity is empty".into());
        }
        Ok(format!("windows-account:{account}"))
    }
    #[cfg(unix)]
    {
        // SAFETY: geteuid has no preconditions and reads the kernel principal.
        let uid = unsafe { libc::geteuid() };
        Ok(format!("unix-uid:{uid}"))
    }
}

/// Validate only the final record using bounded I/O, regardless of accumulated
/// history size. One extra byte admits the preceding newline at the record cap.
fn validate_history_tail(file: &mut (impl Read + Seek)) -> Result<(), String> {
    let mut inspect = || -> Result<(), String> {
        let length = file
            .seek(SeekFrom::End(0))
            .map_err(|error| error.to_string())?;
        if length == 0 {
            return Ok(());
        }
        let tail_length = length.min((MAX_RECORD_BYTES + 1) as u64);
        let offset = length - tail_length;
        file.seek(SeekFrom::Start(offset))
            .map_err(|error| error.to_string())?;
        let mut tail = vec![0; tail_length as usize];
        file.read_exact(&mut tail)
            .map_err(|error| error.to_string())?;
        if !tail.ends_with(b"\n") {
            return Err("incomplete final record".into());
        }
        let terminated = &tail[..tail.len() - 1];
        let start = match terminated.iter().rposition(|byte| *byte == b'\n') {
            Some(index) => index + 1,
            None if offset == 0 => 0,
            None => return Err("final record exceeds inspection bound".into()),
        };
        let record = &terminated[start..];
        if record.len() + 1 > MAX_RECORD_BYTES {
            return Err("final record exceeds inspection bound".into());
        }
        serde_json::from_slice::<serde_json::Value>(record)
            .map_err(|error| format!("malformed final record: {error}"))?;
        if file
            .seek(SeekFrom::End(0))
            .map_err(|error| error.to_string())?
            != length
        {
            return Err("history changed during tail validation".into());
        }
        Ok(())
    };
    inspect()
        .map_err(|error| format!("cannot validate lifecycle history tail; hold retained: {error}"))
}

/// Preserve all prior bytes. Only the tail and new record are bounded; total
/// history size never requires truncation or archival to authorize release.
fn append_authorization(home: &Path, authorization: &Authorization) -> Result<(), String> {
    let path = home
        .join("agents")
        .join(&authorization.hold.agent_id)
        .join("lifecycle-history.jsonl");
    let mut body = serde_json::to_vec(authorization).map_err(|error| error.to_string())?;
    body.push(b'\n');
    if body.len() > MAX_RECORD_BYTES {
        return Err("lifecycle authorization record exceeds size limit".into());
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .append(true)
        .open(&path)
        .map_err(|error| format!("cannot open lifecycle history; hold retained: {error}"))?;
    validate_history_tail(&mut file)?;
    file.write_all(&body)
        .and_then(|_| file.sync_all())
        .map_err(|error| format!("cannot sync lifecycle authorization; hold retained: {error}"))?;
    #[cfg(unix)]
    std::fs::File::open(path.parent().ok_or("history parent is missing")?)
        .and_then(|parent| parent.sync_all())
        .map_err(|error| {
            format!("cannot sync lifecycle history directory; hold retained: {error}")
        })?;
    Ok(())
}

#[cfg(test)]
mod tests;
