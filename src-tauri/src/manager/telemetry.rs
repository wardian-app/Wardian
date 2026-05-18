use crate::state::AppState;
use crate::utils::fs::get_wardian_home;
use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex};
use wardian_core::models::{AgentTelemetry, AppTelemetry};

use crate::providers::transcript::extract_transcript_message;

use super::claude::{claude_is_real_user_query, claude_project_dir_name, claude_status_from_log};
use super::codex::{
    codex_log_lookup_session_id, codex_session_file_path, codex_status_from_log,
    latest_codex_session_index_entry,
};
use super::display_log_path;
use super::opencode::{
    apply_opencode_log_metrics, opencode_extract_created_session_id, opencode_last_assistant_text,
    opencode_log_dirs, opencode_log_path_after, opencode_log_path_in, opencode_session_diff_path,
    opencode_should_fallback_to_idle,
};

fn normalize_cpu_usage(raw_cpu_usage: f32, logical_cpu_count: usize) -> f32 {
    let divisor = logical_cpu_count.max(1) as f32;
    (raw_cpu_usage / divisor).clamp(0.0, 100.0)
}

fn bytes_to_mib(bytes: u64) -> f64 {
    bytes as f64 / 1_048_576.0
}

#[derive(Debug, PartialEq, Eq)]
struct GeminiLogMetrics {
    query_count: usize,
    init_timestamp: Option<String>,
    status: Option<&'static str>,
}

fn gemini_message_kind(value: &serde_json::Value) -> Option<&str> {
    value
        .get("type")
        .and_then(|v| v.as_str())
        .or_else(|| value.get("role").and_then(|v| v.as_str()))
}

fn gemini_status_from_last_kind(kind: Option<&str>) -> Option<&'static str> {
    match kind {
        Some("user") => Some("Processing..."),
        Some("gemini") | Some("assistant") | Some("model") => Some("Idle"),
        _ => None,
    }
}

fn gemini_jsonl_completed_message(value: &serde_json::Value) -> bool {
    value.get("tokens").is_some()
        || value.get("usage").is_some()
        || value.get("finishReason").is_some()
        || value.get("finish_reason").is_some()
}

fn gemini_jsonl_record_status(value: &serde_json::Value) -> Option<&'static str> {
    match gemini_message_kind(value) {
        Some("user") => Some("Processing..."),
        Some("result") => Some("Idle"),
        Some("gemini") | Some("assistant") | Some("model")
            if gemini_jsonl_completed_message(value) =>
        {
            Some("Idle")
        }
        _ => None,
    }
}

fn gemini_log_matches_session(content: &str, target_id: &str) -> bool {
    let target_id = target_id.trim();
    if target_id.is_empty() {
        return false;
    }

    if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(content) {
        if parsed.get("sessionId").and_then(|v| v.as_str()) == Some(target_id) {
            return true;
        }
    }

    content.lines().any(|line| {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            return false;
        }
        serde_json::from_str::<serde_json::Value>(trimmed)
            .ok()
            .is_some_and(|value| value.get("sessionId").and_then(|v| v.as_str()) == Some(target_id))
    })
}

fn parse_gemini_log_metrics(content: &str) -> Option<GeminiLogMetrics> {
    if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(content) {
        if let Some(messages) = parsed.get("messages").and_then(|v| v.as_array()) {
            let query_count = messages
                .iter()
                .filter(|message| gemini_message_kind(message) == Some("user"))
                .count();
            let status =
                gemini_status_from_last_kind(messages.last().and_then(gemini_message_kind));
            return Some(GeminiLogMetrics {
                query_count,
                init_timestamp: parsed
                    .get("startTime")
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
                status,
            });
        }
    }

    let mut query_count = 0usize;
    let mut init_timestamp = None;
    let mut status = None;
    let mut saw_gemini_record = false;

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(record) = serde_json::from_str::<serde_json::Value>(trimmed) else {
            continue;
        };

        if init_timestamp.is_none() {
            init_timestamp = record
                .get("startTime")
                .and_then(|v| v.as_str())
                .map(str::to_string);
        }

        if let Some(kind) = gemini_message_kind(&record) {
            match kind {
                "user" => {
                    query_count += 1;
                    status = Some("Processing...");
                    saw_gemini_record = true;
                }
                "gemini" | "assistant" | "model" | "result" => {
                    if let Some(record_status) = gemini_jsonl_record_status(&record) {
                        status = Some(record_status);
                    }
                    saw_gemini_record = true;
                }
                _ => {}
            }
        }
    }

    if !saw_gemini_record && init_timestamp.is_none() {
        return None;
    }

    Some(GeminiLogMetrics {
        query_count,
        init_timestamp,
        status,
    })
}

