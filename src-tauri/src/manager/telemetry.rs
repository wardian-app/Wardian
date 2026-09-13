use crate::providers::transcript::extract_transcript_message;
use crate::state::AppState;
use crate::utils::fs::{get_wardian_home, observe_codex_indexes};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::OnceLock;
use std::sync::{Arc, Mutex};
use wardian_core::models::{AgentTelemetry, AppTelemetry};

use super::claude::{claude_is_real_user_query, claude_project_dir_name, claude_status_from_log};
use super::codex::{codex_log_lookup_session_id, codex_session_file_path, codex_status_from_log};
use super::display_log_path;
use super::opencode::{
    apply_opencode_log_metrics, opencode_last_assistant_text, opencode_log_dirs,
    opencode_log_path_in, opencode_telemetry_session_id,
    provider_should_fallback_to_idle_after_quiet_period,
};
use crate::providers::antigravity::AntigravityProvider;
use crate::providers::pi::PiProvider;
use wardian_core::control::{ProviderInputReadiness, ProviderReadyEvidence};

const TELEMETRY_SLOW_PASS_THRESHOLD: std::time::Duration = std::time::Duration::from_millis(500);

/// A full process inventory is needed to discover newly spawned descendants,
/// but refreshing every process on every five-second status tick is expensive
/// on Windows. Between inventory refreshes, only the last known agent trees
/// are sampled.
const PROCESS_INVENTORY_REFRESH_TTL: std::time::Duration = std::time::Duration::from_secs(30);

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
/// For capped files the query count and last-query timestamp are derived from
/// the retained tail, while the init timestamp falls back to persisted Born
/// time.
const LOG_PARSE_TAIL_BYTES: u64 = 4 * 1024 * 1024;

/// Restart hydration gets one larger, still bounded, lookback for a provider
/// user record that sits just before a very large assistant/tool record.
const LOG_QUERY_TIMESTAMP_LOOKBACK_BYTES: u64 = 64 * 1024 * 1024;

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

fn read_log_suffix(path: &std::path::Path, max_bytes: u64) -> std::io::Result<String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(path)?;
    let len = file.metadata()?.len();
    let start = len.saturating_sub(max_bytes);
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::with_capacity((len - start) as usize);
    file.read_to_end(&mut bytes)?;
    if start > 0 {
        let Some(first_line_end) = bytes.iter().position(|byte| *byte == b'\n') else {
            return Ok(String::new());
        };
        bytes.drain(..=first_line_end);
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn is_user_query_log_record(provider: &str, value: &serde_json::Value) -> bool {
    match provider {
        "codex" => {
            value.get("type").and_then(|value| value.as_str()) == Some("event_msg")
                && value
                    .get("payload")
                    .and_then(|payload| payload.get("type"))
                    .and_then(|value| value.as_str())
                    == Some("user_message")
        }
        "claude" => {
            value.get("type").and_then(|value| value.as_str()) == Some("user")
                && claude_is_real_user_query(value)
        }
        "pi" => {
            value.get("type").and_then(|value| value.as_str()) == Some("message")
                && value
                    .get("message")
                    .and_then(|message| message.get("role"))
                    .and_then(|value| value.as_str())
                    == Some("user")
        }
        "antigravity" => {
            value.get("source").and_then(|value| value.as_str()) == Some("USER_EXPLICIT")
                && value.get("type").and_then(|value| value.as_str()) == Some("USER_INPUT")
        }
        "gemini" => gemini_message_kind(value) == Some("user"),
        _ => false,
    }
}

fn query_timestamp_from_log_record(provider: &str, value: &serde_json::Value) -> Option<String> {
    let timestamp = match provider {
        "codex" => value.get("timestamp").or_else(|| {
            value
                .get("payload")
                .and_then(|payload| payload.get("timestamp"))
        }),
        "pi" => value.get("timestamp").or_else(|| {
            value
                .get("message")
                .and_then(|message| message.get("timestamp"))
        }),
        "antigravity" => value.get("created_at"),
        _ => value.get("timestamp"),
    };
    query_timestamp_from_value(timestamp)
}

pub(crate) fn latest_query_timestamp_from_log_suffix(
    path: &std::path::Path,
    provider: &str,
) -> Option<String> {
    if provider == "opencode" {
        return None;
    }
    read_log_suffix(path, LOG_QUERY_TIMESTAMP_LOOKBACK_BYTES)
        .ok()?
        .lines()
        .rev()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .find_map(|value| {
            is_user_query_log_record(provider, &value)
                .then(|| query_timestamp_from_log_record(provider, &value))
                .flatten()
        })
}

fn is_antigravity_database(provider: &str, path: &std::path::Path) -> bool {
    provider == "antigravity"
        && path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("db"))
}

