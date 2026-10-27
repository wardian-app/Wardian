//! Rebind saved-Off sources in the background owner, never in a display read.

use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::commands::provider_log_acquisition::{
    native_file_identity, published_anchor_matches, ProviderLogCaptureState,
};
use crate::state::AppState;
use wardian_core::models::AgentConfig;

const OWNERSHIP_HEADER_BYTES: u64 = 256 * 1024;

fn saved_session(config: &AgentConfig) -> Option<&str> {
    if !config.is_off || !matches!(config.provider.as_str(), "codex" | "claude") {
        return None;
    }
    let session = config.resume_session.as_deref()?.trim();
    uuid::Uuid::parse_str(session).ok()?;
    if config.provider == "codex"
        && config
            .codex_config()
            .cleared_provider_sessions
            .iter()
            .any(|id| id == session)
    {
        return None;
    }
    Some(session)
}

fn source_roots(config: &AgentConfig, user_home: Option<&Path>) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if config.provider == "codex" {
        if let Some(agent_dir) = wardian_core::paths::agent_conversations_dir(&config.session_id)
            .and_then(|dir| dir.parent().map(Path::to_path_buf))
        {
            let projected = agent_dir.join("habitat").join(".codex");
            roots.push(projected.join("sessions"));
            roots.push(projected.join("archived_sessions"));
        }
        if let Some(home) = user_home {
            roots.push(home.join(".codex").join("sessions"));
            roots.push(home.join(".codex").join("archived_sessions"));
        }
    } else if config.provider == "claude" {
        let workspace = config
            .git_worktree_folder
            .as_deref()
            .unwrap_or(&config.folder);
        if let Some(home) = user_home {
            roots.push(
                home.join(".claude")
                    .join("projects")
                    .join(crate::manager::claude::claude_project_dir_name(workspace)),
            );
        }
    }
    roots
}

fn filename_owned(path: &Path, provider: &str, session: &str) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    if provider == "claude" {
        return name.strip_suffix(".jsonl") == Some(session);
    }
    name.strip_prefix("rollout-")
        .and_then(|name| name.strip_suffix(".jsonl"))
        .filter(|stem| stem.get(19..20) == Some("-"))
        .and_then(|stem| stem.get(20..))
        .and_then(|identity| identity.split('_').next())
        == Some(session)
}

/// Validate only the recorded source, bounded ownership header and continuity
/// proof. No directory search, history acquisition, policy/index publication or
/// provider process is needed. Later readers still revalidate after this open.
fn validated_source(
    config: &AgentConfig,
    captured: &ProviderLogCaptureState,
    roots: &[PathBuf],
) -> Option<PathBuf> {
    let session = saved_session(config)?;
    if captured.provider_source_key != format!("{}:session:{session}", config.provider)
        || !matches!(captured.status.as_str(), "pending" | "complete")
        || !Path::new(&captured.path).is_absolute()
    {
        return None;
    }
    let path = std::fs::canonicalize(&captured.path).ok()?;
    if !roots
        .iter()
        .filter_map(|root| root.canonicalize().ok())
        .any(|root| {
            if config.provider == "claude" {
                path.parent() == Some(root.as_path())
            } else {
                path.starts_with(&root)
            }
        })
        || !filename_owned(&path, &config.provider, session)
    {
        return None;
    }
    let mut file = std::fs::File::open(&path).ok()?;
    if file.metadata().ok()?.len() < captured.committed_offset
        || native_file_identity(&file).ok()? != captured.native_identity
        || !published_anchor_matches(&mut file, &captured.continuity_anchor).ok()?
    {
        return None;
    }
    file.seek(SeekFrom::Start(0)).ok()?;
    let mut header = Vec::new();
    BufReader::new(file.take(OWNERSHIP_HEADER_BYTES + 1))
        .read_until(b'\n', &mut header)
        .ok()?;
    if header.len() > OWNERSHIP_HEADER_BYTES as usize {
        return None;
    }
    let value: serde_json::Value = serde_json::from_slice(&header).ok()?;
    let owned = if config.provider == "codex" {
        value["type"] == "session_meta" && value["payload"]["id"].as_str() == Some(session)
    } else {
        // Claude can start with a file-history snapshot without sessionId.
        // Its exact filename, project root and persisted native proof own it;
        // an explicit conflicting sessionId still fails closed.
        value
            .get("sessionId")
            .is_none_or(|id| id.as_str() == Some(session))
    };
    owned.then_some(path)
}

fn same_binding(current: &AgentConfig, expected: &AgentConfig) -> bool {
    current.session_id == expected.session_id
        && current.provider == expected.provider
        && saved_session(current) == saved_session(expected)
        && current.is_off
        && current.folder == expected.folder
        && current.git_worktree_folder == expected.git_worktree_folder
}

pub(super) async fn bind_for_background(
    state: &AppState,
    session_id: &str,
    incarnation: Option<&Arc<Mutex<String>>>,
) -> Result<(), String> {
    bind_for_background_in(state, session_id, incarnation, dirs::home_dir()).await
}

async fn bind_for_background_in(
    state: &AppState,
    session_id: &str,
    incarnation: Option<&Arc<Mutex<String>>>,
    user_home: Option<PathBuf>,
) -> Result<(), String> {
    let (config_ref, path_ref, current_status) = {
        let agents = state.agents.lock().await;
        let Some(agent) = agents.get(session_id) else {
            return Ok(());
        };
        if agent.child_process.is_some()
            || agent.process_id.is_some()
            || incarnation.is_some_and(|expected| !Arc::ptr_eq(expected, &agent.current_status))
        {
            return Ok(());
        }
        (
            agent.config.clone(),
            agent.log_path.clone(),
            agent.current_status.clone(),
        )
    };
    let expected = config_ref
        .lock()
        .map_err(|_| "restored config unavailable")?
        .clone();
    let Some(session) = saved_session(&expected) else {
        return Ok(());
    };
    if expected.session_id != session_id
        || path_ref
            .lock()
            .map_err(|_| "restored source unavailable")?
            .is_some()
    {
        return Ok(());
    }
    let key = format!("{}:session:{session}", expected.provider);
    let Some(captured) = state
        .conversation_archive
        .provider_log_capture_state(session_id, &key)
        .map_err(|_| "restored source policy unavailable")?
    else {
        return Ok(());
    };
    let roots = source_roots(&expected, user_home.as_deref());
    let validation_config = expected.clone();
    let candidate = tokio::task::spawn_blocking(move || {
        validated_source(&validation_config, &captured, &roots)
    })
    .await
    .map_err(|_| "restored source validation unavailable")?;
    let Some(candidate) = candidate else {
        return Ok(());
    };
    // Release the roster before taking per-agent locks; an incarnation retired
    // during I/O must not publish into its successor. A later source replacement
    // remains fail-closed at the existing capture/read identity checks.
    {
        let agents = state.agents.lock().await;
        let Some(current) = agents.get(session_id) else {
            return Ok(());
        };
        if current.child_process.is_some()
            || current.process_id.is_some()
            || !Arc::ptr_eq(&current.current_status, &current_status)
            || !Arc::ptr_eq(&current.config, &config_ref)
            || !Arc::ptr_eq(&current.log_path, &path_ref)
        {
            return Ok(());
        }
    }
    let current = config_ref
        .lock()
        .map_err(|_| "restored config unavailable")?;
    if same_binding(&current, &expected) {
        let mut cached = path_ref.lock().map_err(|_| "restored source unavailable")?;
        if cached.is_none() {
            *cached = Some(candidate);
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "chat_restore_source_tests.rs"]
mod tests;
