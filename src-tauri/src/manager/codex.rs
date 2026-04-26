use crate::utils::fs::get_wardian_home;
pub(crate) fn codex_bootstrap_workspace_key(workspace_cwd: &std::path::Path) -> String {
    let normalized = workspace_cwd.to_string_lossy().to_ascii_lowercase();
    let mut hash = 0xcbf29ce484222325u64;
    for byte in normalized.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("workspace-{hash:016x}")
}

fn codex_session_file_path_in(
    base: &std::path::Path,
    session_id: &str,
) -> Option<std::path::PathBuf> {
    let base = base.join("sessions");
    let years = std::fs::read_dir(base).ok()?;

    for year in years.flatten() {
        let months = match std::fs::read_dir(year.path()) {
            Ok(entries) => entries,
            Err(_) => continue,
        };
        for month in months.flatten() {
            let days = match std::fs::read_dir(month.path()) {
                Ok(entries) => entries,
                Err(_) => continue,
            };
            for day in days.flatten() {
                let files = match std::fs::read_dir(day.path()) {
                    Ok(entries) => entries,
                    Err(_) => continue,
                };
                for file in files.flatten() {
                    let path = file.path();
                    if !path.is_file() {
                        continue;
                    }
                    let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                    if file_name.ends_with(&format!("{}.jsonl", session_id)) {
                        return Some(path);
                    }
                }
            }
        }
    }

    None
}

pub(crate) fn codex_session_file_path(
    session_id: &str,
    wardian_agent_dir: Option<&str>,
) -> Option<std::path::PathBuf> {
    if let Some(agent_dir) = wardian_agent_dir {
        let projected_home = std::path::Path::new(agent_dir)
            .join("habitat")
            .join(".codex");
        if let Some(path) = codex_session_file_path_in(&projected_home, session_id) {
            return Some(path);
        }
    }

    let global_home = dirs::home_dir()?.join(".codex");
    codex_session_file_path_in(&global_home, session_id)
}

pub(crate) fn codex_log_lookup_session_id<'a>(
    wardian_session_id: &'a str,
    resume_session: Option<&'a str>,
) -> &'a str {
    resume_session
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(wardian_session_id)
}

pub(crate) fn codex_provider_session_is_excluded(candidate: &str, excluded: &[String]) -> bool {
    let candidate = candidate.trim();
    !candidate.is_empty()
        && excluded
            .iter()
            .any(|session_id| session_id.trim() == candidate)
}

fn latest_codex_session_from_index(
    index_path: &std::path::Path,
) -> Result<Option<(String, String)>, String> {
    if !index_path.exists() {
        return Ok(None);
    }

    let content = std::fs::read_to_string(index_path)
        .map_err(|e| format!("Failed to read Codex session index: {}", e))?;
    Ok(content.lines().rev().find_map(|line| {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            return None;
        }

        let parsed: serde_json::Value = serde_json::from_str(trimmed).ok()?;
        let session_id = parsed.get("id")?.as_str()?.trim();
        let updated_at = parsed.get("updated_at")?.as_str()?.trim();
        if session_id.is_empty() || updated_at.is_empty() {
            return None;
        }

        Some((session_id.to_string(), updated_at.to_string()))
    }))
}

fn latest_codex_session_from_history(
    history_path: &std::path::Path,
) -> Result<Option<(String, String)>, String> {
    if !history_path.exists() {
        return Ok(None);
    }

    let content = std::fs::read_to_string(history_path)
        .map_err(|e| format!("Failed to read Codex history: {}", e))?;
    Ok(content.lines().rev().find_map(|line| {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            return None;
        }

        let parsed: serde_json::Value = serde_json::from_str(trimmed).ok()?;
        let session_id = parsed.get("session_id")?.as_str()?.trim();
        if session_id.is_empty() {
            return None;
        }
        let updated_at = parsed
            .get("ts")
            .and_then(|value| {
                value
                    .as_i64()
                    .map(|value| value.to_string())
                    .or_else(|| value.as_str().map(str::to_string))
            })
            .unwrap_or_default();

        Some((session_id.to_string(), updated_at))
    }))
}