/// SQLite commits can update the write-ahead log while leaving the main
/// database file's mtime unchanged. Include both sidecars in the watermark so
/// a live Antigravity conversation is re-read when a new turn reaches WAL.
fn telemetry_source_modified(
    provider: &str,
    path: &std::path::Path,
) -> Option<std::time::SystemTime> {
    let mut latest = std::fs::metadata(path).ok()?.modified().ok()?;
    if is_antigravity_database(provider, path) {
        let file_name = path.file_name()?.to_string_lossy();
        for suffix in ["-wal", "-shm"] {
            let sidecar = path.with_file_name(format!("{file_name}{suffix}"));
            if let Ok(modified) = std::fs::metadata(sidecar).and_then(|meta| meta.modified()) {
                if modified > latest {
                    latest = modified;
                }
            }
        }
    }
    Some(latest)
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

fn should_run_provider_log_telemetry(current_status: &str, process_alive: Option<bool>) -> bool {
    if process_alive == Some(false) {
        // A stopped agent can receive a provider-only prompt that is newer
        // than the durable interaction ledger. Let the source watermark below
        // decide whether the transcript needs parsing.
        return true;
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

fn query_timestamp_from_text(timestamp: &str) -> Option<String> {
    let timestamp = timestamp.trim();
    if timestamp.is_empty() {
        return None;
    }
    if chrono::DateTime::parse_from_rfc3339(timestamp).is_ok() {
        return Some(timestamp.to_string());
    }
    // SQLite's CURRENT_TIMESTAMP is UTC but uses a space separator.
    let sqlite_timestamp = format!("{}Z", timestamp.replace(' ', "T"));
    chrono::DateTime::parse_from_rfc3339(&sqlite_timestamp)
        .ok()
        .map(|parsed| parsed.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
}

fn query_timestamp_from_value(value: Option<&serde_json::Value>) -> Option<String> {
    let value = value?;
    if let Some(timestamp) = value.as_str() {
        return query_timestamp_from_text(timestamp);
    }
    value.as_i64().and_then(|millis| {
        chrono::DateTime::from_timestamp_millis(millis)
            .map(|timestamp| timestamp.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
    })
}

fn query_timestamp_millis(timestamp: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(timestamp)
        .ok()
        .map(|parsed| parsed.timestamp_millis())
}

fn update_latest_query_timestamp(latest: &mut Option<String>, candidate: Option<String>) {
    let Some(candidate) = candidate else {
        return;
    };
    let should_replace = latest.as_deref().is_none_or(|current| {
        match (
            query_timestamp_millis(current),
            query_timestamp_millis(&candidate),
        ) {
            (Some(current), Some(candidate)) => candidate > current,
            _ => candidate.as_str() > current,
        }
    });
    if should_replace {
        *latest = Some(candidate);
    }
}

fn reconcile_cached_last_query_timestamp(
    latest: &mut Option<String>,
    cached_timestamp: &Arc<Mutex<Option<String>>>,
) {
    let Ok(mut cached_timestamp) = cached_timestamp.lock() else {
        return;
    };
    update_latest_query_timestamp(latest, cached_timestamp.clone());
    update_latest_query_timestamp(&mut cached_timestamp, latest.clone());
}

fn latest_user_query_timestamps() -> HashMap<String, String> {
    let mut timestamps = wardian_core::db::list_user_message_timestamp_records()
        .map(|records| latest_user_query_timestamps_from_records(&records))
        .unwrap_or_default();
    if let Ok(records) = wardian_core::db::list_agent_query_timestamp_records() {
        for record in records {
            let mut latest = timestamps.remove(&record.session_id);
            update_latest_query_timestamp(&mut latest, Some(record.last_query_timestamp));
            if let Some(latest) = latest {
                timestamps.insert(record.session_id, latest);
            }
        }
    }
    timestamps
}

fn latest_user_query_timestamps_from_records(
    records: &[wardian_core::db::UserMessageTimestampRecord],
) -> HashMap<String, String> {
    let mut timestamps = HashMap::new();
    for record in records {
        let Some(timestamp) = query_timestamp_from_text(&record.created_at) else {
            continue;
        };
        for session_id in &record.target_session_ids {
            let entry = timestamps
                .entry(session_id.clone())
                .or_insert_with(|| timestamp.clone());
            let mut latest = Some(entry.clone());
            update_latest_query_timestamp(&mut latest, Some(timestamp.clone()));
            if let Some(latest) = latest {
                *entry = latest;
            }
        }
    }
    timestamps
}

#[derive(Debug, PartialEq, Eq)]
struct GeminiLogMetrics {
    query_count: usize,
    init_timestamp: Option<String>,
    last_query_timestamp: Option<String>,
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
                last_query_timestamp: messages
                    .iter()
                    .filter(|message| gemini_message_kind(message) == Some("user"))
                    .fold(None, |mut latest, message| {
                        update_latest_query_timestamp(
                            &mut latest,
                            query_timestamp_from_value(message.get("timestamp")),
                        );
                        latest
                    }),
                status,
            });
        }
    }

    let mut query_count = 0usize;
    let mut init_timestamp = None;
    let mut last_query_timestamp = None;
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
                    update_latest_query_timestamp(
                        &mut last_query_timestamp,
                        query_timestamp_from_value(record.get("timestamp")),
                    );
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
        last_query_timestamp,
        status,
    })
}

