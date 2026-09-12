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
    /// A no-turn host inbox append has no originating model call. Its body is
    /// deliberately irrelevant: quoted packets or model tool results cannot
    /// opt out of activity tracking by mentioning this operation.
    pub(crate) fn is_nonwaking_inbox_output(item: &serde_json::Value) -> bool {
        item["type"] == "function_call_output"
            && item["name"] == "wardian_inbox_delivery"
            && item["namespace"] == "wardian"
            && item.get("call_id").is_none_or(serde_json::Value::is_null)
    }

    pub fn new() -> Self {
        CodexProvider
    }

    #[cfg(target_os = "windows")]
    fn windows_codex_node_launch(path: &std::path::Path) -> Option<(String, Vec<String>)> {
        crate::providers::npm::node_launch_from_npm_cmd_shim(path, "codex")
    }

    #[cfg(target_os = "windows")]
    fn find_windows_codex_in_paths<I>(
        paths: I,
        path_exts: &[String],
    ) -> Option<(String, Vec<String>)>
    where
        I: IntoIterator<Item = std::path::PathBuf>,
    {
        for path in paths {
            if let Some(launch) = Self::windows_codex_node_launch(&path) {
                return Some(launch);
            }

            for ext in path_exts {
                let candidate = path.join(format!("codex{ext}"));
                if candidate.exists() {
                    return Some((candidate.to_string_lossy().to_string(), vec![]));
                }
            }

            let powershell = path.join("codex.ps1");
            if powershell.exists() {
                return Some((powershell.to_string_lossy().to_string(), vec![]));
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

    pub(crate) fn append_common_args(
        &self,
        args: &mut Vec<String>,
        config: &AgentConfig,
        is_exec_mode: bool,
    ) {
        let runtime_policy = crate::utils::load_codex_runtime_policy().unwrap_or_default();
        self.append_common_args_with_runtime_policy(args, config, is_exec_mode, &runtime_policy);
    }

    /// App-server owns execution policy; the attached TUI must not replace it.
    /// Reject launch forms whose semantics have not been verified for remote mode.
    pub(crate) fn shared_server_args(&self, config: &AgentConfig) -> Result<Vec<String>, String> {
        let codex = config.codex_config();
        if codex
            .profile
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty())
            || config
                .custom_args
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty())
        {
            return Err("shared Codex does not yet support profile/custom launch arguments".into());
        }
        let global = crate::utils::load_codex_runtime_policy().unwrap_or_default();
        let policy = effective_codex_runtime_policy(&codex, &global);
        if policy.approval_policy == "untrusted" && !policy.full_auto {
            return Err(
                "installed Codex rejects untrusted; choose an explicit supported policy".into(),
            );
        }
        let mut args = vec!["app-server".to_owned()];
        let mut set = |key: &str, value: &str| {
            args.extend(["-c".into(), format!("{key}={}", toml_basic_string(value))]);
        };
        if let Some(model) = &config.model {
            set("model", model);
        }
        if let Some(effort) = &codex.reasoning_effort {
            set("model_reasoning_effort", effort);
        }
        if policy.full_auto {
            set("sandbox_mode", "danger-full-access");
            set("approval_policy", "never");
        } else if policy.approval_policy == "approve-for-me" {
            set("sandbox_mode", "workspace-write");
            set("approval_policy", "on-request");
            set("approvals_reviewer", "guardian_subagent");
        } else {
            set("sandbox_mode", &policy.sandbox_mode);
            set("approval_policy", &policy.approval_policy);
        }
        if codex.search.unwrap_or(false) {
            set("web_search", "live");
        }
        if policy.trust_workspaces {
            if let Some(project) = codex_trusted_project_override(&config.folder) {
                args.extend(["-c".into(), project]);
            }
        }
        Ok(args)
    }

    /// Append the flags that are valid before the `exec` subcommand.
    ///
    /// Interactive Codex sessions accept these at the top level. Headless
    /// launches reuse the same set before adding `exec`, while its exec-only
    /// flags must be appended after that subcommand.
    pub(crate) fn append_headless_global_args(&self, args: &mut Vec<String>, config: &AgentConfig) {
        let runtime_policy = crate::utils::load_codex_runtime_policy().unwrap_or_default();
        self.append_shared_args_with_runtime_policy(args, config, &runtime_policy, true);
    }

    /// Append flags owned by `codex exec` (or `codex exec resume`).
    ///
    /// Codex rejects these when they appear at the top command level. The
    /// repository bypass defaults on for headless execution so temporary
    /// automation providers can run in a workspace that is not a Git checkout.
    pub(crate) fn append_headless_exec_args(
        &self,
        args: &mut Vec<String>,
        config: Option<&AgentConfig>,
    ) {
        if config
            .and_then(|config| config.codex_config().skip_git_repo_check)
            .unwrap_or(true)
        {
            args.push("--skip-git-repo-check".into());
        }
        if config
            .and_then(|config| config.codex_config().ephemeral)
            .unwrap_or(false)
        {
            args.push("--ephemeral".into());
        }
    }

    /// Inject runtime-owned context at developer priority. Codex discovers
    /// workspace `AGENTS.md` files from the real working directory, while
    /// Wardian keeps generated agent instructions in the isolated habitat.
    /// The config override bridges that boundary without modifying user files.
    pub(crate) fn insert_developer_instructions_arg(
        &self,
        args: &mut Vec<String>,
        instructions: &str,
    ) {
        if instructions.trim().is_empty() {
            return;
        }
        let insert_at = args
            .iter()
            .position(|arg| arg == "exec")
            .unwrap_or(args.len());
        args.splice(
            insert_at..insert_at,
            [
                "-c".to_string(),
                format!("developer_instructions={}", toml_basic_string(instructions)),
            ],
        );
    }

    fn append_common_args_with_runtime_policy(
        &self,
        args: &mut Vec<String>,
        config: &AgentConfig,
        is_exec_mode: bool,
        runtime_policy: &CodexRuntimePolicy,
    ) {
        self.append_shared_args_with_runtime_policy(args, config, runtime_policy, false);

        if is_exec_mode {
            self.append_headless_exec_args(args, Some(config));
        } else {
            // Codex documents this as inline TUI mode that preserves terminal
            // scrollback. Wardian embeds the TUI inside xterm, so interactive
            // sessions should prefer scrollback-friendly output.
            args.push("--no-alt-screen".into());
        }
    }

    fn append_shared_args_with_runtime_policy(
        &self,
        args: &mut Vec<String>,
        config: &AgentConfig,
        runtime_policy: &CodexRuntimePolicy,
        is_headless: bool,
    ) {
        let codex = config.codex_config();
        if let Some(ref model) = config.model {
            args.push("--model".into());
            args.push(model.clone());
        }
        if let Some(effort) = codex
            .reasoning_effort
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            args.push("-c".into());
            args.push(format!(
                "model_reasoning_effort={}",
                toml_basic_string(effort)
            ));
        }

        if let Some(ref profile) = codex.profile {
            if !profile.trim().is_empty() {
                args.push("--profile".into());
                args.push(profile.clone());
            }
        }

        let effective_policy = effective_codex_runtime_policy(&codex, runtime_policy);
        if effective_policy.trust_workspaces {
            if let Some(project_override) = codex_trusted_project_override(&config.folder) {
                args.push("-c".into());
                args.push(project_override);
            }
        }

        // `danger-full-access` plus `never` already grants a headless worker
        // unrestricted tool access. On Windows, passing those two flags alone
        // still lets Codex initialize its configured `unelevated` sandbox,
        // which cannot create the Microsoft Store PowerShell process. Use the
        // explicit bypass form for that equivalent headless policy instead.
        let bypass_sandbox = effective_policy.full_auto
            || (cfg!(target_os = "windows")
                && is_headless
                && effective_policy.sandbox_mode == "danger-full-access"
                && effective_policy.approval_policy == "never");

        if bypass_sandbox {
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
        } else if effective_policy.approval_policy == "approve-for-me" {
            // Codex's automatic-review preset owns both the approval reviewer
            // and its workspace-write sandbox. Do not pass it through the
            // value-taking approval flag; that would be rejected by Codex.
            args.push("--approve-for-me".into());
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

    #[cfg(test)]
    fn spawn_args_with_runtime_policy(
        &self,
        config: &AgentConfig,
        is_resume: bool,
        runtime_policy: &CodexRuntimePolicy,
    ) -> Vec<String> {
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

        self.append_common_args_with_runtime_policy(&mut args, config, false, runtime_policy);

        if let Some(ref custom) = config.custom_args {
            if let Some(parsed) = shlex::split(custom) {
                args.extend(parsed);
            }
        }

        args
    }
}

fn toml_basic_string(value: &str) -> String {
    let mut escaped = String::new();
    for ch in value.chars() {
        match ch {
            '\\' => escaped.push_str("\\\\"),
            '"' => escaped.push_str("\\\""),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            other => escaped.push(other),
        }
    }
    format!("\"{escaped}\"")
}

fn toml_basic_string_key(value: &str) -> String {
    let mut escaped = String::new();
    for ch in value.chars() {
        match ch {
            '\\' => escaped.push_str("\\\\"),
            '"' => escaped.push_str("\\\""),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            ch if ch.is_control() => escaped.push_str(&format!("\\u{:04x}", ch as u32)),
            ch => escaped.push(ch),
        }
    }
    escaped
}

fn codex_trusted_project_override(folder: &str) -> Option<String> {
    let trimmed = folder.trim();
    if trimmed.is_empty() {
        return None;
    }

    let project_key = crate::utils::fs::codex_trusted_project_key(std::path::Path::new(trimmed));
    Some(format!(
        r#"projects."{}".trust_level="trusted""#,
        toml_basic_string_key(&project_key)
    ))
}

fn effective_codex_runtime_policy(
    config: &CodexProviderConfig,
    global_policy: &CodexRuntimePolicy,
) -> CodexRuntimePolicy {
    let explicit_sandbox = config
        .sandbox_mode
        .as_deref()
        .map(str::trim)
        .filter(|value| {
            matches!(
                *value,
                "read-only" | "workspace-write" | "danger-full-access"
            )
        });
    let explicit_approval = config
        .approval_policy
        .as_deref()
        .map(str::trim)
        .filter(|value| {
            matches!(
                *value,
                "untrusted" | "on-request" | "approve-for-me" | "never"
            )
        });
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
        trust_workspaces: global_policy.trust_workspaces,
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

                if let Some((executable, args)) =
                    Self::find_windows_codex_in_paths(std::env::split_paths(&paths), &path_exts)
                {
                    return (executable, args);
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
            "turn.failed" | "turn.aborted" | "turn.cancelled" | "turn.canceled"
            | "turn.interrupted" => Some(AgentEvent::TurnInterrupted),
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
                if Self::is_nonwaking_inbox_output(payload) {
                    return Some(AgentEvent::Unknown);
                }
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
                            "user"
                                if super::chat_transcript::codex_response_item_user_context(
                                    payload,
                                    "response_item",
                                    &wardian_core::models::chat::AgentChatRole::User,
                                ) =>
                            {
                                Some(AgentEvent::Unknown)
                            }
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
                    "task_complete" | "turn_complete" | "turn_completed" => {
                        Some(AgentEvent::TurnCompleted)
                    }
                    "turn_failed" | "turn_aborted" | "turn_cancelled" | "turn_canceled"
                    | "turn_interrupted" => Some(AgentEvent::TurnInterrupted),
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

    mod status;

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
    fn developer_instructions_are_inserted_before_exec_and_toml_escaped() {
        let mut args = vec!["--model".into(), "gpt-5.6-luna".into(), "exec".into()];
        make_provider()
            .insert_developer_instructions_arg(&mut args, "Memory says \"compact\".\nKeep it.");
        assert_eq!(args[2], "-c");
        assert_eq!(
            args[3],
            "developer_instructions=\"Memory says \\\"compact\\\".\\nKeep it.\""
        );
        assert_eq!(args[4], "exec");
    }

    #[test]
    fn spawn_args_resume_and_model() {
        let p = make_provider();
        let config = AgentConfig {
            provider: "codex".into(),
            resume_session: Some("session-abc".into()),
            model: Some("gpt-5.4".into()),
            provider_config: ProviderConfig::Codex(CodexProviderConfig {
                reasoning_effort: Some("high".into()),
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
        assert!(args.contains(&"model_reasoning_effort=\"high\"".to_string()));
        assert!(args.contains(&"--profile".to_string()));
        assert!(args.contains(&"wardian".to_string()));
        assert!(args.contains(&"--sandbox".to_string()));
        assert!(args.contains(&"workspace-write".to_string()));
        assert!(args.contains(&"--ask-for-approval".to_string()));
        assert!(args.contains(&"on-request".to_string()));
        assert!(args.contains(&"--search".to_string()));
        assert!(args.contains(&"--no-alt-screen".to_string()));
        assert!(!args.windows(2).any(|pair| pair[0] == "--disable"));
    }

    #[test]
    fn spawn_args_enable_no_alt_screen_by_default() {
        let p = make_provider();
        let config = AgentConfig::default();

        let args = p.get_spawn_args(&config, false);

        assert!(args.contains(&"--no-alt-screen".to_string()));
        assert!(!args.windows(2).any(|pair| pair[0] == "--disable"));
    }

    #[test]
    fn electrical_engineer_does_not_receive_plugin_specific_launch_flags() {
        let p = make_provider();
        let config = AgentConfig {
            provider: "codex".into(),
            agent_class: "Electrical Engineer".into(),
            ..Default::default()
        };

        let args = p.get_spawn_args(&config, false);

        assert!(args.contains(&"--ask-for-approval".to_string()));
        assert!(!args
            .windows(2)
            .any(|pair| pair[0] == "--disable" && pair[1] == "plugins"));
        assert!(!args
            .windows(2)
            .any(|pair| pair[0] == "--disable" && pair[1] == "apps"));
    }

    #[test]
    fn mechanical_engineer_does_not_receive_plugin_specific_launch_flags() {
        let p = make_provider();
        let config = AgentConfig {
            provider: "codex".into(),
            agent_class: "Mechanical Engineer".into(),
            ..Default::default()
        };

        let args = p.get_spawn_args(&config, false);

        assert!(!args
            .windows(2)
            .any(|pair| pair[0] == "--disable" && pair[1] == "plugins"));
        assert!(!args
            .windows(2)
            .any(|pair| pair[0] == "--disable" && pair[1] == "apps"));
    }

    #[test]
    fn spawn_args_inherit_sandbox_when_not_overridden() {
        let p = make_provider();
        let config = AgentConfig::default();

        let args = p.get_spawn_args(&config, false);

        assert!(!args.contains(&"--dangerously-bypass-approvals-and-sandbox".to_string()));
        assert!(args.contains(&"--sandbox".to_string()));
        assert!(args.contains(&"workspace-write".to_string()));
        assert!(args.contains(&"--ask-for-approval".to_string()));
        assert!(args.contains(&"on-request".to_string()));
        assert!(!args.contains(&"--full-auto".to_string()));
    }

    #[test]
    fn spawn_args_use_codex_automatic_review_policy() {
        let p = make_provider();
        let config = AgentConfig::default();
        let policy = CodexRuntimePolicy {
            approval_policy: "approve-for-me".into(),
            ..Default::default()
        };

        let args = p.spawn_args_with_runtime_policy(&config, false, &policy);

        assert!(args.contains(&"--approve-for-me".to_string()));
        assert!(!args.contains(&"--sandbox".to_string()));
        assert!(!args.contains(&"--ask-for-approval".to_string()));
    }

    #[test]
    fn explicit_codex_automatic_review_override_uses_the_same_launch_flag() {
        let p = make_provider();
        let config = AgentConfig {
            provider: "codex".into(),
            provider_config: ProviderConfig::Codex(CodexProviderConfig {
                approval_policy: Some("approve-for-me".into()),
                ..Default::default()
            }),
            ..Default::default()
        };

        let args = p.get_spawn_args(&config, false);

        assert!(args.contains(&"--approve-for-me".to_string()));
        assert!(!args.contains(&"--sandbox".to_string()));
        assert!(!args.contains(&"--ask-for-approval".to_string()));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn headless_unrestricted_policy_bypasses_windows_sandbox() {
        let p = make_provider();
        let config = AgentConfig::default();
        let policy = CodexRuntimePolicy {
            sandbox_mode: "danger-full-access".into(),
            approval_policy: "never".into(),
            full_auto: false,
            trust_workspaces: false,
        };
        let mut headless_args = Vec::new();
        let mut interactive_args = Vec::new();

        p.append_shared_args_with_runtime_policy(&mut headless_args, &config, &policy, true);
        p.append_shared_args_with_runtime_policy(&mut interactive_args, &config, &policy, false);

        assert!(headless_args.contains(&"--dangerously-bypass-approvals-and-sandbox".to_string()));
        assert!(!headless_args.contains(&"--sandbox".to_string()));
        assert!(!headless_args.contains(&"--ask-for-approval".to_string()));
        assert!(
            !interactive_args.contains(&"--dangerously-bypass-approvals-and-sandbox".to_string())
        );
        assert!(interactive_args.contains(&"--sandbox".to_string()));
        assert!(interactive_args.contains(&"danger-full-access".to_string()));
        assert!(interactive_args.contains(&"--ask-for-approval".to_string()));
        assert!(interactive_args.contains(&"never".to_string()));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn headless_restricted_policy_keeps_windows_sandbox() {
        let p = make_provider();
        let config = AgentConfig::default();
        let policy = CodexRuntimePolicy {
            sandbox_mode: "workspace-write".into(),
            approval_policy: "never".into(),
            full_auto: false,
            trust_workspaces: false,
        };
        let mut args = Vec::new();

        p.append_shared_args_with_runtime_policy(&mut args, &config, &policy, true);

        assert!(!args.contains(&"--dangerously-bypass-approvals-and-sandbox".to_string()));
        assert!(args.contains(&"--sandbox".to_string()));
        assert!(args.contains(&"workspace-write".to_string()));
        assert!(args.contains(&"--ask-for-approval".to_string()));
        assert!(args.contains(&"never".to_string()));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn explicit_full_auto_disables_windows_elevated_sandbox_backend() {
        let p = make_provider();
        let config = AgentConfig {
            provider: "codex".into(),
            provider_config: ProviderConfig::Codex(CodexProviderConfig {
                full_auto: Some(true),
                ..Default::default()
            }),
            ..Default::default()
        };

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
    fn removed_on_failure_policy_falls_back_to_global_default() {
        let policy = CodexRuntimePolicy::default();
        let config = CodexProviderConfig {
            approval_policy: Some("on-failure".into()),
            ..Default::default()
        };

        let effective = effective_codex_runtime_policy(&config, &policy);

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
        assert_eq!(effective.sandbox_mode, "workspace-write");
        assert_eq!(effective.approval_policy, "on-request");
    }

    #[test]
    fn spawn_args_can_mark_launch_workspace_trusted_for_codex() {
        let p = make_provider();
        let config = AgentConfig {
            provider: "codex".into(),
            folder: r#"D:\Development\Wardian.wt\wardian-3"#.into(),
            ..Default::default()
        };
        let policy = CodexRuntimePolicy {
            trust_workspaces: true,
            ..Default::default()
        };

        let args = p.spawn_args_with_runtime_policy(&config, false, &policy);

        assert!(args.windows(2).any(|pair| {
            pair[0] == "-c"
                && pair[1]
                    == r#"projects."D:\\Development\\Wardian.wt\\wardian-3".trust_level="trusted""#
        }));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn spawn_args_trust_workspace_uses_windows_native_path_key() {
        let p = make_provider();
        let config = AgentConfig {
            provider: "codex".into(),
            folder: "C:/workspace/resttrace".into(),
            ..Default::default()
        };
        let policy = CodexRuntimePolicy {
            trust_workspaces: true,
            ..Default::default()
        };

        let args = p.spawn_args_with_runtime_policy(&config, true, &policy);

        assert!(args.windows(2).any(|pair| {
            pair[0] == "-c"
                && pair[1] == r#"projects."C:\\workspace\\resttrace".trust_level="trusted""#
        }));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn trusted_project_key_canonicalizes_existing_windows_paths() {
        let temp = tempfile::tempdir().expect("temp dir");
        let slash_path = temp.path().to_string_lossy().replace('\\', "/");
        let canonical = temp.path().canonicalize().expect("canonical path");
        let expected =
            crate::utils::fs::strip_windows_verbatim_prefix(&canonical.to_string_lossy())
                .replace('/', "\\");

        assert_eq!(
            crate::utils::fs::codex_trusted_project_key(std::path::Path::new(&slash_path)),
            expected
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn trusted_project_key_strips_windows_verbatim_drive_prefix() {
        assert_eq!(
            crate::utils::fs::codex_trusted_project_key(std::path::Path::new(
                r"\\?\C:\workspace\resttrace"
            )),
            r"C:\workspace\resttrace"
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn trusted_project_key_strips_windows_verbatim_unc_prefix() {
        assert_eq!(
            crate::utils::fs::codex_trusted_project_key(std::path::Path::new(
                r"\\?\UNC\server\share\resttrace"
            )),
            r"\\server\share\resttrace"
        );
    }

    #[test]
    fn spawn_args_do_not_trust_workspace_by_default() {
        let p = make_provider();
        let config = AgentConfig {
            provider: "codex".into(),
            folder: r#"D:\Development\Wardian.wt\wardian-3"#.into(),
            ..Default::default()
        };

        let args = p.get_spawn_args(&config, false);

        assert!(!args.iter().any(|arg| arg.contains("trust_level")));
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
        let (executable, args) =
            CodexProvider::find_windows_codex_in_paths([temp.path().to_path_buf()], &path_exts)
                .unwrap();

        assert_eq!(
            executable,
            temp.path().join("codex.ps1").to_string_lossy().to_string()
        );
        assert!(args.is_empty());
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_path_resolution_prefers_node_codex_entrypoint_over_cmd_shim() {
        let temp = tempfile::tempdir().unwrap();
        let codex_js = temp
            .path()
            .join("node_modules")
            .join("@openai")
            .join("codex")
            .join("bin")
            .join("codex.js");
        std::fs::create_dir_all(codex_js.parent().unwrap()).unwrap();
        std::fs::write(
            temp.path().join("codex.cmd"),
            r#"@ECHO off
SET dp0=%~dp0
"%dp0%\node.exe" "%dp0%\node_modules\@openai\codex\bin\codex.js" %*
"#,
        )
        .unwrap();
        std::fs::write(&codex_js, "console.log('codex')").unwrap();

        let path_exts = vec![".cmd".to_string()];
        let (executable, args) =
            CodexProvider::find_windows_codex_in_paths([temp.path().to_path_buf()], &path_exts)
                .unwrap();

        assert_eq!(executable, "node");
        assert_eq!(args, vec![codex_js.to_string_lossy().to_string()]);
    }
}
