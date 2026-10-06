use crate::providers::transcript::extract_transcript_message;
use crate::state::AppState;
use crate::utils::fs::{get_wardian_home, observe_codex_indexes};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};
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

#[cfg(test)]
#[path = "telemetry/background_capture_tests.rs"]
mod background_capture_tests;
mod collection;
use collection::collect_agent_metrics_with_sampler;
mod sampling;
mod status;
mod timings;
#[cfg(test)]
use status::apply_telemetry_provider_readiness;
use status::{apply_provider_status_observations, set_snapshot_status};
use timings::{PhaseClock, TelemetryPassTimings, TelemetrySlowAgent};

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
    conversation_logging: wardian_core::conversations::AgentConversationLoggingSetting,
    capture_conversation: Option<String>,
    provider_generation: u64,
    process_id: Option<u32>,
    query_count: Arc<Mutex<usize>>,
    init_timestamp: Arc<Mutex<Option<String>>>,
    last_query_timestamp: Arc<Mutex<Option<String>>>,
    current_status: Arc<Mutex<String>>,
    status_observation: Mutex<TelemetryStatusDraft>,
    watch_state: Arc<Mutex<crate::state::AgentWatchState>>,
    last_output_at: Arc<Mutex<Option<std::time::SystemTime>>>,
    log_path: Arc<Mutex<Option<std::path::PathBuf>>>,
    log_last_modified: Arc<Mutex<Option<std::time::SystemTime>>>,
}

#[derive(Default)]
struct TelemetryStatusDraft {
    initial_status: String,
    current_status: String,
    initial_status_revision: u64,
    initial_status_intent_revision: u64,
    transitions: Vec<TelemetryStatusTransition>,
}

#[derive(Clone)]
pub(crate) struct TelemetryStatusTransition {
    pub(crate) previous_status: String,
    pub(crate) status: String,
    pub(crate) observed_at: String,
}

#[derive(Default)]
struct TelemetryPassResult {
    metrics: Vec<AgentTelemetry>,
    provider_statuses: Vec<TelemetryProviderStatus>,
    background_captures: Vec<crate::state::background_capture::CaptureRequest>,
}

#[derive(Clone)]
pub(crate) struct TelemetryProviderStatus {
    pub(crate) session_id: String,
    pub(crate) generation: u64,
    pub(crate) initial_status: String,
    pub(crate) initial_status_revision: u64,
    pub(crate) initial_status_intent_revision: u64,
    pub(crate) status: String,
    pub(crate) transitions: Vec<TelemetryStatusTransition>,
    pub(crate) active_execution_conflict: bool,
    pub(crate) current_status: Arc<Mutex<String>>,
}

#[cfg(test)]
impl TelemetryProviderStatus {
    pub(crate) fn current(
        session_id: impl Into<String>,
        generation: u64,
        status: String,
        current_status: Arc<Mutex<String>>,
    ) -> Self {
        Self {
            session_id: session_id.into(),
            generation,
            initial_status: status.clone(),
            initial_status_revision: 0,
            initial_status_intent_revision: 0,
            status,
            transitions: Vec::new(),
            active_execution_conflict: false,
            current_status,
        }
    }
}

impl AgentSnapshot {
    fn capture_initial_status(&self, state: &AppState) {
        let current = self.current_status.lock().unwrap();
        let status = current.clone();
        let initial_status_revision = state.status_revision(&self.session_id, &self.current_status);
        let initial_status_intent_revision =
            state.status_intent_revision(&self.session_id, &self.current_status);
        drop(current);
        let mut observation = self.status_observation.lock().unwrap();
        observation.initial_status = status.clone();
        observation.current_status = status;
        observation.initial_status_revision = initial_status_revision;
        observation.initial_status_intent_revision = initial_status_intent_revision;
        observation.transitions.clear();
    }

    fn telemetry_status(&self) -> String {
        self.status_observation
            .lock()
            .unwrap()
            .current_status
            .clone()
    }

    fn provider_status_observation(
        &self,
        active_execution_conflict: bool,
    ) -> TelemetryProviderStatus {
        let draft = self.status_observation.lock().unwrap();
        TelemetryProviderStatus {
            session_id: self.session_id.clone(),
            generation: self.provider_generation,
            initial_status: draft.initial_status.clone(),
            initial_status_revision: draft.initial_status_revision,
            initial_status_intent_revision: draft.initial_status_intent_revision,
            status: draft.current_status.clone(),
            transitions: draft.transitions.clone(),
            active_execution_conflict,
            current_status: self.current_status.clone(),
        }
    }
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

fn set_snapshot_status_from_log(snap: &AgentSnapshot, next_status: &str, is_initial_replay: bool) {
    if is_initial_replay
        || super::should_suppress_interrupted_status(&snap.current_status, next_status)
    {
        return;
    }
    // An append does not make the rolling log's old turn state belong to this
    // process. Check the live status here: a composer repaint may have ended
    // startup while telemetry was reading the log. Never restore stale Starting.
    if snap.provider == "opencode" {
        let live_starting = snap
            .current_status
            .lock()
            .is_ok_and(|status| status.eq_ignore_ascii_case("Starting"));
        if live_starting {
            return;
        }
    }
    set_snapshot_status(snap, next_status);
}

fn telemetry_display_status(status: &str, active_execution_conflict: bool) -> String {
    if active_execution_conflict
        && matches!(
            wardian_core::identity::normalize_status(status).as_str(),
            "off" | "error"
        )
    {
        "Headless".to_string()
    } else {
        status.to_string()
    }
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
            provider_provenance: None,
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

/// Samples every agent and publishes the resulting status observations.
///
/// Anything that can wait on a lifecycle gate or the provider is handed to
/// detached workers rather than awaited, so the tick never sits behind it.
pub async fn get_all_metrics(state: &AppState, app: &tauri::AppHandle) -> Vec<AgentTelemetry> {
    let (metrics, follow_up) = collect_agent_metrics(state).await;
    status::spawn_follow_up(app, follow_up);
    metrics
}

async fn collect_agent_metrics(state: &AppState) -> (Vec<AgentTelemetry>, status::StatusFollowUp) {
    collect_agent_metrics_with_sampler(state, sampling::sample_processes).await
}

pub(crate) fn commit_telemetry_status_observation(
    state: &AppState,
    observation: &TelemetryProviderStatus,
    current_status: &Arc<Mutex<String>>,
    last_status_at: &Arc<Mutex<Option<String>>>,
    watch_state: &Arc<Mutex<crate::state::AgentWatchState>>,
    codex_attachment_ready: bool,
) -> Option<(String, u64, u64)> {
    status::commit_snapshot_status_observation(
        state,
        observation,
        current_status,
        last_status_at,
        watch_state,
        codex_attachment_ready,
    )
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
#[path = "telemetry/tests.rs"]
pub(crate) mod tests;