#[derive(Debug, Default, PartialEq, Eq)]
struct PiLogMetrics {
    query_count: usize,
    init_timestamp: Option<String>,
    last_query_timestamp: Option<String>,
}

fn parse_pi_log_metrics(content: &str) -> Option<PiLogMetrics> {
    let mut metrics = PiLogMetrics::default();
    let mut saw_record = false;

    for line in content.lines() {
        let Ok(record) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        saw_record = true;

        if record.get("type").and_then(|value| value.as_str()) == Some("session") {
            if metrics.init_timestamp.is_none() {
                metrics.init_timestamp = record
                    .get("timestamp")
                    .and_then(|value| value.as_str())
                    .map(str::to_string);
            }
            continue;
        }

        if record.get("type").and_then(|value| value.as_str()) != Some("message") {
            continue;
        }
        let Some(message) = record.get("message") else {
            continue;
        };
        if message.get("role").and_then(|value| value.as_str()) != Some("user") {
            continue;
        }

        metrics.query_count += 1;
        update_latest_query_timestamp(
            &mut metrics.last_query_timestamp,
            query_timestamp_from_value(
                record.get("timestamp").or_else(|| message.get("timestamp")),
            ),
        );
    }

    (saw_record && (metrics.init_timestamp.is_some() || metrics.query_count > 0)).then_some(metrics)
}

struct AgentSnapshot {
    session_id: String,
    provider: String,
    folder: String,
    is_off: bool,
    resume_session: Option<String>,
    provider_generation: u64,
    process_id: Option<u32>,
    query_count: Arc<Mutex<usize>>,
    init_timestamp: Arc<Mutex<Option<String>>>,
    last_query_timestamp: Arc<Mutex<Option<String>>>,
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

pub(crate) struct TelemetryProviderStatus {
    pub(crate) session_id: String,
    pub(crate) generation: u64,
    pub(crate) status: String,
    pub(crate) current_status: Arc<Mutex<String>>,
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
    children_map: Arc<HashMap<u32, Vec<u32>>>,
    processes: Arc<HashMap<u32, ProcessSample>>,
    sys_refresh: std::time::Duration,
    #[cfg(windows)]
    session_roots: HashMap<String, Vec<u32>>,
}

#[derive(Debug, Clone)]
struct ProcessInventoryCache {
    agent_key: Vec<(String, Option<u32>)>,
    children_map: Arc<HashMap<u32, Vec<u32>>>,
    processes: Arc<HashMap<u32, ProcessSample>>,
    logical_cpu_count: usize,
    refreshed_at: std::time::Instant,
}

static PROCESS_INVENTORY_CACHE: OnceLock<Mutex<Option<ProcessInventoryCache>>> = OnceLock::new();

fn process_inventory_cache() -> &'static Mutex<Option<ProcessInventoryCache>> {
    PROCESS_INVENTORY_CACHE.get_or_init(|| Mutex::new(None))
}

fn process_inventory_agent_key(
    agent_roots: &[(String, Option<u32>)],
) -> Vec<(String, Option<u32>)> {
    let mut key = agent_roots.to_vec();
    key.sort_unstable();
    key
}

