use crate::state::AppState;
use crate::utils::fs::get_wardian_home;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::OnceLock;
use std::sync::{Arc, Mutex};
use wardian_core::models::{AgentTelemetry, AppTelemetry};

use crate::providers::transcript::extract_transcript_message;

use super::claude::{claude_is_real_user_query, claude_project_dir_name, claude_status_from_log};
use super::codex::{codex_log_lookup_session_id, codex_session_file_path, codex_status_from_log};
use super::display_log_path;
use super::opencode::{
    apply_opencode_log_metrics, opencode_last_assistant_text, opencode_log_dirs,
    opencode_log_path_after, opencode_log_path_in, opencode_session_diff_path,
    opencode_should_fallback_to_idle,
};
use crate::providers::antigravity::AntigravityProvider;
use wardian_core::control::{ProviderInputReadiness, ProviderReadyEvidence};

const TELEMETRY_SLOW_PASS_THRESHOLD: std::time::Duration = std::time::Duration::from_millis(500);

/// Reading every process's command line and environment block (PEB reads on
/// Windows) is far too expensive to do on every 5s tick, so marker-based
/// session-root discovery runs at most this often.
#[cfg(windows)]
const SESSION_ROOT_DISCOVERY_TTL: std::time::Duration = std::time::Duration::from_secs(60);

/// The gemini fallback scan walks every chat file under ~/.gemini/tmp, which
/// can be hundreds of thousands of files. Retry it at most this often per
/// agent when no matching log has been found.
const GEMINI_FALLBACK_SCAN_TTL: std::time::Duration = std::time::Duration::from_secs(60);

static TELEMETRY_AGENT_WORK_IN_FLIGHT: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();

static GEMINI_FALLBACK_SCAN_ATTEMPTS: OnceLock<Mutex<HashMap<String, std::time::Instant>>> =
    OnceLock::new();

static LAST_APP_TELEMETRY: OnceLock<Mutex<AppTelemetry>> = OnceLock::new();

fn last_app_telemetry_cache() -> &'static Mutex<AppTelemetry> {
    LAST_APP_TELEMETRY.get_or_init(|| {
        Mutex::new(AppTelemetry {
            cpu_usage: 0.0,
            memory_mb: 0.0,
        })
    })
}

#[cfg(windows)]
struct SessionRootsCache {
    roots: HashMap<String, Vec<u32>>,
    refreshed_at: std::time::Instant,
    session_key: Vec<String>,
}

#[cfg(windows)]
static SESSION_ROOTS_CACHE: OnceLock<Mutex<Option<SessionRootsCache>>> = OnceLock::new();

#[cfg(windows)]
fn session_roots_cache() -> &'static Mutex<Option<SessionRootsCache>> {
    SESSION_ROOTS_CACHE.get_or_init(|| Mutex::new(None))
}

#[cfg(windows)]
fn sorted_session_key(session_ids: &[String]) -> Vec<String> {
    let mut key = session_ids.to_vec();
    key.sort_unstable();
    key
}

#[cfg(windows)]
fn session_root_discovery_due(session_ids: &[String]) -> bool {
    let Ok(cache) = session_roots_cache().lock() else {
        return true;
    };
    match cache.as_ref() {
        Some(cache) => {
            cache.refreshed_at.elapsed() >= SESSION_ROOT_DISCOVERY_TTL
                || cache.session_key != sorted_session_key(session_ids)
        }
        None => true,
    }
}

#[cfg(windows)]
fn cached_session_roots() -> HashMap<String, Vec<u32>> {
    session_roots_cache()
        .lock()
        .ok()
        .and_then(|cache| cache.as_ref().map(|cache| cache.roots.clone()))
        .unwrap_or_default()
}

#[cfg(windows)]
fn store_session_roots(session_ids: &[String], roots: HashMap<String, Vec<u32>>) {
    if let Ok(mut cache) = session_roots_cache().lock() {
        *cache = Some(SessionRootsCache {
            roots,
            refreshed_at: std::time::Instant::now(),
            session_key: sorted_session_key(session_ids),
        });
    }
}

/// Cap the fallback scan to the most recently modified chat files; a session
/// being discovered was active recently, and unbounded scans have to read
/// every chat file ever written (gigabytes on long-lived machines).
const GEMINI_FALLBACK_SCAN_MAX_FILES: usize = 128;

/// Gemini chat logs carry their `sessionId` near the start of the file, so a
/// bounded prefix read is enough to reject non-matching candidates without
/// reading whole multi-megabyte transcripts.
const GEMINI_LOG_SESSION_PREFIX_BYTES: u64 = 64 * 1024;

fn gemini_log_prefix_contains(path: &std::path::Path, target_id: &str) -> bool {
    use std::io::Read;
    let target_id = target_id.trim();
    if target_id.is_empty() {
        return false;
    }
    let Ok(file) = std::fs::File::open(path) else {
        return false;
    };
    let mut prefix = Vec::new();
    if file
        .take(GEMINI_LOG_SESSION_PREFIX_BYTES)
        .read_to_end(&mut prefix)
        .is_err()
    {
        return false;
    }
    String::from_utf8_lossy(&prefix).contains(target_id)
}

