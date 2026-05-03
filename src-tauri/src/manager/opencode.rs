use wardian_core::models::AgentConfig;
use crate::utils::fs::*;
use super::codex::codex_bootstrap_workspace_key;
use chrono::TimeZone;
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

pub(crate) fn opencode_session_diff_path(session_id: &str) -> std::path::PathBuf {
    let base = dirs::data_local_dir()
        .or_else(|| dirs::home_dir().map(|home| home.join(".local").join("share")));
    let base = base.unwrap_or_else(|| std::path::PathBuf::from("."));
    base.join("opencode")
        .join("storage")
        .join("session_diff")
        .join(format!("{session_id}.json"))
}

pub(crate) fn opencode_should_fallback_to_idle(
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

/// Find the newest OpenCode log file whose filesystem mtime is at or after
/// `spawn_time`.  OpenCode creates one log file per process invocation using
/// a timestamp-based name, so the file(s) created after the PTY was launched
/// belong to this session.
///
/// OpenCode sometimes spawns a background server subprocess that writes to a
/// SEPARATE log file roughly 1 s after the parent process starts.  The watcher
/// calls this function on every tick while `ses_id` is still unknown so it can
/// switch to that newer server log once it appears.
///
/// Returns the file with the highest mtime among all qualifying candidates so
/// that the server log (newer) wins over the parent log (older).
#[cfg(test)]
fn opencode_log_path_after(
    base: &std::path::Path,
    spawn_time: std::time::SystemTime,
) -> Option<std::path::PathBuf> {
    let mut candidates: Vec<_> = std::fs::read_dir(base)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("log"))
        .filter_map(|p| {
            let mtime = p.metadata().ok()?.modified().ok()?;
            if mtime >= spawn_time {
                Some((p, mtime))
            } else {
                None
            }
        })
        .collect();

    // Newest mtime last → next_back() returns the most recently created file.
    candidates.sort_by_key(|(_, mtime)| *mtime);
    candidates.into_iter().next_back().map(|(p, _)| p)
}

