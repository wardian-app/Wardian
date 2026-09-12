use super::codex::codex_bootstrap_workspace_key;
use crate::utils::fs::*;
use chrono::TimeZone;
use wardian_core::models::AgentConfig;
pub(crate) fn opencode_status_from_title(title: &str) -> Option<&'static str> {
    let trimmed = title.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed == "OpenCode" {
        return Some("Idle");
    }
    if trimmed.contains("Action Required") {
        return Some("Action Needed");
    }
    if trimmed.starts_with("OC | ") {
        return Some("Processing...");
    }
    None
}

/// Extracts the owning session id from a 1.17+ prompt-loop log line
/// (`message=loop session.id=ses_… step=N` or
/// `message="exiting loop" session.id=ses_…`). Returns `None` for any other
/// line, including pre-1.17 markers and permission lines (which carry no
/// session id at all).
fn opencode_loop_line_session_id(line: &str) -> Option<String> {
    if !line.contains("message=loop") && !line.contains("message=\"exiting loop\"") {
        return None;
    }
    line.split_whitespace()
        .find_map(|token| token.strip_prefix("session.id="))
        .map(str::to_string)
}

fn opencode_data_dirs_from_roots(
    xdg_data_home: Option<&std::path::Path>,
    data_local_dir: Option<std::path::PathBuf>,
    data_dir: Option<std::path::PathBuf>,
    home_dir: Option<std::path::PathBuf>,
) -> Vec<std::path::PathBuf> {
    let mut dirs = Vec::new();
    let mut push_unique = |path: std::path::PathBuf| {
        if !path.as_os_str().is_empty() && !dirs.contains(&path) {
            dirs.push(path);
        }
    };

    // OpenCode follows XDG_DATA_HOME even on Windows. It must win over the
    // platform default so isolated provider runs do not get attributed to a
    // different installation's database or rolling log.
    if let Some(xdg_data_home) = xdg_data_home {
        push_unique(xdg_data_home.join("opencode"));
    }
    if let Some(data_local_dir) = data_local_dir {
        push_unique(data_local_dir.join("opencode"));
    }
    if let Some(data_dir) = data_dir {
        push_unique(data_dir.join("opencode"));
    }
    if let Some(home_dir) = home_dir {
        push_unique(home_dir.join(".local").join("share").join("opencode"));
    }
    dirs
}

fn opencode_data_dirs() -> Vec<std::path::PathBuf> {
    let xdg_data_home = std::env::var_os("XDG_DATA_HOME").map(std::path::PathBuf::from);
    opencode_data_dirs_from_roots(
        xdg_data_home.as_deref(),
        dirs::data_local_dir(),
        dirs::data_dir(),
        dirs::home_dir(),
    )
}

pub(crate) fn opencode_database_path() -> Option<std::path::PathBuf> {
    opencode_data_dirs()
        .into_iter()
        .map(|dir| dir.join("opencode.db"))
        .find(|path| path.exists())
}

pub(crate) fn opencode_telemetry_session_id(config: &AgentConfig) -> Option<String> {
    if config.provider != "opencode" {
        return config.resume_session.clone();
    }
    config
        .resume_session
        .as_deref()
        .filter(|value| value.starts_with("ses_"))
        .map(ToString::to_string)
        .or_else(|| {
            config
                .fresh_provider_session_id
                .as_deref()
                .filter(|value| value.starts_with("ses_"))
                .map(ToString::to_string)
        })
}

/// Captures the OpenCode session created by this Wardian launch. OpenCode's
/// session list is global to the provider data root, so a directory/time match
/// alone is not ownership evidence when multiple agents share a workspace.
/// The provider log records the run that loaded the agent's generated config;
/// only a unique session created by that same run is accepted.
pub(crate) fn opencode_recent_session_for_workspace(
    workspace: &std::path::Path,
    created_after_ms: i64,
    wardian_session_id: &str,
) -> Option<String> {
    let mut candidates = std::collections::HashSet::new();
    for directory in opencode_log_dirs() {
        for path in opencode_log_files_in(&directory) {
            let Ok(content) = std::fs::read_to_string(path) else {
                continue;
            };
            if let Some(session_id) = select_opencode_session_from_launch_log(
                &content,
                workspace,
                created_after_ms,
                wardian_session_id,
            ) {
                candidates.insert(session_id);
            }
        }
    }

    (candidates.len() == 1).then(|| candidates.into_iter().next().expect("one candidate"))
}

type OpenCodeLogRevision = Vec<(std::path::PathBuf, u64, Option<std::time::SystemTime>)>;

/// Discovers an owned session independently of title-derived activity. Unchanged
/// logs require metadata checks only, including while an idle agent awaits input.
#[derive(Default)]
pub(crate) struct OpenCodeSessionDiscovery {
    previous: Option<OpenCodeLogRevision>,
}

impl OpenCodeSessionDiscovery {
    pub(crate) fn poll(
        &mut self,
        status: &str,
        workspace: &std::path::Path,
        created_after_ms: i64,
        wardian_session_id: &str,
    ) -> Option<String> {
        let mut revision = opencode_log_dirs()
            .iter()
            .flat_map(|directory| opencode_log_files_in(directory))
            .filter_map(|path| {
                let metadata = std::fs::metadata(&path).ok()?;
                Some((path, metadata.len(), metadata.modified().ok()))
            })
            .collect::<Vec<_>>();
        revision.sort_by(|left, right| left.0.cmp(&right.0));
        self.poll_with(status, revision, || {
            opencode_recent_session_for_workspace(workspace, created_after_ms, wardian_session_id)
        })
    }

    fn poll_with(
        &mut self,
        status: &str,
        revision: OpenCodeLogRevision,
        lookup: impl FnOnce() -> Option<String>,
    ) -> Option<String> {
        if wardian_core::identity::normalize_status(status) == "off"
            || self.previous.as_ref() == Some(&revision)
        {
            return None;
        }
        self.previous = Some(revision);
        lookup()
    }
}

fn normalize_opencode_workspace(path: &str) -> String {
    #[cfg(windows)]
    {
        path.replace('/', "\\")
            .trim_end_matches(['\\', '/'])
            .to_ascii_lowercase()
    }
    #[cfg(not(windows))]
    {
        let normalized = path.trim_end_matches('/');
        if normalized.is_empty() {
            "/".to_string()
        } else {
            normalized.to_string()
        }
    }
}

fn opencode_log_field(line: &str, key: &str) -> Option<String> {
    let marker = format!("{key}=");
    let start = line.find(&marker)? + marker.len();
    let value = &line[start..];
    if let Some(value) = value.strip_prefix('"') {
        let mut decoded = String::new();
        let mut characters = value.chars().peekable();
        while let Some(character) = characters.next() {
            match character {
                '"' => return Some(decoded),
                '\\' if matches!(characters.peek(), Some('\\' | '"')) => {
                    decoded.push(characters.next().expect("peeked escaped character"));
                }
                '\\' => decoded.push('\\'),
                character => decoded.push(character),
            }
        }
        None
    } else {
        value.split_whitespace().next().map(ToString::to_string)
    }
}