struct AgentSnapshot {
    session_id: String,
    provider: String,
    folder: String,
    resume_session: Option<String>,
    process_id: Option<u32>,
    query_count: Arc<Mutex<usize>>,
    init_timestamp: Arc<Mutex<Option<String>>>,
    current_status: Arc<Mutex<String>>,
    last_status_at: Arc<Mutex<Option<String>>>,
    watch_state: Arc<Mutex<crate::state::AgentWatchState>>,
    last_output_at: Arc<Mutex<Option<std::time::SystemTime>>>,
    log_path: Arc<Mutex<Option<std::path::PathBuf>>>,
    log_last_modified: Arc<Mutex<Option<std::time::SystemTime>>>,
}

fn set_snapshot_status(snap: &AgentSnapshot, next_status: &str) {
    let mut status = snap.current_status.lock().unwrap();
    if *status == next_status {
        return;
    }
    *status = next_status.to_string();
    drop(status);

    let observed_at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let _ = wardian_core::db::update_agent_status(&snap.session_id, next_status, None);
    if let Ok(mut last_status_at) = snap.last_status_at.lock() {
        *last_status_at = Some(observed_at.clone());
    }
    if let Ok(mut watch_state) = snap.watch_state.lock() {
        watch_state.push_event(
            "status",
            serde_json::json!({
                "status": wardian_core::identity::normalize_status(next_status),
                "observed_at": observed_at,
            }),
        );
    }
}

fn set_snapshot_status_from_log(snap: &AgentSnapshot, next_status: &str, is_initial_replay: bool) {
    if is_initial_replay {
        return;
    }
    set_snapshot_status(snap, next_status);
}

fn record_opencode_assistant_text(snap: &AgentSnapshot, session_id: &str, text: &str) {
    let text = text.trim();
    if text.is_empty() {
        return;
    }

    if let Ok(mut watch_state) = snap.watch_state.lock() {
        let latest = watch_state
            .snapshot_since(None, Some(4096))
            .ok()
            .map(|snapshot| snapshot.transcript.latest_text)
            .unwrap_or_default();
        if latest == text {
            return;
        }
        watch_state.push_output(format!("{text}\r\n").as_bytes());
        watch_state.push_transcript(wardian_core::control::WatchTranscriptMessage {
            role: "assistant".to_string(),
            text: text.to_string(),
            provider: "opencode".to_string(),
            turn_id: Some(session_id.to_string()),
            source: Some("opencode_db".to_string()),
        });
    }

    if let Ok(mut stamp) = snap.last_output_at.lock() {
        *stamp = Some(std::time::SystemTime::now());
    }
}

fn record_latest_opencode_assistant_text(snap: &AgentSnapshot, session_id: &str) {
    match opencode_last_assistant_text(session_id) {
        Ok(Some(text)) => record_opencode_assistant_text(snap, session_id, &text),
        Ok(None) => {}
        Err(error) => crate::utils::logging::log_debug(&format!(
            "[Wardian] Failed to read OpenCode assistant text for {session_id}: {error}"
        )),
    }
}

