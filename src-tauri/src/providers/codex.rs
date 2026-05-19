use crate::utils::CodexRuntimePolicy;
use wardian_core::models::provider::{AgentEvent, AgentProvider};
use wardian_core::models::{AgentConfig, CodexProviderConfig};

/// Provider adapter for the OpenAI Codex CLI.
pub struct CodexProvider;

impl Default for CodexProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl CodexProvider {
    pub fn new() -> Self {
        CodexProvider
    }

    #[cfg(target_os = "windows")]
    fn find_windows_codex_in_paths<I>(paths: I, path_exts: &[String]) -> Option<String>
    where
        I: IntoIterator<Item = std::path::PathBuf>,
    {
        for path in paths {
            for ext in path_exts {
                let candidate = path.join(format!("codex{ext}"));
                if candidate.exists() {
                    return Some(candidate.to_string_lossy().to_string());
                }
            }

            let powershell = path.join("codex.ps1");
            if powershell.exists() {
                return Some(powershell.to_string_lossy().to_string());
            }
        }

        None
    }

    #[cfg(not(target_os = "windows"))]
    fn find_unix_codex_in_paths<I>(paths: I) -> Option<String>
    where
        I: IntoIterator<Item = std::path::PathBuf>,
    {
        for path in paths {
            let candidate = path.join("codex");
            if candidate.is_file() {
                return Some(candidate.to_string_lossy().to_string());
            }
        }

        None
    }

    #[cfg(not(target_os = "windows"))]
    fn codex_unix_fallback_paths() -> Vec<std::path::PathBuf> {
        let home = dirs::home_dir().unwrap_or_default();
        vec![
            home.join(".local/bin/codex"),
            std::path::PathBuf::from("/usr/local/bin/codex"),
            std::path::PathBuf::from("/opt/homebrew/bin/codex"),
            std::path::PathBuf::from("/opt/homebrew/sbin/codex"),
            home.join(".npm-global/bin/codex"),
            home.join(".volta/bin/codex"),
        ]
    }

    fn parse_action_required_from_arguments(arguments: &str) -> Option<String> {
        let parsed: serde_json::Value = serde_json::from_str(arguments).ok()?;
        let sandbox_permissions = parsed.get("sandbox_permissions")?.as_str()?;
        if sandbox_permissions != "require_escalated" {
            return None;
        }

        let justification = parsed
            .get("justification")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty());
        if let Some(justification) = justification {
            return Some(justification.to_string());
        }

        let command = parsed
            .get("command")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty());
        if let Some(command) = command {
            return Some(command.to_string());
        }

        Some("Approval required".to_string())
    }

    fn append_common_args(&self, args: &mut Vec<String>, config: &AgentConfig, is_exec_mode: bool) {
        let codex = config.codex_config();
        if let Some(ref model) = config.model {
            args.push("--model".into());
            args.push(model.clone());
        }

        if let Some(ref profile) = codex.profile {
            if !profile.trim().is_empty() {
                args.push("--profile".into());
                args.push(profile.clone());
            }
        }

        let runtime_policy = crate::utils::load_codex_runtime_policy().unwrap_or_default();
        let effective_policy = effective_codex_runtime_policy(&codex, &runtime_policy);
        if effective_policy.full_auto {
            #[cfg(target_os = "windows")]
            {
                // Codex can still inherit `[windows].sandbox = "elevated"` from
                // config.toml, which launches a UAC setup helper during tool
                // execution even when the session is otherwise in YOLO mode.
                // `unelevated` is Codex's non-admin Windows sandbox backend;
                // the top-level bypass flag still requests unsandboxed tools,
                // and this prevents a fallback path from using UAC.
                args.push("-c".into());
                args.push(r#"windows.sandbox="unelevated""#.into());
            }
            args.push("--dangerously-bypass-approvals-and-sandbox".into());
        } else {
            if !effective_policy.sandbox_mode.trim().is_empty() {
                args.push("--sandbox".into());
                args.push(effective_policy.sandbox_mode);
            }

            if !effective_policy.approval_policy.trim().is_empty() {
                args.push("--ask-for-approval".into());
                args.push(effective_policy.approval_policy);
            }
        }

        if codex.search.unwrap_or(false) {
            args.push("--search".into());
        }

        if is_exec_mode {
            if codex.skip_git_repo_check.unwrap_or(true) {
                args.push("--skip-git-repo-check".into());
            }

            if codex.ephemeral.unwrap_or(false) {
                args.push("--ephemeral".into());
            }
        } else {
            // Codex documents this as inline TUI mode that preserves terminal
            // scrollback. Wardian embeds the TUI inside xterm, so interactive
            // sessions should prefer scrollback-friendly output.
            args.push("--no-alt-screen".into());
            // Wardian supplies its own local skills/app surface. Disabling
            // Codex's plugin/app startup sync prevents large provider-owned
            // downloads from being triggered just by opening an agent terminal.
            args.push("--disable".into());
            args.push("plugins".into());
            args.push("--disable".into());
            args.push("apps".into());
        }

        let mut explicit_includes = Vec::new();
        if let Some(ref user_dirs) = config.include_directories {
            for dir in user_dirs {
                if !explicit_includes.contains(dir) {
                    explicit_includes.push(dir.clone());
                }
            }
        }
        for dir in explicit_includes {
            args.push("--add-dir".into());
            args.push(dir);
        }
    }
}