fn discover_gemini_log_in_tmp(
    tmp_dir: &std::path::Path,
    session_id: &str,
) -> Option<std::path::PathBuf> {
    let mut candidates: Vec<(std::time::SystemTime, std::path::PathBuf)> = Vec::new();
    for entry in std::fs::read_dir(tmp_dir).ok()?.flatten() {
        let chat_dir = entry.path().join("chats");
        let Ok(chat_files) = std::fs::read_dir(chat_dir) else {
            continue;
        };
        for chat_file in chat_files.flatten() {
            let modified = chat_file
                .metadata()
                .ok()
                .and_then(|meta| meta.modified().ok())
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
            candidates.push((modified, chat_file.path()));
        }
    }
    candidates.sort_by_key(|candidate| std::cmp::Reverse(candidate.0));
    candidates
        .into_iter()
        .take(GEMINI_FALLBACK_SCAN_MAX_FILES)
        .map(|(_, path)| path)
        .find(|path| {
            // Cheap prefix rejection first; confirm probable hits with the
            // full session check so match semantics stay unchanged.
            gemini_log_prefix_contains(path, session_id)
                && std::fs::read_to_string(path)
                    .is_ok_and(|content| gemini_log_matches_session(&content, session_id))
        })
}

/// Provider logs are re-parsed whenever they change and grow to hundreds of
/// megabytes for long-lived codex sessions. Status is derived from the most
/// recent lines, so parsing is capped to this tail; files under the cap are
/// read whole (gemini legacy logs are a single JSON document and stay intact).
/// For capped files the query count is tail-bounded and the init timestamp
/// falls back to the persisted Born time.
const LOG_PARSE_TAIL_BYTES: u64 = 4 * 1024 * 1024;

fn read_log_bounded(path: &std::path::Path) -> std::io::Result<String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(path)?;
    let len = file.metadata()?.len();
    if len <= LOG_PARSE_TAIL_BYTES {
        let mut content = String::new();
        file.read_to_string(&mut content)?;
        return Ok(content);
    }
    file.seek(SeekFrom::Start(len - LOG_PARSE_TAIL_BYTES))?;
    let mut bytes = Vec::with_capacity(LOG_PARSE_TAIL_BYTES as usize);
    file.read_to_end(&mut bytes)?;
    let content = String::from_utf8_lossy(&bytes);
    // Drop the first (possibly partial) line so parsing starts on a boundary.
    Ok(content
        .split_once('\n')
        .map(|(_, rest)| rest.to_string())
        .unwrap_or_default())
}

fn gemini_fallback_scan_due(session_id: &str) -> bool {
    let attempts = GEMINI_FALLBACK_SCAN_ATTEMPTS.get_or_init(|| Mutex::new(HashMap::new()));
    let Ok(mut attempts) = attempts.lock() else {
        return true;
    };
    let now = std::time::Instant::now();
    match attempts.get(session_id) {
        Some(last) if now.duration_since(*last) < GEMINI_FALLBACK_SCAN_TTL => false,
        _ => {
            attempts.insert(session_id.to_string(), now);
            true
        }
    }
}

fn should_run_provider_log_telemetry(
    _provider: &str,
    current_status: &str,
    process_alive: Option<bool>,
) -> bool {
    if process_alive == Some(false) {
        return false;
    }
    !(wardian_core::identity::normalize_status(current_status) == "off"
        && process_alive != Some(true))
}

fn reconcile_live_opencode_log_status(
    provider: &str,
    current_status: &str,
    log_status: String,
    process_alive: Option<bool>,
    last_output_at: Option<std::time::SystemTime>,
) -> String {
    if provider != "opencode"
        || wardian_core::identity::normalize_status(&log_status) != "error"
        || process_alive != Some(true)
        || last_output_at.is_none()
    {
        return log_status;
    }

    match wardian_core::identity::normalize_status(current_status).as_str() {
        "idle" | "processing" | "action_required" => current_status.to_string(),
        _ => log_status,
    }
}

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
    provider_generation: u64,
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

#[derive(Default)]
struct TelemetryPassResult {
    metrics: Vec<AgentTelemetry>,
    provider_statuses: Vec<TelemetryProviderStatus>,
}

struct TelemetryProviderStatus {
    session_id: String,
    generation: u64,
    status: String,
}

#[derive(Debug, Clone)]
struct ProcessSample {
    cpu_usage: f32,
    memory: u64,
    run_time: u64,
}

#[cfg(windows)]
#[derive(Debug, Clone)]
struct ProcessMarkerSnapshot {
    pid: u32,
    process_name: String,
    command_line: String,
    environ: Vec<String>,
}

#[derive(Debug, Clone)]
struct SystemProcessSnapshot {
    logical_cpu_count: usize,
    children_map: HashMap<u32, Vec<u32>>,
    processes: HashMap<u32, ProcessSample>,
    sys_refresh: std::time::Duration,
    #[cfg(windows)]
    session_roots: HashMap<String, Vec<u32>>,
}