fn latest_gemini_assistant_message(
    content: &str,
) -> Option<wardian_core::control::WatchTranscriptMessage> {
    if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(content) {
        if let Some(messages) = parsed.get("messages").and_then(|value| value.as_array()) {
            return messages
                .iter()
                .rev()
                .find_map(|message| extract_transcript_message("gemini", &message.to_string()));
        }
    }

    content
        .lines()
        .rev()
        .filter_map(|line| {
            let trimmed = line.trim();
            (!trimmed.is_empty()).then_some(trimmed)
        })
        .find_map(|line| extract_transcript_message("gemini", line))
}

fn record_latest_gemini_assistant_text(snap: &AgentSnapshot, content: &str) {
    let Some(message) = latest_gemini_assistant_message(content) else {
        return;
    };

    if let Ok(mut watch_state) = snap.watch_state.lock() {
        let latest = watch_state
            .snapshot_since(None, Some(4096))
            .ok()
            .and_then(|snapshot| snapshot.transcript.messages.last().cloned());
        if latest.as_ref().is_some_and(|latest| {
            latest.provider == message.provider
                && latest.turn_id == message.turn_id
                && latest.text == message.text
        }) {
            return;
        }
        watch_state.push_transcript(message);
    }

    if let Ok(mut stamp) = snap.last_output_at.lock() {
        *stamp = Some(std::time::SystemTime::now());
    }
}

fn timestamp_to_system_time(timestamp: Option<&str>) -> Option<std::time::SystemTime> {
    let timestamp = timestamp?;
    let parsed = chrono::DateTime::parse_from_rfc3339(timestamp).ok()?;
    let millis = parsed.timestamp_millis();
    if millis < 0 {
        return None;
    }
    Some(std::time::UNIX_EPOCH + std::time::Duration::from_millis(millis as u64))
}

fn collect_descendant_pids(
    pid: u32,
    children_map: &HashMap<u32, Vec<u32>>,
    related_pids: &mut BTreeSet<u32>,
) {
    if !related_pids.insert(pid) {
        return;
    }

    if let Some(children) = children_map.get(&pid) {
        for &child_pid in children {
            collect_descendant_pids(child_pid, children_map, related_pids);
        }
    }
}

fn collect_related_pids(
    primary_pid: Option<u32>,
    discovered_roots: &[u32],
    children_map: &HashMap<u32, Vec<u32>>,
) -> BTreeSet<u32> {
    let mut related_pids = BTreeSet::new();

    if let Some(pid) = primary_pid {
        collect_descendant_pids(pid, children_map, &mut related_pids);
    }

    for &pid in discovered_roots {
        collect_descendant_pids(pid, children_map, &mut related_pids);
    }

    related_pids
}

fn collect_app_process_pids(
    app_pid: u32,
    excluded_roots: &[u32],
    children_map: &HashMap<u32, Vec<u32>>,
) -> BTreeSet<u32> {
    let mut app_pids = collect_related_pids(Some(app_pid), &[], children_map);
    let excluded_pids = collect_related_pids(None, excluded_roots, children_map);

    for pid in excluded_pids {
        app_pids.remove(&pid);
    }

    app_pids
}