fn opencode_agent_config_path_matches(path: &str, wardian_session_id: &str) -> bool {
    let marker = if cfg!(windows) {
        format!(
            "agents\\{}\\habitat\\.opencode\\opencode.json",
            wardian_session_id.trim()
        )
    } else {
        format!(
            "agents/{}/habitat/.opencode/opencode.json",
            wardian_session_id.trim()
        )
    };
    normalize_opencode_workspace(path).contains(&normalize_opencode_workspace(&marker))
}

fn opencode_log_files_in(base: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut paths = Vec::new();
    let rolling = base.join("opencode.log");
    if rolling.is_file() {
        paths.push(rolling);
        return paths;
    }
    let mut dated = std::fs::read_dir(base)
        .ok()
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name().and_then(|name| name.to_str()) != Some("opencode.log")
                && path.extension().and_then(|extension| extension.to_str()) == Some("log")
        })
        .collect::<Vec<_>>();
    dated.sort();
    dated.reverse();
    paths.extend(dated);
    paths
}

fn select_opencode_session_from_launch_log(
    content: &str,
    workspace: &std::path::Path,
    created_after_ms: i64,
    wardian_session_id: &str,
) -> Option<String> {
    let expected = normalize_opencode_workspace(&workspace.to_string_lossy());
    let owned_runs = content
        .lines()
        .filter(|line| line.contains("message=loading"))
        .filter_map(|line| {
            let path = opencode_log_field(line, "path")?;
            opencode_agent_config_path_matches(&path, wardian_session_id)
                .then(|| opencode_log_field(line, "run"))
                .flatten()
        })
        .collect::<std::collections::HashSet<_>>();

    let candidates = content
        .lines()
        .filter(|line| line.contains("message=created"))
        .filter_map(|line| {
            let run = opencode_log_field(line, "run")?;
            let id = opencode_log_field(line, "id")?;
            let directory = opencode_log_field(line, "directory")?;
            let created = opencode_log_field(line, "time.created")?
                .parse::<i64>()
                .ok()?;
            (owned_runs.contains(&run)
                && id.starts_with("ses_")
                && normalize_opencode_workspace(&directory) == expected
                && created >= created_after_ms)
                .then_some(id)
        })
        .collect::<std::collections::HashSet<_>>();

    (candidates.len() == 1).then(|| candidates.into_iter().next().expect("one candidate"))
}

pub(crate) fn opencode_last_assistant_text(session_id: &str) -> Result<Option<String>, String> {
    let Some(db_path) = opencode_database_path() else {
        return Ok(None);
    };
    opencode_last_assistant_text_from_db(&db_path, session_id)
}

pub(crate) fn opencode_last_assistant_text_from_db(
    db_path: &std::path::Path,
    session_id: &str,
) -> Result<Option<String>, String> {
    let conn =
        rusqlite::Connection::open_with_flags(db_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|err| err.to_string())?;

    let mut stmt = conn
        .prepare(
            "SELECT p.data, m.data
             FROM part p
             JOIN message m ON m.id = p.message_id
             WHERE p.session_id = ?1 AND m.session_id = ?1
             ORDER BY p.time_created DESC
             LIMIT 100",
        )
        .map_err(|err| err.to_string())?;

    let rows = stmt
        .query_map([session_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|err| err.to_string())?;

    for row in rows {
        let (part_data, message_data) = row.map_err(|err| err.to_string())?;
        let message: serde_json::Value =
            serde_json::from_str(&message_data).map_err(|err| err.to_string())?;
        if message.get("role").and_then(|value| value.as_str()) != Some("assistant") {
            continue;
        }

        let part: serde_json::Value =
            serde_json::from_str(&part_data).map_err(|err| err.to_string())?;
        if part.get("type").and_then(|value| value.as_str()) != Some("text") {
            continue;
        }

        if let Some(text) = part
            .get("text")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|text| !text.is_empty())
        {
            return Ok(Some(text.to_string()));
        }
    }

    Ok(None)
}

/// Provider-agnostic quiet-period fallback: after 6s without PTY output while
/// a full-screen TUI shows "Processing...", treat the turn as finished. Shared
/// by opencode, claude, and antigravity (see telemetry.rs) despite living in
/// this module for historical reasons.
pub(crate) fn provider_should_fallback_to_idle_after_quiet_period(
    current_status: &str,
    last_output_at: Option<std::time::SystemTime>,
    now: std::time::SystemTime,
) -> bool {
    if current_status != "Processing..." {
        return false;
    }
    let Some(last_output_at) = last_output_at else {
        return false;
    };
    now.duration_since(last_output_at)
        .map(|duration| duration >= std::time::Duration::from_secs(6))
        .unwrap_or(false)
}

pub(crate) fn opencode_runtime_config_content(
    class_name: &str,
    session_id: Option<&str>,
    config: Option<&AgentConfig>,
) -> Option<String> {
    let roots = resolve_opencode_runtime_roots(
        class_name,
        session_id,
        config.and_then(|cfg| cfg.system_include_directories.as_deref()),
        config.and_then(|cfg| cfg.include_directories.as_deref()),
    );
    let runtime_config = build_opencode_runtime_config(&roots);
    runtime_config
        .as_object()
        .filter(|map| !map.is_empty())
        .map(|_| runtime_config.to_string())
}

fn opencode_runtime_roots(
    class_name: &str,
    session_id: Option<&str>,
    config: Option<&AgentConfig>,
) -> Vec<std::path::PathBuf> {
    resolve_opencode_runtime_roots(
        class_name,
        session_id,
        config.and_then(|cfg| cfg.system_include_directories.as_deref()),
        config.and_then(|cfg| cfg.include_directories.as_deref()),
    )
}

fn opencode_custom_config_dir(
    cwd: &std::path::Path,
    class_name: &str,
    session_id: Option<&str>,
    config: Option<&AgentConfig>,
) -> Result<Option<std::path::PathBuf>, String> {
    let roots = opencode_runtime_roots(class_name, session_id, config);
    if roots.is_empty() {
        return Ok(None);
    }

    let wardian_home = get_wardian_home().ok_or("Could not find Wardian home")?;
    let key = codex_bootstrap_workspace_key(cwd);
    let config_dir =
        if let Some(session_id) = session_id.map(str::trim).filter(|sid| !sid.is_empty()) {
            wardian_home
                .join("agents")
                .join(session_id)
                .join("habitat")
                .join(".opencode")
        } else {
            wardian_home
                .join("provider-bootstrap")
                .join("opencode")
                .join(key)
                .join(".opencode")
        };

    // Sync the custom config dir and create the merged skills tree.
    crate::utils::fs::sync_opencode_config_dir(&config_dir, &roots)?;
    Ok(Some(config_dir))
}

pub(crate) fn opencode_env(
    cwd: &std::path::Path,
    class_name: &str,
    session_id: Option<&str>,
    config: Option<&AgentConfig>,
) -> Result<Vec<(String, String)>, String> {
    let mut envs = vec![("COLORTERM".to_string(), "truecolor".to_string())];
    if let Some(tui_config) = crate::utils::get_opencode_tui_path() {
        envs.push((
            "OPENCODE_TUI_CONFIG".to_string(),
            tui_config.to_string_lossy().to_string(),
        ));
    }
    if let Some(config_dir) = opencode_custom_config_dir(cwd, class_name, session_id, config)? {
        let config_path = config_dir.join("opencode.json");

        // Build the runtime config (instructions), and pair it with a
        // custom config directory so OpenCode can discover projected skills.
        let runtime_config: serde_json::Value =
            opencode_runtime_config_content(class_name, session_id, config)
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_else(|| serde_json::json!({}));

        std::fs::write(&config_path, runtime_config.to_string()).map_err(|e| e.to_string())?;
        envs.push((
            "OPENCODE_CONFIG_DIR".to_string(),
            config_dir.to_string_lossy().to_string(),
        ));
        envs.push((
            "OPENCODE_CONFIG".to_string(),
            config_path.to_string_lossy().to_string(),
        ));
    }
    Ok(envs)
}