fn effective_codex_runtime_policy(
    config: &CodexProviderConfig,
    global_policy: &CodexRuntimePolicy,
) -> CodexRuntimePolicy {
    let explicit_sandbox = config
        .sandbox_mode
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let explicit_approval = config
        .approval_policy
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let explicit_policy = explicit_sandbox.is_some() || explicit_approval.is_some();
    let full_auto = config.full_auto.unwrap_or({
        if explicit_policy {
            false
        } else {
            global_policy.full_auto
        }
    });

    CodexRuntimePolicy {
        sandbox_mode: explicit_sandbox
            .unwrap_or(global_policy.sandbox_mode.as_str())
            .to_string(),
        approval_policy: explicit_approval
            .unwrap_or(global_policy.approval_policy.as_str())
            .to_string(),
        full_auto,
    }
}

impl AgentProvider for CodexProvider {
    fn name(&self) -> &str {
        "Codex"
    }

    fn get_executable(&self) -> (String, Vec<String>) {
        #[cfg(target_os = "windows")]
        {
            if let Some(paths) = std::env::var_os("PATH") {
                let path_exts = std::env::var("PATHEXT")
                    .ok()
                    .map(|value| {
                        value
                            .split(';')
                            .filter_map(|segment| {
                                let trimmed = segment.trim();
                                if trimmed.is_empty() {
                                    None
                                } else {
                                    Some(trimmed.to_ascii_lowercase())
                                }
                            })
                            .collect::<Vec<_>>()
                    })
                    .filter(|exts| !exts.is_empty())
                    .unwrap_or_else(|| {
                        vec![".exe".to_string(), ".cmd".to_string(), ".bat".to_string()]
                    });

                if let Some(executable) =
                    Self::find_windows_codex_in_paths(std::env::split_paths(&paths), &path_exts)
                {
                    return (executable, vec![]);
                }
            }

            ("codex".to_string(), vec![])
        }

        #[cfg(not(target_os = "windows"))]
        {
            #[cfg(target_os = "macos")]
            {
                for path in Self::codex_unix_fallback_paths() {
                    if path.is_file() {
                        return (path.to_string_lossy().to_string(), vec![]);
                    }
                }
            }

            if let Some(paths) = std::env::var_os("PATH") {
                if let Some(executable) =
                    Self::find_unix_codex_in_paths(std::env::split_paths(&paths))
                {
                    return (executable, vec![]);
                }
            }

            #[cfg(not(target_os = "macos"))]
            {
                for path in Self::codex_unix_fallback_paths() {
                    if path.is_file() {
                        return (path.to_string_lossy().to_string(), vec![]);
                    }
                }
            }

            ("codex".to_string(), vec![])
        }
    }

    fn get_spawn_args(&self, config: &AgentConfig, is_resume: bool) -> Vec<String> {
        let mut args: Vec<String> = Vec::new();

        if is_resume {
            args.push("resume".into());
            if let Some(resume_id) = config
                .resume_session
                .as_ref()
                .filter(|s| !s.trim().is_empty())
            {
                args.push(resume_id.clone());
            }
        }

        self.append_common_args(&mut args, config, false);

        if let Some(ref custom) = config.custom_args {
            if let Some(parsed) = shlex::split(custom) {
                args.extend(parsed);
            }
        }

        args
    }

