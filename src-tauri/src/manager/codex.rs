use crate::utils::fs::sync_codex_windows_sandbox_support;
pub(crate) fn codex_bootstrap_workspace_key(workspace_cwd: &std::path::Path) -> String {
    let normalized = workspace_cwd.to_string_lossy().to_ascii_lowercase();
    let mut hash = 0xcbf29ce484222325u64;
    for byte in normalized.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("workspace-{hash:016x}")
}

pub(crate) fn codex_session_file_path_in(
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

pub(crate) fn codex_log_lookup_session_id(resume_session: Option<&str>) -> Option<&str> {
    resume_session.filter(|value| !value.trim().is_empty())
}

pub(crate) fn codex_provider_session_is_excluded(candidate: &str, excluded: &[String]) -> bool {
    let candidate = candidate.trim();
    !candidate.is_empty()
        && excluded
            .iter()
            .any(|session_id| session_id.trim() == candidate)
}

pub(crate) fn codex_status_from_log(lines: &[serde_json::Value]) -> Option<String> {
    for line in lines.iter().rev() {
        let payload = line.get("payload")?;
        let payload_type = payload.get("type").and_then(|v| v.as_str())?;
        match payload_type {
            "exec_approval_request" => return Some("Action Needed".to_string()),
            "task_started"
            | "agent_message"
            | "exec_command_begin"
            | "exec_command_start"
            | "custom_tool_call"
            | "custom_tool_call_output"
            | "function_call"
            | "function_call_output"
            | "reasoning" => {
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
    sync_codex_windows_sandbox_support(bootstrap_home, final_home)?;

    let entries = std::fs::read_dir(bootstrap_home).map_err(|e| e.to_string())?;
    for entry in entries.flatten() {
        let source = entry.path();
        let name = entry.file_name();
        let target = final_home.join(&name);
        let name_str = name.to_string_lossy();

        if matches!(
            name_str.as_ref(),
            "auth.json"
                | "config.toml"
                | "cap_sid"
                | "state_5.sqlite"
                | "state_5.sqlite-shm"
                | "state_5.sqlite-wal"
                | "logs_2.sqlite"
                | "logs_2.sqlite-shm"
                | "logs_2.sqlite-wal"
                | ".sandbox"
                | ".sandbox-bin"
                | ".sandbox-secrets"
        ) {
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

        if name_str == "sessions" {
            merge_missing_codex_session_entries(&source, &target)?;
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

/// Merge fresh bootstrap rollouts into an existing agent-local Codex session tree.
///
/// Established agents already have a `sessions` directory, so treating it as an
/// all-or-nothing top-level entry leaves every newly bootstrapped rollout behind.
/// Session rollouts have unique thread IDs; retain any existing file on collision
/// and move only missing entries into the agent's projected `CODEX_HOME`.
fn merge_missing_codex_session_entries(
    source_dir: &std::path::Path,
    target_dir: &std::path::Path,
) -> Result<(), String> {
    std::fs::create_dir_all(target_dir).map_err(|e| e.to_string())?;

    let entries = std::fs::read_dir(source_dir).map_err(|e| e.to_string())?;
    for entry in entries.flatten() {
        let source = entry.path();
        let target = target_dir.join(entry.file_name());

        if source.is_dir() {
            if target.exists() || target.symlink_metadata().is_ok() {
                if target.is_dir() {
                    merge_missing_codex_session_entries(&source, &target)?;
                    let _ = std::fs::remove_dir(&source);
                }
                continue;
            }
        } else if target.exists() || target.symlink_metadata().is_ok() {
            continue;
        }

        std::fs::rename(&source, &target).map_err(|e| e.to_string())?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manager::{strip_flag_value_pairs, strip_standalone_flag};
    use std::path::Path;
    use wardian_core::models::AgentConfig;
    #[test]
    fn codex_log_lookup_prefers_provider_thread_id_when_available() {
        assert_eq!(
            codex_log_lookup_session_id(Some("codex-thread-123")),
            Some("codex-thread-123")
        );
    }

    #[test]
    fn codex_log_lookup_waits_until_provider_thread_id_is_known() {
        assert_eq!(codex_log_lookup_session_id(Some("   ")), None,);
        assert_eq!(codex_log_lookup_session_id(None), None);
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
        std::fs::write(bootstrap_home.join("state_5.sqlite"), "state").expect("write state");
        std::fs::write(bootstrap_home.join("logs_2.sqlite"), "logs").expect("write logs");
        std::fs::create_dir_all(bootstrap_home.join("sessions")).expect("create sessions dir");
        std::fs::write(
            bootstrap_home.join("sessions").join("session.jsonl"),
            "session",
        )
        .expect("write session file");

        migrate_codex_bootstrap_home(&bootstrap_home, &final_home).expect("migrate bootstrap home");

        assert!(bootstrap_home.exists());
        assert!(bootstrap_home.join("config.toml").exists());
        assert!(bootstrap_home.join("state_5.sqlite").exists());
        assert!(bootstrap_home.join("logs_2.sqlite").exists());
        assert!(!final_home.join("state_5.sqlite").exists());
        assert!(!final_home.join("logs_2.sqlite").exists());
        assert!(final_home.join("sessions").join("session.jsonl").exists());
    }

    #[test]
    fn migrate_codex_bootstrap_home_merges_sessions_into_existing_agent_history() {
        let temp = tempfile::tempdir().expect("temp dir");
        let bootstrap_home = temp.path().join("bootstrap").join(".codex");
        let final_home = temp.path().join("final").join(".codex");
        let old_rollout = final_home
            .join("sessions")
            .join("2026")
            .join("07")
            .join("19")
            .join("old.jsonl");
        let fresh_rollout = bootstrap_home
            .join("sessions")
            .join("2026")
            .join("07")
            .join("20")
            .join("fresh.jsonl");

        std::fs::create_dir_all(old_rollout.parent().expect("old rollout parent"))
            .expect("create existing agent session tree");
        std::fs::write(&old_rollout, "old session").expect("write old rollout");
        std::fs::create_dir_all(fresh_rollout.parent().expect("fresh rollout parent"))
            .expect("create bootstrap session tree");
        std::fs::write(&fresh_rollout, "fresh session").expect("write fresh rollout");

        migrate_codex_bootstrap_home(&bootstrap_home, &final_home)
            .expect("merge bootstrap session history");

        assert_eq!(
            std::fs::read_to_string(&old_rollout).unwrap(),
            "old session"
        );
        assert_eq!(
            std::fs::read_to_string(
                final_home
                    .join("sessions")
                    .join("2026")
                    .join("07")
                    .join("20")
                    .join("fresh.jsonl")
            )
            .unwrap(),
            "fresh session"
        );
        assert!(
            !fresh_rollout.exists(),
            "the bootstrap rollout must leave the reusable bootstrap home"
        );
    }

    #[cfg(windows)]
    #[test]
    fn migrate_codex_bootstrap_home_projects_windows_sandbox_support_without_moving_it() {
        let temp = tempfile::tempdir().expect("temp dir");
        let bootstrap_home = temp.path().join("bootstrap").join(".codex");
        let final_home = temp.path().join("final").join(".codex");

        std::fs::create_dir_all(bootstrap_home.join(".sandbox-secrets"))
            .expect("create sandbox secrets");
        std::fs::create_dir_all(bootstrap_home.join(".sandbox-bin")).expect("create sandbox bin");
        std::fs::create_dir_all(bootstrap_home.join(".sandbox")).expect("create sandbox dir");
        std::fs::create_dir_all(final_home.join(".sandbox")).expect("create final sandbox dir");
        std::fs::create_dir_all(bootstrap_home.join("sessions")).expect("create sessions dir");
        std::fs::write(
            bootstrap_home
                .join(".sandbox-secrets")
                .join("sandbox_users.json"),
            "secrets",
        )
        .expect("write secrets");
        std::fs::write(
            bootstrap_home
                .join(".sandbox-bin")
                .join("codex-command-runner.exe"),
            "runner",
        )
        .expect("write runner");
        std::fs::write(
            bootstrap_home.join(".sandbox").join("setup_marker.json"),
            "marker",
        )
        .expect("write marker");
        std::fs::write(bootstrap_home.join(".sandbox").join("sandbox.log"), "log")
            .expect("write log");
        std::fs::write(final_home.join(".sandbox").join("sandbox.log"), "final log")
            .expect("write final log");
        std::fs::write(
            bootstrap_home
                .join("sessions")
                .join("bootstrap-session.jsonl"),
            "session",
        )
        .expect("write session");

        migrate_codex_bootstrap_home(&bootstrap_home, &final_home).expect("migrate bootstrap home");

        assert!(bootstrap_home.join(".sandbox-secrets").exists());
        assert!(bootstrap_home.join(".sandbox-bin").exists());
        assert!(bootstrap_home.join(".sandbox").exists());
        assert_eq!(
            std::fs::read_to_string(
                final_home
                    .join(".sandbox-secrets")
                    .join("sandbox_users.json")
            )
            .expect("read final secrets"),
            "secrets"
        );
        assert_eq!(
            std::fs::read_to_string(
                final_home
                    .join(".sandbox-bin")
                    .join("codex-command-runner.exe")
            )
            .expect("read final runner"),
            "runner"
        );
        assert_eq!(
            std::fs::read_to_string(final_home.join(".sandbox").join("setup_marker.json"))
                .expect("read final marker"),
            "marker"
        );
        assert_eq!(
            std::fs::read_to_string(final_home.join(".sandbox").join("sandbox.log"))
                .expect("read final log"),
            "final log"
        );
        assert!(
            final_home
                .join("sessions")
                .join("bootstrap-session.jsonl")
                .exists(),
            "session logs must still migrate for resume/log tracking"
        );
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
    fn codex_status_from_log_treats_response_items_as_processing() {
        let lines = vec![
            serde_json::json!({
                "type": "response_item",
                "payload": {
                    "type": "function_call",
                    "name": "shell_command"
                }
            }),
            serde_json::json!({
                "type": "event_msg",
                "payload": {
                    "type": "token_count"
                }
            }),
        ];

        assert_eq!(
            codex_status_from_log(&lines).as_deref(),
            Some("Processing...")
        );
    }

    #[test]
    fn codex_status_from_log_treats_custom_tool_items_as_processing() {
        let lines = vec![serde_json::json!({
            "type": "response_item",
            "payload": {
                "type": "custom_tool_call_output",
                "call_id": "call_1"
            }
        })];

        assert_eq!(
            codex_status_from_log(&lines).as_deref(),
            Some("Processing...")
        );
    }

    #[test]
    fn codex_bootstrap_exec_mode_keeps_skip_git_repo_check() {
        let config = AgentConfig {
            provider: "codex".to_string(),
            provider_config: wardian_core::models::ProviderConfig::Codex(
                wardian_core::models::CodexProviderConfig {
                    skip_git_repo_check: Some(true),
                    ..Default::default()
                },
            ),
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
        if config.codex_config().skip_git_repo_check.unwrap_or(true) {
            provider_args.push("--skip-git-repo-check".to_string());
        }

        assert!(provider_args.contains(&"--skip-git-repo-check".to_string()));
        assert!(!provider_args.contains(&"--no-alt-screen".to_string()));
    }
}