/// Find the OpenCode log file for a session by content-searching for the
/// Wardian session UUID.  The UUID appears in log entries because
/// `OPENCODE_CONFIG` points to a config file whose path embeds the UUID.
///
/// Used for sessions recovered after an app restart (where no live watcher
/// is running).
pub(crate) fn opencode_log_path_in(base: &std::path::Path, session_id: &str) -> Option<std::path::PathBuf> {
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
/// Tries platform-native data dirs first (Windows: %LOCALAPPDATA%, %APPDATA%),
/// then the XDG fallback (~/.local/share).
pub(crate) fn opencode_log_dirs() -> Vec<std::path::PathBuf> {
    let mut dirs: Vec<std::path::PathBuf> = Vec::new();
    if let Some(d) = dirs::data_local_dir() {
        dirs.push(d.join("opencode").join("log"));
    }
    if let Some(d) = dirs::data_dir() {
        let p = d.join("opencode").join("log");
        if !dirs.contains(&p) {
            dirs.push(p);
        }
    }
    if let Some(h) = dirs::home_dir() {
        let p = h.join(".local").join("share").join("opencode").join("log");
        if !dirs.contains(&p) {
            dirs.push(p);
        }
    }
    dirs
}

/// Extract the real opencode session ID (ses_xxx) from a log file.
/// Looks for `service=session id=ses_xxx ... created` lines and returns
/// the last one found (most recently created session in the log).
pub fn opencode_extract_created_session_id(log_path: &std::path::Path) -> Option<String> {
    let content = std::fs::read_to_string(log_path).ok()?;
    content
        .lines()
        .filter(|line| line.contains("service=session") && line.contains("created"))
        .filter_map(|line| {
            line.split_whitespace()
                .find(|w| w.starts_with("id=ses_"))
                .and_then(|w| w.strip_prefix("id="))
                .map(|id| id.to_string())
        })
        .next_back()
}

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct OpenCodeLogMetrics {
    query_count: usize,
    init_timestamp: Option<String>,
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

    for line in content.lines() {
        if !line.contains(session_id) {
            continue;
        }

        if metrics.init_timestamp.is_none() {
            metrics.init_timestamp = line
                .split_whitespace()
                .nth(1)
                .and_then(opencode_log_timestamp_to_rfc3339);
        }

        if line.contains("service=session.prompt") {
            if line.contains("exiting loop") {
                last_prompt_exited = true;
                saw_prompt = true;
            } else if line.contains(" step=") {
                metrics.query_count += 1;
                last_prompt_exited = false;
                saw_prompt = true;
            }
            continue;
        }

        if line.starts_with("ERROR ") || line.contains(" ERROR ") {
            saw_error = true;
        }
    }

    metrics.status = if saw_error && !last_prompt_exited {
        Some("Error".to_string())
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
    current_status: &mut String,
) {
    let metrics = opencode_metrics_from_log(content, session_id);
    if metrics.query_count > 0 {
        *query_count = metrics.query_count;
    }
    if init_timestamp.is_none() && metrics.init_timestamp.is_some() {
        *init_timestamp = metrics.init_timestamp;
    }
    if let Some(status) = metrics.status {
        *current_status = status;
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::manager::{
        finalize_interactive_spawn_args, interactive_provider_args, interactive_provider_launch,
        session_bootstrap_prompt,
    };
    use crate::manager::headless::{headless_provider_args, headless_provider_launch};
    use wardian_core::models::AgentConfig;
    use std::path::Path;
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

        #[test]
    fn opencode_idle_fallback_triggers_after_quiet_period() {
        let now = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(10);
        let last = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(3);

        assert!(opencode_should_fallback_to_idle(
            "Processing...",
            Some(last),
            now
        ));
        assert!(!opencode_should_fallback_to_idle("Idle", Some(last), now));
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
    fn opencode_log_path_after_returns_newest_file_created_after_spawn() {
        let temp = tempfile::tempdir().expect("temp dir");
        let log_dir = temp.path().join("log");
        std::fs::create_dir_all(&log_dir).expect("create log dir");

        // Pre-existing log (before spawn — must be ignored).
        let old_file = log_dir.join("2026-04-12T100000.log");
        std::fs::write(&old_file, "old session log").expect("write old");

        std::thread::sleep(std::time::Duration::from_millis(20));
        let spawn_time = std::time::SystemTime::now();
        std::thread::sleep(std::time::Duration::from_millis(20));

        // Parent log — created first after spawn.
        let parent_log = log_dir.join("2026-04-12T100001.log");
        std::fs::write(&parent_log, "parent process startup").expect("write parent");

        std::thread::sleep(std::time::Duration::from_millis(20));

        // Server log — created ~1 s later; should win because it has higher mtime.
        let server_log = log_dir.join("2026-04-12T100002.log");
        std::fs::write(&server_log, "server session events").expect("write server");

        let found =
            opencode_log_path_after(&log_dir, spawn_time).expect("should find a log after spawn");

        assert_eq!(
            found, server_log,
            "should return the newest (server) log, not the parent log"
        );
    }

    #[test]
    fn opencode_log_path_after_returns_none_when_no_files_after_spawn() {
        let temp = tempfile::tempdir().expect("temp dir");
        let log_dir = temp.path().join("log");
        std::fs::create_dir_all(&log_dir).expect("create log dir");

        let file = log_dir.join("2026-04-12T100001.log");
        std::fs::write(&file, "session started").expect("write");

        let future = std::time::SystemTime::now() + std::time::Duration::from_secs(3600);
        assert!(opencode_log_path_after(&log_dir, future).is_none());
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
        let mut current_status = "Processing...".to_string();

        apply_opencode_log_metrics(
            current,
            "ses_target",
            &mut query_count,
            &mut init_timestamp,
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
    }

    #[test]
    fn opencode_log_metrics_preserve_existing_rfc3339_birth_timestamp() {
        let current = concat!(
            "INFO  2026-04-26T04:28:39 +1ms service=session.prompt session.id=ses_target step=0 loop\n",
            "INFO  2026-04-26T04:28:42 +1ms service=session.prompt session.id=ses_target exiting loop\n",
        );

        let mut query_count = 0;
        let mut init_timestamp = Some("2026-04-26T04:20:00.000Z".to_string());
        let mut current_status = "Processing...".to_string();

        apply_opencode_log_metrics(
            current,
            "ses_target",
            &mut query_count,
            &mut init_timestamp,
            &mut current_status,
        );

        assert_eq!(init_timestamp, Some("2026-04-26T04:20:00.000Z".to_string()));
        assert_eq!(current_status, "Idle");
    }

        #[test]
    fn opencode_interactive_args_include_dir_for_real_workspace_anchor() {
        let workspace_cwd = Path::new("D:/Development/Wardian");

        let args = interactive_provider_args("opencode", workspace_cwd, workspace_cwd, Vec::new());

        assert_eq!(args, vec!["D:/Development/Wardian".to_string()]);
    }

        #[cfg(target_os = "windows")]
    #[test]
    fn opencode_interactive_launch_wraps_cmd_shims_through_cmd_exe() {
        let launch = interactive_provider_launch(
            "opencode",
            r"C:\nvm4w\nodejs\opencode.cmd",
            &["--session".to_string(), "ses_test".to_string()],
        )
        .expect("launch spec");

        assert!(
            launch.executable.ends_with(r"\cmd.exe")
                || launch.executable.eq_ignore_ascii_case("cmd"),
            "expected cmd host, got {}",
            launch.executable
        );
        assert_eq!(launch.args[..2], ["/d".to_string(), "/c".to_string()]);
        assert!(launch.args[2].contains(r"C:\nvm4w\nodejs\opencode.cmd"));
        assert!(launch.args[2].contains("--session"));
        assert!(launch.args[2].contains("ses_test"));
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
    fn opencode_headless_launch_uses_cmd_host_on_windows() {
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

        assert!(
            launch.executable.to_ascii_lowercase().ends_with("cmd.exe"),
            "expected cmd.exe host, got {}",
            launch.executable
        );
        assert_eq!(launch.args[0], "/d");
        assert_eq!(launch.args[1], "/c");
        assert!(launch.args[2].contains("opencode"));
        assert!(launch.args[2].contains("--format"));
        assert!(launch.args[2].contains(session_bootstrap_prompt()));
    }

        #[test]
    fn opencode_fresh_headless_args_omit_session_flag_but_keep_config() {
        let provider = crate::providers::ProviderFactory::resolve("opencode").unwrap();
        let config = AgentConfig {
            opencode_agent: Some("build".into()),
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
}
