use crate::state::AppState;
use crate::utils::fs::get_wardian_home;
use std::collections::{BTreeSet, HashMap};
use wardian_core::models::{AgentTelemetry, AppTelemetry};

use super::claude::{claude_is_real_user_query, claude_project_dir_name, claude_status_from_log};
use super::codex::{
    codex_log_lookup_session_id, codex_session_file_path, codex_status_from_log,
    latest_codex_session_index_entry,
};
use super::display_log_path;
use super::opencode::{
    apply_opencode_log_metrics, opencode_log_dirs, opencode_log_path_in,
    opencode_session_diff_path, opencode_should_fallback_to_idle,
};

fn normalize_cpu_usage(raw_cpu_usage: f32, logical_cpu_count: usize) -> f32 {
    let divisor = logical_cpu_count.max(1) as f32;
    (raw_cpu_usage / divisor).clamp(0.0, 100.0)
}

fn bytes_to_mib(bytes: u64) -> f64 {
    bytes as f64 / 1_048_576.0
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
    use std::collections::{BTreeSet, HashMap};

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
}