pub(crate) fn opencode_interactive_env(
    cwd: &std::path::Path,
    config: &AgentConfig,
) -> Result<Vec<(String, String)>, String> {
    opencode_env(
        cwd,
        &config.agent_class,
        Some(config.session_id.as_str()),
        Some(config),
    )
}

fn opencode_log_timestamp_to_rfc3339(timestamp: &str) -> Option<String> {
    let parsed = chrono::NaiveDateTime::parse_from_str(timestamp, "%Y-%m-%dT%H:%M:%S").ok()?;
    let local = chrono::Local.from_local_datetime(&parsed).earliest()?;
    Some(
        local
            .with_timezone(&chrono::Utc)
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
    )
}

/// Find the OpenCode status log for a provider session. Current OpenCode
/// versions append to `opencode.log`; prefer that rolling file even before the
/// resumed session emits its first marker. Older builds used dated log files,
/// which still require exact provider-session evidence in their contents.
///
/// Used for sessions recovered after an app restart (where no live watcher
/// is running).
pub(crate) fn opencode_log_path_in(
    base: &std::path::Path,
    session_id: &str,
) -> Option<std::path::PathBuf> {
    let rolling = base.join("opencode.log");
    if rolling.is_file() {
        return Some(rolling);
    }

    let mut candidates = std::fs::read_dir(base)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("log"))
        .collect::<Vec<_>>();

    candidates.sort();
    candidates.reverse();

    candidates.into_iter().find(|path| {
        std::fs::read_to_string(path)
            .map(|content| content.contains(session_id))
            .unwrap_or(false)
    })
}

/// Return the ordered list of directories where opencode writes its log files.
/// Honors XDG_DATA_HOME first, then tries platform-native data dirs (Windows:
/// %LOCALAPPDATA%, %APPDATA%), followed by the home-directory XDG fallback.
pub(crate) fn opencode_log_dirs() -> Vec<std::path::PathBuf> {
    opencode_data_dirs()
        .into_iter()
        .map(|path| path.join("log"))
        .collect()
}

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct OpenCodeLogMetrics {
    query_count: usize,
    init_timestamp: Option<String>,
    last_query_timestamp: Option<String>,
    status: Option<String>,
}

/// Derive status and metrics from an opencode log.
///
/// Status is determined semantically from `service=session.prompt` markers:
/// - `exiting loop`  → the prompt loop finished → **Idle**
/// - `step=N loop`   → the prompt loop is active → **Processing…**
///
/// This avoids timestamp comparisons entirely, which would be unreliable
/// because opencode logs timestamps in local time while `now` is UTC.
pub(crate) fn opencode_metrics_from_log(content: &str, session_id: &str) -> OpenCodeLogMetrics {
    let mut metrics = OpenCodeLogMetrics::default();
    // true  = last session.prompt event was "exiting loop" (Idle)
    // false = last session.prompt event was "step=N loop"  (Processing)
    let mut last_prompt_exited = false;
    let mut saw_prompt = false;
    let mut saw_error = false;
    // Permission-prompt detection: opencode writes `message=asking id=per_…`
    // when the TUI shows a "Permission required" prompt, but that line — unlike
    // loop markers — carries no `session.id`, so it cannot be filtered by
    // session. An ask is therefore attributed positionally: to this session
    // only while its prompt loop is the single open loop at that point in the
    // chronological log, and until any later loop activity for the session
    // proves the prompt was answered.
    let mut open_loops: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut pending_ask = false;

    for line in content.lines() {
        if let Some(loop_owner) = opencode_loop_line_session_id(line) {
            if line.contains("exiting loop") {
                open_loops.remove(&loop_owner);
                if loop_owner == session_id {
                    last_prompt_exited = true;
                    saw_prompt = true;
                    pending_ask = false;
                }
            } else if line.contains(" step=") {
                open_loops.insert(loop_owner.clone());
                if loop_owner == session_id {
                    metrics.query_count += 1;
                    metrics.last_query_timestamp = line
                        .split_whitespace()
                        .find_map(|token| token.strip_prefix("timestamp="))
                        .filter(|value| value.ends_with('Z'))
                        .map(str::to_string)
                        .or_else(|| {
                            line.split_whitespace()
                                .nth(1)
                                .and_then(opencode_log_timestamp_to_rfc3339)
                        });
                    last_prompt_exited = false;
                    saw_prompt = true;
                    pending_ask = false;
                }
            }
        }

        if line.contains("message=asking") && !line.contains("session.id=") {
            if open_loops.len() == 1 && open_loops.contains(session_id) {
                pending_ask = true;
            }
            continue;
        }

        if !line.contains(session_id) {
            continue;
        }

        if metrics.init_timestamp.is_none() {
            // 1.17+ format: a `timestamp=<rfc3339>` token; pre-1.17 format:
            // a local-time timestamp as the second whitespace-separated token.
            metrics.init_timestamp = line
                .split_whitespace()
                .find_map(|token| token.strip_prefix("timestamp="))
                .filter(|value| value.ends_with('Z'))
                .map(|value| value.to_string())
                .or_else(|| {
                    line.split_whitespace()
                        .nth(1)
                        .and_then(opencode_log_timestamp_to_rfc3339)
                });
        }

        // Pre-1.17 prompt-loop markers. The 1.17+ `message=loop` markers were
        // already handled above (they carry `session.id` for every session).
        if line.contains("service=session.prompt") {
            if line.contains("exiting loop") {
                last_prompt_exited = true;
                saw_prompt = true;
            } else if line.contains(" step=") {
                metrics.query_count += 1;
                metrics.last_query_timestamp = line
                    .split_whitespace()
                    .nth(1)
                    .and_then(opencode_log_timestamp_to_rfc3339);
                last_prompt_exited = false;
                saw_prompt = true;
            }
            continue;
        }

        if line.starts_with("ERROR ") || line.contains(" ERROR ") || line.contains("level=ERROR") {
            saw_error = true;
        }
    }

    metrics.status = if saw_error && !last_prompt_exited {
        Some("Error".to_string())
    } else if pending_ask {
        // A live "Permission required" prompt outranks the derived Processing
        // state; the loop is paused until the operator answers it.
        Some("Action Needed".to_string())
    } else if !saw_prompt {
        // No prompt activity yet — return None so we don't override a
        // status set by the PTY reader (e.g. "Pending…" or "Off").
        if metrics.init_timestamp.is_some() {
            Some("Idle".to_string())
        } else {
            None
        }
    } else if last_prompt_exited {
        Some("Idle".to_string())
    } else {
        Some("Processing...".to_string())
    };

    metrics
}