fn latest_codex_session_from_logs(
    codex_home: &std::path::Path,
) -> Result<Option<(String, String)>, String> {
    let sessions_root = codex_home.join("sessions");
    if !sessions_root.exists() {
        return Ok(None);
    }

    let mut stack = vec![sessions_root];
    let mut latest: Option<(String, String)> = None;
    while let Some(dir) = stack.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if path.extension().and_then(|ext| ext.to_str()) != Some("jsonl") {
                continue;
            }
            let Ok(content) = std::fs::read_to_string(&path) else {
                continue;
            };
            let Some((session_id, updated_at)) = content.lines().find_map(|line| {
                let parsed: serde_json::Value = serde_json::from_str(line.trim()).ok()?;
                if parsed.get("type").and_then(|value| value.as_str()) != Some("session_meta") {
                    return None;
                }
                let payload = parsed.get("payload")?;
                let session_id = payload.get("id")?.as_str()?.trim();
                let updated_at = payload
                    .get("timestamp")
                    .and_then(|value| value.as_str())
                    .unwrap_or("");
                if session_id.is_empty() || updated_at.is_empty() {
                    return None;
                }
                Some((session_id.to_string(), updated_at.to_string()))
            }) else {
                continue;
            };
            if latest
                .as_ref()
                .is_none_or(|(_latest_session_id, latest_at)| updated_at > *latest_at)
            {
                latest = Some((session_id, updated_at));
            }
        }
    }

    Ok(latest)
}

pub(crate) fn latest_codex_session_entry_in(
    codex_home: &std::path::Path,
) -> Result<Option<(String, String)>, String> {
    if let Some(entry) = latest_codex_session_from_index(&codex_home.join("session_index.jsonl"))? {
        return Ok(Some(entry));
    }
    if let Some(entry) = latest_codex_session_from_history(&codex_home.join("history.jsonl"))? {
        return Ok(Some(entry));
    }
    latest_codex_session_from_logs(codex_home)
}

pub fn latest_codex_session_index_entry(
    wardian_session_id: &str,
) -> Result<Option<(String, String)>, String> {
    let wardian_home = get_wardian_home().ok_or("Could not resolve Wardian home")?;
    let codex_home = wardian_home
        .join("agents")
        .join(wardian_session_id)
        .join("habitat")
        .join(".codex");

    latest_codex_session_entry_in(&codex_home)
}

pub(crate) fn codex_status_from_log(lines: &[serde_json::Value]) -> Option<String> {
    for line in lines.iter().rev() {
        let payload = line.get("payload")?;
        let payload_type = payload.get("type").and_then(|v| v.as_str())?;
        match payload_type {
            "exec_approval_request" => return Some("Action Needed".to_string()),
            "task_started" | "agent_message" | "exec_command_begin" | "exec_command_start" => {
                return Some("Processing...".to_string());
            }
            "task_complete" => return Some("Idle".to_string()),
            _ => {}
        }
    }
    None
}

pub(crate) fn codex_bootstrap_launch_context(
    wardian_home: &std::path::Path,
    workspace_cwd: &std::path::Path,
) -> (std::path::PathBuf, std::path::PathBuf) {
    let bootstrap_home = wardian_home
        .join("provider-bootstrap")
        .join("codex")
        .join(codex_bootstrap_workspace_key(workspace_cwd))
        .join(".codex");
    (workspace_cwd.to_path_buf(), bootstrap_home)
}