    fn parse_output(&self, line: &str) -> Option<AgentEvent> {
        let parsed: serde_json::Value = serde_json::from_str(line).ok()?;
        let msg_type = parsed.get("type")?.as_str()?;

        match msg_type {
            "thread.started" => {
                let session_id = parsed
                    .get("thread_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                Some(AgentEvent::Init {
                    session_id,
                    timestamp: None,
                })
            }
            "turn.started" => Some(AgentEvent::UserQuery),
            "turn.completed" => Some(AgentEvent::TurnCompleted),
            "item.completed" => {
                let item_type = parsed
                    .get("item")
                    .and_then(|v| v.get("type"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                match item_type {
                    "agent_message" => Some(AgentEvent::Unknown),
                    _ => Some(AgentEvent::Unknown),
                }
            }
            "response_item" => {
                let payload = parsed.get("payload")?;
                let payload_type = payload.get("type").and_then(|v| v.as_str()).unwrap_or("");
                match payload_type {
                    "function_call" => {
                        let arguments = payload
                            .get("arguments")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        Self::parse_action_required_from_arguments(arguments)
                            .map(|message| AgentEvent::ActionRequired { message })
                            .or(Some(AgentEvent::Generating))
                    }
                    "custom_tool_call"
                    | "custom_tool_call_output"
                    | "function_call_output"
                    | "reasoning" => Some(AgentEvent::Generating),
                    "message" => {
                        let role = payload.get("role").and_then(|v| v.as_str()).unwrap_or("");
                        match role {
                            "assistant" => Some(AgentEvent::Unknown),
                            "user" => Some(AgentEvent::UserQuery),
                            _ => Some(AgentEvent::Unknown),
                        }
                    }
                    _ => Some(AgentEvent::Unknown),
                }
            }
            "event_msg" => {
                let inner_type = parsed
                    .get("payload")
                    .and_then(|v| v.get("type"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                match inner_type {
                    "task_started" | "exec_command_begin" | "exec_command_start" => {
                        Some(AgentEvent::Generating)
                    }
                    "user_message" => Some(AgentEvent::UserQuery),
                    "agent_message" => Some(AgentEvent::Unknown),
                    "task_complete" => Some(AgentEvent::TurnCompleted),
                    "exec_approval_request" => {
                        let message = parsed
                            .get("payload")
                            .and_then(|v| v.get("command"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("command")
                            .to_string();
                        Some(AgentEvent::ActionRequired { message })
                    }
                    _ => Some(AgentEvent::Unknown),
                }
            }
            _ => Some(AgentEvent::Unknown),
        }
    }

    fn get_instruction_filename(&self) -> &str {
        "AGENTS.md"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wardian_core::models::ProviderConfig;

    fn make_provider() -> CodexProvider {
        CodexProvider::new()
    }

    #[test]
    fn name_returns_codex() {
        let p = make_provider();
        assert_eq!(p.name(), "Codex");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_codex_readiness_and_launch_use_extended_path_fallback() {
        let _lock = crate::utils::wardian_test_env_lock();
        let previous_home = std::env::var_os("HOME");
        let previous_path = std::env::var_os("PATH");
        let temp = tempfile::tempdir().expect("temp dir");
        let bin_dir = temp.path().join(".local").join("bin");
        std::fs::create_dir_all(&bin_dir).expect("create bin dir");
        let codex_path = bin_dir.join("codex");
        std::fs::write(&codex_path, "#!/bin/sh\n").expect("write codex shim");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = std::fs::metadata(&codex_path)
                .expect("codex metadata")
                .permissions();
            permissions.set_mode(0o755);
            std::fs::set_permissions(&codex_path, permissions).expect("set executable bit");
        }

        unsafe {
            std::env::set_var("HOME", temp.path());
            std::env::set_var("PATH", "/usr/bin:/bin:/usr/sbin:/sbin");
        }

        let expected = codex_path.to_string_lossy().to_string();
        let (executable, args) = make_provider().get_executable();
        let readiness = crate::providers::readiness::provider_readiness("codex");

        assert_eq!(executable, expected);
        assert!(args.is_empty());
        assert!(readiness.available);
        assert_eq!(readiness.executable.as_deref(), Some(expected.as_str()));

        match previous_home {
            Some(value) => unsafe { std::env::set_var("HOME", value) },
            None => unsafe { std::env::remove_var("HOME") },
        }
        match previous_path {
            Some(value) => unsafe { std::env::set_var("PATH", value) },
            None => unsafe { std::env::remove_var("PATH") },
        }
    }

    #[test]
    fn instruction_filename_is_agents_md() {
        let p = make_provider();
        assert_eq!(p.get_instruction_filename(), "AGENTS.md");
    }

    #[test]
    fn spawn_args_resume_and_model() {
        let p = make_provider();
        let config = AgentConfig {
            provider: "codex".into(),
            resume_session: Some("session-abc".into()),
            model: Some("gpt-5.4".into()),
            provider_config: ProviderConfig::Codex(CodexProviderConfig {
                profile: Some("wardian".into()),
                sandbox_mode: Some("workspace-write".into()),
                approval_policy: Some("on-request".into()),
                search: Some(true),
                ..Default::default()
            }),
            ..Default::default()
        };
        let args = p.get_spawn_args(&config, true);
        assert_eq!(args[0], "resume");
        assert!(args.contains(&"session-abc".to_string()));
        assert!(args.contains(&"--model".to_string()));
        assert!(args.contains(&"gpt-5.4".to_string()));
        assert!(args.contains(&"--profile".to_string()));
        assert!(args.contains(&"wardian".to_string()));
        assert!(args.contains(&"--sandbox".to_string()));
        assert!(args.contains(&"workspace-write".to_string()));
        assert!(args.contains(&"--ask-for-approval".to_string()));
        assert!(args.contains(&"on-request".to_string()));
        assert!(args.contains(&"--search".to_string()));
        assert!(args.contains(&"--no-alt-screen".to_string()));
        assert!(args
            .windows(2)
            .any(|pair| pair[0] == "--disable" && pair[1] == "plugins"));
        assert!(args
            .windows(2)
            .any(|pair| pair[0] == "--disable" && pair[1] == "apps"));
    }

    #[test]
    fn spawn_args_enable_no_alt_screen_by_default() {
        let p = make_provider();
        let config = AgentConfig::default();

        let args = p.get_spawn_args(&config, false);

        assert!(args.contains(&"--no-alt-screen".to_string()));
        assert!(args
            .windows(2)
            .any(|pair| pair[0] == "--disable" && pair[1] == "plugins"));
        assert!(args
            .windows(2)
            .any(|pair| pair[0] == "--disable" && pair[1] == "apps"));
    }

    #[test]
    fn spawn_args_inherit_sandbox_when_not_overridden() {
        let p = make_provider();
        let config = AgentConfig::default();

        let args = p.get_spawn_args(&config, false);

        assert!(args.contains(&"--dangerously-bypass-approvals-and-sandbox".to_string()));
        assert!(!args.contains(&"--sandbox".to_string()));
        assert!(!args.contains(&"--ask-for-approval".to_string()));
        assert!(!args.contains(&"--full-auto".to_string()));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn full_auto_disables_windows_elevated_sandbox_backend() {
        let p = make_provider();
        let config = AgentConfig::default();

        let args = p.get_spawn_args(&config, false);

        assert!(args
            .windows(2)
            .any(|pair| pair[0] == "-c" && pair[1] == r#"windows.sandbox="unelevated""#));
    }

    #[test]
    fn explicit_codex_sandbox_policy_disables_global_full_auto_default() {
        let policy = CodexRuntimePolicy::default();
        let config = CodexProviderConfig {
            sandbox_mode: Some("workspace-write".into()),
            approval_policy: Some("on-request".into()),
            full_auto: Some(false),
            ..Default::default()
        };

        let effective = effective_codex_runtime_policy(&config, &policy);

        assert!(!effective.full_auto);
        assert_eq!(effective.sandbox_mode, "workspace-write");
        assert_eq!(effective.approval_policy, "on-request");
    }

    #[test]
    fn explicit_codex_full_auto_uses_bypass_even_with_policy_values() {
        let p = make_provider();
        let config = AgentConfig {
            provider: "codex".into(),
            provider_config: ProviderConfig::Codex(CodexProviderConfig {
                full_auto: Some(true),
                sandbox_mode: Some("workspace-write".into()),
                approval_policy: Some("on-request".into()),
                ..Default::default()
            }),
            ..Default::default()
        };

        let args = p.get_spawn_args(&config, false);

        assert!(args.contains(&"--dangerously-bypass-approvals-and-sandbox".to_string()));
        assert!(!args.contains(&"--sandbox".to_string()));
        assert!(!args.contains(&"--ask-for-approval".to_string()));
    }

    #[test]
    fn explicit_codex_full_auto_false_disables_global_full_auto_default() {
        let policy = CodexRuntimePolicy::default();
        let config = CodexProviderConfig {
            full_auto: Some(false),
            ..Default::default()
        };

        let effective = effective_codex_runtime_policy(&config, &policy);

        assert!(!effective.full_auto);
        assert_eq!(effective.sandbox_mode, "danger-full-access");
        assert_eq!(effective.approval_policy, "never");
    }

    #[test]
    fn spawn_args_include_only_user_directories() {
        let p = make_provider();
        let config = AgentConfig {
            system_include_directories: Some(vec!["/sys/dir".into()]),
            include_directories: Some(vec!["/user/dir".into()]),
            ..Default::default()
        };
        let args = p.get_spawn_args(&config, false);
        let count = args.iter().filter(|a| *a == "--add-dir").count();
        assert_eq!(count, 1);
        assert!(args.contains(&"/user/dir".to_string()));
        assert!(!args.contains(&"/sys/dir".to_string()));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_path_resolution_prefers_direct_codex_shim_paths_for_interactive_launch() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("codex.ps1"), "echo test").unwrap();

        let path_exts = vec![".exe".to_string(), ".cmd".to_string(), ".bat".to_string()];
        let executable =
            CodexProvider::find_windows_codex_in_paths([temp.path().to_path_buf()], &path_exts)
                .unwrap();

        assert_eq!(
            executable,
            temp.path().join("codex.ps1").to_string_lossy().to_string()
        );
    }

    #[test]
    fn parse_output_thread_started_event() {
        let p = make_provider();
        let line = r#"{"type":"thread.started","thread_id":"abc-123"}"#;
        let event = p.parse_output(line).unwrap();
        assert_eq!(
            event,
            AgentEvent::Init {
                session_id: "abc-123".into(),
                timestamp: None,
            }
        );
    }

    #[test]
    fn parse_output_turn_started_event() {
        let p = make_provider();
        let line = r#"{"type":"turn.started"}"#;
        assert_eq!(p.parse_output(line).unwrap(), AgentEvent::UserQuery);
    }

    #[test]
    fn parse_output_turn_completed_event() {
        let p = make_provider();
        let line = r#"{"type":"turn.completed","usage":{"input_tokens":1}}"#;
        assert_eq!(p.parse_output(line).unwrap(), AgentEvent::TurnCompleted);
    }

    #[test]
    fn parse_output_agent_message_event() {
        let p = make_provider();
        let line = r#"{"type":"item.completed","item":{"type":"agent_message","text":"hello"}}"#;
        assert_eq!(p.parse_output(line).unwrap(), AgentEvent::Unknown);
    }

    #[test]
    fn parse_output_task_started_event() {
        let p = make_provider();
        let line = r#"{"type":"event_msg","payload":{"type":"task_started","turn_id":"abc"}}"#;
        assert_eq!(p.parse_output(line).unwrap(), AgentEvent::Generating);
    }

    #[test]
    fn parse_output_task_complete_event() {
        let p = make_provider();
        let line = r#"{"type":"event_msg","payload":{"type":"task_complete","turn_id":"abc"}}"#;
        assert_eq!(p.parse_output(line).unwrap(), AgentEvent::TurnCompleted);
    }

    #[test]
    fn parse_output_agent_message_does_not_change_status() {
        let p = make_provider();
        let line = r#"{"type":"event_msg","payload":{"type":"agent_message","message":"Waiting for approval"}}"#;
        assert_eq!(p.parse_output(line).unwrap(), AgentEvent::Unknown);
    }

    #[test]
    fn parse_output_exec_command_begin_sets_generating() {
        let p = make_provider();
        let line = r#"{"type":"event_msg","payload":{"type":"exec_command_begin","command":"git status"}}"#;
        assert_eq!(p.parse_output(line).unwrap(), AgentEvent::Generating);
    }

    #[test]
    fn parse_output_function_call_output_resumes_processing() {
        let p = make_provider();
        let line =
            r#"{"type":"response_item","payload":{"type":"function_call_output","call_id":"abc"}}"#;
        assert_eq!(p.parse_output(line).unwrap(), AgentEvent::Generating);
    }

    #[test]
    fn parse_output_live_activity_response_items_set_generating() {
        let p = make_provider();
        for payload_type in [
            "reasoning",
            "function_call",
            "custom_tool_call",
            "custom_tool_call_output",
        ] {
            let line = format!(
                r#"{{"type":"response_item","payload":{{"type":"{}","call_id":"abc"}}}}"#,
                payload_type
            );

            assert_eq!(p.parse_output(&line).unwrap(), AgentEvent::Generating);
        }
    }

    #[test]
    fn parse_output_response_item_function_call_requires_approval() {
        let p = make_provider();
        let line = r#"{"type":"response_item","payload":{"type":"function_call","arguments":"{\"command\":\"Get-Content foo\",\"sandbox_permissions\":\"require_escalated\",\"justification\":\"Allow reading foo?\"}"}}"#;
        assert_eq!(
            p.parse_output(line).unwrap(),
            AgentEvent::ActionRequired {
                message: "Allow reading foo?".into(),
            }
        );
    }

    #[test]
    fn parse_output_response_item_function_call_without_approval_sets_generating() {
        let p = make_provider();
        let line = r#"{"type":"response_item","payload":{"type":"function_call","arguments":"{\"command\":\"Get-Content foo\"}"}}"#;
        assert_eq!(p.parse_output(line).unwrap(), AgentEvent::Generating);
    }
}