pub async fn get_all_metrics(state: &AppState) -> Vec<AgentTelemetry> {
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
                    last_status_at: agent.last_status_at.clone(),
                    watch_state: agent.watch_state.clone(),
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
        let logical_cpu_count = sys.cpus().len();

        let mut children_map: HashMap<u32, Vec<u32>> = HashMap::new();
        for (pid, process) in sys.processes() {
            if let Some(parent) = process.parent() {
                children_map
                    .entry(parent.as_u32())
                    .or_default()
                    .push(pid.as_u32());
            }
        }

        for snap in &snapshots {
            let mut cpu = 0.0;
            let mut mem = 0.0;
            let mut uptime = 0;
            let mut related_process_ids = BTreeSet::new();

            if let Some(pid) = snap.process_id {
                #[cfg(windows)]
                let discovered_roots = crate::utils::process::find_wardian_session_process_roots(
                    &snap.session_id,
                    Some(pid),
                );
                #[cfg(not(windows))]
                let discovered_roots = Vec::new();

                related_process_ids =
                    collect_related_pids(Some(pid), &discovered_roots, &children_map);
                let mut raw_cpu = 0.0;
                let mut memory_bytes = 0_u64;
                for pid in &related_process_ids {
                    if let Some(process) = sys.process(sysinfo::Pid::from_u32(*pid)) {
                        raw_cpu += process.cpu_usage();
                        memory_bytes = memory_bytes.saturating_add(process.memory());
                        uptime = std::cmp::max(uptime, process.run_time());
                    }
                }
                cpu = normalize_cpu_usage(raw_cpu, logical_cpu_count);
                mem = bytes_to_mib(memory_bytes);

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
            let process_alive = related_process_ids
                .iter()
                .any(|pid| sys.process(sysinfo::Pid::from_u32(*pid)).is_some());

            let mut q_count = *snap.query_count.lock().unwrap();
            let mut i_ts = snap.init_timestamp.lock().unwrap().clone();
            let mut log_path_lock = snap.log_path.lock().unwrap_or_else(|e| e.into_inner());
            let opencode_session_id = snap
                .resume_session
                .as_deref()
                .filter(|value| value.starts_with("ses_"))
                .unwrap_or(&snap.session_id);
            let gemini_session_id = snap.resume_session.as_deref().unwrap_or(&snap.session_id);

            if snap.provider == "gemini" {
                let stale_gemini_log = log_path_lock.as_ref().is_some_and(|path| {
                    std::fs::read_to_string(path).ok().is_none_or(|content| {
                        !gemini_log_matches_session(&content, gemini_session_id)
                    })
                });
                if stale_gemini_log {
                    *log_path_lock = None;
                    if let Ok(mut last_modified) = snap.log_last_modified.lock() {
                        *last_modified = None;
                    }
                }
            }

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
                if discovered_log.is_none()
                    && snap
                        .resume_session
                        .as_deref()
                        .is_none_or(|value| !value.starts_with("ses_"))
                {
                    let spawn_time = snap
                        .init_timestamp
                        .lock()
                        .ok()
                        .and_then(|timestamp| timestamp_to_system_time(timestamp.as_deref()));
                    if let Some(spawn_time) = spawn_time {
                        for dir in &log_dirs {
                            if let Some(path) = opencode_log_path_after(dir, spawn_time) {
                                discovered_log = Some(path);
                                break;
                            }
                        }
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
                        let codex_session_id =
                            codex_log_lookup_session_id(snap.resume_session.as_deref())
                                .map(str::to_string)
                                .or_else(|| {
                                    latest_codex_session_index_entry(&snap.session_id)
                                        .ok()
                                        .flatten()
                                        .map(|(session_id, _updated_at)| session_id)
                                });
                        if let Some(codex_session_id) = codex_session_id {
                            if let Some(path) =
                                codex_session_file_path(&codex_session_id, agent_home.as_deref())
                            {
                                *log_path_lock = Some(path);
                            }
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
                                                if gemini_log_matches_session(
                                                    &content,
                                                    gemini_session_id,
                                                ) {
                                                    *log_path_lock = Some(chat_file.path());
                                                    break;
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
                let mut is_initial_log_replay = snap
                    .log_last_modified
                    .lock()
                    .map(|last| last.is_none())
                    .unwrap_or(false);
                if let Ok(metadata) = std::fs::metadata(path) {
                    if let Ok(modified) = metadata.modified() {
                        let last_mod = *snap.log_last_modified.lock().unwrap();
                        if last_mod == Some(modified) {
                            should_parse = false;
                        } else {
                            is_initial_log_replay = last_mod.is_none();
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
                                    set_snapshot_status_from_log(
                                        snap,
                                        &status,
                                        is_initial_log_replay,
                                    );
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
                                        set_snapshot_status_from_log(
                                            snap,
                                            &status,
                                            is_initial_log_replay,
                                        );
                                    }
                                }
                            }
                            "opencode" => {
                                let mut status = snap.current_status.lock().unwrap().clone();
                                let effective_session_id =
                                    opencode_extract_created_session_id(path)
                                        .unwrap_or_else(|| opencode_session_id.to_string());
                                apply_opencode_log_metrics(
                                    &content,
                                    &effective_session_id,
                                    &mut q_count,
                                    &mut i_ts,
                                    &mut status,
                                );
                                if wardian_core::identity::normalize_status(&status) == "idle" {
                                    record_latest_opencode_assistant_text(
                                        snap,
                                        &effective_session_id,
                                    );
                                }
                                set_snapshot_status_from_log(snap, &status, is_initial_log_replay);
                            }
                            _ => {
                                if let Some(metrics) = parse_gemini_log_metrics(&content) {
                                    q_count = metrics.query_count;
                                    if let Some(status) = metrics.status {
                                        set_snapshot_status_from_log(
                                            snap,
                                            status,
                                            is_initial_log_replay,
                                        );
                                    }
                                    if let Some(start_time) = metrics.init_timestamp {
                                        i_ts = Some(start_time);
                                    }
                                }
                                if snap.provider == "gemini" {
                                    record_latest_gemini_assistant_text(snap, &content);
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
                    set_snapshot_status(snap, "Idle");
                }
            }

            // If the process has terminated, force status to "Off" so the UI
            // doesn't stay stuck on "Processing..." or "Action Needed".
            if !process_alive && snap.process_id.is_some() {
                set_snapshot_status(snap, "Off");
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

pub async fn get_app_metrics(state: &AppState) -> AppTelemetry {
    let agent_roots: Vec<(String, u32)> = {
        let agents = state.agents.lock().await;
        agents
            .iter()
            .filter_map(|(session_id, agent)| {
                agent
                    .process_id
                    .map(|process_id| (session_id.clone(), process_id))
            })
            .collect()
    };
    let sys_metrics = state.system_metrics.clone();
    tokio::task::spawn_blocking(move || {
        // Reuse the snapshot refreshed by get_all_metrics in the telemetry loop.
        // Refreshing again immediately would reset sysinfo's CPU deltas.
        let sys = sys_metrics.blocking_lock();
        let logical_cpu_count = sys.cpus().len();

        let mut children_map: HashMap<u32, Vec<u32>> = HashMap::new();
        for (pid, process) in sys.processes() {
            if let Some(parent) = process.parent() {
                children_map
                    .entry(parent.as_u32())
                    .or_default()
                    .push(pid.as_u32());
            }
        }

        let mut excluded_roots: BTreeSet<u32> = BTreeSet::new();
        for (session_id, process_id) in &agent_roots {
            excluded_roots.insert(*process_id);
            #[cfg(not(windows))]
            let _ = session_id;
            #[cfg(windows)]
            for discovered_pid in crate::utils::process::find_wardian_session_process_roots(
                session_id,
                Some(*process_id),
            ) {
                excluded_roots.insert(discovered_pid);
            }
        }
        let excluded_roots: Vec<u32> = excluded_roots.into_iter().collect();
        let related_process_ids =
            collect_app_process_pids(std::process::id(), &excluded_roots, &children_map);
        let mut raw_cpu = 0.0;
        let mut memory_bytes = 0_u64;
        for pid in &related_process_ids {
            if let Some(process) = sys.process(sysinfo::Pid::from_u32(*pid)) {
                raw_cpu += process.cpu_usage();
                memory_bytes = memory_bytes.saturating_add(process.memory());
            }
        }

        AppTelemetry {
            cpu_usage: normalize_cpu_usage(raw_cpu, logical_cpu_count),
            memory_mb: bytes_to_mib(memory_bytes),
        }
    })
    .await
    .unwrap_or(AppTelemetry {
        cpu_usage: 0.0,
        memory_mb: 0.0,
    })
}

#[cfg(test)]
mod tests {
    use super::AgentSnapshot;
    use std::collections::{BTreeSet, HashMap};
    use std::sync::{Arc, Mutex};

    fn test_snapshot(status: &str) -> AgentSnapshot {
        AgentSnapshot {
            session_id: "agent-1".to_string(),
            provider: "opencode".to_string(),
            folder: "D:/work".to_string(),
            resume_session: None,
            process_id: Some(1234),
            query_count: Arc::new(Mutex::new(0)),
            init_timestamp: Arc::new(Mutex::new(None)),
            current_status: Arc::new(Mutex::new(status.to_string())),
            last_status_at: Arc::new(Mutex::new(None)),
            watch_state: Arc::new(Mutex::new(crate::state::AgentWatchState::new(
                "agent-1".to_string(),
                16,
                1024,
            ))),
            last_output_at: Arc::new(Mutex::new(None)),
            log_path: Arc::new(Mutex::new(None)),
            log_last_modified: Arc::new(Mutex::new(None)),
        }
    }

    #[test]
    fn normalizes_process_tree_cpu_to_whole_machine_capacity() {
        assert_eq!(super::normalize_cpu_usage(260.0, 4), 65.0);
        assert_eq!(super::normalize_cpu_usage(800.0, 4), 100.0);
        assert_eq!(super::normalize_cpu_usage(-5.0, 4), 0.0);
    }

    #[test]
    fn treats_missing_cpu_count_as_single_cpu() {
        assert_eq!(super::normalize_cpu_usage(260.0, 0), 100.0);
    }

    #[test]
    fn converts_resident_bytes_to_mib() {
        assert_eq!(super::bytes_to_mib(1_048_576), 1.0);
        assert_eq!(super::bytes_to_mib(2_621_440), 2.5);
    }

    #[test]
    fn collects_root_descendants_and_discovered_session_roots_without_duplicates() {
        let children_map =
            HashMap::from([(1, vec![2, 4]), (2, vec![3]), (4, vec![5]), (9, vec![10])]);

        let related = super::collect_related_pids(Some(1), &[2, 9], &children_map);

        assert_eq!(related, BTreeSet::from([1_u32, 2, 3, 4, 5, 9, 10]));
    }

    #[test]
    fn app_process_pids_exclude_agent_trees_to_prevent_double_counting() {
        let children_map = HashMap::from([
            (1, vec![2, 3, 6]),
            (3, vec![4, 5]),
            (6, vec![7]),
            (8, vec![9]),
        ]);

        let app_pids = super::collect_app_process_pids(1, &[3, 7, 8], &children_map);

        assert_eq!(app_pids, BTreeSet::from([1_u32, 2, 6]));
    }

    #[test]
    fn telemetry_status_change_records_watch_status_event() {
        let snap = test_snapshot("Processing...");

        super::set_snapshot_status(&snap, "Idle");

        assert_eq!(*snap.current_status.lock().unwrap(), "Idle");
        assert!(snap.last_status_at.lock().unwrap().is_some());
        let snapshot = snap
            .watch_state
            .lock()
            .unwrap()
            .snapshot_since(None, None)
            .unwrap();
        assert!(snapshot.events.iter().any(|event| {
            event.kind == "status"
                && event.payload.get("status").and_then(|value| value.as_str()) == Some("idle")
        }));
    }

    #[test]
    fn telemetry_status_noop_does_not_emit_duplicate_watch_event() {
        let snap = test_snapshot("Idle");

        super::set_snapshot_status(&snap, "Idle");

        let snapshot = snap
            .watch_state
            .lock()
            .unwrap()
            .snapshot_since(None, None)
            .unwrap();
        assert!(snapshot.events.is_empty());
    }

    #[test]
    fn initial_log_replay_does_not_record_status_transition() {
        let snap = test_snapshot("Off");

        super::set_snapshot_status_from_log(&snap, "Idle", true);

        assert_eq!(*snap.current_status.lock().unwrap(), "Off");
        assert!(snap.last_status_at.lock().unwrap().is_none());
        let snapshot = snap
            .watch_state
            .lock()
            .unwrap()
            .snapshot_since(None, None)
            .unwrap();
        assert!(snapshot.events.is_empty());
    }

    #[test]
    fn live_log_update_records_status_transition() {
        let snap = test_snapshot("Processing...");

        super::set_snapshot_status_from_log(&snap, "Idle", false);

        assert_eq!(*snap.current_status.lock().unwrap(), "Idle");
        assert!(snap.last_status_at.lock().unwrap().is_some());
        let snapshot = snap
            .watch_state
            .lock()
            .unwrap()
            .snapshot_since(None, None)
            .unwrap();
        assert_eq!(snapshot.events.len(), 1);
        assert_eq!(
            snapshot.events[0]
                .payload
                .get("status")
                .and_then(|value| value.as_str()),
            Some("idle")
        );
    }

    #[test]
    fn opencode_assistant_text_records_watch_output_and_transcript() {
        let snap = test_snapshot("Processing...");

        super::record_opencode_assistant_text(&snap, "ses_test", "OC_DONE");

        let snapshot = snap
            .watch_state
            .lock()
            .unwrap()
            .snapshot_since(None, Some(4096))
            .unwrap();
        assert!(snapshot.output.text.contains("OC_DONE"));
        assert_eq!(snapshot.transcript.latest_text, "OC_DONE");
        assert_eq!(snapshot.transcript.messages[0].provider, "opencode");
        assert_eq!(
            snapshot.transcript.messages[0].turn_id.as_deref(),
            Some("ses_test")
        );
    }

    #[test]
    fn gemini_assistant_text_records_watch_transcript() {
        let snap = test_snapshot("Processing...");
        let content = concat!(
            r#"{"sessionId":"gemini-session-1","projectHash":"project","startTime":"2026-05-14T12:00:00.000Z"}"#,
            "\n",
            r#"{"id":"m1","timestamp":"2026-05-14T12:00:01.000Z","type":"user","content":"hello"}"#,
            "\n",
            r#"{"id":"m2","timestamp":"2026-05-14T12:00:03.000Z","type":"model","content":"Gemini answer","tokens":{"input":10,"output":2,"total":12}}"#,
            "\n"
        );

        super::record_latest_gemini_assistant_text(&snap, content);

        let snapshot = snap
            .watch_state
            .lock()
            .unwrap()
            .snapshot_since(None, Some(4096))
            .unwrap();
        assert_eq!(snapshot.transcript.latest_text, "Gemini answer");
        assert_eq!(snapshot.transcript.messages[0].provider, "gemini");
        assert_eq!(
            snapshot.transcript.messages[0].turn_id.as_deref(),
            Some("m2")
        );
    }

    #[test]
    fn gemini_log_matches_legacy_json_session_id() {
        let content = r#"{
          "sessionId": "gemini-session-1",
          "messages": []
        }"#;

        assert!(super::gemini_log_matches_session(
            content,
            "gemini-session-1"
        ));
        assert!(!super::gemini_log_matches_session(content, "other-session"));
    }

    #[test]
    fn gemini_log_matches_jsonl_metadata_session_id() {
        let content = concat!(
            r#"{"sessionId":"gemini-session-1","projectHash":"project","startTime":"2026-05-14T12:00:00.000Z"}"#,
            "\n",
            r#"{"id":"m1","timestamp":"2026-05-14T12:00:01.000Z","type":"user","content":"hello"}"#,
            "\n"
        );

        assert!(super::gemini_log_matches_session(
            content,
            "gemini-session-1"
        ));
        assert!(!super::gemini_log_matches_session(content, "other-session"));
    }

    #[test]
    fn gemini_log_metrics_parse_legacy_json() {
        let content = r#"{
          "sessionId": "gemini-session-1",
          "startTime": "2026-05-14T12:00:00.000Z",
          "messages": [
            { "type": "user", "content": "hello" },
            { "type": "gemini", "content": "hi" }
          ]
        }"#;

        let metrics = super::parse_gemini_log_metrics(content).expect("metrics");

        assert_eq!(metrics.query_count, 1);
        assert_eq!(
            metrics.init_timestamp.as_deref(),
            Some("2026-05-14T12:00:00.000Z")
        );
        assert_eq!(metrics.status, Some("Idle"));
    }

    #[test]
    fn gemini_log_metrics_parse_jsonl_completed_message_record() {
        let content = concat!(
            r#"{"sessionId":"gemini-session-1","projectHash":"project","startTime":"2026-05-14T12:00:00.000Z"}"#,
            "\n",
            r#"{"id":"m1","timestamp":"2026-05-14T12:00:01.000Z","type":"user","content":"hello"}"#,
            "\n",
            r#"{"$set":{"lastUpdated":"2026-05-14T12:00:02.000Z"}}"#,
            "\n",
            r#"{"id":"m2","timestamp":"2026-05-14T12:00:03.000Z","type":"gemini","content":"hi","tokens":{"input":10,"output":1,"total":11}}"#,
            "\n"
        );

        let metrics = super::parse_gemini_log_metrics(content).expect("metrics");

        assert_eq!(metrics.query_count, 1);
        assert_eq!(
            metrics.init_timestamp.as_deref(),
            Some("2026-05-14T12:00:00.000Z")
        );
        assert_eq!(metrics.status, Some("Idle"));
    }

    #[test]
    fn gemini_log_metrics_jsonl_model_chunk_without_completion_stays_processing() {
        let content = concat!(
            r#"{"sessionId":"gemini-session-1","projectHash":"project","startTime":"2026-05-14T12:00:00.000Z"}"#,
            "\n",
            r#"{"id":"m1","timestamp":"2026-05-14T12:00:01.000Z","type":"user","content":"hello"}"#,
            "\n",
            r#"{"id":"m2","timestamp":"2026-05-14T12:00:03.000Z","type":"model","content":"partial"}"#,
            "\n"
        );

        let metrics = super::parse_gemini_log_metrics(content).expect("metrics");

        assert_eq!(metrics.query_count, 1);
        assert_eq!(metrics.status, Some("Processing..."));
    }

    #[test]
    fn gemini_log_metrics_jsonl_result_marks_idle() {
        let content = concat!(
            r#"{"sessionId":"gemini-session-1","projectHash":"project","startTime":"2026-05-14T12:00:00.000Z"}"#,
            "\n",
            r#"{"id":"m1","timestamp":"2026-05-14T12:00:01.000Z","type":"user","content":"hello"}"#,
            "\n",
            r#"{"id":"m2","timestamp":"2026-05-14T12:00:03.000Z","type":"model","content":"partial"}"#,
            "\n",
            r#"{"type":"result"}"#,
            "\n"
        );

        let metrics = super::parse_gemini_log_metrics(content).expect("metrics");

        assert_eq!(metrics.query_count, 1);
        assert_eq!(metrics.status, Some("Idle"));
    }

    #[test]
    fn gemini_log_metrics_jsonl_last_user_is_processing() {
        let content = concat!(
            r#"{"sessionId":"gemini-session-1","projectHash":"project","startTime":"2026-05-14T12:00:00.000Z"}"#,
            "\n",
            r#"{"id":"m1","timestamp":"2026-05-14T12:00:01.000Z","type":"user","content":"hello"}"#,
            "\n"
        );

        let metrics = super::parse_gemini_log_metrics(content).expect("metrics");

        assert_eq!(metrics.query_count, 1);
        assert_eq!(metrics.status, Some("Processing..."));
    }
}