pub(crate) fn migrate_codex_bootstrap_home(
    bootstrap_home: &std::path::Path,
    final_home: &std::path::Path,
) -> Result<(), String> {
    if !bootstrap_home.exists() {
        return Ok(());
    }

    std::fs::create_dir_all(final_home).map_err(|e| e.to_string())?;

    let entries = std::fs::read_dir(bootstrap_home).map_err(|e| e.to_string())?;
    for entry in entries.flatten() {
        let source = entry.path();
        let name = entry.file_name();
        let target = final_home.join(&name);
        let name_str = name.to_string_lossy();

        if matches!(name_str.as_ref(), "auth.json" | "config.toml" | "cap_sid") {
            continue;
        }

        if name_str == "skills" {
            std::fs::create_dir_all(&target).map_err(|e| e.to_string())?;
            let skill_entries = std::fs::read_dir(&source).map_err(|e| e.to_string())?;
            for skill_entry in skill_entries.flatten() {
                let skill_source = skill_entry.path();
                let skill_target = target.join(skill_entry.file_name());
                if skill_target.exists() || skill_target.symlink_metadata().is_ok() {
                    continue;
                }
                std::fs::rename(&skill_source, &skill_target).map_err(|e| e.to_string())?;
            }
            let _ = std::fs::remove_dir_all(&source);
            continue;
        }

        if target.exists() || target.symlink_metadata().is_ok() {
            continue;
        }

        std::fs::rename(&source, &target).map_err(|e| e.to_string())?;
    }

    std::fs::create_dir_all(bootstrap_home).map_err(|e| e.to_string())?;

    Ok(())
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::manager::{strip_flag_value_pairs, strip_standalone_flag};
    use crate::models::AgentConfig;
    use std::path::Path;
        #[test]
    fn codex_log_lookup_prefers_provider_thread_id_when_available() {
        assert_eq!(
            codex_log_lookup_session_id(
                "22ff532b-007a-44c9-a4b4-9b7c0f546274",
                Some("codex-thread-123")
            ),
            "codex-thread-123"
        );
        assert_eq!(
            codex_log_lookup_session_id("22ff532b-007a-44c9-a4b4-9b7c0f546274", Some("   ")),
            "22ff532b-007a-44c9-a4b4-9b7c0f546274"
        );
        assert_eq!(
            codex_log_lookup_session_id("22ff532b-007a-44c9-a4b4-9b7c0f546274", None),
            "22ff532b-007a-44c9-a4b4-9b7c0f546274"
        );
    }

        #[test]
    fn latest_codex_session_entry_reads_history_when_index_is_missing() {
        let temp = tempfile::tempdir().expect("temp dir");
        let codex_home = temp.path();
        std::fs::write(
            codex_home.join("history.jsonl"),
            concat!(
                "{\"session_id\":\"019db2df-5679-7e91-b553-ce7a434bc31c\",\"ts\":1776823760,\"text\":\"Test\"}\n",
                "{\"session_id\":\"019db2f3-22de-7861-8bc6-1b86db1686db\",\"ts\":1776823781,\"text\":\"What was my last message?\"}\n",
            ),
        )
        .expect("write history");

        assert_eq!(
            latest_codex_session_entry_in(codex_home).expect("latest entry"),
            Some((
                "019db2f3-22de-7861-8bc6-1b86db1686db".to_string(),
                "1776823781".to_string()
            ))
        );
    }

    #[test]
    fn latest_codex_session_entry_reads_newest_session_meta_when_history_is_missing() {
        let temp = tempfile::tempdir().expect("temp dir");
        let codex_home = temp.path();
        let older_dir = codex_home
            .join("sessions")
            .join("2026")
            .join("04")
            .join("20");
        let newer_dir = codex_home
            .join("sessions")
            .join("2026")
            .join("04")
            .join("21");
        std::fs::create_dir_all(&older_dir).expect("create older dir");
        std::fs::create_dir_all(&newer_dir).expect("create newer dir");
        std::fs::write(
            older_dir.join("rollout-older.jsonl"),
            "{\"type\":\"session_meta\",\"payload\":{\"id\":\"older-session\",\"timestamp\":\"2026-04-21T01:00:00.000Z\"}}\n",
        )
        .expect("write older log");
        std::fs::write(
            newer_dir.join("rollout-newer.jsonl"),
            "{\"type\":\"session_meta\",\"payload\":{\"id\":\"newer-session\",\"timestamp\":\"2026-04-22T02:09:32.024Z\"}}\n",
        )
        .expect("write newer log");

        assert_eq!(
            latest_codex_session_entry_in(codex_home).expect("latest entry"),
            Some((
                "newer-session".to_string(),
                "2026-04-22T02:09:32.024Z".to_string()
            ))
        );
    }

        #[test]
    fn codex_bootstrap_launch_context_is_stable_for_same_workspace() {
        let wardian_home = Path::new("C:/Users/test/.wardian");
        let workspace_cwd = Path::new("D:/Development/Wardian");

        let (_, first_bootstrap_home) = codex_bootstrap_launch_context(wardian_home, workspace_cwd);
        let (_, second_bootstrap_home) =
            codex_bootstrap_launch_context(wardian_home, workspace_cwd);

        assert_eq!(first_bootstrap_home, second_bootstrap_home);
    }

    #[test]
    fn migrate_codex_bootstrap_home_keeps_bootstrap_root_for_reuse() {
        let temp = tempfile::tempdir().expect("temp dir");
        let bootstrap_home = temp.path().join("bootstrap").join(".codex");
        let final_home = temp.path().join("final").join(".codex");

        std::fs::create_dir_all(&bootstrap_home).expect("create bootstrap home");
        std::fs::write(bootstrap_home.join("config.toml"), "config").expect("write config");
        std::fs::create_dir_all(bootstrap_home.join("sessions")).expect("create sessions dir");
        std::fs::write(
            bootstrap_home.join("sessions").join("session.jsonl"),
            "session",
        )
        .expect("write session file");

        migrate_codex_bootstrap_home(&bootstrap_home, &final_home).expect("migrate bootstrap home");

        assert!(bootstrap_home.exists());
        assert!(bootstrap_home.join("config.toml").exists());
        assert!(final_home.join("sessions").join("session.jsonl").exists());
    }

    #[test]
    fn codex_bootstrap_launch_context_uses_real_workspace_cwd() {
        let wardian_home = Path::new("C:/Users/test/.wardian");
        let workspace_cwd = Path::new("D:/Development/Wardian");
        let (provider_cwd, bootstrap_home) =
            codex_bootstrap_launch_context(wardian_home, workspace_cwd);

        assert_eq!(provider_cwd, workspace_cwd);
        assert!(bootstrap_home.starts_with(wardian_home.join("provider-bootstrap").join("codex")));
        assert_ne!(bootstrap_home, workspace_cwd);
    }

        #[test]
    fn codex_bootstrap_exec_mode_keeps_skip_git_repo_check() {
        let config = AgentConfig {
            provider: "codex".to_string(),
            codex_skip_git_repo_check: Some(true),
            ..Default::default()
        };

        let provider = crate::providers::ProviderFactory::resolve("codex").unwrap();
        let (_bin, mut provider_args) = provider.get_executable();
        provider_args.push("--cd".to_string());
        provider_args.push("D:/Development/Wardian".to_string());
        provider_args.push("exec".to_string());
        let spawn_args =
            strip_flag_value_pairs(provider.get_spawn_args(&config, false), "--add-dir");
        provider_args.extend(strip_standalone_flag(spawn_args, "--no-alt-screen"));
        if config.codex_skip_git_repo_check.unwrap_or(true) {
            provider_args.push("--skip-git-repo-check".to_string());
        }

        assert!(provider_args.contains(&"--skip-git-repo-check".to_string()));
        assert!(!provider_args.contains(&"--no-alt-screen".to_string()));
    }
}