pub(crate) fn apply_opencode_log_metrics(
    content: &str,
    session_id: &str,
    query_count: &mut usize,
    init_timestamp: &mut Option<String>,
    last_query_timestamp: &mut Option<String>,
    current_status: &mut String,
) {
    let metrics = opencode_metrics_from_log(content, session_id);
    if metrics.query_count > 0 {
        *query_count = metrics.query_count;
    }
    if init_timestamp.is_none() && metrics.init_timestamp.is_some() {
        *init_timestamp = metrics.init_timestamp;
    }
    update_latest_query_timestamp(last_query_timestamp, metrics.last_query_timestamp);
    if let Some(status) = metrics.status {
        *current_status = status;
    }
}

fn update_latest_query_timestamp(latest: &mut Option<String>, candidate: Option<String>) {
    let Some(candidate) = candidate else {
        return;
    };
    let should_replace = latest.as_deref().is_none_or(|current| {
        match (
            chrono::DateTime::parse_from_rfc3339(current),
            chrono::DateTime::parse_from_rfc3339(&candidate),
        ) {
            (Ok(current), Ok(candidate)) => candidate > current,
            _ => candidate.as_str() > current,
        }
    });
    if should_replace {
        *latest = Some(candidate);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manager::headless::{headless_provider_args, headless_provider_launch};
    use crate::manager::{
        finalize_interactive_spawn_args, interactive_provider_args, interactive_provider_launch,
        session_bootstrap_prompt,
    };
    use std::path::Path;
    use wardian_core::models::AgentConfig;

    #[test]
    fn opencode_data_root_prefers_xdg_provider_storage() {
        let xdg_data_home = std::path::PathBuf::from("D:/isolated/data");
        let platform_local = std::path::PathBuf::from("C:/Users/test/AppData/Local");
        let platform_data = std::path::PathBuf::from("C:/Users/test/AppData/Roaming");
        let home = std::path::PathBuf::from("C:/Users/test");

        let roots = opencode_data_dirs_from_roots(
            Some(&xdg_data_home),
            Some(platform_local),
            Some(platform_data),
            Some(home),
        );

        assert_eq!(roots[0], xdg_data_home.join("opencode"));
        assert!(roots
            .iter()
            .any(|path| path == &std::path::PathBuf::from("C:/Users/test/AppData/Local/opencode")));
    }

    #[test]
    fn opencode_telemetry_prefers_a_valid_resume_or_fresh_session() {
        let mut config = AgentConfig {
            provider: "opencode".to_string(),
            resume_session: Some("wardian-agent-id".to_string()),
            fresh_provider_session_id: Some("ses_fresh".to_string()),
            ..AgentConfig::default()
        };
        assert_eq!(
            opencode_telemetry_session_id(&config).as_deref(),
            Some("ses_fresh")
        );

        config.resume_session = Some("ses_resume".to_string());
        assert_eq!(
            opencode_telemetry_session_id(&config).as_deref(),
            Some("ses_resume")
        );
    }

    #[test]
    fn opencode_session_recovery_requires_the_agent_owned_launch_run() {
        let workspace = if cfg!(windows) {
            std::path::PathBuf::from(r"D:\work\project")
        } else {
            std::path::PathBuf::from("/work/project")
        };
        let config_path = if cfg!(windows) {
            r#"D:\wardian\agents\agent-1\habitat\.opencode\opencode.json"#
        } else {
            "/wardian/agents/agent-1/habitat/.opencode/opencode.json"
        };
        let directory = workspace.to_string_lossy();
        let log = format!(
            "run=owned message=loading path=\"{config_path}\"\n\
             run=other message=created id=ses_other directory=\"{directory}\" time.created=200\n\
             run=owned message=created id=ses_owner directory=\"{directory}\" time.created=300"
        );

        assert_eq!(
            select_opencode_session_from_launch_log(&log, &workspace, 250, "agent-1").as_deref(),
            Some("ses_owner")
        );
        let mut discovery = OpenCodeSessionDiscovery::default();
        let revision = vec![(
            std::path::PathBuf::from("opencode.log"),
            log.len() as u64,
            None,
        )];
        assert_eq!(
            discovery
                .poll_with("Idle", revision.clone(), || {
                    select_opencode_session_from_launch_log(&log, &workspace, 250, "agent-1")
                })
                .as_deref(),
            Some("ses_owner"),
            "A completed owned session must be discovered without a Processing title"
        );
        assert_eq!(
            discovery.poll_with("Idle", revision.clone(), || panic!("unchanged log reread")),
            None
        );
        let changed = vec![(
            std::path::PathBuf::from("opencode.log"),
            log.len() as u64 + 1,
            None,
        )];
        assert_eq!(
            discovery.poll_with("Off", changed.clone(), || panic!("stopped owner scanned")),
            None
        );
        assert_eq!(
            discovery
                .poll_with("Idle", changed, || Some("ses_next".into()))
                .as_deref(),
            Some("ses_next")
        );
    }

    #[test]
    fn opencode_session_recovery_fails_closed_for_ambiguous_owned_runs() {
        let workspace = if cfg!(windows) {
            std::path::PathBuf::from(r"D:\work\project")
        } else {
            std::path::PathBuf::from("/work/project")
        };
        let config_path = if cfg!(windows) {
            r#"D:\wardian\agents\agent-1\habitat\.opencode\opencode.json"#
        } else {
            "/wardian/agents/agent-1/habitat/.opencode/opencode.json"
        };
        let directory = workspace.to_string_lossy();
        let log = format!(
            "run=first message=loading path=\"{config_path}\"\n\
             run=first message=created id=ses_first directory=\"{directory}\" time.created=300\n\
             run=second message=loading path=\"{config_path}\"\n\
             run=second message=created id=ses_second directory=\"{directory}\" time.created=301"
        );

        assert_eq!(
            select_opencode_session_from_launch_log(&log, &workspace, 250, "agent-1"),
            None
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn opencode_workspace_matching_keeps_posix_case_and_backslashes_literal() {
        let workspace = Path::new("/work/project");
        let log = [
            r#"run=case message=loading path="/wardian/agents/agent-1/habitat/.opencode/opencode.json""#,
            r#"run=case message=created id=ses_case directory="/work/Project" time.created=300"#,
            r#"run=slash message=loading path="/wardian/agents/agent-1/habitat/.opencode/opencode.json""#,
            r#"run=slash message=created id=ses_slash directory="/work/project\nested" time.created=301"#,
        ]
        .join("\n");

        assert_eq!(
            select_opencode_session_from_launch_log(&log, workspace, 250, "agent-1"),
            None
        );
    }
    #[test]
    fn opencode_title_maps_to_status() {
        assert_eq!(opencode_status_from_title("OpenCode"), Some("Idle"));
        assert_eq!(
            opencode_status_from_title("OC | Working"),
            Some("Processing...")
        );
        assert_eq!(
            opencode_status_from_title("OC | Action Required: approve tool"),
            Some("Action Needed")
        );
        assert_eq!(opencode_status_from_title(""), None);
    }

    fn permission_log_line(permission: &str, pattern: &str, action: &str) -> String {
        format!(
            "timestamp=2026-08-21T15:59:53.288Z level=INFO run=r1 message=evaluated \
             permission={permission} pattern=\"{pattern}\" action.permission={permission} \
             action.pattern=* action.action={action}"
        )
    }

    #[test]
    fn opencode_log_permission_prompt_maps_to_action_needed() {
        let log = [
            "timestamp=2026-08-21T18:04:18.494Z level=INFO run=r1 message=loop session.id=ses_a step=1",
            &permission_log_line("bash", "ls", "ask"),
            "timestamp=2026-08-21T18:04:20.001Z level=INFO run=r1 message=asking id=per_1 permission=bash patterns=\"[\\\"ls\\\"]\"",
            "timestamp=2026-08-21T18:05:10.000Z level=INFO run=r1 message=\"exiting loop\" session.id=ses_b",
        ]
        .join("\n");

        let metrics = opencode_metrics_from_log(&log, "ses_a");

        assert_eq!(metrics.status.as_deref(), Some("Action Needed"));
        assert_eq!(metrics.query_count, 1);
    }

    #[test]
    fn opencode_log_permission_prompt_clears_after_resume() {
        let log = [
            "timestamp=2026-08-21T18:04:18.494Z level=INFO run=r1 message=loop session.id=ses_a step=1",
            "timestamp=2026-08-21T18:04:20.001Z level=INFO run=r1 message=asking id=per_1 permission=bash patterns=\"[\\\"ls\\\"]\"",
            // The operator answered the prompt; the loop resumed.
            "timestamp=2026-08-21T18:05:00.000Z level=INFO run=r1 message=loop session.id=ses_a step=2",
        ]
        .join("\n");

        let metrics = opencode_metrics_from_log(&log, "ses_a");

        assert_eq!(metrics.status.as_deref(), Some("Processing..."));
        assert_eq!(metrics.query_count, 2);
    }

    #[test]
    fn opencode_log_permission_prompt_after_exit_is_not_attributed() {
        let log = [
            "timestamp=2026-08-21T18:03:11.768Z level=INFO run=r1 message=\"exiting loop\" session.id=ses_a",
            "timestamp=2026-08-21T18:04:20.001Z level=INFO run=r1 message=asking id=per_1 permission=bash patterns=\"[\\\"ls\\\"]\"",
        ]
        .join("\n");

        let metrics = opencode_metrics_from_log(&log, "ses_a");

        assert_eq!(metrics.status.as_deref(), Some("Idle"));
    }

    #[test]
    fn opencode_log_permission_prompt_with_concurrent_loop_is_skipped() {
        // Two sessions mid-turn when the ask appears: attribution is ambiguous.
        let log = [
            "timestamp=2026-08-21T18:04:18.000Z level=INFO run=r1 message=loop session.id=ses_a step=1",
            "timestamp=2026-08-21T18:04:19.000Z level=INFO run=r2 message=loop session.id=ses_b step=4",
            "timestamp=2026-08-21T18:04:20.001Z level=INFO run=r1 message=asking id=per_1 permission=bash patterns=\"[\\\"ls\\\"]\"",
        ]
        .join("\n");

        let metrics = opencode_metrics_from_log(&log, "ses_a");

        assert_eq!(metrics.status.as_deref(), Some("Processing..."));
        assert_eq!(metrics.query_count, 1);
    }

    #[test]
    fn provider_idle_fallback_triggers_after_quiet_period() {
        let now = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(10);
        let last = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(3);

        assert!(provider_should_fallback_to_idle_after_quiet_period(
            "Processing...",
            Some(last),
            now
        ));
        assert!(!provider_should_fallback_to_idle_after_quiet_period(
            "Idle",
            Some(last),
            now
        ));
    }

    #[test]
    fn opencode_log_path_finds_newest_matching_log() {
        let temp = tempfile::tempdir().expect("temp dir");
        let log_dir = temp.path().join("log");
        std::fs::create_dir_all(&log_dir).expect("create log dir");

        let older = log_dir.join("2026-04-11T210615.log");
        let newer = log_dir.join("2026-04-11T210616.log");
        let unrelated = log_dir.join("2026-04-11T210617.log");

        std::fs::write(
            &older,
            r#"INFO  2026-04-11T21:06:15 +0ms service=default args=[\"attach\",\"http://127.0.0.1:57079\",\"--session\",\"ses_target\"] opencode"#,
        )
        .expect("write older log");
        std::fs::write(
            &newer,
            r#"INFO  2026-04-11T21:06:16 +0ms service=default args=[\"attach\",\"http://127.0.0.1:57079\",\"--session\",\"ses_target\"] opencode"#,
        )
        .expect("write newer log");
        std::fs::write(
            &unrelated,
            r#"INFO  2026-04-11T21:06:17 +0ms service=default args=[\"attach\",\"http://127.0.0.1:57079\",\"--session\",\"ses_other\"] opencode"#,
        )
        .expect("write unrelated log");

        let found = opencode_log_path_in(&log_dir, "ses_target").expect("matching log path");

        assert_eq!(found, newer);
    }

    #[test]
    fn opencode_log_path_prefers_active_rolling_log_before_session_marker() {
        let temp = tempfile::tempdir().expect("temp dir");
        let log_dir = temp.path().join("log");
        std::fs::create_dir_all(&log_dir).expect("create log dir");

        let archived = log_dir.join("2026-08-21T051619.log");
        let rolling = log_dir.join("opencode.log");
        std::fs::write(&archived, "session.id=ses_target").expect("write archive");
        std::fs::write(&rolling, "service=default message=booting").expect("write rolling log");

        assert_eq!(opencode_log_path_in(&log_dir, "ses_target"), Some(rolling));
    }

    #[test]
    fn opencode_last_assistant_text_from_db_returns_newest_assistant_text() {
        let temp = tempfile::tempdir().expect("temp dir");
        let db_path = temp.path().join("opencode.db");
        let conn = rusqlite::Connection::open(&db_path).expect("open db");
        conn.execute_batch(
            r#"
            CREATE TABLE message (
                id text PRIMARY KEY,
                session_id text NOT NULL,
                time_created integer NOT NULL,
                time_updated integer NOT NULL,
                data text NOT NULL
            );
            CREATE TABLE part (
                id text PRIMARY KEY,
                message_id text NOT NULL,
                session_id text NOT NULL,
                time_created integer NOT NULL,
                time_updated integer NOT NULL,
                data text NOT NULL
            );
            "#,
        )
        .expect("create schema");

        conn.execute(
            "INSERT INTO message (id, session_id, time_created, time_updated, data) VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params!["user-1", "ses_test", 1, 1, r#"{"role":"user"}"#],
        )
        .expect("insert user message");
        conn.execute(
            "INSERT INTO part (id, message_id, session_id, time_created, time_updated, data) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![
                "user-part",
                "user-1",
                "ses_test",
                2,
                2,
                r#"{"type":"text","text":"Prompt text"}"#,
            ],
        )
        .expect("insert user part");
        conn.execute(
            "INSERT INTO message (id, session_id, time_created, time_updated, data) VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params!["assistant-1", "ses_test", 3, 3, r#"{"role":"assistant"}"#],
        )
        .expect("insert assistant message");
        conn.execute(
            "INSERT INTO part (id, message_id, session_id, time_created, time_updated, data) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![
                "assistant-finish",
                "assistant-1",
                "ses_test",
                5,
                5,
                r#"{"type":"step-finish","reason":"stop"}"#,
            ],
        )
        .expect("insert finish part");
        conn.execute(
            "INSERT INTO part (id, message_id, session_id, time_created, time_updated, data) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![
                "assistant-text",
                "assistant-1",
                "ses_test",
                4,
                4,
                r#"{"type":"text","text":"Actual assistant text"}"#,
            ],
        )
        .expect("insert assistant text");
        drop(conn);

        let text = opencode_last_assistant_text_from_db(&db_path, "ses_test")
            .expect("read assistant text");

        assert_eq!(text, Some("Actual assistant text".to_string()));
    }

    #[test]
    fn opencode_last_assistant_text_from_db_skips_empty_text() {
        let temp = tempfile::tempdir().expect("temp dir");
        let db_path = temp.path().join("opencode.db");
        let conn = rusqlite::Connection::open(&db_path).expect("open db");
        conn.execute_batch(
            r#"
            CREATE TABLE message (
                id text PRIMARY KEY,
                session_id text NOT NULL,
                time_created integer NOT NULL,
                time_updated integer NOT NULL,
                data text NOT NULL
            );
            CREATE TABLE part (
                id text PRIMARY KEY,
                message_id text NOT NULL,
                session_id text NOT NULL,
                time_created integer NOT NULL,
                time_updated integer NOT NULL,
                data text NOT NULL
            );
            INSERT INTO message VALUES ('assistant-1', 'ses_test', 1, 1, '{"role":"assistant"}');
            INSERT INTO part VALUES ('blank', 'assistant-1', 'ses_test', 2, 2, '{"type":"text","text":"   "}');
            "#,
        )
        .expect("create db");
        drop(conn);

        let text = opencode_last_assistant_text_from_db(&db_path, "ses_test")
            .expect("read assistant text");

        assert_eq!(text, None);
    }

    #[test]
    fn opencode_metrics_from_log_counts_session_prompt_steps() {
        let content = concat!(
            "INFO  2026-03-30T07:35:53 +0ms service=session.prompt step=0 sessionID=ses_target loop\n",
            "INFO  2026-03-30T07:35:53 +1ms service=llm providerID=opencode sessionID=ses_target stream\n",
            "INFO  2026-03-30T07:36:02 +0ms service=session.prompt step=1 sessionID=ses_target loop\n",
            "INFO  2026-03-30T07:36:04 +0ms service=session.prompt step=0 sessionID=ses_other loop\n"
        );

        let metrics = opencode_metrics_from_log(content, "ses_target");

        assert_eq!(metrics.query_count, 2);
    }

    #[test]
    fn opencode_metrics_from_log_derives_processing_idle_and_error_status() {
        // Processing: last session.prompt event is "step=N loop" (no exiting yet)
        let processing = concat!(
            "INFO  2026-03-30T07:35:53 +0ms service=session.prompt step=0 sessionID=ses_target loop\n",
            "INFO  2026-03-30T07:35:53 +1ms service=llm providerID=opencode sessionID=ses_target stream\n"
        );
        // Idle: last session.prompt event is "exiting loop"
        let idle = concat!(
            "INFO  2026-03-30T07:35:53 +0ms service=session.prompt step=0 sessionID=ses_target loop\n",
            "INFO  2026-03-30T07:35:53 +1ms service=llm providerID=opencode sessionID=ses_target stream\n",
            "INFO  2026-03-30T07:35:56 +0ms service=session.prompt step=1 sessionID=ses_target loop\n",
            "INFO  2026-03-30T07:35:56 +0ms service=session.prompt sessionID=ses_target exiting loop\n"
        );
        // Error: ERROR line with no subsequent "exiting loop"
        let errored = concat!(
            "INFO  2026-03-30T07:35:53 +0ms service=session.prompt step=0 sessionID=ses_target loop\n",
            "ERROR 2026-03-30T07:35:54 +997ms service=llm providerID=opencode sessionID=ses_target error={\"error\":{}} stream error\n"
        );

        let processing_metrics = opencode_metrics_from_log(processing, "ses_target");
        let idle_metrics = opencode_metrics_from_log(idle, "ses_target");
        let errored_metrics = opencode_metrics_from_log(errored, "ses_target");

        assert_eq!(processing_metrics.status, Some("Processing...".to_string()));
        assert_eq!(idle_metrics.status, Some("Idle".to_string()));
        assert_eq!(errored_metrics.status, Some("Error".to_string()));
    }

    #[test]
    fn opencode_log_metrics_update_status_and_query_count_from_current_logs() {
        let current = concat!(
            "INFO  2026-04-26T04:28:39 +1ms service=session.prompt session.id=ses_target step=0 loop\n",
            "INFO  2026-04-26T04:28:42 +0ms service=session.status publishing\n",
            "INFO  2026-04-26T04:28:42 +0ms service=session.prompt session.id=ses_target step=1 loop\n",
            "INFO  2026-04-26T04:28:42 +1ms service=session.prompt session.id=ses_target exiting loop\n",
            "INFO  2026-04-26T04:28:42 +1ms service=session.idle publishing\n",
        );

        let mut query_count = 0;
        let mut init_timestamp = None;
        let mut last_query_timestamp = None;
        let mut current_status = "Processing...".to_string();

        apply_opencode_log_metrics(
            current,
            "ses_target",
            &mut query_count,
            &mut init_timestamp,
            &mut last_query_timestamp,
            &mut current_status,
        );

        assert_eq!(query_count, 2);
        let parsed_timestamp = init_timestamp
            .as_deref()
            .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
            .expect("OpenCode log timestamp should be normalized to RFC3339");
        assert_eq!(
            parsed_timestamp
                .with_timezone(&chrono::Local)
                .format("%Y-%m-%dT%H:%M:%S")
                .to_string(),
            "2026-04-26T04:28:39"
        );
        assert_eq!(current_status, "Idle");
        let parsed_last_query = last_query_timestamp
            .as_deref()
            .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
            .expect("OpenCode last query timestamp should be normalized to RFC3339");
        assert_eq!(
            parsed_last_query
                .with_timezone(&chrono::Local)
                .format("%Y-%m-%dT%H:%M:%S")
                .to_string(),
            "2026-04-26T04:28:42"
        );
    }

    #[test]
    fn opencode_log_metrics_preserve_existing_rfc3339_birth_timestamp() {
        let current = concat!(
            "INFO  2026-04-26T04:28:39 +1ms service=session.prompt session.id=ses_target step=0 loop\n",
            "INFO  2026-04-26T04:28:42 +1ms service=session.prompt session.id=ses_target exiting loop\n",
        );

        let mut query_count = 0;
        let mut init_timestamp = Some("2026-04-26T04:20:00.000Z".to_string());
        let mut last_query_timestamp = None;
        let mut current_status = "Processing...".to_string();

        apply_opencode_log_metrics(
            current,
            "ses_target",
            &mut query_count,
            &mut init_timestamp,
            &mut last_query_timestamp,
            &mut current_status,
        );

        assert_eq!(init_timestamp, Some("2026-04-26T04:20:00.000Z".to_string()));
        assert_eq!(current_status, "Idle");
    }

    #[test]
    fn opencode_log_metrics_preserve_a_newer_existing_query_timestamp() {
        let current =
            "INFO  2026-04-26T04:28:42 +1ms service=session.prompt session.id=ses_target step=0 loop\n";
        let mut query_count = 0;
        let mut init_timestamp = None;
        let mut last_query_timestamp = Some("2026-04-26T12:30:00.000Z".to_string());
        let mut current_status = "Processing...".to_string();

        apply_opencode_log_metrics(
            current,
            "ses_target",
            &mut query_count,
            &mut init_timestamp,
            &mut last_query_timestamp,
            &mut current_status,
        );

        assert_eq!(
            last_query_timestamp.as_deref(),
            Some("2026-04-26T12:30:00.000Z")
        );
    }

    #[test]
    fn opencode_interactive_args_include_dir_for_real_workspace_anchor() {
        let workspace_cwd = Path::new("D:/Development/Wardian");

        let args = interactive_provider_args("opencode", workspace_cwd, workspace_cwd, Vec::new());

        assert_eq!(args, vec!["D:/Development/Wardian".to_string()]);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn opencode_interactive_launch_uses_configured_shell_for_cmd_shims() {
        let _guard = crate::utils::wardian_test_env_lock();
        let previous_home = std::env::var_os("WARDIAN_HOME");
        let previous_comspec = std::env::var_os("ComSpec");
        let home = tempfile::tempdir().expect("temp dir");
        std::env::set_var("WARDIAN_HOME", home.path());
        std::env::set_var(
            "ComSpec",
            r"D:\Development\Wardian\target\release\Wardian.exe",
        );
        let settings_path = home.path().join("settings").join("shell.json");
        std::fs::create_dir_all(settings_path.parent().expect("settings parent")).unwrap();
        std::fs::write(
            &settings_path,
            r#"{
              "shell_id": "custom",
              "custom_executable": "pwsh.exe",
              "custom_args": "-NoProfile -Command",
              "agent_session_persistence": "resume"
            }"#,
        )
        .unwrap();

        let launch = interactive_provider_launch(
            "opencode",
            r"C:\nvm4w\nodejs\opencode.cmd",
            &["--session".to_string(), "ses_test".to_string()],
        )
        .expect("launch spec");

        assert_eq!(launch.executable, "pwsh.exe");
        assert_eq!(
            launch.args[..2],
            ["-NoProfile".to_string(), "-Command".to_string()]
        );
        assert!(launch.args[2].contains(r"C:\nvm4w\nodejs\opencode.cmd"));
        assert!(launch.args[2].contains("--session"));
        assert!(launch.args[2].contains("ses_test"));
        assert!(!launch.args[2].contains("ComSpec"));

        match previous_home {
            Some(value) => std::env::set_var("WARDIAN_HOME", value),
            None => std::env::remove_var("WARDIAN_HOME"),
        }
        match previous_comspec {
            Some(value) => std::env::set_var("ComSpec", value),
            None => std::env::remove_var("ComSpec"),
        }
    }

    #[test]
    fn opencode_runtime_config_content_uses_class_system_and_user_roots() {
        let _guard = crate::utils::wardian_test_env_lock();
        let temp = tempfile::tempdir().expect("temp dir");
        let wardian_home = temp.path().join(".wardian");
        let common = wardian_home.join("common");
        let class_dir = wardian_home.join("classes").join("Builder");
        let user_dir = temp.path().join("user-root");

        std::fs::create_dir_all(common.join(".agents").join("skills").join("common-skill"))
            .expect("common skill dir");
        std::fs::create_dir_all(class_dir.join(".agents").join("skills").join("class-skill"))
            .expect("class skill dir");
        std::fs::create_dir_all(user_dir.join(".agents").join("skills").join("user-skill"))
            .expect("user skill dir");
        std::fs::write(common.join("AGENTS.md"), "common").expect("common AGENTS");
        std::fs::write(class_dir.join("AGENTS.md"), "class").expect("class AGENTS");
        std::fs::write(user_dir.join("AGENTS.md"), "user").expect("user AGENTS");

        unsafe { std::env::set_var("WARDIAN_HOME", wardian_home.to_string_lossy().to_string()) };

        let config = AgentConfig {
            include_directories: Some(vec![user_dir.to_string_lossy().to_string()]),
            ..Default::default()
        };

        let content =
            opencode_runtime_config_content("Builder", None, Some(&config)).expect("config");

        unsafe { std::env::remove_var("WARDIAN_HOME") };

        let parsed: serde_json::Value = serde_json::from_str(&content).expect("json config");
        let instructions = parsed["instructions"]
            .as_array()
            .expect("instructions array");

        assert_eq!(instructions.len(), 3);
        assert!(parsed.get("skills").is_none());
    }

    #[test]
    fn opencode_interactive_env_includes_runtime_config_file_and_truecolor() {
        let _guard = crate::utils::wardian_test_env_lock();
        let temp = tempfile::tempdir().expect("temp dir");
        let wardian_home = temp.path().join(".wardian");
        let common = wardian_home.join("common");
        let class_dir = wardian_home.join("classes").join("Builder");
        let agent_dir = wardian_home.join("agents").join("ses_123");
        let user_dir = temp.path().join("user-root");

        std::fs::create_dir_all(common.join(".agents").join("skills").join("common-skill"))
            .expect("common skill dir");
        std::fs::create_dir_all(class_dir.join(".agents").join("skills").join("class-skill"))
            .expect("class skill dir");
        std::fs::create_dir_all(agent_dir.join(".agents").join("skills").join("agent-skill"))
            .expect("agent skill dir");
        std::fs::create_dir_all(user_dir.join(".agents").join("skills").join("user-skill"))
            .expect("user skill dir");
        std::fs::write(common.join("AGENTS.md"), "common").expect("common AGENTS");
        std::fs::write(class_dir.join("AGENTS.md"), "class").expect("class AGENTS");
        std::fs::write(agent_dir.join("AGENTS.md"), "agent").expect("agent AGENTS");
        std::fs::write(user_dir.join("AGENTS.md"), "user").expect("user AGENTS");

        unsafe { std::env::set_var("WARDIAN_HOME", wardian_home.to_string_lossy().to_string()) };

        let config = AgentConfig {
            session_id: "ses_123".into(),
            agent_class: "Builder".into(),
            include_directories: Some(vec![user_dir.to_string_lossy().to_string()]),
            ..Default::default()
        };

        let envs = opencode_interactive_env(Path::new("D:/Development/Wardian"), &config)
            .expect("interactive envs");

        unsafe { std::env::remove_var("WARDIAN_HOME") };

        assert!(envs.contains(&("COLORTERM".to_string(), "truecolor".to_string())));
        let config_path = envs
            .iter()
            .find(|(key, _)| key == "OPENCODE_CONFIG")
            .map(|(_, value)| value)
            .expect("interactive runtime config path");
        let parsed: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(config_path).expect("read runtime config"),
        )
        .expect("json config");
        let instructions = parsed["instructions"]
            .as_array()
            .expect("instructions array");

        assert_eq!(instructions.len(), 4);

        // Config JSON must NOT contain skills.paths — OpenCode 1.4.3 does not
        // expose a skills.paths config key, so Wardian omits it entirely.
        assert!(
            parsed.get("skills").is_none(),
            "skills key must not be present in the config"
        );

        let config_dir = envs
            .iter()
            .find(|(key, _)| key == "OPENCODE_CONFIG_DIR")
            .map(|(_, value)| value)
            .expect("interactive runtime config dir");

        assert!(
            std::path::Path::new(config_dir)
                .join("skills")
                .join("common-skill")
                .exists(),
            "OPENCODE_CONFIG_DIR should expose projected skills"
        );
    }

    #[cfg(windows)]
    #[test]
    fn opencode_headless_launch_uses_configured_shell_on_windows() {
        let _guard = crate::utils::wardian_test_env_lock();
        let previous_home = std::env::var_os("WARDIAN_HOME");
        let previous_comspec = std::env::var_os("ComSpec");
        let home = tempfile::tempdir().expect("temp dir");
        std::env::set_var("WARDIAN_HOME", home.path());
        std::env::set_var(
            "ComSpec",
            r"D:\Development\Wardian\target\release\Wardian.exe",
        );
        let settings_path = home.path().join("settings").join("shell.json");
        std::fs::create_dir_all(settings_path.parent().expect("settings parent")).unwrap();
        std::fs::write(
            &settings_path,
            r#"{
              "shell_id": "custom",
              "custom_executable": "pwsh.exe",
              "custom_args": "-NoProfile -Command",
              "agent_session_persistence": "resume"
            }"#,
        )
        .unwrap();

        let launch = headless_provider_launch(
            "opencode",
            "C:/nvm4w/nodejs/opencode",
            &[
                "run".to_string(),
                "--format".to_string(),
                "json".to_string(),
                "--dir".to_string(),
                "D:/Development/Wardian".to_string(),
                session_bootstrap_prompt().to_string(),
            ],
        )
        .expect("headless launch spec");

        assert_eq!(launch.executable, "pwsh.exe");
        assert_eq!(
            launch.args[..2],
            ["-NoProfile".to_string(), "-Command".to_string()]
        );
        assert!(launch.args[2].contains("opencode"));
        assert!(launch.args[2].contains("--format"));
        assert!(launch.args[2].contains(session_bootstrap_prompt()));
        assert!(!launch.args[2].contains("ComSpec"));

        match previous_home {
            Some(value) => std::env::set_var("WARDIAN_HOME", value),
            None => std::env::remove_var("WARDIAN_HOME"),
        }
        match previous_comspec {
            Some(value) => std::env::set_var("ComSpec", value),
            None => std::env::remove_var("ComSpec"),
        }
    }

    #[test]
    fn opencode_fresh_headless_args_omit_session_flag_but_keep_config() {
        let provider = crate::providers::ProviderFactory::resolve("opencode").unwrap();
        let config = AgentConfig {
            provider: "opencode".into(),
            provider_config: wardian_core::models::ProviderConfig::OpenCode(
                wardian_core::models::OpenCodeProviderConfig {
                    agent: Some("build".into()),
                    ..Default::default()
                },
            ),
            ..Default::default()
        };

        let args = headless_provider_args(
            "opencode",
            provider.as_ref(),
            Path::new("D:/Development/Wardian"),
            "task",
            "text",
            None,
            Some(&config),
        );

        assert!(args.contains(&"run".to_string()));
        assert!(args.contains(&"--agent".to_string()));
        assert!(args.contains(&"build".to_string()));
        assert!(!args.contains(&"--session".to_string()));
    }

    #[test]
    fn opencode_interactive_launch_bypasses_shell_wrapper() {
        let launch = interactive_provider_launch(
            "opencode",
            "C:/real/opencode.exe",
            &[
                "--session".to_string(),
                "ses_test".to_string(),
                "D:/Development/Wardian".to_string(),
            ],
        )
        .expect("interactive launch spec");

        assert_eq!(launch.executable, "C:/real/opencode.exe");
        assert_eq!(
            launch.args,
            vec![
                "--session".to_string(),
                "ses_test".to_string(),
                "D:/Development/Wardian".to_string(),
            ]
        );
    }

    #[test]
    fn fresh_opencode_interactive_spawn_keeps_explicit_session_after_bootstrap() {
        let args = finalize_interactive_spawn_args(
            "opencode",
            false,
            &Some("ses_test".to_string()),
            vec!["--session".to_string(), "ses_test".to_string()],
        );

        assert_eq!(args, vec!["--session".to_string(), "ses_test".to_string()]);
    }

    #[test]
    fn restored_opencode_interactive_spawn_keeps_explicit_session() {
        let args = finalize_interactive_spawn_args(
            "opencode",
            true,
            &Some("ses_test".to_string()),
            vec!["--session".to_string(), "ses_test".to_string()],
        );

        assert_eq!(args, vec!["--session".to_string(), "ses_test".to_string()]);
    }

    #[test]
    fn opencode_interactive_args_append_dir_after_flags() {
        let args = interactive_provider_args(
            "opencode",
            Path::new("C:/Users/test/.wardian/agents/ses_test/habitat"),
            Path::new("D:/Development/Wardian"),
            vec!["--session".to_string(), "ses_test".to_string()],
        );

        assert_eq!(
            args,
            vec![
                "--session".to_string(),
                "ses_test".to_string(),
                "C:/Users/test/.wardian/agents/ses_test/habitat/workspace".to_string(),
            ]
        );
    }

    #[test]
    fn opencode_metrics_from_log_parses_117_rolling_log_format() {
        let content = concat!(
            "timestamp=2026-06-12T13:56:30.468Z level=INFO run=2afdc4b8 message=loop session.id=ses_target step=0\n",
            "timestamp=2026-06-12T13:56:30.482Z level=INFO run=2afdc4b8 message=stream providerID=opencode session.id=ses_target small=true\n",
            "timestamp=2026-06-12T13:56:33.236Z level=INFO run=2afdc4b8 message=loop session.id=ses_target step=1\n",
            "timestamp=2026-06-12T13:56:33.237Z level=INFO run=2afdc4b8 message=\"exiting loop\" session.id=ses_target\n",
            "timestamp=2026-06-12T13:56:34.000Z level=INFO run=ffff9999 message=loop session.id=ses_other step=0\n",
        );

        let metrics = opencode_metrics_from_log(content, "ses_target");

        assert_eq!(metrics.query_count, 2);
        assert_eq!(metrics.status.as_deref(), Some("Idle"));
        assert_eq!(
            metrics.init_timestamp.as_deref(),
            Some("2026-06-12T13:56:30.468Z")
        );
    }

    #[test]
    fn opencode_metrics_from_log_117_format_reports_processing_and_error() {
        let processing = "timestamp=2026-06-12T13:56:30.468Z level=INFO run=2afdc4b8 message=loop session.id=ses_target step=0\n";
        let metrics = opencode_metrics_from_log(processing, "ses_target");
        assert_eq!(metrics.status.as_deref(), Some("Processing..."));

        let errored = concat!(
            "timestamp=2026-06-12T13:56:30.468Z level=INFO run=2afdc4b8 message=loop session.id=ses_target step=0\n",
            "timestamp=2026-06-12T13:56:31.000Z level=ERROR run=2afdc4b8 message=failed session.id=ses_target\n",
        );
        let metrics = opencode_metrics_from_log(errored, "ses_target");
        assert_eq!(metrics.status.as_deref(), Some("Error"));
    }
}