struct TelemetryAgentWorkGuard {
    session_id: String,
}

impl Drop for TelemetryAgentWorkGuard {
    fn drop(&mut self) {
        let in_flight = TELEMETRY_AGENT_WORK_IN_FLIGHT.get_or_init(|| Mutex::new(HashSet::new()));
        if let Ok(mut in_flight) = in_flight.lock() {
            in_flight.remove(&self.session_id);
        }
    }
}

fn try_begin_agent_telemetry_work(session_id: &str) -> Option<TelemetryAgentWorkGuard> {
    let in_flight = TELEMETRY_AGENT_WORK_IN_FLIGHT.get_or_init(|| Mutex::new(HashSet::new()));
    let mut in_flight = in_flight.lock().ok()?;
    if !in_flight.insert(session_id.to_string()) {
        return None;
    }
    Some(TelemetryAgentWorkGuard {
        session_id: session_id.to_string(),
    })
}

#[cfg(windows)]
fn discover_session_roots_from_process_markers(
    session_ids: &[String],
    markers: &[ProcessMarkerSnapshot],
) -> HashMap<String, Vec<u32>> {
    let mut roots = session_ids
        .iter()
        .map(|session_id| (session_id.clone(), Vec::new()))
        .collect::<HashMap<_, _>>();

    for marker in markers {
        for session_id in session_ids {
            if crate::utils::process::is_wardian_session_environment_candidate(
                &marker.environ,
                session_id,
            ) || crate::utils::process::is_wardian_session_process_candidate(
                &marker.process_name,
                &marker.command_line,
                session_id,
            ) {
                roots
                    .entry(session_id.clone())
                    .or_default()
                    .push(marker.pid);
            }
        }
    }

    for pids in roots.values_mut() {
        pids.sort_unstable();
        pids.dedup();
    }

    roots
}

fn refresh_system_process_snapshot(
    sys_metrics: &tokio::sync::Mutex<sysinfo::System>,
    #[cfg_attr(not(windows), allow(unused_variables))] session_ids: &[String],
) -> Option<SystemProcessSnapshot> {
    let mut sys = match sys_metrics.try_lock() {
        Ok(sys) => sys,
        Err(_) => {
            crate::utils::logging::log_debug(
                "[Wardian] Telemetry skipped system sampling because previous refresh is still running",
            );
            return None;
        }
    };
    // A full refresh_all() walks every process's command line and environment
    // block, which costs over a second per tick on busy systems. Sample only
    // CPU and memory each tick; marker data is fetched on TTL'd discovery
    // passes below (OnlyIfNotSet keeps already-fetched markers cached inside
    // sysinfo, so even discovery passes only read new processes).
    #[cfg(windows)]
    let discovery_due = session_root_discovery_due(session_ids);
    let refresh_kind = sysinfo::ProcessRefreshKind::nothing()
        .with_cpu()
        .with_memory();
    #[cfg(windows)]
    let refresh_kind = if discovery_due {
        refresh_kind
            .with_cmd(sysinfo::UpdateKind::OnlyIfNotSet)
            .with_environ(sysinfo::UpdateKind::OnlyIfNotSet)
    } else {
        refresh_kind
    };
    let sys_refresh_started = std::time::Instant::now();
    sys.refresh_processes_specifics(sysinfo::ProcessesToUpdate::All, true, refresh_kind);
    let sys_refresh = sys_refresh_started.elapsed();
    let logical_cpu_count = sys.cpus().len();
    let mut children_map: HashMap<u32, Vec<u32>> = HashMap::new();
    let mut processes = HashMap::new();
    #[cfg(windows)]
    let mut process_markers = Vec::new();

    for (pid, process) in sys.processes() {
        let pid = pid.as_u32();
        if let Some(parent) = process.parent() {
            children_map.entry(parent.as_u32()).or_default().push(pid);
        }
        processes.insert(
            pid,
            ProcessSample {
                cpu_usage: process.cpu_usage(),
                memory: process.memory(),
                run_time: process.run_time(),
            },
        );
        #[cfg(windows)]
        if discovery_due {
            process_markers.push(ProcessMarkerSnapshot {
                pid,
                process_name: process.name().to_string_lossy().to_string(),
                command_line: process
                    .cmd()
                    .iter()
                    .map(|part| part.to_string_lossy())
                    .collect::<Vec<_>>()
                    .join(" "),
                environ: process
                    .environ()
                    .iter()
                    .map(|entry| entry.to_string_lossy().to_string())
                    .collect::<Vec<_>>(),
            });
        }
    }

    #[cfg(windows)]
    let session_roots = if discovery_due {
        let roots = discover_session_roots_from_process_markers(session_ids, &process_markers);
        store_session_roots(session_ids, roots.clone());
        roots
    } else {
        cached_session_roots()
    };

    Some(SystemProcessSnapshot {
        logical_cpu_count,
        children_map,
        processes,
        sys_refresh,
        #[cfg(windows)]
        session_roots,
    })
}

#[derive(Debug, Clone)]
struct TelemetrySlowAgent {
    session_id: String,
    provider: String,
    duration: std::time::Duration,
}