fn tracked_process_ids(
    cache: &ProcessInventoryCache,
    agent_roots: &[(String, Option<u32>)],
) -> BTreeSet<u32> {
    let mut tracked = BTreeSet::new();
    // App telemetry reuses the same system sample. Keep the desktop process
    // tree current on fast refreshes without refreshing unrelated processes.
    tracked.extend(collect_related_pids(
        Some(std::process::id()),
        &[],
        cache.children_map.as_ref(),
    ));
    #[cfg(windows)]
    let session_roots = cached_session_roots();
    for (session_id, process_id) in agent_roots {
        #[cfg(windows)]
        let discovered_roots = session_roots
            .get(session_id)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        #[cfg(not(windows))]
        let discovered_roots: &[u32] = &[];

        tracked.extend(collect_related_pids(
            *process_id,
            discovered_roots,
            cache.children_map.as_ref(),
        ));
    }
    tracked
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
    agent_roots: &[(String, Option<u32>)],
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

    let agent_key = process_inventory_agent_key(agent_roots);
    let mut inventory_cache = process_inventory_cache()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let inventory_due = inventory_cache.as_ref().is_none_or(|cache| {
        cache.agent_key != agent_key
            || cache.refreshed_at.elapsed() >= PROCESS_INVENTORY_REFRESH_TTL
    });
    #[cfg(windows)]
    // Process-ID changes invalidate the lightweight inventory cache, but they
    // do not mean that every process's command line and environment need to
    // be re-read. Provider restarts are common, and coupling them to marker
    // discovery turns a cheap inventory refresh into an expensive Windows PEB
    // scan. Marker discovery has its own session/TTL invalidation above.
    let discovery_due = session_root_discovery_due(session_ids);
    #[cfg(not(windows))]
    let discovery_due = false;

    // A full refresh walks every process and, when discovery is due, reads
    // every command line/environment block. Between those refreshes, sample
    // only the last known agent trees. New descendants are picked up on the
    // next inventory refresh, while existing agents retain five-second CPU,
    // memory, and liveness updates.
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
    if inventory_due || discovery_due {
        sys.refresh_processes_specifics(sysinfo::ProcessesToUpdate::All, true, refresh_kind);
    } else {
        let tracked = tracked_process_ids(
            inventory_cache
                .as_ref()
                .expect("an inventory cache is required for a tracked refresh"),
            agent_roots,
        );
        let tracked_pids = tracked
            .iter()
            .map(|pid| sysinfo::Pid::from_u32(*pid))
            .collect::<Vec<_>>();
        if !tracked_pids.is_empty() {
            sys.refresh_processes_specifics(
                sysinfo::ProcessesToUpdate::Some(&tracked_pids),
                true,
                refresh_kind,
            );
        }
    }
    let sys_refresh = sys_refresh_started.elapsed();

    if !inventory_due && !discovery_due {
        let tracked = tracked_process_ids(
            inventory_cache
                .as_ref()
                .expect("an inventory cache is required for a tracked refresh"),
            agent_roots,
        );
        let mut processes = inventory_cache
            .as_ref()
            .expect("an inventory cache is required for a tracked refresh")
            .processes
            .as_ref()
            .clone();
        for pid in tracked {
            let key = sysinfo::Pid::from_u32(pid);
            if let Some(process) = sys.process(key) {
                processes.insert(
                    pid,
                    ProcessSample {
                        cpu_usage: process.cpu_usage(),
                        memory: process.memory(),
                        run_time: process.run_time(),
                    },
                );
            } else {
                processes.remove(&pid);
            }
        }
        let processes = Arc::new(processes);
        let cache = inventory_cache
            .as_mut()
            .expect("an inventory cache is required for a tracked refresh");
        cache.processes = processes.clone();
        return Some(SystemProcessSnapshot {
            logical_cpu_count: cache.logical_cpu_count,
            children_map: cache.children_map.clone(),
            processes,
            sys_refresh,
            #[cfg(windows)]
            session_roots: cached_session_roots(),
        });
    }

    let logical_cpu_count = sys.cpus().len();
    let mut children_map: HashMap<u32, Vec<u32>> = HashMap::new();
    #[cfg(windows)]
    let mut process_markers = Vec::new();

    for (pid, process) in sys.processes() {
        let pid = pid.as_u32();
        if let Some(parent) = process.parent() {
            children_map.entry(parent.as_u32()).or_default().push(pid);
        }
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

    let mut cache = ProcessInventoryCache {
        agent_key,
        children_map: Arc::new(children_map),
        processes: Arc::new(HashMap::new()),
        logical_cpu_count,
        refreshed_at: std::time::Instant::now(),
    };
    let tracked = tracked_process_ids(&cache, agent_roots);
    let processes = tracked
        .into_iter()
        .filter_map(|pid| {
            sys.process(sysinfo::Pid::from_u32(pid)).map(|process| {
                (
                    pid,
                    ProcessSample {
                        cpu_usage: process.cpu_usage(),
                        memory: process.memory(),
                        run_time: process.run_time(),
                    },
                )
            })
        })
        .collect::<HashMap<_, _>>();
    cache.processes = Arc::new(processes);
    let snapshot = SystemProcessSnapshot {
        logical_cpu_count,
        children_map: cache.children_map.clone(),
        processes: cache.processes.clone(),
        sys_refresh,
        #[cfg(windows)]
        session_roots,
    };
    *inventory_cache = Some(cache);
    Some(snapshot)
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
    if is_initial_replay
        || super::should_suppress_interrupted_status(&snap.current_status, next_status)
    {
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

fn parse_antigravity_log_metrics(
    content: &str,
) -> (usize, Option<String>, Option<&'static str>, Option<String>) {
    let mut query_count = 0;
    let mut init_timestamp = None;
    let mut last_query_timestamp = None;
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
                update_latest_query_timestamp(
                    &mut last_query_timestamp,
                    query_timestamp_from_value(parsed.get("created_at")),
                );
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

    (query_count, init_timestamp, status, last_query_timestamp)
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
                    is_off: config.is_off,
                    resume_session: opencode_telemetry_session_id(&config),
                    provider_generation: 0,
                    process_id: agent.process_id,
                    query_count: agent.query_count.clone(),
                    init_timestamp: agent.init_timestamp.clone(),
                    last_query_timestamp: agent.last_query_timestamp.clone(),
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
        let mut last_user_query_timestamps = latest_user_query_timestamps();
        let agent_roots = snapshots
            .iter()
            .map(|snap| (snap.session_id.clone(), snap.process_id))
            .collect::<Vec<_>>();
        let system_snapshot =
            refresh_system_process_snapshot(&sys_metrics, &session_ids, &agent_roots);
        let mut slow_agents = Vec::new();
        observe_codex_indexes();

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
            let opencode_session_id = snap.resume_session.as_deref();
            let gemini_session_id = snap.resume_session.as_deref();
            let status_before_log_work = snap.current_status.lock().unwrap().clone();
            let mut last_query_timestamp = last_user_query_timestamps.remove(&snap.session_id);
            reconcile_cached_last_query_timestamp(
                &mut last_query_timestamp,
                &snap.last_query_timestamp,
            );
            let run_provider_log_work =
                should_run_provider_log_telemetry(&status_before_log_work, process_alive);

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
                        let mut discovered_log = None;
                        if let Some(opencode_session_id) = opencode_session_id {
                            for dir in opencode_log_dirs() {
                                if let Some(path) = opencode_log_path_in(&dir, opencode_session_id)
                                {
                                    discovered_log = Some(path);
                                    break;
                                }
                            }
                        }
                        *log_path_lock = discovered_log;
                    } else if snap.provider == "antigravity" {
                        let conversation_id = snap
                            .resume_session
                            .as_ref()
                            .map(|value| value.trim().to_string())
                            .filter(|value| !value.is_empty());
                        if let (Some(home), Some(conversation_id)) =
                            (AntigravityProvider::antigravity_home(), conversation_id)
                        {
                            if let Some(candidate) = AntigravityProvider::conversation_status_path(
                                &home,
                                &conversation_id,
                            ) {
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
                    } else if snap.provider == "pi" {
                        if let Some(provider_session_id) = snap.resume_session.as_deref() {
                            if let Some(session_dir) = PiProvider::session_dir(&snap.session_id) {
                                if let Some(path) =
                                    PiProvider::session_file(&session_dir, provider_session_id)
                                {
                                    *log_path_lock = Some(path);
                                }
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
                        if let Some(modified) =
                            telemetry_source_modified(snap.provider.as_str(), path)
                        {
                            let last_mod = *snap.log_last_modified.lock().unwrap();
                            if last_mod == Some(modified) {
                                should_parse = false;
                            } else {
                                is_initial_log_replay = last_mod.is_none();
                                new_mtime = Some(modified);
                            }
                        }

                        if should_parse {
                            if is_antigravity_database(snap.provider.as_str(), path) {
                                if let Ok(metrics) =
                                    AntigravityProvider::conversation_metrics_from_database(path)
                                {
                                    if let Some(mtime) = new_mtime {
                                        *snap.log_last_modified.lock().unwrap() = Some(mtime);
                                    }
                                    q_count = metrics.query_count;
                                    update_latest_query_timestamp(
                                        &mut last_query_timestamp,
                                        metrics.last_query_timestamp,
                                    );
                                    if let Some(status) = metrics.status {
                                        set_snapshot_status_from_log(
                                            snap,
                                            status,
                                            is_initial_log_replay,
                                        );
                                    }
                                    if metrics.init_timestamp.is_some() {
                                        i_ts = metrics.init_timestamp;
                                    }
                                }
                            } else if let Ok(content) = read_log_bounded(path) {
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

                                        for line in lines.iter().filter(|line| {
                                            line.get("type").and_then(|value| value.as_str())
                                                == Some("event_msg")
                                                && line
                                                    .get("payload")
                                                    .and_then(|value| value.get("type"))
                                                    .and_then(|value| value.as_str())
                                                    == Some("user_message")
                                        }) {
                                            update_latest_query_timestamp(
                                                &mut last_query_timestamp,
                                                query_timestamp_from_value(
                                                    line.get("timestamp").or_else(|| {
                                                        line.get("payload").and_then(|payload| {
                                                            payload.get("timestamp")
                                                        })
                                                    }),
                                                ),
                                            );
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

                                        for line in lines.iter().filter(|line| {
                                            line.get("type").and_then(|value| value.as_str())
                                                == Some("user")
                                                && claude_is_real_user_query(line)
                                        }) {
                                            update_latest_query_timestamp(
                                                &mut last_query_timestamp,
                                                query_timestamp_from_value(line.get("timestamp")),
                                            );
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
                                            &mut last_query_timestamp,
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
                                        let (queries, start_time, status, latest_query) =
                                            parse_antigravity_log_metrics(&content);
                                        q_count = queries;
                                        update_latest_query_timestamp(
                                            &mut last_query_timestamp,
                                            latest_query,
                                        );
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
                                    "pi" => {
                                        if let Some(metrics) = parse_pi_log_metrics(&content) {
                                            q_count = metrics.query_count;
                                            if let Some(start_time) = metrics.init_timestamp {
                                                i_ts = Some(start_time);
                                            }
                                            update_latest_query_timestamp(
                                                &mut last_query_timestamp,
                                                metrics.last_query_timestamp,
                                            );
                                        }
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
                                            update_latest_query_timestamp(
                                                &mut last_query_timestamp,
                                                metrics.last_query_timestamp,
                                            );
                                        }
                                        if snap.provider == "gemini" {
                                            record_latest_gemini_assistant_text(snap, &content);
                                        }
                                    }
                                }
                                if is_initial_log_replay {
                                    update_latest_query_timestamp(
                                        &mut last_query_timestamp,
                                        latest_query_timestamp_from_log_suffix(
                                            path,
                                            snap.provider.as_str(),
                                        ),
                                    );
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

            reconcile_cached_last_query_timestamp(
                &mut last_query_timestamp,
                &snap.last_query_timestamp,
            );
            if let Some(timestamp) = last_query_timestamp.as_deref() {
                let _ = wardian_core::db::update_agent_query_timestamp(&snap.session_id, timestamp);
            }

            if (snap.provider == "opencode"
                || snap.provider == "claude"
                || snap.provider == "antigravity")
                && (snap.process_id.is_none() || process_alive == Some(true))
            {
                let current_status = snap.current_status.lock().unwrap().clone();
                let last_output_at = *snap.last_output_at.lock().unwrap();
                if provider_should_fallback_to_idle_after_quiet_period(
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

            let observed_status = snap.current_status.lock().unwrap().clone();
            let is_offline = snap.is_off
                || matches!(
                    wardian_core::identity::normalize_status(&observed_status).as_str(),
                    "off" | "error"
                );
            let current_status = if is_offline
                && wardian_core::conversation_lease::find_active_execution_conflict(
                    &active_leases,
                    &snap.session_id,
                    snap.resume_session.as_deref().unwrap_or_default(),
                    &lease_now,
                )
                .is_some()
            {
                "Headless".to_string()
            } else {
                observed_status
            };
            provider_statuses.push(TelemetryProviderStatus {
                session_id: snap.session_id.clone(),
                generation: snap.provider_generation,
                status: snap.current_status.lock().unwrap().clone(),
                current_status: snap.current_status.clone(),
            });

            results.push(AgentTelemetry {
                session_id: snap.session_id.clone(),
                cpu_usage: cpu,
                memory_mb: mem,
                uptime_seconds: uptime,
                query_count: *snap.query_count.lock().unwrap(),
                init_timestamp: snap.init_timestamp.lock().unwrap().clone(),
                last_query_timestamp,
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
        let readiness = super::publish_telemetry_status_observation(state, observation).await;
        let ready_evidence = (readiness == ProviderInputReadiness::Ready)
            .then_some(ProviderReadyEvidence::ProviderEvent);
        let (_, became_ready) = state
            .interactions
            .record_provider_input_state_with_transition(
                &observation.session_id,
                observation.generation,
                readiness,
                ready_evidence,
            )
            .await;
        if became_ready {
            crate::control::drain_mailbox_for_idle_agent_from_status_observation(
                None,
                state,
                &observation.session_id,
            )
            .await;
        }
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
    use rusqlite::Connection;
    use std::collections::{BTreeSet, HashMap};
    use std::sync::{Arc, Mutex};

    fn test_snapshot(status: &str) -> AgentSnapshot {
        AgentSnapshot {
            session_id: "agent-1".to_string(),
            provider: "opencode".to_string(),
            folder: "D:/work".to_string(),
            is_off: false,
            resume_session: None,
            provider_generation: 0,
            process_id: Some(1234),
            query_count: Arc::new(Mutex::new(0)),
            init_timestamp: Arc::new(Mutex::new(None)),
            last_query_timestamp: Arc::new(Mutex::new(None)),
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
    fn cached_provider_query_timestamp_survives_an_unchanged_log_pass() {
        let snap = test_snapshot("Idle");
        *snap.last_query_timestamp.lock().unwrap() = Some("2026-05-14T12:00:03.000Z".to_string());
        let mut latest = Some("2026-05-14T12:00:01.000Z".to_string());

        super::reconcile_cached_last_query_timestamp(&mut latest, &snap.last_query_timestamp);

        assert_eq!(latest.as_deref(), Some("2026-05-14T12:00:03.000Z"));
        assert_eq!(
            snap.last_query_timestamp.lock().unwrap().as_deref(),
            Some("2026-05-14T12:00:03.000Z")
        );
    }

    #[test]
    fn stopped_agents_reconcile_provider_logs_even_with_durable_queries() {
        assert!(super::should_run_provider_log_telemetry("Off", Some(false)));
    }

    #[test]
    fn antigravity_wal_activity_advances_the_telemetry_watermark() {
        let temp = tempfile::tempdir().expect("temp dir");
        let database = temp.path().join("conversation.db");
        let writer = Connection::open(&database).expect("open database");
        writer
            .execute_batch(
                "PRAGMA journal_mode = WAL;
                 CREATE TABLE steps (idx INTEGER, step_type INTEGER, metadata BLOB);
                 INSERT INTO steps (idx, step_type) VALUES (1, 14);",
            )
            .expect("create WAL fixture");
        let before = super::telemetry_source_modified("antigravity", &database)
            .expect("initial database watermark");

        std::thread::sleep(std::time::Duration::from_millis(20));
        writer
            .execute(
                "INSERT INTO steps (idx, step_type) VALUES (?1, ?2)",
                rusqlite::params![2_i64, 14_i64],
            )
            .expect("append WAL user message");

        assert!(database.with_file_name("conversation.db-wal").exists());
        let after = super::telemetry_source_modified("antigravity", &database)
            .expect("updated database watermark");
        assert!(after > before, "WAL activity must invalidate the cache");
    }

    #[test]
    fn restart_hydration_recovers_a_user_timestamp_before_a_large_jsonl_tail() {
        let temp = tempfile::tempdir().expect("temp dir");
        let log = temp.path().join("rollout.jsonl");
        let user_message = serde_json::json!({
            "type": "event_msg",
            "timestamp": "2026-08-26T12:00:00.000Z",
            "payload": { "type": "user_message", "message": "hello" },
        });
        let large_assistant_record = serde_json::json!({
            "type": "response_item",
            "payload": {
                "type": "message",
                "role": "assistant",
                "content": "x".repeat((super::LOG_PARSE_TAIL_BYTES + 1024) as usize),
            },
        });
        std::fs::write(
            &log,
            format!(
                "{}\n{}\n",
                user_message,
                serde_json::to_string(&large_assistant_record).expect("serialize assistant")
            ),
        )
        .expect("write oversized provider log");

        assert!(std::fs::metadata(&log).expect("log metadata").len() > super::LOG_PARSE_TAIL_BYTES);
        assert_eq!(
            super::latest_query_timestamp_from_log_suffix(&log, "codex").as_deref(),
            Some("2026-08-26T12:00:00.000Z")
        );
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

    #[cfg(windows)]
    #[test]
    fn tracked_process_refresh_reuses_inventory_and_costs_less_than_full_scan() {
        let mut cache = super::process_inventory_cache().lock().unwrap();
        *cache = None;
        drop(cache);

        let system = tokio::sync::Mutex::new(sysinfo::System::new());
        let session_ids = (0..58)
            .map(|index| format!("agent-{index}"))
            .collect::<Vec<_>>();
        let agent_roots = session_ids
            .iter()
            .cloned()
            .map(|session_id| (session_id, Some(std::process::id())))
            .collect::<Vec<_>>();

        let full = super::refresh_system_process_snapshot(&system, &session_ids, &agent_roots)
            .expect("full inventory refresh should succeed");
        let tracked = super::refresh_system_process_snapshot(&system, &session_ids, &agent_roots)
            .expect("tracked refresh should succeed");

        assert!(std::sync::Arc::ptr_eq(
            &full.children_map,
            &tracked.children_map
        ));
        assert!(tracked.processes.contains_key(&std::process::id()));
        eprintln!(
            "telemetry process refresh: full={:?}, tracked={:?}",
            full.sys_refresh, tracked.sys_refresh
        );
        assert!(tracked.sys_refresh < full.sys_refresh);
    }

    #[test]
    fn process_inventory_agent_key_is_order_independent() {
        let left = super::process_inventory_agent_key(&[
            ("agent-2".to_string(), Some(2)),
            ("agent-1".to_string(), Some(1)),
        ]);
        let right = super::process_inventory_agent_key(&[
            ("agent-1".to_string(), Some(1)),
            ("agent-2".to_string(), Some(2)),
        ]);

        assert_eq!(left, right);
    }

    #[cfg(windows)]
    #[test]
    fn changing_agent_process_id_does_not_force_marker_discovery() {
        let session_id = "pid-churn-marker-discovery-test".to_string();
        let session_ids = vec![session_id.clone()];
        let cached_markers = HashMap::from([(session_id.clone(), vec![12345])]);

        *super::process_inventory_cache().lock().unwrap() = None;
        *super::session_roots_cache().lock().unwrap() = Some(super::SessionRootsCache {
            roots: cached_markers.clone(),
            refreshed_at: std::time::Instant::now(),
            session_key: super::sorted_session_key(&session_ids),
        });

        let system = tokio::sync::Mutex::new(sysinfo::System::new());
        super::refresh_system_process_snapshot(
            &system,
            &session_ids,
            &[(session_id.clone(), Some(101))],
        )
        .expect("initial inventory refresh should succeed");

        // Re-seed the marker cache so the assertion observes whether the PID
        // change caused a second marker scan, rather than its initial setup.
        *super::session_roots_cache().lock().unwrap() = Some(super::SessionRootsCache {
            roots: cached_markers.clone(),
            refreshed_at: std::time::Instant::now(),
            session_key: super::sorted_session_key(&session_ids),
        });

        super::refresh_system_process_snapshot(&system, &session_ids, &[(session_id, Some(202))])
            .expect("PID-churn inventory refresh should succeed");

        assert_eq!(super::cached_session_roots(), cached_markers);
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
            { "type": "user", "timestamp": "2026-05-14T12:00:01.000Z", "content": "hello" },
            { "type": "gemini", "content": "hi" }
          ]
        }"#;

        let metrics = super::parse_gemini_log_metrics(content).expect("metrics");

        assert_eq!(metrics.query_count, 1);
        assert_eq!(
            metrics.init_timestamp.as_deref(),
            Some("2026-05-14T12:00:00.000Z")
        );
        assert_eq!(
            metrics.last_query_timestamp.as_deref(),
            Some("2026-05-14T12:00:01.000Z")
        );
        assert_eq!(metrics.status, Some("Idle"));
    }

    #[test]
    fn pi_log_metrics_parse_latest_user_message_timestamp() {
        let content = concat!(
            r#"{"type":"session","id":"pi-session-1","timestamp":"2026-05-14T12:00:00.000Z"}"#,
            "\n",
            r#"{"type":"message","timestamp":"2026-05-14T12:00:01.000Z","message":{"role":"user","content":"first"}}"#,
            "\n",
            r#"{"type":"message","timestamp":"2026-05-14T12:00:03.000Z","message":{"role":"user","content":"latest"}}"#,
            "\n"
        );

        let metrics = super::parse_pi_log_metrics(content).expect("metrics");

        assert_eq!(metrics.query_count, 2);
        assert_eq!(
            metrics.init_timestamp.as_deref(),
            Some("2026-05-14T12:00:00.000Z")
        );
        assert_eq!(
            metrics.last_query_timestamp.as_deref(),
            Some("2026-05-14T12:00:03.000Z")
        );
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
        assert_eq!(
            metrics.last_query_timestamp.as_deref(),
            Some("2026-05-14T12:00:01.000Z")
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
        assert_eq!(
            metrics.last_query_timestamp.as_deref(),
            Some("2026-05-14T12:00:01.000Z")
        );
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
