use crate::models::AgentTelemetry;
use crate::state::AppState;
use crate::utils::fs::get_wardian_home;
use std::collections::HashMap;

use super::claude::{claude_is_real_user_query, claude_project_dir_name, claude_status_from_log};
use super::codex::{
    codex_log_lookup_session_id, codex_session_file_path, codex_status_from_log,
};
use super::opencode::{
    apply_opencode_log_metrics, opencode_log_dirs, opencode_log_path_in,
    opencode_session_diff_path, opencode_should_fallback_to_idle,
};
use super::display_log_path;
pub async fn get_all_metrics(state: &AppState) -> Vec<AgentTelemetry> {
    struct AgentSnapshot {
        session_id: String,
        provider: String,
        folder: String,
        resume_session: Option<String>,
        process_id: Option<u32>,
        query_count: std::sync::Arc<std::sync::Mutex<usize>>,
        init_timestamp: std::sync::Arc<std::sync::Mutex<Option<String>>>,
        current_status: std::sync::Arc<std::sync::Mutex<String>>,
        last_output_at: std::sync::Arc<std::sync::Mutex<Option<std::time::SystemTime>>>,
        log_path: std::sync::Arc<std::sync::Mutex<Option<std::path::PathBuf>>>,
        log_last_modified: std::sync::Arc<std::sync::Mutex<Option<std::time::SystemTime>>>,
    }

    let snapshots: Vec<AgentSnapshot> = {
        let agents = state.agents.lock().await;
        agents
            .iter()
            .map(|(sid, agent)| {
                let config = agent.config.lock().unwrap();
                AgentSnapshot {
                    session_id: sid.clone(),
                    provider: config.provider.clone(),
                    folder: config.folder.clone(),
                    resume_session: config.resume_session.clone(),
                    process_id: agent.process_id,
                    query_count: agent.query_count.clone(),
                    init_timestamp: agent.init_timestamp.clone(),
                    current_status: agent.current_status.clone(),
                    last_output_at: agent.last_output_at.clone(),
                    log_path: agent.log_path.clone(),
                    log_last_modified: agent.log_last_modified.clone(),
                }
            })
            .collect()
    };

    let sys_metrics = state.system_metrics.clone();
    tokio::task::spawn_blocking(move || {
        let mut results = Vec::new();
        let mut sys = sys_metrics.blocking_lock();
        sys.refresh_all();

        let mut children_map: HashMap<sysinfo::Pid, Vec<sysinfo::Pid>> = HashMap::new();
        for (pid, process) in sys.processes() {
            if let Some(parent) = process.parent() {
                children_map.entry(parent).or_default().push(*pid);
            }
        }

        for snap in &snapshots {
            let mut cpu = 0.0;
            let mut mem = 0.0;
            let mut uptime = 0;

            if let Some(pid) = snap.process_id {
                let root_pid = sysinfo::Pid::from_u32(pid);
                fn sum_tree(
                    pid: sysinfo::Pid,
                    sys: &sysinfo::System,
                    cmap: &HashMap<sysinfo::Pid, Vec<sysinfo::Pid>>,
                    cpu: &mut f32,
                    mem: &mut f64,
                    uptime: &mut u64,
                ) {
                    if let Some(p) = sys.process(pid) {
                        *cpu += p.cpu_usage();
                        *mem += p.memory() as f64 / 1_048_576.0;
                        *uptime = std::cmp::max(*uptime, p.run_time());
                    }
                    if let Some(children) = cmap.get(&pid) {
                        for &cpid in children {
                            sum_tree(cpid, sys, cmap, cpu, mem, uptime);
                        }
                    }
                }
                sum_tree(
                    root_pid,
                    &sys,
                    &children_map,
                    &mut cpu,
                    &mut mem,
                    &mut uptime,
                );

                // Phase 3: Uptime Alignment
                // If we have a 'Born' date, calculate total lifetime uptime while active.
                // Otherwise, fallback to the OS process runtime gathered above.
                if let Ok(born_lock) = snap.init_timestamp.lock() {
                    if let Some(ref born_str) = *born_lock {
                        if let Ok(born_dt) = chrono::DateTime::parse_from_rfc3339(born_str) {
                            let now = chrono::Utc::now();
                            let duration =
                                now.signed_duration_since(born_dt.with_timezone(&chrono::Utc));
                            let secs = duration.num_seconds();
                            if secs > 0 {
                                uptime = secs as u64;
                            }
                        }
                    }
                }
            }

            // Detect whether the agent process is still alive
            let process_alive = snap
                .process_id
                .map(|pid| sys.process(sysinfo::Pid::from_u32(pid)).is_some())
                .unwrap_or(false);

            let mut q_count = *snap.query_count.lock().unwrap();
            let mut i_ts = snap.init_timestamp.lock().unwrap().clone();
            let mut log_path_lock = snap.log_path.lock().unwrap_or_else(|e| e.into_inner());
            let opencode_session_id = snap
                .resume_session
                .as_deref()
                .filter(|value| value.starts_with("ses_"))
                .unwrap_or(&snap.session_id);

            // Provider-aware log discovery
            if snap.provider == "opencode" {
                let mut discovered_log = None;
                let log_dirs = opencode_log_dirs();
                for dir in &log_dirs {
                    if let Some(path) = opencode_log_path_in(dir, opencode_session_id) {
                        discovered_log = Some(path);
                        break;
                    }
                }
                *log_path_lock = discovered_log
                    .or_else(|| Some(opencode_session_diff_path(opencode_session_id)));
            } else if snap.provider == "claude" && snap.resume_session.is_some() {
                // For Claude, if we have a resume_session (Conversation ID), always re-verify
                // the path so it updates immediately after a Clear rotation.
                if let Some(home) = dirs::home_dir() {
                    let project_dir = claude_project_dir_name(&snap.folder);
                    let session_id_to_find = snap.resume_session.as_deref().unwrap();
                    let candidate = home
                        .join(".claude")
                        .join("projects")
                        .join(&project_dir)
                        .join(format!("{}.jsonl", session_id_to_find));
                    if candidate.exists() {
                        *log_path_lock = Some(candidate);
                    }
                }
            } else if log_path_lock.is_none() {
                match snap.provider.as_str() {
                    "codex" => {
                        let agent_home = get_wardian_home()
                            .map(|home| home.join("agents").join(&snap.session_id))
                            .filter(|path| path.exists())
                            .map(|path| path.to_string_lossy().to_string());
                        let codex_session_id = codex_log_lookup_session_id(
                            &snap.session_id,
                            snap.resume_session.as_deref(),
                        );
                        if let Some(path) =
                            codex_session_file_path(codex_session_id, agent_home.as_deref())
                        {
                            *log_path_lock = Some(path);
                        }
                    }
                    "claude" => {
                        // Fallback for initial spawn where resume_session might not be set yet
                        if let Some(home) = dirs::home_dir() {
                            let project_dir = claude_project_dir_name(&snap.folder);
                            let candidate = home
                                .join(".claude")
                                .join("projects")
                                .join(&project_dir)
                                .join(format!("{}.jsonl", snap.session_id));
                            if candidate.exists() {
                                *log_path_lock = Some(candidate);
                            }
                        }
                    }
                    _ => {
                        // Gemini: scan ~/.gemini/tmp for chat log files
                        if let Some(home) = dirs::home_dir() {
                            let tmp_dir = home.join(".gemini").join("tmp");
                            if let Ok(entries) = std::fs::read_dir(tmp_dir) {
                                for entry in entries.flatten() {
                                    let chat_dir = entry.path().join("chats");
                                    if let Ok(chat_files) = std::fs::read_dir(chat_dir) {
                                        for chat_file in chat_files.flatten() {
                                            if let Ok(content) =
                                                std::fs::read_to_string(chat_file.path())
                                            {
                                                if let Ok(p) =
                                                    serde_json::from_str::<serde_json::Value>(
                                                        &content,
                                                    )
                                                {
                                                    let target_id = snap
                                                        .resume_session
                                                        .as_deref()
                                                        .unwrap_or(&snap.session_id);
                                                    if p.get("sessionId").and_then(|v| v.as_str())
                                                        == Some(target_id)
                                                    {
                                                        *log_path_lock = Some(chat_file.path());
                                                        break;
                                                    }
                                                }
                                            }
                                        }
                                    }
                                    if log_path_lock.is_some() {
                                        break;
                                    }
                                }
                            }
                        }
                    }
                }
            }

            // Provider-aware log parsing for status/query enrichment
            if let Some(ref path) = *log_path_lock {
                let mut should_parse = true;
                let mut new_mtime = None;
                if let Ok(metadata) = std::fs::metadata(path) {
                    if let Ok(modified) = metadata.modified() {
                        let last_mod = *snap.log_last_modified.lock().unwrap();
                        if last_mod == Some(modified) {
                            should_parse = false;
                        } else {
                            new_mtime = Some(modified);
                        }
                    }
                }

                if should_parse {
                    if let Ok(content) = std::fs::read_to_string(path) {
                        if let Some(mtime) = new_mtime {
                            *snap.log_last_modified.lock().unwrap() = Some(mtime);
                        }
                        match snap.provider.as_str() {
                            "codex" => {
                                let lines: Vec<serde_json::Value> = content
                                    .lines()
                                    .filter_map(|l| serde_json::from_str(l).ok())
                                    .collect();

                                q_count = lines
                                    .iter()
                                    .filter(|l| {
                                        l.get("type").and_then(|v| v.as_str()) == Some("event_msg")
                                            && l.get("payload")
                                                .and_then(|v| v.get("type"))
                                                .and_then(|v| v.as_str())
                                                == Some("user_message")
                                    })
                                    .count();

                                if let Some(meta) = lines.iter().find(|l| {
                                    l.get("type").and_then(|v| v.as_str()) == Some("session_meta")
                                }) {
                                    if let Some(ts) = meta
                                        .get("payload")
                                        .and_then(|v| v.get("timestamp"))
                                        .and_then(|v| v.as_str())
                                    {
                                        i_ts = Some(ts.to_string());
                                    }
                                }

                                if let Some(status) = codex_status_from_log(&lines) {
                                    *snap.current_status.lock().unwrap() = status;
                                }
                            }
                            "claude" => {
                                // Claude logs are JSONL — one JSON object per line
                                let lines: Vec<serde_json::Value> = content
                                    .lines()
                                    .filter_map(|l| serde_json::from_str(l).ok())
                                    .collect();

                                q_count = lines
                                    .iter()
                                    .filter(|l| {
                                        l.get("type").and_then(|v| v.as_str()) == Some("user")
                                            && claude_is_real_user_query(l)
                                    })
                                    .count();

                                if let Some(first) = lines.first() {
                                    if let Some(ts) =
                                        first.get("timestamp").and_then(|v| v.as_str())
                                    {
                                        i_ts = Some(ts.to_string());
                                    } else if let Some(ts_num) =
                                        first.get("timestamp").and_then(|v| v.as_i64())
                                    {
                                        // Fallback if timestamp is an epoch number
                                        if let Some(dt) =
                                            chrono::DateTime::from_timestamp_millis(ts_num)
                                        {
                                            i_ts = Some(dt.to_rfc3339_opts(
                                                chrono::SecondsFormat::Millis,
                                                true,
                                            ));
                                        }
                                    }
                                }

                                let current_status_snap =
                                    snap.current_status.lock().unwrap().clone();
                                if !current_status_snap.starts_with("Action Required")
                                    && !current_status_snap.starts_with("Action Needed")
                                {
                                    if let Some(status) = claude_status_from_log(&lines) {
                                        *snap.current_status.lock().unwrap() = status;
                                    }
                                }
                            }
                            "opencode" => {
                                let mut status = snap.current_status.lock().unwrap().clone();
                                apply_opencode_log_metrics(
                                    &content,
                                    opencode_session_id,
                                    &mut q_count,
                                    &mut i_ts,
                                    &mut status,
                                );
                                *snap.current_status.lock().unwrap() = status;
                            }
                            _ => {
                                // Gemini logs are a single JSON object with a messages array
                                if let Ok(p) = serde_json::from_str::<serde_json::Value>(&content) {
                                    if let Some(msgs) = p.get("messages").and_then(|v| v.as_array())
                                    {
                                        q_count = msgs
                                            .iter()
                                            .filter(|m| {
                                                m.get("type").and_then(|v| v.as_str())
                                                    == Some("user")
                                                    || m.get("role").and_then(|v| v.as_str())
                                                        == Some("user")
                                            })
                                            .count();

                                        if let Some(last_msg) = msgs.last() {
                                            let msg_type = last_msg
                                                .get("type")
                                                .and_then(|v| v.as_str())
                                                .or_else(|| {
                                                    last_msg.get("role").and_then(|v| v.as_str())
                                                });
                                            if msg_type == Some("user") {
                                                *snap.current_status.lock().unwrap() =
                                                    "Processing...".to_string();
                                            } else if msg_type == Some("gemini")
                                                || msg_type == Some("assistant")
                                            {
                                                *snap.current_status.lock().unwrap() =
                                                    "Idle".to_string();
                                            }
                                        }
                                    }
                                    if let Some(st) = p.get("startTime").and_then(|v| v.as_str()) {
                                        i_ts = Some(st.to_string());
                                    }
                                }
                            }
                        }
                    }
                }
            }

            if q_count > 0 {
                *snap.query_count.lock().unwrap() = q_count;
            }
            if let Some(ts) = i_ts {
                *snap.init_timestamp.lock().unwrap() = Some(ts);
            }

            if snap.provider == "opencode" || snap.provider == "claude" {
                let current_status = snap.current_status.lock().unwrap().clone();
                let last_output_at = *snap.last_output_at.lock().unwrap();
                if opencode_should_fallback_to_idle(
                    &current_status,
                    last_output_at,
                    std::time::SystemTime::now(),
                ) {
                    *snap.current_status.lock().unwrap() = "Idle".to_string();
                }
            }

            // If the process has terminated, force status to "Off" so the UI
            // doesn't stay stuck on "Processing..." or "Action Needed".
            if !process_alive && snap.process_id.is_some() {
                *snap.current_status.lock().unwrap() = "Off".to_string();
            }

            results.push(AgentTelemetry {
                session_id: snap.session_id.clone(),
                cpu_usage: cpu,
                memory_mb: mem,
                uptime_seconds: uptime,
                query_count: *snap.query_count.lock().unwrap(),
                init_timestamp: snap.init_timestamp.lock().unwrap().clone(),
                current_status: snap.current_status.lock().unwrap().clone(),
                log_path: log_path_lock.as_ref().map(|p| display_log_path(p)),
            });
        }
        results
    })
    .await
    .unwrap_or_default()
}