#[derive(Debug, Clone)]
struct TelemetryPassTimings {
    total: std::time::Duration,
    sys_refresh: std::time::Duration,
    agent_count: usize,
    slow_agents: Vec<TelemetrySlowAgent>,
}

impl TelemetryPassTimings {
    fn slow_log_message(&self, threshold: std::time::Duration) -> Option<String> {
        if self.total < threshold && self.sys_refresh < threshold && self.slow_agents.is_empty() {
            return None;
        }

        let slow_agents = if self.slow_agents.is_empty() {
            "none".to_string()
        } else {
            self.slow_agents
                .iter()
                .map(|agent| {
                    format!(
                        "{}:{}:{}ms",
                        agent.session_id,
                        agent.provider,
                        agent.duration.as_millis()
                    )
                })
                .collect::<Vec<_>>()
                .join(",")
        };

        Some(format!(
            "[Wardian] Slow telemetry pass total_ms={} sys_refresh_ms={} agent_count={} slow_agents={}",
            self.total.as_millis(),
            self.sys_refresh.as_millis(),
            self.agent_count,
            slow_agents
        ))
    }
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

fn apply_claude_log_status(
    snap: &AgentSnapshot,
    lines: &[serde_json::Value],
    is_initial_replay: bool,
) {
    if let Some(status) = claude_status_from_log(lines) {
        set_snapshot_status_from_log(snap, &status, is_initial_replay);
    }
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

fn latest_antigravity_assistant_message(
    content: &str,
) -> Option<wardian_core::control::WatchTranscriptMessage> {
    content
        .lines()
        .rev()
        .filter_map(|line| {
            let trimmed = line.trim();
            (!trimmed.is_empty()).then_some(trimmed)
        })
        .find_map(|line| extract_transcript_message("antigravity", line))
}

fn record_latest_antigravity_assistant_text(snap: &AgentSnapshot, content: &str) {
    let Some(message) = latest_antigravity_assistant_message(content) else {
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

fn parse_antigravity_log_metrics(content: &str) -> (usize, Option<String>, Option<&'static str>) {
    let mut query_count = 0;
    let mut init_timestamp = None;
    let mut status = None;

    for line in content.lines() {
        let Ok(parsed) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if init_timestamp.is_none() {
            init_timestamp = parsed
                .get("created_at")
                .and_then(|value| value.as_str())
                .map(str::to_string);
        }
        match (
            parsed.get("source").and_then(|value| value.as_str()),
            parsed.get("type").and_then(|value| value.as_str()),
            parsed.get("status").and_then(|value| value.as_str()),
        ) {
            (Some("USER_EXPLICIT"), Some("USER_INPUT"), _) => {
                query_count += 1;
                status = Some("Processing...");
            }
            (Some("MODEL"), Some("PLANNER_RESPONSE"), Some("DONE")) => {
                status = Some("Idle");
            }
            (Some("MODEL"), Some("PLANNER_RESPONSE"), _) => {
                status = Some("Processing...");
            }
            _ => {}
        }
    }

    (query_count, init_timestamp, status)
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

fn discover_opencode_telemetry_log_path(
    log_dirs: &[std::path::PathBuf],
    _wardian_session_id: &str,
    resume_session: Option<&str>,
    spawn_time: Option<std::time::SystemTime>,
) -> Option<std::path::PathBuf> {
    if let Some(opencode_session_id) = resume_session
        .map(str::trim)
        .filter(|value| value.starts_with("ses_"))
    {
        for dir in log_dirs {
            if let Some(path) = opencode_log_path_in(dir, opencode_session_id) {
                return Some(path);
            }
        }
        return Some(opencode_session_diff_path(opencode_session_id));
    }

    let spawn_time = spawn_time?;
    for dir in log_dirs {
        if let Some(path) = opencode_log_path_after(dir, spawn_time) {
            return Some(path);
        }
    }

    None
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
    let mut snapshots: Vec<AgentSnapshot> = {
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
                    provider_generation: 0,
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
    for snapshot in &mut snapshots {
        snapshot.provider_generation = state
            .interactions
            .current_provider_input_generation(&snapshot.session_id)
            .await
            .unwrap_or(0);
    }

    let sys_metrics = state.system_metrics.clone();
    let result = tokio::task::spawn_blocking(move || {
        let session_ids = snapshots
            .iter()
            .map(|snap| snap.session_id.clone())
            .collect::<Vec<_>>();
        let active_leases = wardian_core::conversation_lease::load_leases();
        let lease_now = chrono::Utc::now().to_rfc3339();
        let pass_started = std::time::Instant::now();
        let mut results = Vec::new();
        let mut provider_statuses = Vec::new();
        let system_snapshot = refresh_system_process_snapshot(&sys_metrics, &session_ids);
        let mut slow_agents = Vec::new();

        for snap in &snapshots {
            let agent_started = std::time::Instant::now();
            let mut cpu = 0.0;
            let mut mem = 0.0;
            let mut uptime = 0;
            let mut related_process_ids = BTreeSet::new();

            if let (Some(system_snapshot), Some(pid)) = (&system_snapshot, snap.process_id) {
                #[cfg(windows)]
                let discovered_roots = system_snapshot
                    .session_roots
                    .get(&snap.session_id)
                    .cloned()
                    .unwrap_or_default();
                #[cfg(not(windows))]
                let discovered_roots = Vec::new();

                related_process_ids = collect_related_pids(
                    Some(pid),
                    &discovered_roots,
                    &system_snapshot.children_map,
                );
                let mut raw_cpu = 0.0;
                let mut memory_bytes = 0_u64;
                for pid in &related_process_ids {
                    if let Some(process) = system_snapshot.processes.get(pid) {
                        raw_cpu += process.cpu_usage;
                        memory_bytes = memory_bytes.saturating_add(process.memory);
                        uptime = std::cmp::max(uptime, process.run_time);
                    }
                }
                cpu = normalize_cpu_usage(raw_cpu, system_snapshot.logical_cpu_count);
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

            // Detect whether the agent process is still alive. If system sampling
            // is skipped, liveness is unknown and must not force a status change.
            let process_alive = system_snapshot.as_ref().map(|system_snapshot| {
                related_process_ids
                    .iter()
                    .any(|pid| system_snapshot.processes.contains_key(pid))
            });

            let mut q_count = *snap.query_count.lock().unwrap();
            let mut i_ts = snap.init_timestamp.lock().unwrap().clone();
            let mut log_path_display = snap
                .log_path
                .try_lock()
                .ok()
                .and_then(|path| path.as_ref().map(|p| display_log_path(p)));
            let opencode_session_id = snap
                .resume_session
                .as_deref()
                .filter(|value| value.starts_with("ses_"));
            let gemini_session_id = snap.resume_session.as_deref();
            let status_before_log_work = snap.current_status.lock().unwrap().clone();
            let run_provider_log_work = should_run_provider_log_telemetry(
                &snap.provider,
                &status_before_log_work,
                process_alive,
            );

            if run_provider_log_work {
                if let Some(_agent_work_guard) = try_begin_agent_telemetry_work(&snap.session_id) {
                    let mut log_path_lock = snap.log_path.lock().unwrap_or_else(|e| e.into_inner());

                    if snap.provider == "gemini" {
                        // Re-verifying the session id requires reading the whole
                        // log, so only do it when the file changed (or vanished)
                        // since the last parse; unchanged content cannot go stale.
                        let last_parsed_mtime =
                            snap.log_last_modified.lock().ok().and_then(|last| *last);
                        let stale_gemini_log = gemini_session_id.is_none()
                            || log_path_lock.as_ref().is_some_and(|path| {
                                let current_mtime = std::fs::metadata(path)
                                    .and_then(|meta| meta.modified())
                                    .ok();
                                match (current_mtime, last_parsed_mtime) {
                                    (Some(current), Some(last)) if current == last => false,
                                    _ => std::fs::read_to_string(path).ok().is_none_or(|content| {
                                        !gemini_log_matches_session(
                                            &content,
                                            gemini_session_id.unwrap_or_default(),
                                        )
                                    }),
                                }
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
                        let log_dirs = opencode_log_dirs();
                        let spawn_time =
                            snap.init_timestamp.lock().ok().and_then(|timestamp| {
                                timestamp_to_system_time(timestamp.as_deref())
                            });
                        *log_path_lock = discover_opencode_telemetry_log_path(
                            &log_dirs,
                            &snap.session_id,
                            snap.resume_session.as_deref(),
                            spawn_time,
                        );
                    } else if snap.provider == "antigravity" {
                        let conversation_id = snap
                            .resume_session
                            .as_ref()
                            .map(|value| value.trim().to_string())
                            .filter(|value| !value.is_empty());
                        if let (Some(home), Some(conversation_id)) =
                            (AntigravityProvider::antigravity_home(), conversation_id)
                        {
                            let candidate =
                                AntigravityProvider::transcript_path(&home, &conversation_id);
                            if candidate.exists() {
                                *log_path_lock = Some(candidate);
                            }
                        }
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
                                        .map(str::to_string);
                                if let Some(codex_session_id) = codex_session_id {
                                    if let Some(path) = codex_session_file_path(
                                        &codex_session_id,
                                        agent_home.as_deref(),
                                    ) {
                                        *log_path_lock = Some(path);
                                    }
                                }
                            }
                            "claude" => {}
                            _ => {
                                // Gemini: scan ~/.gemini/tmp for chat log files.
                                // Bounded to recent candidates with prefix reads,
                                // and retried only after the backoff TTL when
                                // nothing matched.
                                if let Some(gemini_session_id) = gemini_session_id {
                                    if let Some(home) = dirs::home_dir()
                                        .filter(|_| gemini_fallback_scan_due(&snap.session_id))
                                    {
                                        let tmp_dir = home.join(".gemini").join("tmp");
                                        if let Some(path) =
                                            discover_gemini_log_in_tmp(&tmp_dir, gemini_session_id)
                                        {
                                            *log_path_lock = Some(path);
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
                            if let Ok(content) = read_log_bounded(path) {
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
                                                l.get("type").and_then(|v| v.as_str())
                                                    == Some("event_msg")
                                                    && l.get("payload")
                                                        .and_then(|v| v.get("type"))
                                                        .and_then(|v| v.as_str())
                                                        == Some("user_message")
                                            })
                                            .count();

                                        if let Some(meta) = lines.iter().find(|l| {
                                            l.get("type").and_then(|v| v.as_str())
                                                == Some("session_meta")
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
                                                l.get("type").and_then(|v| v.as_str())
                                                    == Some("user")
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

                                        apply_claude_log_status(
                                            snap,
                                            &lines,
                                            is_initial_log_replay,
                                        );
                                    }
                                    "opencode" => {
                                        let mut status =
                                            snap.current_status.lock().unwrap().clone();
                                        let Some(effective_session_id) = opencode_session_id else {
                                            continue;
                                        };
                                        apply_opencode_log_metrics(
                                            &content,
                                            effective_session_id,
                                            &mut q_count,
                                            &mut i_ts,
                                            &mut status,
                                        );
                                        status = reconcile_live_opencode_log_status(
                                            &snap.provider,
                                            &status_before_log_work,
                                            status,
                                            process_alive,
                                            *snap.last_output_at.lock().unwrap(),
                                        );
                                        if wardian_core::identity::normalize_status(&status)
                                            == "idle"
                                        {
                                            record_latest_opencode_assistant_text(
                                                snap,
                                                effective_session_id,
                                            );
                                        }
                                        set_snapshot_status_from_log(
                                            snap,
                                            &status,
                                            is_initial_log_replay,
                                        );
                                    }
                                    "antigravity" => {
                                        let (queries, start_time, status) =
                                            parse_antigravity_log_metrics(&content);
                                        q_count = queries;
                                        if let Some(status) = status {
                                            set_snapshot_status_from_log(
                                                snap,
                                                status,
                                                is_initial_log_replay,
                                            );
                                        }
                                        if start_time.is_some() {
                                            i_ts = start_time;
                                        }
                                        record_latest_antigravity_assistant_text(snap, &content);
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
                    log_path_display = log_path_lock.as_ref().map(|p| display_log_path(p));
                } else {
                    crate::utils::logging::log_debug(&format!(
                        "[Wardian] Skipped overlapping telemetry log work for {}",
                        snap.session_id
                    ));
                }
            }

            if (snap.provider == "opencode"
                || snap.provider == "claude"
                || snap.provider == "antigravity")
                && (snap.process_id.is_none() || process_alive == Some(true))
            {
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
            if process_alive == Some(false) && snap.process_id.is_some() {
                set_snapshot_status(snap, "Off");
            }

            let current_status = if wardian_core::conversation_lease::find_active_conflict(
                &active_leases,
                &snap.session_id,
                snap.resume_session.as_deref().unwrap_or_default(),
                &lease_now,
            )
            .is_some()
            {
                "Headless".to_string()
            } else {
                snap.current_status.lock().unwrap().clone()
            };
            provider_statuses.push(TelemetryProviderStatus {
                session_id: snap.session_id.clone(),
                generation: snap.provider_generation,
                status: snap.current_status.lock().unwrap().clone(),
            });

            results.push(AgentTelemetry {
                session_id: snap.session_id.clone(),
                cpu_usage: cpu,
                memory_mb: mem,
                uptime_seconds: uptime,
                query_count: *snap.query_count.lock().unwrap(),
                init_timestamp: snap.init_timestamp.lock().unwrap().clone(),
                current_status,
                log_path: log_path_display,
            });
            let agent_duration = agent_started.elapsed();
            if agent_duration >= TELEMETRY_SLOW_PASS_THRESHOLD {
                slow_agents.push(TelemetrySlowAgent {
                    session_id: snap.session_id.clone(),
                    provider: snap.provider.clone(),
                    duration: agent_duration,
                });
            }
        }
        let timings = TelemetryPassTimings {
            total: pass_started.elapsed(),
            sys_refresh: system_snapshot
                .as_ref()
                .map(|snapshot| snapshot.sys_refresh)
                .unwrap_or_default(),
            agent_count: snapshots.len(),
            slow_agents,
        };
        if let Some(message) = timings.slow_log_message(TELEMETRY_SLOW_PASS_THRESHOLD) {
            crate::utils::logging::log_debug(&message);
        }
        TelemetryPassResult {
            metrics: results,
            provider_statuses,
        }
    })
    .await
    .unwrap_or_default();
    apply_provider_status_observations(state, &result.provider_statuses).await;
    result.metrics
}

async fn apply_provider_status_observations(
    state: &AppState,
    observations: &[TelemetryProviderStatus],
) {
    for observation in observations {
        let readiness = provider_readiness_from_status(&observation.status);
        let ready_evidence = (readiness == ProviderInputReadiness::Ready)
            .then_some(ProviderReadyEvidence::ProviderEvent);
        state
            .interactions
            .record_provider_input_state(
                &observation.session_id,
                observation.generation,
                readiness,
                ready_evidence,
            )
            .await;
        if readiness == ProviderInputReadiness::Ready {
            crate::control::drain_mailbox_for_idle_agent_from_status_observation(
                None,
                state,
                &observation.session_id,
            )
            .await;
        }
    }
}

fn provider_readiness_from_status(status: &str) -> ProviderInputReadiness {
    match wardian_core::identity::normalize_status(status).as_str() {
        "idle" => ProviderInputReadiness::Ready,
        "processing" => ProviderInputReadiness::Busy,
        "action_required" => ProviderInputReadiness::ActionRequired,
        "off" | "error" => ProviderInputReadiness::Unavailable,
        _ => ProviderInputReadiness::Unknown,
    }
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
        let Ok(sys) = sys_metrics.try_lock() else {
            crate::utils::logging::log_debug(
                "[Wardian] App telemetry reusing last sample because system sampling is still running",
            );
            return last_app_telemetry_cache()
                .lock()
                .map(|telemetry| telemetry.clone())
                .unwrap_or(AppTelemetry {
                    cpu_usage: 0.0,
                    memory_mb: 0.0,
                });
        };
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
        // Reuse the marker-discovered roots cached by the telemetry loop;
        // rebuilding them here would re-convert every process's environment
        // block on each tick.
        #[cfg(windows)]
        let session_roots = cached_session_roots();
        for (session_id, process_id) in &agent_roots {
            excluded_roots.insert(*process_id);
            #[cfg(not(windows))]
            let _ = session_id;
            #[cfg(windows)]
            {
                for discovered_pid in session_roots
                    .get(session_id)
                    .into_iter()
                    .flat_map(|pids| pids.iter().copied())
                {
                    excluded_roots.insert(discovered_pid);
                }
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

        let telemetry = AppTelemetry {
            cpu_usage: normalize_cpu_usage(raw_cpu, logical_cpu_count),
            memory_mb: bytes_to_mib(memory_bytes),
        };
        if let Ok(mut last) = last_app_telemetry_cache().lock() {
            *last = telemetry.clone();
        }
        telemetry
    })
    .await
    .unwrap_or(AppTelemetry {
        cpu_usage: 0.0,
        memory_mb: 0.0,
    })
}

#[cfg(test)]
mod tests {
    use super::{AgentSnapshot, TelemetryPassTimings, TelemetrySlowAgent};
    use std::collections::{BTreeSet, HashMap};
    use std::sync::{Arc, Mutex};

    fn test_snapshot(status: &str) -> AgentSnapshot {
        AgentSnapshot {
            session_id: "agent-1".to_string(),
            provider: "opencode".to_string(),
            folder: "D:/work".to_string(),
            resume_session: None,
            provider_generation: 0,
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

    #[cfg(windows)]
    #[test]
    fn discovers_session_roots_for_multiple_agents_from_one_process_marker_snapshot() {
        let markers = vec![
            super::ProcessMarkerSnapshot {
                pid: 10,
                process_name: "cmd.exe".to_string(),
                command_line: "cmd.exe /d /c codex.cmd resume session-a --cd D:/repo".to_string(),
                environ: Vec::new(),
            },
            super::ProcessMarkerSnapshot {
                pid: 11,
                process_name: "node.exe".to_string(),
                command_line: "node codex".to_string(),
                environ: vec!["WARDIAN_SESSION_ID=session-a".to_string()],
            },
            super::ProcessMarkerSnapshot {
                pid: 20,
                process_name: "node.exe".to_string(),
                command_line: "node other".to_string(),
                environ: vec!["WARDIAN_SESSION_ID=session-b".to_string()],
            },
            super::ProcessMarkerSnapshot {
                pid: 30,
                process_name: "pwsh.exe".to_string(),
                command_line: "pwsh -NoLogo".to_string(),
                environ: Vec::new(),
            },
        ];

        let roots = super::discover_session_roots_from_process_markers(
            &["session-a".to_string(), "session-b".to_string()],
            &markers,
        );

        assert_eq!(roots["session-a"], vec![10, 11]);
        assert_eq!(roots["session-b"], vec![20]);
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
    fn slow_telemetry_report_only_formats_slow_passes() {
        let report = TelemetryPassTimings {
            total: std::time::Duration::from_millis(750),
            sys_refresh: std::time::Duration::from_millis(25),
            agent_count: 3,
            slow_agents: vec![TelemetrySlowAgent {
                session_id: "agent-1".to_string(),
                provider: "codex".to_string(),
                duration: std::time::Duration::from_millis(620),
            }],
        };

        let message = report.slow_log_message(std::time::Duration::from_millis(500));

        assert!(message.is_some_and(|message| {
            message.contains("total_ms=750")
                && message.contains("agent_count=3")
                && message.contains("agent-1:codex:620ms")
        }));
        assert!(TelemetryPassTimings {
            total: std::time::Duration::from_millis(250),
            sys_refresh: std::time::Duration::from_millis(25),
            agent_count: 1,
            slow_agents: Vec::new(),
        }
        .slow_log_message(std::time::Duration::from_millis(500))
        .is_none());
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
    fn live_opencode_tui_output_prevents_log_error_from_masking_running_status() {
        let current_status = "Processing...";
        let log_status = "Error".to_string();
        let last_output_at = Some(std::time::SystemTime::now());

        let status = super::reconcile_live_opencode_log_status(
            "opencode",
            current_status,
            log_status,
            Some(true),
            last_output_at,
        );

        assert_eq!(status, current_status);
    }

    #[test]
    fn opencode_log_error_still_applies_without_live_tui_evidence() {
        let status = super::reconcile_live_opencode_log_status(
            "opencode",
            "Processing...",
            "Error".to_string(),
            Some(true),
            None,
        );

        assert_eq!(status, "Error");

        let status = super::reconcile_live_opencode_log_status(
            "opencode",
            "Processing...",
            "Error".to_string(),
            Some(false),
            Some(std::time::SystemTime::now()),
        );

        assert_eq!(status, "Error");
    }

    #[test]
    fn opencode_telemetry_discovery_ignores_uuid_matching_logs_before_spawn() {
        let temp = tempfile::tempdir().expect("temp dir");
        let log_dir = temp.path().join("log");
        std::fs::create_dir_all(&log_dir).expect("log dir");
        let stale_log = log_dir.join("opencode.log");
        std::fs::write(
            &stale_log,
            "timestamp=2026-06-12T13:47:45.000Z level=INFO run=old message=loading path=\"C:\\\\tmp\\\\agents\\\\opencode-agent\\\\habitat\\\\.opencode\\\\opencode.json\"\n",
        )
        .expect("write stale log");

        let discovered = super::discover_opencode_telemetry_log_path(
            &[log_dir],
            "opencode-agent",
            None,
            Some(std::time::SystemTime::now() + std::time::Duration::from_secs(60)),
        );

        assert_eq!(discovered, None);
    }

    #[test]
    fn opencode_telemetry_discovery_uses_post_spawn_log_until_real_session_exists() {
        let temp = tempfile::tempdir().expect("temp dir");
        let log_dir = temp.path().join("log");
        std::fs::create_dir_all(&log_dir).expect("log dir");
        let log = log_dir.join("opencode.log");
        std::fs::write(
            &log,
            "timestamp=2026-06-12T13:47:45.000Z level=INFO run=new message=loading path=\"C:\\\\tmp\\\\agents\\\\opencode-agent\\\\habitat\\\\.opencode\\\\opencode.json\"\n",
        )
        .expect("write log");

        let discovered = super::discover_opencode_telemetry_log_path(
            &[log_dir],
            "opencode-agent",
            None,
            Some(std::time::UNIX_EPOCH),
        );

        assert_eq!(discovered.as_deref(), Some(log.as_path()));
    }

    #[test]
    fn claude_log_status_can_clear_stale_action_needed() {
        let snap = test_snapshot("Action Needed");
        let lines = vec![
            serde_json::json!({
                "type": "user",
                "message": { "role": "user", "content": "Run a tool" }
            }),
            serde_json::json!({
                "type": "system",
                "subtype": "permission_request",
                "tool_name": "Bash"
            }),
            serde_json::json!({
                "type": "user",
                "message": {
                    "role": "user",
                    "content": [{
                        "type": "tool_result",
                        "tool_use_id": "tool-1",
                        "content": "ok"
                    }]
                }
            }),
            serde_json::json!({ "type": "system", "subtype": "turn_duration" }),
        ];

        super::apply_claude_log_status(&snap, &lines, false);

        assert_eq!(*snap.current_status.lock().unwrap(), "Idle");
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
    fn discover_gemini_log_finds_matching_chat_file() {
        let temp = tempfile::tempdir().expect("temp dir");
        let chats = temp.path().join("project-a").join("chats");
        std::fs::create_dir_all(&chats).expect("chats dir");
        std::fs::write(
            chats.join("other.json"),
            r#"{"sessionId":"other-session","messages":[]}"#,
        )
        .expect("write other chat");
        std::fs::write(
            chats.join("target.json"),
            r#"{"sessionId":"gemini-session-1","messages":[]}"#,
        )
        .expect("write target chat");

        let found = super::discover_gemini_log_in_tmp(temp.path(), "gemini-session-1")
            .expect("matching chat file");
        assert!(found.ends_with("target.json"));
        assert!(super::discover_gemini_log_in_tmp(temp.path(), "missing-session").is_none());
    }

    #[test]
    fn gemini_log_prefix_rejects_id_beyond_prefix_window() {
        let temp = tempfile::tempdir().expect("temp dir");
        let path = temp.path().join("big.json");
        let mut content = String::from("{\"messages\":[\"");
        content.push_str(&"x".repeat(super::GEMINI_LOG_SESSION_PREFIX_BYTES as usize));
        content.push_str("gemini-session-1\"]}");
        std::fs::write(&path, content).expect("write big chat");

        assert!(!super::gemini_log_prefix_contains(
            &path,
            "gemini-session-1"
        ));
        assert!(!super::gemini_log_prefix_contains(&path, ""));
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
