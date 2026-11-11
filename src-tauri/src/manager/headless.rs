use crate::providers::antigravity::{changed_workspace_conversation, AntigravityProvider};
use crate::providers::codex::CodexProvider;
use crate::providers::opencode::OpenCodeProvider;
use crate::providers::ProviderFactory;
use crate::utils::fs::*;
use crate::utils::process::new_headless_command;
use crate::utils::shell::build_program_launch;
use std::time::Duration;
use wardian_core::conversation_lease::ConversationLeaseOwner;
use wardian_core::models::{AgentConfig, AgentProvider};

use super::codex::{
    codex_bootstrap_launch_context, codex_session_file_path_in, migrate_codex_bootstrap_home,
};
use super::opencode::opencode_env;
use super::{
    interactive_provider_cwd, persisted_agent_config, session_bootstrap_prompt,
    strip_flag_value_pairs,
};
use crate::utils::logging::log_debug;

mod opencode_stdin;

#[cfg(target_os = "macos")]
use super::macos_extended_path;
pub(crate) fn headless_provider_launch(
    provider_name: &str,
    bin: &str,
    provider_args: &[String],
) -> Result<crate::utils::shell::ShellLaunchSpec, String> {
    #[cfg(windows)]
    {
        let lower_bin = bin.to_ascii_lowercase();
        if provider_name == "opencode" && !lower_bin.ends_with(".exe") {
            return build_program_launch(bin, provider_args);
        }
        if !lower_bin.ends_with(".cmd")
            && !lower_bin.ends_with(".bat")
            && !lower_bin.ends_with(".ps1")
        {
            return Ok(crate::utils::shell::ShellLaunchSpec {
                executable: bin.to_string(),
                args: provider_args.to_vec(),
            });
        }
    }

    #[cfg(not(windows))]
    let _ = provider_name;

    build_program_launch(bin, provider_args)
}

pub const DEFAULT_HEADLESS_RUN_TIMEOUT: Duration = Duration::from_secs(15 * 60);

const HEADLESS_PROCESS_POLL_INTERVAL: Duration = Duration::from_millis(200);
const HEADLESS_LEASE_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(60);
const HEADLESS_LEASE_DURATION: chrono::Duration = chrono::Duration::minutes(20);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeadlessRunErrorKind {
    DefiniteFailure,
    Uncertain,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeadlessRunError {
    kind: HeadlessRunErrorKind,
    message: String,
}

impl HeadlessRunError {
    pub fn definite(message: impl Into<String>) -> Self {
        Self {
            kind: HeadlessRunErrorKind::DefiniteFailure,
            message: message.into(),
        }
    }

    pub fn uncertain(message: impl Into<String>) -> Self {
        Self {
            kind: HeadlessRunErrorKind::Uncertain,
            message: message.into(),
        }
    }

    pub fn cancelled(message: impl Into<String>) -> Self {
        Self {
            kind: HeadlessRunErrorKind::Cancelled,
            message: message.into(),
        }
    }

    pub fn kind(&self) -> HeadlessRunErrorKind {
        self.kind
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn contains(&self, pattern: &str) -> bool {
        self.message.contains(pattern)
    }

    pub fn with_context(&self, context: impl std::fmt::Display) -> Self {
        Self {
            kind: self.kind,
            message: format!("{}; {context}", self.message),
        }
    }
}

impl std::fmt::Display for HeadlessRunError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for HeadlessRunError {}

impl From<String> for HeadlessRunError {
    fn from(message: String) -> Self {
        Self::definite(message)
    }
}

impl From<&str> for HeadlessRunError {
    fn from(message: &str) -> Self {
        Self::definite(message)
    }
}

pub struct HeadlessRunOptions<'a> {
    pub cwd: &'a std::path::Path,
    pub prompt: &'a str,
    pub wardian_session_id: &'a str,
    /// Registered agent that owns recall and in-process `wardian memory`
    /// commands. Fresh automation workers may keep a synthetic provider session
    /// while using this durable memory identity. `None` disables durable
    /// memory for ephemeral automation providers.
    pub memory_agent_id: Option<&'a str>,
    pub resume_session: Option<&'a str>,
    pub output_format: &'a str,
    pub provider_name: &'a str,
    pub config_override: Option<&'a AgentConfig>,
    /// A hard ceiling ensures a stuck provider cannot retain a conversation
    /// lease indefinitely.
    pub timeout: Duration,
    /// Present only when this run owns a persisted provider-conversation lease.
    /// The manager renews it while the provider process is still alive.
    pub lease_owner: Option<ConversationLeaseOwner>,
    /// Run-owned marker checked while the provider process is active. This is
    /// never shared with permanent-agent or unrelated provider lifecycles.
    pub cancellation_marker: Option<&'a std::path::Path>,
}

#[derive(Debug)]
struct HeadlessProviderContext {
    class_name: String,
    command_cwd: std::path::PathBuf,
    args_cwd: std::path::PathBuf,
    habitat_root: Option<std::path::PathBuf>,
}

/// Owns the provider's full process tree while an async headless run is in
/// flight. `kill_on_drop` only reaches Tokio's direct child; this guard closes
/// the shell-wrapper/descendant gap when the enclosing future is cancelled.
struct HeadlessProcessTreeGuard {
    pid: Option<u32>,
}

impl HeadlessProcessTreeGuard {
    fn new(pid: Option<u32>) -> Self {
        Self { pid }
    }

    fn disarm(&mut self) {
        self.pid = None;
    }
}

impl Drop for HeadlessProcessTreeGuard {
    fn drop(&mut self) {
        if let Some(pid) = self.pid.take() {
            terminate_headless_process_tree(pid);
        }
    }
}

fn terminate_headless_process_tree(pid: u32) {
    #[cfg(windows)]
    {
        if let Err(error) = crate::utils::process::force_kill_process_tree(pid) {
            log_debug(&format!(
                "[Wardian] Failed to terminate headless process tree rooted at PID {pid}: {error}"
            ));
        }
    }

    #[cfg(unix)]
    {
        // `run_headless_with_options` starts the provider as a process-group
        // leader. Signalling the negative PID reaches shell wrappers and every
        // descendant before the direct child can be reaped.
        let result = unsafe { libc::kill(-(pid as libc::pid_t), libc::SIGKILL) };
        if result != 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() != std::io::ErrorKind::NotFound {
                log_debug(&format!(
                    "[Wardian] Failed to terminate headless process group rooted at PID {pid}: {error}"
                ));
            }
        }
    }

    #[cfg(not(any(windows, unix)))]
    {
        let _ = pid;
    }
}

fn headless_provider_context(
    provider_name: &str,
    cwd: &std::path::Path,
    wardian_session_id: &str,
    config_override: Option<&AgentConfig>,
    persisted_config: Option<&AgentConfig>,
) -> Result<HeadlessProviderContext, String> {
    let class_name = config_override
        .or(persisted_config)
        .map(|config| config.agent_class.trim().to_string())
        .filter(|class_name| !class_name.is_empty())
        .unwrap_or_default();
    let habitat_root =
        prepare_provider_habitat(provider_name, cwd, &class_name, Some(wardian_session_id))?;
    let command_cwd =
        super::interactive_provider_cwd(provider_name, cwd, habitat_root.as_deref(), None);
    let args_cwd = if provider_name == "opencode" {
        cwd.to_path_buf()
    } else {
        command_cwd.clone()
    };

    Ok(HeadlessProviderContext {
        class_name,
        command_cwd,
        args_cwd,
        habitat_root,
    })
}

fn effective_headless_provider_config(
    provider_name: &str,
    cwd: &std::path::Path,
    wardian_session_id: &str,
    resume_session: Option<&str>,
    config_override: Option<&AgentConfig>,
    persisted_config: Option<&AgentConfig>,
) -> Option<AgentConfig> {
    let mut config = config_override
        .cloned()
        .or_else(|| persisted_config.cloned())
        .or_else(|| {
            // Provider-bound automation workers do not have an agent profile,
            // but Codex still needs the persisted runtime policy flags.
            matches!(provider_name, "codex" | "pi").then(|| AgentConfig {
                provider: provider_name.to_string(),
                session_id: wardian_session_id.to_string(),
                folder: cwd.to_string_lossy().to_string(),
                fresh_provider_session_id: (provider_name == "pi")
                    .then(|| uuid::Uuid::new_v4().to_string()),
                provider_config: wardian_core::models::ProviderConfig::default_for_provider_name(
                    provider_name,
                ),
                ..Default::default()
            })
        })?;

    if provider_name == "pi" && resume_session.is_none_or(|session_id| session_id.trim().is_empty())
    {
        // A registered background-fresh automation carries the agent's profile
        // as an override, but its synthetic Wardian session must own a distinct
        // Pi transcript and provider identity.
        let owns_different_session = config.session_id != wardian_session_id;
        config.session_id = wardian_session_id.to_string();
        if owns_different_session
            || config
                .fresh_provider_session_id
                .as_deref()
                .is_none_or(|session_id| session_id.trim().is_empty())
        {
            config.fresh_provider_session_id = Some(uuid::Uuid::new_v4().to_string());
        }
    }

    Some(config)
}

pub(crate) fn headless_provider_args(
    provider_name: &str,
    provider: &dyn AgentProvider,
    provider_cwd: &std::path::Path,
    prompt: &str,
    output_format: &str,
    resume_session: Option<&str>,
    config_override: Option<&AgentConfig>,
) -> Vec<String> {
    let (_bin, mut provider_args) = provider.get_executable();
    match provider_name {
        "codex" => {
            provider_args.push("--cd".to_string());
            provider_args.push(provider_cwd.to_string_lossy().to_string());
            if let Some(config) = config_override {
                CodexProvider::new().append_headless_global_args(&mut provider_args, config);
                if let Some(custom) = config.custom_args.as_ref() {
                    if let Some(parsed) = shlex::split(custom) {
                        provider_args.extend(parsed);
                    }
                }
            }
            provider_args.push("exec".to_string());
            if let Some(resume_id) = resume_session.filter(|s| !s.trim().is_empty()) {
                provider_args.push("resume".to_string());
                provider_args.push(resume_id.to_string());
            }
            CodexProvider::new().append_headless_exec_args(&mut provider_args, config_override);
            provider_args.push("--json".to_string());
            provider_args.push(prompt.to_string());
        }
        "claude" => {
            if let Some(config) = config_override {
                let mut config = config.clone();
                config.resume_session = resume_session.map(str::to_string);
                let spawn_args = strip_flag_value_pairs(
                    strip_flag_value_pairs(
                        strip_flag_value_pairs(
                            provider.get_spawn_args(&config, resume_session.is_some()),
                            "--session-id",
                        ),
                        "--resume",
                    ),
                    "--input-format",
                );
                let spawn_args = strip_flag_value_pairs(spawn_args, "--output-format");
                provider_args.extend(spawn_args);
                if let Some(max_turns) = config.claude_config().max_turns.filter(|turns| *turns > 0)
                {
                    provider_args.push("--max-turns".to_string());
                    provider_args.push(max_turns.to_string());
                }
            }
            provider_args.push("--print".to_string());
            provider_args.push("--output-format".to_string());
            provider_args.push(output_format.to_string());
            if let Some(resume_id) = resume_session.filter(|s| !s.trim().is_empty()) {
                provider_args.push("--resume".to_string());
                provider_args.push(resume_id.to_string());
            }
            provider_args.push(prompt.to_string());
        }
        "mock" => {
            provider_args.push("--print".to_string());
            provider_args.push(prompt.to_string());
        }
        "gemini" => {
            if let Some(config) = config_override {
                let mut config = config.clone();
                config.resume_session = resume_session.map(str::to_string);
                let spawn_args = provider.get_spawn_args(&config, resume_session.is_some());
                let spawn_args = strip_flag_value_pairs(spawn_args, "--session-id");
                let spawn_args = strip_flag_value_pairs(spawn_args, "--resume");
                let spawn_args = strip_flag_value_pairs(spawn_args, "--output-format");
                provider_args.extend(spawn_args);
            }
            provider_args.push("-p".to_string());
            provider_args.push(prompt.to_string());
            provider_args.push("--output-format".to_string());
            provider_args.push(output_format.to_string());
            if let Some(resume_id) = resume_session.filter(|s| !s.trim().is_empty()) {
                provider_args.push("--resume".to_string());
                provider_args.push(resume_id.to_string());
            }
        }
        "opencode" => {
            provider_args.push("run".to_string());
            if let Some(config) = config_override {
                let mut config = config.clone();
                config.resume_session = resume_session.map(str::to_string);
                provider_args.extend(provider.get_spawn_args(&config, resume_session.is_some()));
            } else if let Some(resume_id) = resume_session.filter(|s| !s.trim().is_empty()) {
                provider_args.push("--session".to_string());
                provider_args.push(resume_id.to_string());
            }
            provider_args.push("--format".to_string());
            provider_args.push("json".to_string());
            provider_args.push("--dir".to_string());
            provider_args.push(provider_cwd.to_string_lossy().to_string());
            // `run` re-quotes positional messages; send exact prompt bytes on stdin.
        }
        "antigravity" => {
            if let Some(config) = config_override {
                let mut config = config.clone();
                config.resume_session = resume_session.map(str::to_string);
                provider_args.extend(provider.get_spawn_args(&config, resume_session.is_some()));
                let antigravity = config.antigravity_config();
                if let Some(timeout) = antigravity
                    .print_timeout
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                {
                    provider_args.push("--print-timeout".to_string());
                    provider_args.push(timeout.to_string());
                }
            } else if let Some(resume_id) = resume_session.filter(|s| !s.trim().is_empty()) {
                provider_args.push("--conversation".to_string());
                provider_args.push(resume_id.to_string());
            }
            provider_args.push("--print".to_string());
            provider_args.push(prompt.to_string());
        }
        "pi" => {
            if let Some(config) = config_override {
                let mut config = config.clone();
                let resume_session =
                    resume_session.filter(|session_id| !session_id.trim().is_empty());
                config.resume_session = resume_session.map(str::to_string);
                let mut spawn_args = provider.get_spawn_args(&config, resume_session.is_some());
                spawn_args = strip_flag_value_pairs(spawn_args, "--tui-mode");
                provider_args.extend(spawn_args);
            } else if let Some(resume_id) = resume_session.filter(|s| !s.trim().is_empty()) {
                provider_args.push("--session".into());
                provider_args.push(resume_id.into());
            } else {
                provider_args.push("--no-session".into());
            }
            provider_args.push("--mode".into());
            provider_args.push("json".into());
            provider_args.push(prompt.to_string());
        }
        _ => {
            provider_args.push("-p".to_string());
            provider_args.push(prompt.to_string());
            provider_args.push("--output-format".to_string());
            provider_args.push(output_format.to_string());
            if let Some(resume_id) = resume_session.filter(|s| !s.trim().is_empty()) {
                provider_args.push("--resume".to_string());
                provider_args.push(resume_id.to_string());
            }
        }
    }
    provider_args
}

pub async fn run_headless_with_options(
    options: HeadlessRunOptions<'_>,
) -> Result<serde_json::Value, HeadlessRunError> {
    use tokio::io::{AsyncBufReadExt, BufReader};
    super::validate_session_values_for_launch(options.wardian_session_id, options.resume_session)?;
    let cwd = options.cwd;
    let prompt = options.prompt;
    let wardian_session_id = options.wardian_session_id;
    let memory_agent_id = options
        .memory_agent_id
        .filter(|value| !value.trim().is_empty());
    let resume_session = options.resume_session;
    let output_format = options.output_format;
    let provider_name = options.provider_name;
    let antigravity_workspace_before = if provider_name == "antigravity"
        && options
            .resume_session
            .is_none_or(|value| value.trim().is_empty())
    {
        AntigravityProvider::antigravity_home()
            .and_then(|home| AntigravityProvider::conversation_for_workspace(&home, options.cwd))
    } else {
        None
    };
    let config_override = options.config_override;
    let provider = ProviderFactory::resolve(provider_name)?;
    crate::providers::readiness::ensure_provider_available_for_launch(provider_name)?;
    let persisted_config = persisted_agent_config(wardian_session_id);
    let memory_enabled = crate::utils::memory_feature_enabled();
    let memory_process_key = resume_session
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| format!("fresh:{}:{}", wardian_session_id, uuid::Uuid::new_v4()));
    let memory_workspace = cwd.to_string_lossy().to_string();
    let memory_setup = if memory_enabled {
        memory_agent_id.and_then(|memory_agent_id| {
            match wardian_core::memory::MemoryStore::from_default_home() {
                Ok(store) => match store.compile_brief(
                    &wardian_core::memory::MemoryActor::agent(memory_agent_id),
                    memory_agent_id,
                    Some(&memory_workspace),
                    provider_name,
                    &memory_process_key,
                    resume_session.is_some_and(|value| !value.trim().is_empty()),
                    12_000,
                ) {
                    Ok(brief) => Some((store, brief)),
                    Err(error) => {
                        log_debug(&format!(
                            "[Wardian] headless memory recall unavailable for {memory_agent_id}: {error}"
                        ));
                        None
                    }
                },
                Err(error) => {
                    log_debug(&format!(
                        "[Wardian] headless memory store unavailable for {memory_agent_id}: {error}"
                    ));
                    None
                }
            }
        })
    } else {
        None
    };
    let provider_context = headless_provider_context(
        provider_name,
        cwd,
        wardian_session_id,
        config_override,
        persisted_config.as_ref(),
    )?;
    if memory_enabled {
        if let (Some(_), Some(root)) = (memory_agent_id, provider_context.habitat_root.as_ref()) {
            append_habitat_memory_instructions(
                root,
                memory_setup.as_ref().and_then(|(_, brief)| {
                    (!brief.is_empty).then_some(brief.context_text.as_str())
                }),
            )?;
        }
    }
    let mut effective_provider_config = effective_headless_provider_config(
        provider_name,
        cwd,
        wardian_session_id,
        resume_session,
        config_override,
        persisted_config.as_ref(),
    );
    if let Some(root) = provider_context.habitat_root.as_ref() {
        if !provider_uses_projected_workspace(provider_name) {
            if let Some(config) = effective_provider_config.as_mut() {
                let include = root.to_string_lossy().to_string();
                let includes = config
                    .system_include_directories
                    .get_or_insert_with(Vec::new);
                if !includes.contains(&include) {
                    includes.push(include);
                }
            }
        }
    }
    let memory_runtime_instructions = if memory_enabled {
        memory_agent_id
            .map(|_| {
                wardian_memory_instructions(memory_setup.as_ref().and_then(|(_, brief)| {
                    (!brief.is_empty).then_some(brief.context_text.as_str())
                }))
            })
            .unwrap_or_default()
    } else {
        String::new()
    };
    let (bin, _) = provider.get_executable();
    let claude_hook = if provider_name == "claude" {
        // Headless runs report their own result; they record no Stop rounds.
        ensure_claude_permission_hook(wardian_session_id, None).ok()
    } else {
        None
    };

    let mut provider_args = headless_provider_args(
        provider_name,
        provider.as_ref(),
        &provider_context.args_cwd,
        prompt,
        output_format,
        resume_session,
        effective_provider_config.as_ref(),
    );
    if provider_name == "codex" {
        let config = effective_provider_config
            .as_ref()
            .ok_or("Codex headless launch requires an effective configuration")?;
        CodexProvider::new().insert_managed_instructions_arg(
            &mut provider_args,
            config,
            Some(&memory_runtime_instructions),
        )?;
    }
    if let Some(hook) = claude_hook.as_ref() {
        if provider_name == "claude" {
            provider_args.insert(0, hook.settings_arg.clone());
            provider_args.insert(0, "--settings".to_string());
        }
    }

    let launch_spec = headless_provider_launch(provider_name, &bin, &provider_args)?;
    let mut cmd = new_headless_command(&launch_spec.executable);
    for arg in &launch_spec.args {
        cmd.arg(arg);
    }
    let _memory_capability = apply_headless_identity_env(
        &mut cmd,
        wardian_session_id,
        memory_agent_id,
        memory_enabled,
    );
    super::apply_managed_cli_path_to_process(&mut cmd);
    super::apply_process_provider_runtime_env(provider_name, &mut cmd)?;
    if let Some(config) = effective_provider_config.as_ref() {
        for (key, value) in super::worktree_build_env(config)? {
            cmd.env(key, value);
        }
    }
    if provider_name == "codex" {
        if let Some(root) = provider_context.habitat_root.as_ref() {
            cmd.env("CODEX_HOME", habitat_codex_home(root));
        }
    } else if provider_name == "opencode" {
        let opencode_scope_session = if resume_session.is_some() {
            resume_session
        } else {
            (!wardian_session_id.trim().is_empty()).then_some(wardian_session_id)
        };
        for (key, value) in opencode_env(
            cwd,
            &provider_context.class_name,
            opencode_scope_session,
            effective_provider_config.as_ref(),
        )? {
            cmd.env(key, value);
        }
        opencode_stdin::configure(&mut cmd);
    } else if matches!(provider_name, "antigravity" | "pi") {
        cmd.stdin(std::process::Stdio::null());
    } else if provider_name == "mock" {
        if let Ok(scenario) = std::env::var("WARDIAN_MOCK_SCENARIO") {
            cmd.env("WARDIAN_MOCK_SCENARIO", scenario);
        }
        if let Ok(delay) = std::env::var("WARDIAN_MOCK_DELAY_MS") {
            cmd.env("WARDIAN_MOCK_DELAY_MS", delay);
        }
        if let Ok(script) = std::env::var("WARDIAN_MOCK_SCRIPT") {
            cmd.env("WARDIAN_MOCK_SCRIPT", script);
        }
    }

    #[cfg(target_os = "macos")]
    cmd.env("PATH", macos_extended_path());

    #[cfg(unix)]
    cmd.process_group(0);

    cmd.current_dir(&provider_context.command_cwd)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    log_debug(&format!(
        "[Wardian] run_headless: provider={}, session_id={}, cwd={}, prompt_len={}, output_format={}",
        provider_name,
        if wardian_session_id.is_empty() {
            "<none>"
        } else {
            wardian_session_id
        },
        provider_context.command_cwd.display(),
        prompt.len(),
        output_format
    ));
    log_debug(&format!(
        "[Wardian] run_headless launch: exe={}, arg_count={}, resume={}",
        launch_spec.executable,
        launch_spec.args.len(),
        resume_session.is_some_and(|value| !value.trim().is_empty())
    ));
    if provider_name == "codex" {
        let bypasses_sandbox = launch_spec
            .args
            .iter()
            .any(|arg| arg == "--dangerously-bypass-approvals-and-sandbox");
        let automatic_review = launch_spec.args.iter().any(|arg| arg == "--approve-for-me");
        let sandbox_mode = launch_spec
            .args
            .iter()
            .position(|arg| arg == "--sandbox")
            .and_then(|index| launch_spec.args.get(index + 1))
            .map(String::as_str)
            .or(automatic_review.then_some("workspace-write"))
            .unwrap_or("<bypassed>");
        let approval_policy = if automatic_review {
            "approve-for-me"
        } else {
            launch_spec
                .args
                .iter()
                .position(|arg| arg == "--ask-for-approval")
                .and_then(|index| launch_spec.args.get(index + 1))
                .map(String::as_str)
                .unwrap_or("<bypassed>")
        };
        log_debug(&format!(
            "[Wardian] run_headless Codex policy: bypass={}, sandbox={}, approval={}",
            bypasses_sandbox, sandbox_mode, approval_policy
        ));
    }

    // If the control request is cancelled, dropping the child must terminate
    // the provider rather than leaving it running against a leased session.
    cmd.kill_on_drop(true);
    let mut child = cmd.spawn().map_err(|e| e.to_string())?;
    let mut process_tree_guard = HeadlessProcessTreeGuard::new(child.id());
    // Read stdout and stderr concurrently to avoid deadlock when stderr buffer fills.
    let stdout_handle = {
        let stdout = child.stdout.take();
        tokio::spawn(async move {
            let mut out = String::new();
            if let Some(stream) = stdout {
                let mut reader = BufReader::new(stream);
                let mut line = String::new();
                while let Ok(n) = reader.read_line(&mut line).await {
                    if n == 0 {
                        break;
                    }
                    out.push_str(&line);
                    line.clear();
                }
            }
            out
        })
    };

    let stderr_handle = {
        let stderr = child.stderr.take();
        tokio::spawn(async move {
            let mut err = String::new();
            if let Some(stream) = stderr {
                let mut reader = BufReader::new(stream);
                let mut line = String::new();
                while let Ok(n) = reader.read_line(&mut line).await {
                    if n == 0 {
                        break;
                    }
                    err.push_str(&line);
                    line.clear();
                }
            }
            err
        })
    };

    let status = if provider_name == "opencode" {
        opencode_stdin::wait(
            &mut child,
            prompt,
            options.timeout,
            options.lease_owner.as_ref(),
            options.cancellation_marker,
            &mut process_tree_guard,
        )
        .await
    } else {
        wait_for_headless_child(
            &mut child,
            provider_name,
            options.timeout,
            options.lease_owner.as_ref(),
            options.cancellation_marker,
        )
        .await
    };
    if status.is_ok() {
        process_tree_guard.disarm();
    }

    let (output, err_output) = tokio::join!(stdout_handle, stderr_handle);
    let output = output.unwrap_or_default();
    let err_output = err_output.unwrap_or_default();
    let status = status?;

    if !err_output.is_empty() {
        log_debug(&format!(
            "[Wardian] Headless provider wrote {} stderr bytes.",
            err_output.len()
        ));
    }
    if !status.success() {
        let detail = if !err_output.trim().is_empty() {
            err_output.trim()
        } else if !output.trim().is_empty() {
            output.trim()
        } else {
            "provider exited without output"
        };
        return Err(HeadlessRunError::definite(format!(
            "Headless provider {provider_name} exited with status {}: {detail}",
            status.code().unwrap_or(-1)
        )));
    }

    if let Some((memory_store, memory_brief)) = memory_setup {
        let memory_agent_id = memory_agent_id.expect("memory setup requires an owner");
        if let Err(error) = memory_store.record_injection(
            &wardian_core::memory::MemoryActor::agent(memory_agent_id),
            memory_agent_id,
            Some(&memory_workspace),
            provider_name,
            &memory_process_key,
            &memory_brief,
        ) {
            log_debug(&format!(
                "[Wardian] headless memory injection receipt unavailable for {memory_agent_id}: {error}"
            ));
        }
    }

    if provider_name == "codex" {
        let thread_id = bootstrap_output_session_id("codex", &output).or_else(|| {
            resume_session
                .filter(|value| !value.trim().is_empty())
                .map(str::to_string)
        });
        let mut last_message = None;
        for line in output.lines() {
            if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(line) {
                match parsed.get("type").and_then(|v| v.as_str()).unwrap_or("") {
                    "item.completed"
                        if parsed
                            .get("item")
                            .and_then(|v| v.get("type"))
                            .and_then(|v| v.as_str())
                            == Some("agent_message") =>
                    {
                        last_message = parsed
                            .get("item")
                            .and_then(|v| v.get("text"))
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string());
                    }
                    "event_msg"
                        if parsed
                            .get("payload")
                            .and_then(|v| v.get("type"))
                            .and_then(|v| v.as_str())
                            == Some("agent_message") =>
                    {
                        last_message = parsed
                            .get("payload")
                            .and_then(|v| v.get("message"))
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string());
                    }
                    _ => {}
                }
            }
        }

        if output_format == "json" {
            Ok(serde_json::json!({
                "thread_id": thread_id,
                "response": last_message.unwrap_or_else(|| output.clone()),
                "raw": output,
            }))
        } else {
            Ok(serde_json::json!({ "text": last_message.unwrap_or(output) }))
        }
    } else if provider_name == "claude" {
        normalize_claude_headless_output(&output, output_format)
            .map_err(HeadlessRunError::uncertain)
    } else if provider_name == "opencode" {
        let summary = OpenCodeProvider::summarize_run_output(&output);
        let response = summary.last_text.unwrap_or_else(|| output.clone());

        if output_format == "json" {
            Ok(serde_json::json!({
                "session_id": summary.session_id,
                "response": response,
                "raw": output,
            }))
        } else {
            Ok(serde_json::json!({ "text": response }))
        }
    } else if provider_name == "antigravity" {
        let conversation_id = resume_session
            .filter(|value| !value.trim().is_empty())
            .map(str::to_string)
            .or_else(|| {
                let after = AntigravityProvider::antigravity_home()
                    .and_then(|home| AntigravityProvider::conversation_for_workspace(&home, cwd));
                changed_workspace_conversation(
                    antigravity_workspace_before.as_deref(),
                    after.as_deref(),
                )
            });
        let summary = conversation_id.as_deref().and_then(|conversation_id| {
            AntigravityProvider::antigravity_home().and_then(|home| {
                AntigravityProvider::summarize_conversation(&home, conversation_id).ok()
            })
        });
        let response = summary
            .as_ref()
            .and_then(|summary| summary.last_text.clone())
            .filter(|text| !text.trim().is_empty())
            .unwrap_or_else(|| output.clone());

        if output_format == "json" {
            Ok(serde_json::json!({
                "session_id": conversation_id,
                "response": response,
                "raw": output,
            }))
        } else {
            Ok(serde_json::json!({ "text": response }))
        }
    } else if provider_name == "pi" {
        normalize_pi_headless_output(&output, output_format).map_err(HeadlessRunError::uncertain)
    } else if output_format == "json" {
        serde_json::from_str(&output).map_err(|e| {
            HeadlessRunError::uncertain(format!(
                "Failed to parse JSON output: {}. Raw: {}",
                e, output
            ))
        })
    } else {
        Ok(serde_json::json!({ "text": output }))
    }
}

fn normalize_pi_headless_output(
    output: &str,
    output_format: &str,
) -> Result<serde_json::Value, String> {
    let mut session_id = None;
    let mut response = None;
    for line in output.lines().filter(|line| !line.trim().is_empty()) {
        let parsed: serde_json::Value = serde_json::from_str(line.trim())
            .map_err(|error| format!("Failed to parse Pi JSON event: {error}. Raw: {output}"))?;
        match parsed.get("type").and_then(|value| value.as_str()) {
            Some("session") => {
                session_id = parsed
                    .get("id")
                    .and_then(|value| value.as_str())
                    .map(str::to_string);
            }
            Some(event_type @ ("message" | "message_end")) => {
                let message = parsed.get("message").unwrap_or(&parsed);
                let is_assistant =
                    message.get("role").and_then(|value| value.as_str()) == Some("assistant");
                let is_complete = match message.get("stopReason").and_then(|value| value.as_str()) {
                    Some("stop" | "length") => true,
                    Some(_) => false,
                    None => event_type == "message_end",
                };
                if is_assistant && is_complete {
                    response = pi_message_text(message).or(response);
                }
            }
            _ => {}
        }
    }
    let response =
        response.ok_or_else(|| "Pi completed without an assistant response".to_string())?;
    if output_format == "json" {
        Ok(serde_json::json!({
            "session_id": session_id,
            "response": response,
            "raw": output,
        }))
    } else {
        Ok(serde_json::json!({ "text": response }))
    }
}

fn pi_message_text(message: &serde_json::Value) -> Option<String> {
    let content = message.get("content")?;
    if let Some(text) = content.as_str() {
        return (!text.trim().is_empty()).then(|| text.trim().to_string());
    }
    let text = content
        .as_array()?
        .iter()
        .filter(|block| block.get("type").and_then(|value| value.as_str()) == Some("text"))
        .filter_map(|block| block.get("text").and_then(|value| value.as_str()))
        .collect::<Vec<_>>()
        .join("");
    (!text.trim().is_empty()).then(|| text.trim().to_string())
}

async fn wait_for_headless_child(
    child: &mut tokio::process::Child,
    provider_name: &str,
    timeout: Duration,
    lease_owner: Option<&ConversationLeaseOwner>,
    cancellation_marker: Option<&std::path::Path>,
) -> Result<std::process::ExitStatus, HeadlessRunError> {
    wait_for_headless_child_with_intervals(
        child,
        provider_name,
        timeout,
        lease_owner,
        cancellation_marker,
        HEADLESS_PROCESS_POLL_INTERVAL,
        HEADLESS_LEASE_HEARTBEAT_INTERVAL,
    )
    .await
}

#[derive(Debug, PartialEq, Eq)]
enum HeadlessWaitEvent {
    Deadline,
    Cancelled,
    ProcessPoll,
    Heartbeat,
}

/// Observes execution limits independently of OS termination/reaping latency.
struct HeadlessWaitTiming {
    deadline: tokio::time::Instant,
    process_poll: tokio::time::Interval,
    heartbeat: tokio::time::Interval,
}

impl HeadlessWaitTiming {
    fn new(
        timeout: Duration,
        process_poll_interval: Duration,
        heartbeat_interval: Duration,
    ) -> Self {
        let now = tokio::time::Instant::now();
        Self {
            deadline: now + timeout,
            process_poll: tokio::time::interval_at(
                now + process_poll_interval,
                process_poll_interval,
            ),
            heartbeat: tokio::time::interval_at(now + heartbeat_interval, heartbeat_interval),
        }
    }

    async fn next_event(
        &mut self,
        cancellation_marker: Option<&std::path::Path>,
        heartbeat_enabled: bool,
    ) -> HeadlessWaitEvent {
        tokio::select! {
            _ = tokio::time::sleep_until(self.deadline) => HeadlessWaitEvent::Deadline,
            _ = self.process_poll.tick() => {
                if cancellation_marker.is_some_and(std::path::Path::exists) {
                    HeadlessWaitEvent::Cancelled
                } else {
                    HeadlessWaitEvent::ProcessPoll
                }
            }
            _ = self.heartbeat.tick(), if heartbeat_enabled => HeadlessWaitEvent::Heartbeat,
        }
    }
}

async fn wait_for_headless_child_with_intervals(
    child: &mut tokio::process::Child,
    provider_name: &str,
    timeout: Duration,
    lease_owner: Option<&ConversationLeaseOwner>,
    cancellation_marker: Option<&std::path::Path>,
    process_poll_interval: Duration,
    lease_heartbeat_interval: Duration,
) -> Result<std::process::ExitStatus, HeadlessRunError> {
    let mut timing =
        HeadlessWaitTiming::new(timeout, process_poll_interval, lease_heartbeat_interval);

    loop {
        match timing
            .next_event(cancellation_marker, lease_owner.is_some())
            .await
        {
            HeadlessWaitEvent::Deadline => {
                terminate_headless_child(child).await;
                return Err(HeadlessRunError::uncertain(format!(
                    "Headless provider {provider_name} exceeded its {} second execution limit",
                    timeout.as_secs()
                )));
            }
            HeadlessWaitEvent::Cancelled => {
                terminate_headless_child(child).await;
                return Err(HeadlessRunError::cancelled(format!(
                    "Headless provider {provider_name} cancelled by its owning automation run"
                )));
            }
            HeadlessWaitEvent::ProcessPoll => {
                if let Some(status) = child
                    .try_wait()
                    .map_err(|error| HeadlessRunError::uncertain(error.to_string()))?
                {
                    return Ok(status);
                }
            }
            HeadlessWaitEvent::Heartbeat => {
                let owner = lease_owner.expect("lease owner checked by select guard");
                let now = chrono::Utc::now();
                let renewed = match wardian_core::conversation_lease::renew_lease_owner_persisted(
                    owner,
                    &now.to_rfc3339(),
                    &(now + HEADLESS_LEASE_DURATION).to_rfc3339(),
                ) {
                    Ok(renewed) => renewed,
                    Err(error) => {
                        terminate_headless_child(child).await;
                        return Err(HeadlessRunError::uncertain(format!(
                            "Headless provider {provider_name} lease observation failed before completion: {error}"
                        )));
                    }
                };
                if !renewed {
                    terminate_headless_child(child).await;
                    return Err(HeadlessRunError::uncertain(format!(
                        "Headless provider {provider_name} lost its conversation lease before completion"
                    )));
                }
            }
        }
    }
}

async fn terminate_headless_child(child: &mut tokio::process::Child) {
    if let Some(pid) = child.id() {
        terminate_headless_process_tree(pid);
    }
    let _ = child.start_kill();
    let _ = child.wait().await;
}

fn normalize_claude_headless_output(
    output: &str,
    output_format: &str,
) -> Result<serde_json::Value, String> {
    let parsed = serde_json::from_str::<serde_json::Value>(output.trim())
        .map_err(|error| format!("Failed to parse Claude JSON output: {error}. Raw: {output}"))?;
    let result = claude_headless_result(&parsed)?;
    let response = if result["type"] == "result" {
        result["result"]
            .as_str()
            .expect("validated Claude terminal result")
            .trim()
            .to_owned()
    } else {
        claude_headless_response(result).unwrap_or_else(|| output.to_string())
    };

    if output_format == "json" {
        Ok(serde_json::json!({
            "session_id": result.get("session_id").and_then(|value| value.as_str()),
            "response": response,
            "raw": output,
        }))
    } else {
        Ok(serde_json::json!({ "text": response }))
    }
}

/// Verbose Claude JSON contains conversation events followed by one terminal
/// result. Earlier assistant/tool text is not the completed task's answer.
fn claude_headless_result(value: &serde_json::Value) -> Result<&serde_json::Value, String> {
    let result = if let Some(events) = value.as_array() {
        let result = events
            .last()
            .filter(|event| event["type"] == "result")
            .ok_or("Claude headless event array has no terminal result")?;
        if events
            .iter()
            .filter(|event| event["type"] == "result")
            .count()
            != 1
        {
            return Err("Claude headless event array has ambiguous results".into());
        }
        result
    } else {
        value
    };
    if result["type"] == "result" {
        if !result["result"].is_string() {
            return Err("Claude headless terminal result has no answer text".into());
        }
        if result
            .get("is_error")
            .is_some_and(|flag| flag.as_bool() != Some(false))
            || result
                .get("subtype")
                .is_some_and(|kind| kind.as_str() != Some("success"))
        {
            return Err("Claude headless terminal result did not succeed".into());
        }
    }
    Ok(result)
}

#[cfg(test)]
#[path = "headless/claude_output_tests.rs"]
mod claude_output_tests;

fn claude_headless_response(value: &serde_json::Value) -> Option<String> {
    for key in ["result", "response", "text"] {
        if let Some(text) = value.get(key).and_then(|value| value.as_str()) {
            let text = text.trim();
            if !text.is_empty() {
                return Some(text.to_string());
            }
        }
    }

    value
        .get("message")
        .and_then(|message| message.get("content"))
        .and_then(|content| content.as_str())
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(ToString::to_string)
}

fn append_codex_bootstrap_args(
    provider_args: &mut Vec<String>,
    provider_cwd: &std::path::Path,
    config: Option<&AgentConfig>,
    selected_policy: Option<&crate::utils::CodexRuntimePolicy>,
) {
    provider_args.push("--cd".to_string());
    provider_args.push(provider_cwd.to_string_lossy().to_string());

    // Codex global options must precede `exec`; exec only accepts its own
    // subcommand options after this point.
    if let Some(config) = config {
        let provider = CodexProvider::new();
        if let Some(policy) = selected_policy {
            provider.append_headless_global_args_with_effective_policy(
                provider_args,
                config,
                policy,
            );
        } else {
            provider.append_headless_global_args(provider_args, config);
        }
        if let Some(custom) = config.custom_args.as_ref() {
            if let Some(parsed) = shlex::split(custom) {
                provider_args.extend(parsed);
            }
        }
    }

    provider_args.push("exec".to_string());
    CodexProvider::new().append_headless_exec_args(provider_args, config);

    provider_args.push("--json".to_string());
    provider_args.push(session_bootstrap_prompt().to_string());
}

fn materialize_codex_session_rollout(
    codex_home: &std::path::Path,
    cwd: &std::path::Path,
) -> Result<String, String> {
    let session_id = uuid::Uuid::new_v4().to_string();
    let now = chrono::Utc::now();
    let timestamp = now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let sessions_dir = codex_home
        .join("sessions")
        .join(now.format("%Y").to_string())
        .join(now.format("%m").to_string())
        .join(now.format("%d").to_string());
    std::fs::create_dir_all(&sessions_dir)
        .map_err(|_| "Could not create the Codex session directory".to_string())?;
    let rollout_path = sessions_dir.join(format!(
        "rollout-{}-{}.jsonl",
        now.format("%Y-%m-%dT%H-%M-%S"),
        session_id
    ));
    let rollout = serde_json::json!({
        "timestamp": timestamp,
        "type": "session_meta",
        "payload": {
            "id": session_id,
            "session_id": session_id,
            "timestamp": timestamp,
            "cwd": cwd,
            "originator": "Wardian",
            "cli_version": env!("CARGO_PKG_VERSION"),
            "source": "vscode",
            "model_provider": "openai",
        }
    });
    std::fs::write(&rollout_path, format!("{rollout}\n"))
        .map_err(|_| "Could not create the Codex session rollout".to_string())?;

    if codex_session_file_path_in(codex_home, &session_id).is_none() {
        return Err("Codex did not recognize the fresh session rollout".to_string());
    }

    Ok(session_id)
}

pub async fn obtain_session_id(
    cwd: &std::path::Path,
    agent_class: Option<&str>,
    config: Option<&AgentConfig>,
) -> Result<String, String> {
    obtain_session_id_inner(cwd, agent_class, config, None, None).await
}

pub(crate) async fn obtain_session_id_with_codex_context(
    cwd: &std::path::Path,
    agent_class: Option<&str>,
    config: Option<&AgentConfig>,
    codex_launch_context: &crate::utils::codex_home::CodexLaunchContext,
    operation: Option<(&str, &str)>,
) -> Result<String, String> {
    obtain_session_id_inner(
        cwd,
        agent_class,
        config,
        Some(codex_launch_context),
        operation,
    )
    .await
}

async fn obtain_session_id_inner(
    cwd: &std::path::Path,
    agent_class: Option<&str>,
    config: Option<&AgentConfig>,
    codex_launch_context: Option<&crate::utils::codex_home::CodexLaunchContext>,
    operation: Option<(&str, &str)>,
) -> Result<String, String> {
    let provider_name = config.map(|c| c.provider.as_str()).unwrap_or("claude");
    let provider = ProviderFactory::resolve(provider_name)?;
    crate::providers::readiness::ensure_provider_available_for_launch(provider_name)?;
    let (bin, mut provider_args) = provider.get_executable();
    let class_name = agent_class
        .filter(|name| !name.trim().is_empty())
        .or_else(|| {
            config.and_then(|cfg| {
                (!cfg.agent_class.trim().is_empty()).then_some(cfg.agent_class.as_str())
            })
        })
        .unwrap_or("");
    let bootstrap_session_id = config
        .and_then(|cfg| (!cfg.session_id.trim().is_empty()).then_some(cfg.session_id.as_str()));
    let habitat_root = if provider_name == "codex" {
        if let Some(context) = codex_launch_context {
            let Some(session_id) = bootstrap_session_id else {
                return Err("Codex bootstrap identity is missing".to_string());
            };
            let habitat = prepare_habitat_workspace(cwd, class_name, session_id)?;
            let wardian_home = get_wardian_home().ok_or("Could not find Wardian home")?;
            let _preparation =
                crate::utils::codex_home::acquire_preparation(&wardian_home, session_id)?;
            ensure_codex_home_projection_with_context(&habitat, cwd, session_id, context)?;
            if let crate::utils::codex_messaging::Registration::Unavailable(reason) =
                crate::utils::codex_messaging::ensure_managed_messaging(&wardian_home, session_id)?
            {
                log_debug(&format!("[Wardian] Codex messaging unavailable: {reason}"));
            }
            Some(habitat)
        } else {
            prepare_provider_habitat(provider_name, cwd, class_name, bootstrap_session_id)?
        }
    } else {
        prepare_provider_habitat(provider_name, cwd, class_name, bootstrap_session_id)?
    };
    if provider_name == "codex" && habitat_root.is_some() {
        let agent_id = bootstrap_session_id.ok_or("Codex habitat identity is missing")?;
        let wardian_home = get_wardian_home().ok_or("Could not find Wardian home")?;
        let _preparation = crate::utils::codex_home::acquire_preparation(&wardian_home, agent_id)?;
        let agent_codex_home =
            crate::utils::codex_home::resolve_managed_home(&wardian_home, agent_id)?;

        match materialize_codex_session_rollout(&agent_codex_home, cwd) {
                Ok(session_id) => {
                    let mut identity_config = config.cloned().unwrap_or_else(|| AgentConfig {
                        provider: provider_name.to_string(),
                        ..Default::default()
                    });
                    super::apply_provider_identity(provider_name, &mut identity_config, &session_id)?;
                    log_debug("[WARDIAN-DEBUG] Materialized a fresh local Codex session rollout.");
                    return Ok(session_id);
                }
                Err(error) => log_debug(&format!(
                    "[WARDIAN-DEBUG] Codex session rollout materialization unavailable; using legacy bootstrap: {error}"
                )),
            }
    }
    let codex_bootstrap = if provider_name == "codex" {
        let wardian_home = get_wardian_home().ok_or("Could not find Wardian home")?;
        let attempt_id = operation
            .map(|(_, attempt)| attempt.to_string())
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        Some(codex_bootstrap_launch_context(
            &wardian_home,
            cwd,
            &attempt_id,
        )?)
    } else {
        None
    };
    let mut bootstrap_credentials = codex_bootstrap
        .as_ref()
        .map(|(_, home)| super::codex::BootstrapCredentialRetirement::new(home.clone()));

    let provider_cwd = interactive_provider_cwd(
        provider_name,
        cwd,
        habitat_root.as_deref(),
        codex_bootstrap.as_ref(),
    );
    let antigravity_workspace_before = if provider_name == "antigravity" {
        AntigravityProvider::antigravity_home()
            .and_then(|home| AntigravityProvider::conversation_for_workspace(&home, cwd))
    } else {
        None
    };

    if provider_name == "codex" {
        append_codex_bootstrap_args(
            &mut provider_args,
            &provider_cwd,
            config,
            codex_launch_context.map(|context| context.effective_policy()),
        );
    } else if provider_name == "opencode" {
        provider_args.push("run".to_string());
        if let Some(config) = config {
            provider_args.extend(provider.get_spawn_args(config, false));
        }
        provider_args.push("--format".to_string());
        provider_args.push("json".to_string());
        provider_args.push("--dir".to_string());
        provider_args.push(cwd.to_string_lossy().to_string());
        provider_args.push(session_bootstrap_prompt().to_string());
    } else if provider_name == "antigravity" {
        if let Some(config) = config {
            provider_args.extend(provider.get_spawn_args(config, false));
        }
        provider_args.push("--print".to_string());
        provider_args.push(session_bootstrap_prompt().to_string());
    } else if provider_name == "claude" {
        // This bootstrap print call supplies a prompt argument, not stream-json stdin.
        if let Some(config) = config {
            let spawn_args =
                strip_flag_value_pairs(provider.get_spawn_args(config, false), "--input-format");
            provider_args.extend(spawn_args);
        } else {
            provider_args.push("--verbose".to_string());
            provider_args.push("--output-format".to_string());
            provider_args.push("stream-json".to_string());
        }
        provider_args.push("--print".to_string());
        provider_args.push(session_bootstrap_prompt().to_string());
    } else {
        provider_args.push("-p".to_string());
        provider_args.push(session_bootstrap_prompt().to_string());
        provider_args.push("-o".to_string());
        provider_args.push("stream-json".to_string());
    }

    let launch_spec = headless_provider_launch(provider_name, &bin, &provider_args)?;
    let mut cmd = new_headless_command(&launch_spec.executable);
    for arg in &launch_spec.args {
        cmd.arg(arg);
    }
    let memory_enabled = crate::utils::memory_feature_enabled();
    let _memory_capability = bootstrap_session_id.and_then(|session_id| {
        apply_headless_identity_env(&mut cmd, session_id, Some(session_id), memory_enabled)
    });
    super::apply_managed_cli_path_to_process(&mut cmd);
    super::apply_process_provider_runtime_env(provider_name, &mut cmd)?;

    if provider_name == "codex" {
        if let Some((_, bootstrap_home)) = codex_bootstrap.as_ref() {
            let real_codex_home = match codex_launch_context {
                Some(context) => context.upstream().path.clone(),
                None => dirs::home_dir()
                    .ok_or("Could not find user home directory")?
                    .join(".codex"),
            };
            if let Some(context) = codex_launch_context {
                crate::utils::fs::sync_codex_agent_home_with_context(
                    &real_codex_home,
                    bootstrap_home,
                    std::path::Path::new(""),
                    context,
                )?;
            } else {
                sync_codex_agent_home(&real_codex_home, bootstrap_home, std::path::Path::new(""))?;
            }
            cmd.env("CODEX_HOME", bootstrap_home);
        } else if let Some(root) = habitat_root.as_ref() {
            cmd.env("CODEX_HOME", habitat_codex_home(root));
        }
    } else if provider_name == "opencode" {
        for (key, value) in opencode_env(cwd, class_name, bootstrap_session_id, config)? {
            cmd.env(key, value);
        }
    }
    // Bootstrap commands are unattended. Inheriting stdin lets Codex wait for
    // a prompt that Wardian will never answer, turning a normal clear into a
    // timeout and leaving an incomplete session behind.
    cmd.stdin(std::process::Stdio::null());

    let command_cwd = if provider_name == "claude" {
        cwd.to_path_buf()
    } else {
        provider_cwd.clone()
    };

    cmd.current_dir(&command_cwd)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    #[cfg(target_os = "macos")]
    cmd.env("PATH", macos_extended_path());

    log_debug(&format!(
        "[WARDIAN-DEBUG] Running obtain_session_id for provider {}",
        provider_name
    ));
    log_debug(&format!(
        "[WARDIAN-DEBUG] obtain_session_id launch: exe={} arg_count={} cwd={}",
        launch_spec.executable,
        launch_spec.args.len(),
        command_cwd.display()
    ));
    let process_receipt = run_owned_bootstrap_command(
        &mut cmd,
        provider_name,
        Duration::from_secs(60),
        BOOTSTRAP_CLEANUP_BUDGET,
        codex_launch_context,
        operation,
        bootstrap_credentials.as_mut(),
    )
    .await?;
    if let Some(credentials) = bootstrap_credentials.as_mut() {
        credentials.confirm_closed(process_receipt.containment_empty)?;
    }
    if process_receipt.timed_out {
        return Err("Provider bootstrap exceeded its 60 second work budget.".to_string());
    }
    if !process_receipt.work_complete
        || !process_receipt.exit_success
        || !process_receipt.stdout_eof
        || !process_receipt.stderr_eof
        || !process_receipt.containment_empty
    {
        log_debug(&format!(
            "[WARDIAN-DEBUG] Bootstrap rejected: exit_code={}, stdout_eof={}, stderr_eof={}, containment_empty={}",
            process_receipt.exit_code,
            process_receipt.stdout_eof,
            process_receipt.stderr_eof,
            process_receipt.containment_empty
        ));
        return Err(format!(
            "Provider {} failed during session initialization.",
            provider_name
        ));
    }
    let mut session_id_res = process_receipt.session_id;
    if provider_name == "antigravity" && session_id_res.is_none() {
        let after = AntigravityProvider::antigravity_home()
            .and_then(|home| AntigravityProvider::conversation_for_workspace(&home, cwd));
        session_id_res = changed_workspace_conversation(
            antigravity_workspace_before.as_deref(),
            after.as_deref(),
        );
    }
    if let Some(candidate) = session_id_res.as_deref() {
        let mut identity_config = config.cloned().unwrap_or_else(|| AgentConfig {
            provider: provider_name.to_string(),
            ..Default::default()
        });
        super::apply_provider_identity(provider_name, &mut identity_config, candidate)?;
    }
    if provider_name == "codex" {
        if let (Some((_, bootstrap_home)), Some(agent_habitat_root)) =
            (codex_bootstrap.as_ref(), habitat_root.as_ref())
        {
            migrate_codex_bootstrap_home(bootstrap_home, &habitat_codex_home(agent_habitat_root))?;
        }
    }
    log_debug(&format!(
        "[WARDIAN-DEBUG] Returning session identifier: found={}",
        session_id_res.is_some()
    ));
    session_id_res.ok_or_else(|| {
        if process_receipt.stderr_bytes == 0 {
            format!(
                "Provider {} did not return a session ID during initialization.",
                provider_name
            )
        } else {
            format!(
                "Provider {} failed during session initialization.",
                provider_name
            )
        }
    })
}

const BOOTSTRAP_STDOUT_RETAIN_LIMIT: usize = 256 * 1024;
const BOOTSTRAP_STDERR_RETAIN_LIMIT: usize = 16 * 1024;
const BOOTSTRAP_LINE_LIMIT: usize = 64 * 1024;
const BOOTSTRAP_CLEANUP_BUDGET: Duration = Duration::from_secs(5);

struct BootstrapStreamReceipt {
    session_id: Option<String>,
    retained_output: Vec<u8>,
    bytes_read: u64,
    eof: bool,
}

async fn drain_bootstrap_stdout<R: tokio::io::AsyncRead + Unpin>(
    mut reader: R,
    provider_name: &str,
) -> Result<BootstrapStreamReceipt, String> {
    use tokio::io::AsyncReadExt;

    let mut retained_output = Vec::new();
    let mut pending_line = Vec::new();
    let mut line_overflow = false;
    let mut bytes_read = 0_u64;
    let mut session_id = None;
    let mut buffer = [0_u8; 8192];
    loop {
        let count = reader
            .read(&mut buffer)
            .await
            .map_err(|_| "bootstrap stdout read failed".to_string())?;
        if count == 0 {
            if !line_overflow && !pending_line.is_empty() && session_id.is_none() {
                session_id = bootstrap_output_session_id(
                    provider_name,
                    &String::from_utf8_lossy(&pending_line),
                );
            }
            return Ok(BootstrapStreamReceipt {
                session_id,
                retained_output,
                bytes_read,
                eof: true,
            });
        }
        bytes_read = bytes_read.saturating_add(count as u64);
        let retained = count.min(BOOTSTRAP_STDOUT_RETAIN_LIMIT - retained_output.len());
        retained_output.extend_from_slice(&buffer[..retained]);
        for byte in &buffer[..count] {
            if *byte == b'\n' {
                if !line_overflow && session_id.is_none() {
                    session_id = bootstrap_output_session_id(
                        provider_name,
                        &String::from_utf8_lossy(&pending_line),
                    );
                }
                pending_line.clear();
                line_overflow = false;
            } else if pending_line.len() < BOOTSTRAP_LINE_LIMIT {
                pending_line.push(*byte);
            } else {
                pending_line.clear();
                line_overflow = true;
            }
        }
    }
}

async fn drain_bootstrap_stderr<R: tokio::io::AsyncRead + Unpin>(
    mut reader: R,
) -> Result<BootstrapStreamReceipt, String> {
    use tokio::io::AsyncReadExt;

    let mut retained_output = Vec::new();
    let mut bytes_read = 0_u64;
    let mut buffer = [0_u8; 8192];
    loop {
        let count = reader
            .read(&mut buffer)
            .await
            .map_err(|_| "bootstrap stderr read failed".to_string())?;
        if count == 0 {
            return Ok(BootstrapStreamReceipt {
                session_id: None,
                retained_output,
                bytes_read,
                eof: true,
            });
        }
        bytes_read = bytes_read.saturating_add(count as u64);
        let retained = count.min(BOOTSTRAP_STDERR_RETAIN_LIMIT - retained_output.len());
        retained_output.extend_from_slice(&buffer[..retained]);
    }
}

#[derive(Debug, serde::Serialize)]
struct BootstrapProcessReceipt {
    child_pid: Option<u32>,
    session_id: Option<String>,
    stderr_bytes: u64,
    exit_code: i32,
    exit_observed: bool,
    exit_success: bool,
    stdout_eof: bool,
    stderr_eof: bool,
    containment_empty: bool,
    work_complete: bool,
    timed_out: bool,
}

fn persist_bootstrap_receipt(
    operation: Option<(&str, &str)>,
    receipt: &BootstrapProcessReceipt,
) -> Result<(), String> {
    let Some((operation_id, attempt_id)) = operation else {
        return Ok(());
    };
    let receipt_json = serde_json::to_string(receipt)
        .map_err(|_| "bootstrap closure receipt could not be encoded".to_string())?;
    if !wardian_core::agent_spawn::record_bootstrap_receipt(
        operation_id,
        attempt_id,
        &receipt_json,
    )? {
        return Err("spawn operation lost ownership before bootstrap closure was recorded".into());
    }
    Ok(())
}

async fn run_owned_bootstrap_command(
    command: &mut tokio::process::Command,
    provider_name: &str,
    work_budget: Duration,
    cleanup_budget: Duration,
    codex_launch_context: Option<&crate::utils::codex_home::CodexLaunchContext>,
    operation: Option<(&str, &str)>,
    mut credentials: Option<&mut super::codex::BootstrapCredentialRetirement>,
) -> Result<BootstrapProcessReceipt, String> {
    if let Some(context) = codex_launch_context {
        context.revalidate()?;
    }
    let mut child = crate::utils::process::OwnedBootstrapChild::spawn(command, || {
        if let Some(credentials) = credentials.as_mut() {
            credentials.child_may_start();
        }
    })
    .map_err(|_| format!("Failed to start the {provider_name} bootstrap command."))?;
    let child_pid = child.child_mut().id();
    let stdout = child
        .child_mut()
        .stdout
        .take()
        .ok_or_else(|| "bootstrap stdout pipe is unavailable".to_string())?;
    let stderr = child
        .child_mut()
        .stderr
        .take()
        .ok_or_else(|| "bootstrap stderr pipe is unavailable".to_string())?;
    let stdout_provider = provider_name.to_string();
    let mut stdout_task =
        tokio::spawn(async move { drain_bootstrap_stdout(stdout, &stdout_provider).await });
    let mut stderr_task = tokio::spawn(drain_bootstrap_stderr(stderr));
    let mut stdout_result: Option<Result<BootstrapStreamReceipt, String>> = None;
    let mut stderr_result: Option<Result<BootstrapStreamReceipt, String>> = None;
    let mut exit_status: Option<std::process::ExitStatus> = None;
    let work_deadline = tokio::time::Instant::now() + work_budget;
    let work = async {
        exit_status = Some(
            child
                .child_mut()
                .wait()
                .await
                .map_err(|_| "bootstrap child wait failed".to_string())?,
        );
        stdout_result = Some(
            (&mut stdout_task)
                .await
                .map_err(|_| "bootstrap stdout drain task failed".to_string())?,
        );
        stdout_result
            .as_ref()
            .expect("stdout result was assigned")
            .as_ref()
            .map_err(Clone::clone)?;
        stderr_result = Some(
            (&mut stderr_task)
                .await
                .map_err(|_| "bootstrap stderr drain task failed".to_string())?,
        );
        stderr_result
            .as_ref()
            .expect("stderr result was assigned")
            .as_ref()
            .map_err(Clone::clone)?;
        child.wait_for_containment_empty(work_deadline).await
    };
    let work_result = tokio::time::timeout_at(work_deadline, work).await;
    let work_complete = matches!(&work_result, Ok(Ok(())));
    let timed_out = work_result.is_err();
    let containment_empty;

    if work_complete {
        containment_empty = true;
        child.release_after_verified_exit();
        if let Some(credentials) = credentials.as_mut() {
            credentials.confirm_closed(true)?;
        }
    } else {
        let cleanup_deadline = tokio::time::Instant::now() + cleanup_budget;
        let cleanup = async {
            let cleanup_status = child.terminate_and_reap(cleanup_deadline).await?;
            exit_status.get_or_insert(cleanup_status);
            if stdout_result.is_none() {
                stdout_result = Some(
                    (&mut stdout_task)
                        .await
                        .map_err(|_| "bootstrap stdout drain task failed".to_string())?,
                );
            }
            if stderr_result.is_none() {
                stderr_result = Some(
                    (&mut stderr_task)
                        .await
                        .map_err(|_| "bootstrap stderr drain task failed".to_string())?,
                );
            }
            Ok::<(), String>(())
        };
        let cleanup_result = tokio::time::timeout_at(cleanup_deadline, cleanup).await;
        let stdout_eof = stdout_result
            .as_ref()
            .and_then(|result| result.as_ref().ok())
            .is_some_and(|receipt| receipt.eof);
        let stderr_eof = stderr_result
            .as_ref()
            .and_then(|result| result.as_ref().ok())
            .is_some_and(|receipt| receipt.eof);
        containment_empty = matches!(cleanup_result, Ok(Ok(())))
            && exit_status.is_some()
            && stdout_eof
            && stderr_eof;
        let stdout_bytes = stdout_result
            .as_ref()
            .and_then(|result| result.as_ref().ok())
            .map_or(0, |receipt| receipt.bytes_read);
        let stderr_bytes = stderr_result
            .as_ref()
            .and_then(|result| result.as_ref().ok())
            .map_or(0, |receipt| receipt.bytes_read);
        log_debug(&format!(
            "[WARDIAN-DEBUG] Bootstrap cleanup receipt: exit_observed={}, stdout_eof={} ({} bytes), stderr_eof={} ({} bytes), containment_empty={containment_empty}",
            exit_status.is_some(), stdout_eof, stdout_bytes, stderr_eof, stderr_bytes
        ));
        if !containment_empty {
            let receipt = BootstrapProcessReceipt {
                child_pid,
                session_id: None,
                stderr_bytes,
                exit_code: exit_status
                    .as_ref()
                    .and_then(std::process::ExitStatus::code)
                    .unwrap_or(-1),
                exit_observed: exit_status.is_some(),
                exit_success: exit_status
                    .as_ref()
                    .is_some_and(std::process::ExitStatus::success),
                stdout_eof,
                stderr_eof,
                containment_empty,
                work_complete: false,
                timed_out,
            };
            persist_bootstrap_receipt(operation, &receipt)?;
            return Err(
                "Provider bootstrap cleanup could not verify child and stream completion."
                    .to_string(),
            );
        }
        child.release_after_verified_exit();
        if let Some(credentials) = credentials.as_mut() {
            credentials.confirm_closed(true)?;
        }
    }

    let status = exit_status.ok_or_else(|| "bootstrap child exit was not observed".to_string())?;
    let stdout_receipt = stdout_result
        .and_then(Result::ok)
        .ok_or_else(|| "bootstrap stdout did not reach EOF".to_string())?;
    let stderr_receipt = stderr_result
        .and_then(Result::ok)
        .ok_or_else(|| "bootstrap stderr did not reach EOF".to_string())?;
    log_debug(&format!(
        "[WARDIAN-DEBUG] Bootstrap closure receipt: exit={}, stdout_eof={}, stdout_bytes={}, stdout_retained={}, stderr_eof={}, stderr_bytes={}, stderr_retained={}, containment_empty={containment_empty}",
        status.code().unwrap_or(-1),
        stdout_receipt.eof,
        stdout_receipt.bytes_read,
        stdout_receipt.retained_output.len(),
        stderr_receipt.eof,
        stderr_receipt.bytes_read,
        stderr_receipt.retained_output.len()
    ));
    let receipt = BootstrapProcessReceipt {
        child_pid,
        session_id: stdout_receipt.session_id,
        stderr_bytes: stderr_receipt.bytes_read,
        exit_code: status.code().unwrap_or(-1),
        exit_observed: true,
        exit_success: status.success(),
        stdout_eof: stdout_receipt.eof,
        stderr_eof: stderr_receipt.eof,
        containment_empty,
        work_complete,
        timed_out,
    };
    persist_bootstrap_receipt(operation, &receipt)?;
    Ok(receipt)
}

fn bootstrap_output_session_id(provider_name: &str, output: &str) -> Option<String> {
    output.lines().find_map(|line| {
        let parsed: serde_json::Value = serde_json::from_str(line.trim()).ok()?;
        match provider_name {
            "codex"
                if parsed.get("type").and_then(|value| value.as_str())
                    == Some("thread.started") =>
            {
                parsed
                    .get("thread_id")
                    .and_then(|value| value.as_str())
                    .map(str::to_string)
            }
            "opencode" => parsed
                .get("sessionID")
                .and_then(|value| value.as_str())
                .filter(|value| value.starts_with("ses_") && value.len() > "ses_".len())
                .map(str::to_string),
            _ => None,
        }
    })
}

/// Apply provider identity and an optional memory capability using the setting
/// decision captured for this launch. The helper intentionally does not reread
/// mutable settings after the launch boundary has been crossed.
pub(crate) fn apply_headless_identity_env(
    cmd: &mut tokio::process::Command,
    process_session_id: &str,
    memory_agent_id: Option<&str>,
    memory_enabled: bool,
) -> Option<wardian_core::memory::MemoryCapabilityLease> {
    if let Some(home) = crate::utils::fs::get_wardian_home() {
        cmd.env("WARDIAN_HOME", home);
    }
    let effective_session_id = memory_agent_id.unwrap_or(process_session_id);
    if !effective_session_id.trim().is_empty() {
        cmd.env("WARDIAN_SESSION_ID", effective_session_id);
    }
    if let Some(memory_agent_id) = memory_agent_id {
        if !memory_enabled {
            return None;
        }
        let capability = super::issue_memory_capability(memory_agent_id);
        if let Some(capability) = capability.as_ref() {
            cmd.env(
                wardian_core::memory::MEMORY_CAPABILITY_ENV,
                capability.token(),
            );
        }
        capability
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use std::ffi::OsString;
    use std::path::{Path, PathBuf};

    struct TestWardianHome {
        _lock: tokio::sync::MutexGuard<'static, ()>,
        previous_home: Option<OsString>,
        _home: tempfile::TempDir,
    }

    impl TestWardianHome {
        fn new() -> Self {
            Self::from_guard(crate::utils::wardian_test_env_lock())
        }

        async fn new_async() -> Self {
            Self::from_guard(crate::utils::wardian_test_env_lock_async().await)
        }

        fn from_guard(lock: tokio::sync::MutexGuard<'static, ()>) -> Self {
            let home = tempfile::tempdir().expect("temp wardian home");
            let previous_home = std::env::var_os("WARDIAN_HOME");
            std::fs::create_dir_all(home.path().join("settings")).expect("settings directory");
            std::fs::write(
                home.path().join("settings/app.json"),
                r#"{"schema_version":2,"overrides":{"memory_enabled":true}}"#,
            )
            .expect("enable memory for headless tests");
            std::env::set_var("WARDIAN_HOME", home.path());
            Self {
                _lock: lock,
                previous_home,
                _home: home,
            }
        }
    }

    impl Drop for TestWardianHome {
        fn drop(&mut self) {
            match self.previous_home.take() {
                Some(value) => std::env::set_var("WARDIAN_HOME", value),
                None => std::env::remove_var("WARDIAN_HOME"),
            }
        }
    }

    fn node_available() -> bool {
        std::process::Command::new(if cfg!(windows) { "node.exe" } else { "node" })
            .arg("--version")
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false)
    }

    fn codex_completed_shell_commands(raw: &str) -> Vec<String> {
        raw.lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .filter_map(|event| {
                (event.get("type").and_then(Value::as_str) == Some("item.completed"))
                    .then(|| event.get("item"))
                    .flatten()
                    .filter(|item| {
                        item.get("type").and_then(Value::as_str) == Some("command_execution")
                    })
                    .and_then(|item| item.get("command").and_then(Value::as_str))
                    .map(str::to_owned)
            })
            .collect()
    }

    #[test]
    fn codex_completed_shell_commands_ignore_started_events() {
        let raw = concat!(
            r#"{"type":"item.started","item":{"id":"item-1","type":"command_execution","command":"Write-Output started"}}"#,
            "\n",
            r#"{"type":"item.completed","item":{"id":"item-1","type":"command_execution","command":"Write-Output completed"}}"#,
        );

        assert_eq!(
            codex_completed_shell_commands(raw),
            vec!["Write-Output completed"]
        );
    }

    #[test]
    fn codex_completed_shell_commands_ignore_uncompleted_function_calls() {
        let raw = r#"{"type":"response_item","payload":{"type":"function_call","name":"exec_command","arguments":"{\"cmd\":\"Write-Output from-tool\"}"}}"#;

        assert!(codex_completed_shell_commands(raw).is_empty());
    }

    #[tokio::test]
    async fn mock_headless_projects_memory_before_the_provider_process() {
        if !node_available() {
            return;
        }
        let test_home = TestWardianHome::new_async().await;
        let workspace = tempfile::tempdir().expect("temp workspace");
        let session_id = "headless-memory-agent";
        let store = wardian_core::memory::MemoryStore::from_default_home().unwrap();
        store
            .save(
                &wardian_core::memory::MemoryActor::Operator,
                wardian_core::memory::SaveMemoryRequest {
                    agent_id: session_id.into(),
                    workspace: Some(workspace.path().to_string_lossy().to_string()),
                    kind: wardian_core::memory::MemoryKind::Stable,
                    text: "Prefer metric units".into(),
                    evidence_excerpt: "The user explicitly selected metric units.".into(),
                    sources: vec![],
                    idempotency_key: None,
                },
            )
            .unwrap();
        let previous_scenario = std::env::var_os("WARDIAN_MOCK_SCENARIO");
        std::env::set_var("WARDIAN_MOCK_SCENARIO", "headless");
        let config = AgentConfig {
            session_id: session_id.into(),
            folder: workspace.path().to_string_lossy().to_string(),
            provider: "mock".into(),
            ..Default::default()
        };

        let result = run_headless_with_options(HeadlessRunOptions {
            cwd: workspace.path(),
            prompt: "Report ready.",
            wardian_session_id: session_id,
            memory_agent_id: Some(session_id),
            resume_session: None,
            output_format: "json",
            provider_name: "mock",
            config_override: Some(&config),
            timeout: Duration::from_secs(10),
            lease_owner: None,
            cancellation_marker: None,
        })
        .await;
        match previous_scenario {
            Some(value) => std::env::set_var("WARDIAN_MOCK_SCENARIO", value),
            None => std::env::remove_var("WARDIAN_MOCK_SCENARIO"),
        }
        result.expect("mock headless run");

        let instructions = std::fs::read_to_string(
            test_home
                ._home
                .path()
                .join("agents")
                .join(session_id)
                .join("habitat")
                .join("AGENTS.md"),
        )
        .expect("generated habitat instructions");
        assert!(instructions.contains("Use `wardian memory save`"));
        assert!(instructions.contains("Prefer metric units"));
        let loaded = store
            .list_events(&wardian_core::memory::MemoryActor::Operator, session_id)
            .unwrap()
            .into_iter()
            .find(|event| event.action == "loaded")
            .expect("memory loaded event");
        assert!(loaded.payload["injected_context"]
            .as_str()
            .is_some_and(|text| text.contains("Prefer metric units")));
    }

    #[tokio::test]
    async fn failed_headless_start_does_not_advance_the_memory_checkpoint() {
        if !node_available() {
            return;
        }
        let _test_home = TestWardianHome::new_async().await;
        let workspace = tempfile::tempdir().expect("temp workspace");
        let session_id = "headless-memory-failure";
        let process_key = "headless-memory-failure-process";
        let store = wardian_core::memory::MemoryStore::from_default_home().unwrap();
        store
            .save(
                &wardian_core::memory::MemoryActor::Operator,
                wardian_core::memory::SaveMemoryRequest {
                    agent_id: session_id.into(),
                    workspace: Some(workspace.path().to_string_lossy().to_string()),
                    kind: wardian_core::memory::MemoryKind::Stable,
                    text: "Keep the full startup context".into(),
                    evidence_excerpt: "Durable test evidence".into(),
                    sources: vec![],
                    idempotency_key: None,
                },
            )
            .unwrap();
        let previous_scenario = std::env::var_os("WARDIAN_MOCK_SCENARIO");
        std::env::set_var("WARDIAN_MOCK_SCENARIO", "headless_failure");
        let config = AgentConfig {
            session_id: session_id.into(),
            folder: workspace.path().to_string_lossy().to_string(),
            provider: "mock".into(),
            ..Default::default()
        };

        let result = run_headless_with_options(HeadlessRunOptions {
            cwd: workspace.path(),
            prompt: "Fail before accepting the context.",
            wardian_session_id: session_id,
            memory_agent_id: Some(session_id),
            resume_session: Some(process_key),
            output_format: "json",
            provider_name: "mock",
            config_override: Some(&config),
            timeout: Duration::from_secs(10),
            lease_owner: None,
            cancellation_marker: None,
        })
        .await;
        match previous_scenario {
            Some(value) => std::env::set_var("WARDIAN_MOCK_SCENARIO", value),
            None => std::env::remove_var("WARDIAN_MOCK_SCENARIO"),
        }

        assert!(result.is_err());
        assert!(store
            .list_events(&wardian_core::memory::MemoryActor::Operator, session_id,)
            .unwrap()
            .into_iter()
            .all(|event| event.action != "loaded"));
        store
            .save(
                &wardian_core::memory::MemoryActor::Operator,
                wardian_core::memory::SaveMemoryRequest {
                    agent_id: session_id.into(),
                    workspace: Some(workspace.path().to_string_lossy().to_string()),
                    kind: wardian_core::memory::MemoryKind::Current,
                    text: "Include the changed context on retry".into(),
                    evidence_excerpt: "A later durable change makes an advanced cursor observable."
                        .into(),
                    sources: vec![],
                    idempotency_key: None,
                },
            )
            .unwrap();
        let retry = store
            .compile_brief(
                &wardian_core::memory::MemoryActor::agent(session_id),
                session_id,
                Some(&workspace.path().to_string_lossy()),
                "mock",
                process_key,
                true,
                8_000,
            )
            .unwrap();
        assert_eq!(retry.kind, wardian_core::memory::MemoryBriefKind::Fresh);
        assert!(retry.context_text.contains("Keep the full startup context"));
        assert!(retry
            .context_text
            .contains("Include the changed context on retry"));
    }

    #[tokio::test]
    async fn fresh_worker_uses_registered_agent_as_memory_subject() {
        if !node_available() {
            return;
        }
        let test_home = TestWardianHome::new_async().await;
        let workspace = tempfile::tempdir().expect("temp workspace");
        let agent_id = "registered-memory-agent";
        let worker_id = "automation-bg-run-node";
        let store = wardian_core::memory::MemoryStore::from_default_home().unwrap();
        store
            .save(
                &wardian_core::memory::MemoryActor::Operator,
                wardian_core::memory::SaveMemoryRequest {
                    agent_id: agent_id.into(),
                    workspace: Some(workspace.path().to_string_lossy().to_string()),
                    kind: wardian_core::memory::MemoryKind::Stable,
                    text: "Use the registered agent memory".into(),
                    evidence_excerpt: "The user established a durable project convention.".into(),
                    sources: vec![],
                    idempotency_key: None,
                },
            )
            .unwrap();
        let previous_scenario = std::env::var_os("WARDIAN_MOCK_SCENARIO");
        std::env::set_var("WARDIAN_MOCK_SCENARIO", "headless");
        let config = AgentConfig {
            session_id: agent_id.into(),
            folder: workspace.path().to_string_lossy().to_string(),
            provider: "mock".into(),
            ..Default::default()
        };

        let result = run_headless_with_options(HeadlessRunOptions {
            cwd: workspace.path(),
            prompt: "Report ready.",
            wardian_session_id: worker_id,
            memory_agent_id: Some(agent_id),
            resume_session: None,
            output_format: "json",
            provider_name: "mock",
            config_override: Some(&config),
            timeout: Duration::from_secs(10),
            lease_owner: None,
            cancellation_marker: None,
        })
        .await;
        match previous_scenario {
            Some(value) => std::env::set_var("WARDIAN_MOCK_SCENARIO", value),
            None => std::env::remove_var("WARDIAN_MOCK_SCENARIO"),
        }
        result.expect("mock headless run");

        let instructions = std::fs::read_to_string(
            test_home
                ._home
                .path()
                .join("agents")
                .join(worker_id)
                .join("habitat")
                .join("AGENTS.md"),
        )
        .expect("generated worker habitat instructions");
        assert!(instructions.contains("Use the registered agent memory"));
        assert!(store
            .list_events(&wardian_core::memory::MemoryActor::Operator, agent_id,)
            .unwrap()
            .into_iter()
            .any(|event| event.action == "loaded"));
        assert!(store
            .list_events(&wardian_core::memory::MemoryActor::Operator, worker_id,)
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn ephemeral_worker_receives_no_durable_memory_surface() {
        if !node_available() {
            return;
        }
        let test_home = TestWardianHome::new_async().await;
        let workspace = tempfile::tempdir().expect("temp workspace");
        let worker_id = "automation-bg-temporary-node";
        let previous_scenario = std::env::var_os("WARDIAN_MOCK_SCENARIO");
        std::env::set_var("WARDIAN_MOCK_SCENARIO", "headless");
        let config = AgentConfig {
            session_id: worker_id.into(),
            folder: workspace.path().to_string_lossy().to_string(),
            provider: "mock".into(),
            ..Default::default()
        };

        let result = run_headless_with_options(HeadlessRunOptions {
            cwd: workspace.path(),
            prompt: "Report ready.",
            wardian_session_id: worker_id,
            memory_agent_id: None,
            resume_session: None,
            output_format: "json",
            provider_name: "mock",
            config_override: Some(&config),
            timeout: Duration::from_secs(10),
            lease_owner: None,
            cancellation_marker: None,
        })
        .await;
        match previous_scenario {
            Some(value) => std::env::set_var("WARDIAN_MOCK_SCENARIO", value),
            None => std::env::remove_var("WARDIAN_MOCK_SCENARIO"),
        }
        result.expect("mock headless run");

        let instructions = std::fs::read_to_string(
            test_home
                ._home
                .path()
                .join("agents")
                .join(worker_id)
                .join("habitat")
                .join("AGENTS.md"),
        )
        .expect("generated worker habitat instructions");
        assert!(!instructions.contains("## Wardian memory"));
        assert!(!instructions.contains("wardian memory save"));
        let store = wardian_core::memory::MemoryStore::from_default_home().unwrap();
        assert!(store
            .list_active(
                &wardian_core::memory::MemoryActor::Operator,
                worker_id,
                Some(&workspace.path().to_string_lossy()),
            )
            .unwrap()
            .is_empty());
        assert!(store
            .list_events(&wardian_core::memory::MemoryActor::Operator, worker_id)
            .unwrap()
            .is_empty());
        let connection = rusqlite::Connection::open(test_home._home.path().join("memory.db"))
            .expect("memory database");
        let capabilities: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM memory_process_capabilities WHERE agent_id=?1",
                [worker_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(capabilities, 0);
    }

    #[cfg(windows)]
    #[tokio::test]
    #[ignore = "requires a logged-in Codex CLI; run with cargo test -p Wardian -- --ignored real_codex_headless_projected_home_runs_shell_on_windows"]
    async fn real_codex_headless_projected_home_runs_shell_on_windows() {
        let test_wardian_home = TestWardianHome::new_async().await;
        let settings_path = test_wardian_home
            ._home
            .path()
            .join("settings")
            .join("shell.json");
        std::fs::create_dir_all(settings_path.parent().expect("settings parent"))
            .expect("create settings directory");
        std::fs::write(
            settings_path,
            r#"{
              "schema_version": 2,
              "overrides": {
                "codex_runtime_policy": {
                  "sandbox_mode": "danger-full-access",
                  "approval_policy": "never",
                  "full_auto": false,
                  "trust_workspaces": true
                }
              }
            }"#,
        )
        .expect("write unrestricted Codex policy");

        let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("workspace parent")
            .to_path_buf();
        let expected_path = workspace.to_string_lossy().to_string();
        let session_id = "headless-codex-runtime-smoke";
        let marker_file_name = "wardian-headless-codex-smoke.txt";
        let config = AgentConfig {
            session_id: session_id.to_string(),
            folder: expected_path.clone(),
            provider: "codex".to_string(),
            provider_config: wardian_core::models::ProviderConfig::Codex(
                wardian_core::models::CodexProviderConfig::default(),
            ),
            ..Default::default()
        };

        let result = tokio::time::timeout(
            Duration::from_secs(120),
            run_headless_with_options(HeadlessRunOptions {
                cwd: &workspace,
                prompt: concat!(
                    "Use the shell tool exactly once. In that one shell command, create a file ",
                    "named wardian-headless-codex-smoke.txt in $env:CODEX_HOME whose contents ",
                    "are the output of Get-Location. Then respond with only the returned absolute path."
                ),
                wardian_session_id: session_id,
                memory_agent_id: None,
                resume_session: None,
                output_format: "json",
                provider_name: "codex",
                config_override: Some(&config),
                timeout: Duration::from_secs(110),
                lease_owner: None,
                cancellation_marker: None,
            }),
        )
        .await
        .expect("headless Codex smoke test timed out")
        .expect("headless Codex execution");

        let response = result["response"].as_str().expect("Codex response text");
        assert!(
            !response.trim().is_empty(),
            "expected Codex to return a non-empty response"
        );
        let raw = result["raw"].as_str().expect("Codex JSON-lines output");
        let shell_commands = codex_completed_shell_commands(raw);
        assert_eq!(
            shell_commands.len(),
            1,
            "expected exactly one completed Codex shell command, got {shell_commands:?}"
        );
        assert!(
            shell_commands[0].contains("CODEX_HOME")
                && shell_commands[0].contains(marker_file_name),
            "expected the Codex shell command to use CODEX_HOME and create {marker_file_name:?}, got {:?}",
            shell_commands[0]
        );

        let projected_codex_home = test_wardian_home
            ._home
            .path()
            .join("agents")
            .join(session_id)
            .join("habitat")
            .join(".codex");
        assert!(
            projected_codex_home.join("auth.json").is_file(),
            "headless Codex should run from Wardian's projected CODEX_HOME"
        );
        let marker = std::fs::read_to_string(projected_codex_home.join(marker_file_name))
            .expect("Codex shell should create a marker under its projected CODEX_HOME");
        assert!(
            marker.contains(&expected_path),
            "expected projected-home marker to contain {expected_path:?}, got {marker:?}"
        );
    }

    #[cfg(windows)]
    async fn wait_for_descendant_pid(marker: &Path) -> u32 {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let Ok(value) = std::fs::read_to_string(marker) {
                    if let Ok(pid) = value.trim().parse::<u32>() {
                        return pid;
                    }
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("wrapper recorded descendant PID")
    }

    async fn assert_pending<F: std::future::Future>(mut future: std::pin::Pin<&mut F>) {
        let observed =
            std::future::poll_fn(|cx| std::task::Poll::Ready(future.as_mut().poll(cx))).await;
        assert!(
            observed.is_pending(),
            "execution observation completed too early"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn headless_wait_observes_deadline_before_the_next_process_poll() {
        let started = tokio::time::Instant::now();
        let mut timing = HeadlessWaitTiming::new(
            Duration::from_millis(25),
            HEADLESS_PROCESS_POLL_INTERVAL,
            HEADLESS_LEASE_HEARTBEAT_INTERVAL,
        );
        let event = timing.next_event(None, false);
        tokio::pin!(event);
        assert_pending(event.as_mut()).await;
        tokio::time::advance(Duration::from_millis(24)).await;
        assert_pending(event.as_mut()).await;
        tokio::time::advance(Duration::from_millis(1)).await;
        assert_eq!(event.await, HeadlessWaitEvent::Deadline);
        assert_eq!(started.elapsed(), Duration::from_millis(25));
    }

    #[tokio::test(start_paused = true)]
    async fn headless_wait_observes_cancellation_at_the_next_process_poll() {
        let temp = tempfile::tempdir().expect("cancellation marker directory");
        let marker = temp.path().join("cancel.marker");
        let started = tokio::time::Instant::now();
        let mut timing = HeadlessWaitTiming::new(
            Duration::from_secs(5),
            HEADLESS_PROCESS_POLL_INTERVAL,
            HEADLESS_LEASE_HEARTBEAT_INTERVAL,
        );
        assert_eq!(
            timing.next_event(Some(&marker), false).await,
            HeadlessWaitEvent::ProcessPoll
        );
        std::fs::write(&marker, "cancelled").expect("write cancellation marker");
        let event = timing.next_event(Some(&marker), false);
        tokio::pin!(event);
        assert_pending(event.as_mut()).await;
        tokio::time::advance(HEADLESS_PROCESS_POLL_INTERVAL - Duration::from_millis(1)).await;
        assert_pending(event.as_mut()).await;
        tokio::time::advance(Duration::from_millis(1)).await;
        assert_eq!(event.await, HeadlessWaitEvent::Cancelled);
        assert_eq!(started.elapsed(), HEADLESS_PROCESS_POLL_INTERVAL * 2);
    }

    #[tokio::test]
    async fn headless_wait_heartbeats_an_active_conversation_lease() {
        if !node_available() {
            return;
        }
        let _home = TestWardianHome::new_async().await;
        let now = chrono::Utc::now();
        let original_expires_at = (now + chrono::Duration::minutes(2)).to_rfc3339();
        let lease = wardian_core::conversation_lease::ConversationLease {
            agent_id: "agent-1".to_string(),
            provider: "mock".to_string(),
            resume_session: "provider-session-1".to_string(),
            owner_kind: "message_delivery".to_string(),
            owner_id: "interaction-1".to_string(),
            acquisition_id: "test-acquisition-1".to_string(),
            owner_node_id: None,
            mode: "background_resume".to_string(),
            started_at: now.to_rfc3339(),
            heartbeat_at: now.to_rfc3339(),
            expires_at: original_expires_at.clone(),
        };
        wardian_core::conversation_lease::acquire_lease(lease.clone(), &now.to_rfc3339())
            .expect("lease");
        let owner = lease.owner();

        let mut command = crate::utils::process::new_headless_command(if cfg!(windows) {
            "node.exe"
        } else {
            "node"
        });
        command
            .arg("-e")
            .arg("setTimeout(() => process.exit(0), 75)");
        command.kill_on_drop(true);
        let mut child = command.spawn().expect("spawn node");

        let status = wait_for_headless_child_with_intervals(
            &mut child,
            "mock",
            Duration::from_secs(1),
            Some(&owner),
            None,
            Duration::from_millis(5),
            Duration::from_millis(10),
        )
        .await
        .expect("headless child completed");

        assert!(status.success());
        let renewed = wardian_core::conversation_lease::load_leases()
            .into_iter()
            .next()
            .expect("renewed lease");
        assert_ne!(renewed.expires_at, original_expires_at);
        wardian_core::conversation_lease::release_owner_persisted(
            &owner.owner_kind,
            &owner.owner_id,
        )
        .expect("release lease");
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn headless_timeout_terminates_shell_descendants_before_releasing_lease() {
        if !node_available() {
            return;
        }

        let temp = tempfile::tempdir().expect("temp process marker directory");
        let marker = temp.path().join("headless-descendant.pid");
        let marker_arg = marker.to_string_lossy().to_string();
        let child_script = r#"
            const { spawn } = require('node:child_process');
            const { writeFileSync } = require('node:fs');
            const descendant = spawn(process.execPath, ['-e', 'setInterval(() => {}, 1000)'], {
                stdio: 'ignore',
            });
            writeFileSync(process.argv[1], String(descendant.pid));
            setInterval(() => {}, 1000);
        "#;

        let mut command = crate::utils::process::new_headless_command("node.exe");
        command.arg("-e").arg(child_script).arg(marker_arg);
        command.kill_on_drop(true);
        let mut child = command.spawn().expect("spawn headless wrapper");

        let descendant_pid = wait_for_descendant_pid(&marker).await;

        let error = wait_for_headless_child_with_intervals(
            &mut child,
            "mock",
            Duration::from_millis(25),
            None,
            None,
            Duration::from_millis(5),
            Duration::from_secs(1),
        )
        .await
        .expect_err("long-running wrapper should time out");
        assert!(error.contains("exceeded"));

        for _ in 0..40 {
            if !crate::utils::process::process_exists(descendant_pid) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(
            !crate::utils::process::process_exists(descendant_pid),
            "headless timeout must terminate a shell/provider descendant"
        );
    }

    #[tokio::test]
    async fn run_owned_cancellation_marker_terminates_and_acknowledges_child() {
        if !node_available() {
            return;
        }
        let temp = tempfile::tempdir().expect("temporary cancellation directory");
        let marker = temp.path().join("cancel.marker");
        std::fs::write(&marker, "cancelled").expect("write cancellation marker");
        let mut command = crate::utils::process::new_headless_command(if cfg!(windows) {
            "node.exe"
        } else {
            "node"
        });
        command.arg("-e").arg("setInterval(() => {}, 1000)");
        command.kill_on_drop(true);
        let mut child = command.spawn().expect("spawn cancellable child");

        let error = wait_for_headless_child_with_intervals(
            &mut child,
            "mock",
            Duration::from_secs(5),
            None,
            Some(&marker),
            Duration::from_millis(5),
            Duration::from_secs(1),
        )
        .await
        .expect_err("owned marker must cancel the child");

        assert_eq!(error.kind(), HeadlessRunErrorKind::Cancelled);
        assert!(child.try_wait().expect("reaped child").is_some());
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn dropped_headless_run_terminates_descendants_before_lease_cleanup() {
        if !node_available() {
            return;
        }

        let temp = tempfile::tempdir().expect("temp process marker directory");
        let marker = temp.path().join("cancelled-headless-descendant.pid");
        let marker_arg = marker.to_string_lossy().to_string();
        let child_script = r#"
            const { spawn } = require('node:child_process');
            const { writeFileSync } = require('node:fs');
            const descendant = spawn(process.execPath, ['-e', 'setInterval(() => {}, 1000)'], {
                stdio: 'ignore',
            });
            writeFileSync(process.argv[1], String(descendant.pid));
            setInterval(() => {}, 1000);
        "#;

        let mut command = crate::utils::process::new_headless_command("node.exe");
        command.arg("-e").arg(child_script).arg(marker_arg);
        command.kill_on_drop(true);
        let mut child = command.spawn().expect("spawn cancellable headless wrapper");
        let tree_guard = HeadlessProcessTreeGuard::new(child.id());

        let descendant_pid = wait_for_descendant_pid(&marker).await;

        drop(tree_guard);
        let _ = child.wait().await;
        for _ in 0..40 {
            if !crate::utils::process::process_exists(descendant_pid) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(
            !crate::utils::process::process_exists(descendant_pid),
            "cancelling a headless run must terminate its provider descendants"
        );
    }

    #[test]
    fn codex_bootstrap_uses_thread_started_from_current_output() {
        let output = concat!(
            "{\"type\":\"thread.started\",\"thread_id\":\"019db2f3-22de-7861-8bc6-1b86db1686db\"}\n",
            "{\"type\":\"turn.completed\"}\n"
        );
        assert_eq!(
            bootstrap_output_session_id("codex", output).as_deref(),
            Some("019db2f3-22de-7861-8bc6-1b86db1686db")
        );
    }

    #[test]
    fn codex_bootstrap_does_not_infer_an_id_without_thread_started() {
        assert_eq!(
            bootstrap_output_session_id("codex", "{\"type\":\"turn.completed\"}\n"),
            None
        );
    }

    #[test]
    fn opencode_bootstrap_requires_a_ses_id_from_current_output() {
        assert_eq!(
            bootstrap_output_session_id(
                "opencode",
                "{\"type\":\"text\",\"sessionID\":\"ses_exact\"}\n"
            )
            .as_deref(),
            Some("ses_exact")
        );
        assert_eq!(
            bootstrap_output_session_id("opencode", "{\"type\":\"text\"}\n"),
            None
        );
    }

    #[test]
    fn bootstrap_session_prompt_uses_intro_prompt_for_providers_that_need_bootstrap() {
        assert_eq!(session_bootstrap_prompt(), "Introduce yourself");
    }

    #[test]
    fn codex_materialization_writes_a_resumable_local_rollout() {
        let home = tempfile::tempdir().expect("Codex home");
        let cwd = Path::new("D:/Development/Wardian");

        let session_id =
            materialize_codex_session_rollout(home.path(), cwd).expect("materialize Codex rollout");
        let rollout_path =
            codex_session_file_path_in(home.path(), &session_id).expect("locate Codex rollout");
        let rollout: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(rollout_path).expect("read Codex rollout"),
        )
        .expect("parse Codex rollout");

        assert_eq!(rollout["type"], "session_meta");
        assert_eq!(rollout["payload"]["id"], session_id);
        assert_eq!(rollout["payload"]["session_id"], session_id);
        assert_eq!(rollout["payload"]["cwd"], cwd.to_string_lossy().as_ref());
    }

    #[test]
    fn codex_fresh_headless_args_omit_resume_subcommand() {
        let provider = crate::providers::ProviderFactory::resolve("codex").unwrap();
        let args = headless_provider_args(
            "codex",
            provider.as_ref(),
            Path::new("D:/Development/Wardian"),
            "task",
            "json",
            None,
            None,
        );

        assert!(args.contains(&"exec".to_string()));
        assert!(!args.contains(&"resume".to_string()));
        assert!(!args.contains(&"ses_source".to_string()));
        let exec_index = args.iter().position(|arg| arg == "exec").unwrap();
        let skip_index = args
            .iter()
            .position(|arg| arg == "--skip-git-repo-check")
            .expect("headless Codex defaults to the repository bypass");
        assert!(skip_index > exec_index);
    }

    #[cfg(windows)]
    #[test]
    fn temporary_codex_headless_worker_applies_unrestricted_runtime_policy() {
        let test_wardian_home = TestWardianHome::new();
        let settings_path = test_wardian_home
            ._home
            .path()
            .join("settings")
            .join("shell.json");
        std::fs::create_dir_all(settings_path.parent().expect("settings parent"))
            .expect("create settings directory");
        std::fs::write(
            settings_path,
            r#"{
              "schema_version": 2,
              "overrides": {
                "codex_runtime_policy": {
                  "sandbox_mode": "danger-full-access",
                  "approval_policy": "never"
                }
              }
            }"#,
        )
        .expect("write unrestricted Codex policy");

        let cwd = Path::new("D:/Development/Wardian");
        let config = effective_headless_provider_config("codex", cwd, "agent-1", None, None, None)
            .expect("temporary Codex worker config");
        let provider = crate::providers::ProviderFactory::resolve("codex").unwrap();
        let args = headless_provider_args(
            "codex",
            provider.as_ref(),
            cwd,
            "task",
            "json",
            None,
            Some(&config),
        );

        let exec_index = args.iter().position(|arg| arg == "exec").unwrap();
        assert!(
            args[..exec_index].contains(&"--dangerously-bypass-approvals-and-sandbox".to_string())
        );
        assert!(!args.contains(&"--sandbox".to_string()));
        assert!(!args.contains(&"--ask-for-approval".to_string()));
        assert_eq!(config.folder, cwd.to_string_lossy());
    }

    #[test]
    fn codex_headless_args_keep_exec_only_flags_after_exec() {
        let provider = crate::providers::ProviderFactory::resolve("codex").unwrap();
        let config = AgentConfig {
            provider: "codex".into(),
            provider_config: wardian_core::models::ProviderConfig::Codex(
                wardian_core::models::CodexProviderConfig {
                    skip_git_repo_check: Some(true),
                    ephemeral: Some(true),
                    ..Default::default()
                },
            ),
            ..Default::default()
        };
        let args = headless_provider_args(
            "codex",
            provider.as_ref(),
            Path::new("/workspace"),
            "task",
            "json",
            None,
            Some(&config),
        );

        let exec_index = args.iter().position(|arg| arg == "exec").unwrap();
        let skip_index = args
            .iter()
            .position(|arg| arg == "--skip-git-repo-check")
            .unwrap();
        let ephemeral_index = args.iter().position(|arg| arg == "--ephemeral").unwrap();
        assert!(skip_index > exec_index);
        assert!(ephemeral_index > exec_index);
        assert!(!args[..exec_index].contains(&"--skip-git-repo-check".to_string()));
        assert!(!args[..exec_index].contains(&"--ephemeral".to_string()));
        assert!(!args.contains(&"--no-alt-screen".to_string()));
    }

    #[cfg(windows)]
    #[test]
    fn node_headless_launch_uses_direct_process_args() {
        let args = vec![
            "provider.js".to_string(),
            "--cd".to_string(),
            "D:/Development/Wardian".to_string(),
            "exec".to_string(),
            "--json".to_string(),
            "first line\nsecond line".to_string(),
        ];

        let spec = headless_provider_launch("gemini", "node", &args).unwrap();

        assert_eq!(spec.executable, "node");
        assert_eq!(spec.args, args);
    }

    #[cfg(windows)]
    #[test]
    fn opencode_exe_headless_launch_uses_direct_process_args() {
        let args = vec![
            "run".to_string(),
            "--print-logs".to_string(),
            "first line\nsecond line".to_string(),
        ];

        let spec = headless_provider_launch("opencode", "C:/tools/opencode.exe", &args).unwrap();

        assert_eq!(spec.executable, "C:/tools/opencode.exe");
        assert_eq!(spec.args, args);
    }

    #[cfg(windows)]
    #[test]
    fn opencode_extensionless_headless_launch_uses_configured_shell_on_windows() {
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
        let args = vec![
            "run".to_string(),
            "--print-logs".to_string(),
            "first line\nsecond line".to_string(),
        ];

        let spec = headless_provider_launch("opencode", "C:/nvm4w/nodejs/opencode", &args)
            .expect("launch spec");

        assert_eq!(spec.executable, "pwsh.exe");
        assert_eq!(spec.args[0], "-NoProfile");
        assert_eq!(spec.args[1], "-Command");
        assert!(spec.args[2].contains("C:/nvm4w/nodejs/opencode"));
        assert!(spec.args[2].contains("first line\nsecond line"));
        assert!(!spec.args[2].contains("ComSpec"));

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
    fn claude_fresh_headless_args_omit_resume_flag() {
        let provider = crate::providers::ProviderFactory::resolve("claude").unwrap();
        let args = headless_provider_args(
            "claude",
            provider.as_ref(),
            Path::new("D:/Development/Wardian"),
            "task",
            "text",
            None,
            None,
        );

        assert!(args.contains(&"--print".to_string()));
        assert!(!args.contains(&"--resume".to_string()));
    }

    #[test]
    fn codex_managed_instructions_survive_disabled_memory_and_inherited_launches() {
        let test_home = TestWardianHome::new();
        let workspace = tempfile::tempdir().unwrap();
        let sources = [
            (
                test_home._home.path().join("common/AGENTS.md"),
                "COMMON_SENTINEL",
            ),
            (
                test_home._home.path().join("classes/Builder/AGENTS.md"),
                "CLASS_SENTINEL",
            ),
            (
                test_home._home.path().join("agents/owner/AGENTS.md"),
                "AGENT_SENTINEL",
            ),
            (
                test_home._home.path().join("agents/worker/AGENTS.md"),
                "WRONG_WORKER",
            ),
            (
                test_home
                    ._home
                    .path()
                    .join("agents/owner/habitat/AGENTS.md"),
                "UNTRUSTED_GENERATED_FILE",
            ),
            (workspace.path().join("AGENTS.md"), "WORKSPACE_ONLY"),
        ];
        for (path, text) in &sources {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        let owner = AgentConfig {
            session_id: "owner".into(),
            provider: "codex".into(),
            agent_class: "Builder".into(),
            model: Some("configured-model".into()),
            system_include_directories: Some(vec![workspace.path().to_string_lossy().into()]),
            provider_config: wardian_core::models::ProviderConfig::Codex(
                wardian_core::models::CodexProviderConfig {
                    reasoning_effort: Some("low".into()),
                    approval_policy: Some("on-request".into()),
                    ..Default::default()
                },
            ),
            ..Default::default()
        };
        let provider = CodexProvider::new();
        let memory = wardian_memory_instructions(Some("MEMORY_SENTINEL"));
        for inherited in [false, true] {
            let config = effective_headless_provider_config(
                "codex",
                workspace.path(),
                "worker",
                None,
                inherited.then_some(&owner),
                (!inherited).then_some(&owner),
            )
            .unwrap();
            assert_eq!(config.session_id, "owner");
            for mode in ["interactive", "exec", "app-server"] {
                for memory_context in [None, Some(memory.as_str())] {
                    let mut args = match mode {
                        "exec" => headless_provider_args(
                            "codex",
                            &provider,
                            workspace.path(),
                            "task",
                            "json",
                            None,
                            Some(&config),
                        ),
                        "app-server" => provider.shared_server_args(&config).unwrap(),
                        _ => provider.get_spawn_args(&config, false),
                    };
                    let original = args.clone();
                    provider
                        .insert_managed_instructions_arg(&mut args, &config, memory_context)
                        .unwrap();
                    let index = args
                        .iter()
                        .position(|arg| arg.starts_with("developer_instructions="))
                        .unwrap();
                    assert_eq!(args[index - 1], "-c");
                    let parsed = args[index].parse::<toml_edit::DocumentMut>().unwrap();
                    let text = parsed["developer_instructions"].as_str().unwrap();
                    let common = text.find("COMMON_SENTINEL").unwrap();
                    let class = text.find("CLASS_SENTINEL").unwrap();
                    let agent = text.find("AGENT_SENTINEL").unwrap();
                    assert!(common < class && class < agent);
                    for unexpected in ["WRONG_WORKER", "UNTRUSTED_GENERATED_FILE", "WORKSPACE_ONLY"]
                    {
                        assert!(!text.contains(unexpected));
                    }
                    assert_eq!(
                        text.matches("## Wardian memory").count(),
                        usize::from(memory_context.is_some())
                    );
                    assert_eq!(
                        text.matches("MEMORY_SENTINEL").count(),
                        usize::from(memory_context.is_some())
                    );
                    if mode == "exec" {
                        assert!(index < args.iter().position(|arg| arg == mode).unwrap());
                    } else if mode == "app-server" {
                        assert_eq!(args.first().map(String::as_str), Some("app-server"));
                        assert!(index > 1);
                    }
                    args.drain(index - 1..=index);
                    // Every original argument survives, including cwd, model, effort and task.
                    assert_eq!(args, original);
                }
            }
        }
        for (path, text) in &sources {
            assert_eq!(std::fs::read_to_string(path).unwrap(), *text);
        }
    }

    #[test]
    fn codex_headless_args_include_assigned_profile_config() {
        let provider = crate::providers::ProviderFactory::resolve("codex").unwrap();
        let config = AgentConfig {
            provider: "codex".into(),
            model: Some("gpt-test".into()),
            include_directories: Some(vec!["/workspace/docs".into()]),
            provider_config: wardian_core::models::ProviderConfig::Codex(
                wardian_core::models::CodexProviderConfig {
                    sandbox_mode: Some("workspace-write".into()),
                    approval_policy: Some("on-request".into()),
                    profile: Some("review".into()),
                    full_auto: Some(false),
                    ..Default::default()
                },
            ),
            ..Default::default()
        };

        let args = headless_provider_args(
            "codex",
            provider.as_ref(),
            Path::new("/workspace"),
            "task",
            "json",
            None,
            Some(&config),
        );

        assert!(args.contains(&"--model".to_string()));
        assert!(args.contains(&"gpt-test".to_string()));
        assert!(args.contains(&"--sandbox".to_string()));
        assert!(args.contains(&"workspace-write".to_string()));
        assert!(args.contains(&"--ask-for-approval".to_string()));
        assert!(args.contains(&"on-request".to_string()));
        assert!(args.contains(&"--profile".to_string()));
        assert!(args.contains(&"review".to_string()));
        assert!(args.contains(&"--add-dir".to_string()));
        assert!(args.contains(&"/workspace/docs".to_string()));
        assert!(args.contains(&"exec".to_string()));
        assert!(args.contains(&"--json".to_string()));
    }

    #[test]
    fn codex_bootstrap_places_global_flags_before_exec() {
        let config = AgentConfig {
            provider: "codex".into(),
            custom_args: Some("--custom-flag custom-value".into()),
            provider_config: wardian_core::models::ProviderConfig::Codex(
                wardian_core::models::CodexProviderConfig {
                    sandbox_mode: Some("danger-full-access".into()),
                    approval_policy: Some("never".into()),
                    ..Default::default()
                },
            ),
            ..Default::default()
        };
        let mut args = Vec::new();

        append_codex_bootstrap_args(&mut args, Path::new("/workspace"), Some(&config), None);

        let exec_index = args.iter().position(|arg| arg == "exec").unwrap();
        let policy_index = args
            .iter()
            .position(|arg| {
                arg == "--dangerously-bypass-approvals-and-sandbox" || arg == "--sandbox"
            })
            .unwrap();
        assert!(policy_index < exec_index);
        assert!(args[..exec_index].contains(&"--custom-flag".to_string()));
        assert!(args[..exec_index].contains(&"custom-value".to_string()));
        #[cfg(windows)]
        {
            assert!(args[..exec_index]
                .contains(&"--dangerously-bypass-approvals-and-sandbox".to_string()));
            assert!(!args.contains(&"--sandbox".to_string()));
            assert!(!args.contains(&"--ask-for-approval".to_string()));
        }
        assert!(args[exec_index + 1..].contains(&"--json".to_string()));
        assert!(args[exec_index + 1..].contains(&"--skip-git-repo-check".to_string()));
    }

    #[test]
    fn claude_headless_output_exposes_result_text_for_automations() {
        let output =
            r#"{"type":"result","session_id":"claude-session-1","result":"automation complete"}"#;

        let normalized = normalize_claude_headless_output(output, "json").unwrap();

        assert_eq!(normalized["session_id"], "claude-session-1");
        assert_eq!(normalized["response"], "automation complete");
        assert_eq!(normalized["raw"], output);
    }

    #[test]
    fn claude_headless_output_falls_back_to_assistant_message_content() {
        let output =
            r#"{"session_id":"claude-session-2","message":{"content":"assistant response"}}"#;

        let normalized = normalize_claude_headless_output(output, "text").unwrap();

        assert_eq!(normalized["text"], "assistant response");
    }

    #[test]
    fn claude_headless_args_include_assigned_profile_config_without_session_reuse() {
        let provider = crate::providers::ProviderFactory::resolve("claude").unwrap();
        let config = AgentConfig {
            session_id: "visible-agent".into(),
            provider: "claude".into(),
            model: Some("claude-test".into()),
            include_directories: Some(vec!["/workspace/docs".into()]),
            provider_config: wardian_core::models::ProviderConfig::Claude(
                wardian_core::models::ClaudeProviderConfig {
                    permission_mode: Some("acceptEdits".into()),
                    max_turns: Some(4),
                    ..Default::default()
                },
            ),
            ..Default::default()
        };

        let args = headless_provider_args(
            "claude",
            provider.as_ref(),
            Path::new("/workspace"),
            "task",
            "text",
            None,
            Some(&config),
        );

        assert!(args.contains(&"--model".to_string()));
        assert!(args.contains(&"claude-test".to_string()));
        assert!(args.contains(&"--add-dir".to_string()));
        assert!(args.contains(&"/workspace/docs".to_string()));
        assert!(args.contains(&"--permission-mode".to_string()));
        assert!(args.contains(&"acceptEdits".to_string()));
        assert!(args.contains(&"--max-turns".to_string()));
        assert!(args.contains(&"4".to_string()));
        assert!(args.contains(&"--print".to_string()));
        assert!(!args.contains(&"--session-id".to_string()));
        assert!(!args.contains(&"visible-agent".to_string()));
    }

    #[test]
    fn gemini_headless_args_include_config_without_session_reuse() {
        let provider = crate::providers::ProviderFactory::resolve("gemini").unwrap();
        let config = AgentConfig {
            session_id: "wardian-agent".into(),
            resume_session: Some("provider-session".into()),
            system_include_directories: Some(vec!["C:/wardian/common".into()]),
            include_directories: Some(vec!["C:/workspace/docs".into()]),
            model: Some("gemini-test-model".into()),
            provider_config: wardian_core::models::ProviderConfig::Gemini(
                wardian_core::models::GeminiProviderConfig {
                    output_format: Some("text".into()),
                    ..Default::default()
                },
            ),
            ..Default::default()
        };

        let args = headless_provider_args(
            "gemini",
            provider.as_ref(),
            Path::new("D:/Development/Wardian"),
            "task",
            "json",
            None,
            Some(&config),
        );

        assert!(args.contains(&"--include-directories".to_string()));
        assert!(args.contains(&"C:/wardian/common,C:/workspace/docs".to_string()));
        assert!(args.contains(&"--model".to_string()));
        assert!(args.contains(&"gemini-test-model".to_string()));
        assert!(!args.contains(&"--session-id".to_string()));
        assert!(!args.contains(&"wardian-agent".to_string()));
        assert!(!args.contains(&"--resume".to_string()));
        assert!(!args.contains(&"provider-session".to_string()));
        let output_format_index = args
            .iter()
            .position(|arg| arg == "--output-format")
            .expect("output format flag");
        assert_eq!(args[output_format_index + 1], "json");
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
    fn antigravity_headless_args_use_print_and_conversation_resume() {
        let provider = crate::providers::ProviderFactory::resolve("antigravity").unwrap();
        let config = AgentConfig {
            provider: "antigravity".into(),
            provider_config: wardian_core::models::ProviderConfig::Antigravity(
                wardian_core::models::AntigravityProviderConfig {
                    print_timeout: Some("90s".into()),
                    sandbox: Some(true),
                    dangerously_skip_permissions: Some(true),
                    ..Default::default()
                },
            ),
            ..Default::default()
        };

        let args = headless_provider_args(
            "antigravity",
            provider.as_ref(),
            Path::new("/workspace"),
            "task",
            "json",
            Some("conversation-123"),
            Some(&config),
        );

        assert!(args.contains(&"--print".to_string()));
        assert!(args.contains(&"task".to_string()));
        assert!(args.contains(&"--print-timeout".to_string()));
        assert!(args.contains(&"90s".to_string()));
        assert!(args.contains(&"--conversation".to_string()));
        assert!(args.contains(&"conversation-123".to_string()));
        assert!(args.contains(&"--sandbox".to_string()));
        assert!(args.contains(&"--dangerously-skip-permissions".to_string()));
        assert!(!args.contains(&"--output-format".to_string()));
        assert!(!args.contains(&"--resume".to_string()));
    }

    #[test]
    fn gemini_headless_context_projects_persisted_class_skills() {
        let _guard = crate::utils::wardian_test_env_lock();
        let previous_home = std::env::var_os("WARDIAN_HOME");
        let wardian_home = tempfile::tempdir().expect("wardian home");
        let workspace = tempfile::tempdir().expect("workspace");
        let class_skill = wardian_home
            .path()
            .join("classes")
            .join("Personal Assistant")
            .join(".agents")
            .join("skills")
            .join("gws");
        std::fs::create_dir_all(&class_skill).expect("create class skill");
        std::fs::write(class_skill.join("SKILL.md"), "gws skill").expect("write class skill");
        std::env::set_var("WARDIAN_HOME", wardian_home.path());

        let persisted = AgentConfig {
            session_id: "agent-1".into(),
            agent_class: "Personal Assistant".into(),
            provider: "gemini".into(),
            ..Default::default()
        };
        let context = headless_provider_context(
            "gemini",
            workspace.path(),
            "agent-1",
            None,
            Some(&persisted),
        )
        .expect("headless context");

        let expected_habitat = wardian_home
            .path()
            .join("agents")
            .join("agent-1")
            .join("habitat");
        assert_eq!(context.habitat_root, Some(expected_habitat.clone()));
        assert_eq!(context.command_cwd, expected_habitat.join("workspace"));
        assert_eq!(context.args_cwd, context.command_cwd);
        assert!(expected_habitat.join("AGENTS.md").is_file());
        assert!(!expected_habitat.join("GEMINI.md").exists());
        assert!(!expected_habitat.join("CLAUDE.md").exists());
        assert!(expected_habitat
            .join(".agents")
            .join("skills")
            .join("gws")
            .join("SKILL.md")
            .is_file());

        match previous_home {
            Some(value) => std::env::set_var("WARDIAN_HOME", value),
            None => std::env::remove_var("WARDIAN_HOME"),
        }
    }

    #[test]
    fn opencode_headless_context_keeps_real_workspace_as_run_dir() {
        let _guard = crate::utils::wardian_test_env_lock();
        let previous_home = std::env::var_os("WARDIAN_HOME");
        let wardian_home = tempfile::tempdir().expect("wardian home");
        let workspace = tempfile::tempdir().expect("workspace");
        std::env::set_var("WARDIAN_HOME", wardian_home.path());

        let persisted = AgentConfig {
            session_id: "agent-1".into(),
            agent_class: "Builder".into(),
            provider: "opencode".into(),
            ..Default::default()
        };
        let context = headless_provider_context(
            "opencode",
            workspace.path(),
            "agent-1",
            None,
            Some(&persisted),
        )
        .expect("headless context");

        let expected_habitat = wardian_home
            .path()
            .join("agents")
            .join("agent-1")
            .join("habitat");
        assert_eq!(context.command_cwd, expected_habitat);
        assert_eq!(context.args_cwd, PathBuf::from(workspace.path()));

        match previous_home {
            Some(value) => std::env::set_var("WARDIAN_HOME", value),
            None => std::env::remove_var("WARDIAN_HOME"),
        }
    }

    #[test]
    fn headless_identity_env_is_exported_when_session_id_exists() {
        let test_home = TestWardianHome::new();
        let mut cmd = crate::utils::process::new_headless_command("node");

        let _memory_capability = apply_headless_identity_env(
            &mut cmd,
            "automation-bg-run-node",
            Some("wardian-session-123"),
            true,
        );

        let envs: Vec<_> = cmd.as_std().get_envs().collect();
        assert!(envs.iter().any(|(key, value)| {
            key.to_string_lossy() == "WARDIAN_SESSION_ID"
                && value.map(|value| value.to_string_lossy()) == Some("wardian-session-123".into())
        }));
        let capability = envs
            .iter()
            .find(|(key, _)| key.to_string_lossy() == wardian_core::memory::MEMORY_CAPABILITY_ENV)
            .and_then(|(_, value)| *value)
            .expect("memory capability")
            .to_string_lossy();
        assert!(
            wardian_core::memory::MemoryStore::open(test_home._home.path().join("memory.db"))
                .unwrap()
                .validate_capability("wardian-session-123", &capability)
                .unwrap()
        );
    }

    #[test]
    fn disabled_memory_setting_omits_headless_memory_capability() {
        let test_home = TestWardianHome::new();
        std::fs::write(
            test_home._home.path().join("settings/app.json"),
            r#"{"schema_version":2,"overrides":{}}"#,
        )
        .expect("disable memory for headless identity test");
        let mut cmd = crate::utils::process::new_headless_command("node");

        let capability = apply_headless_identity_env(
            &mut cmd,
            "automation-bg-run-node",
            Some("wardian-session-123"),
            false,
        );

        assert!(capability.is_none());
        assert!(!cmd.as_std().get_envs().any(|(key, _)| {
            key.to_string_lossy() == wardian_core::memory::MEMORY_CAPABILITY_ENV
        }));
        assert!(!test_home._home.path().join("memory.db").exists());
    }

    #[test]
    fn ephemeral_headless_identity_has_no_memory_capability() {
        let test_home = TestWardianHome::new();
        let mut cmd = crate::utils::process::new_headless_command("node");

        let capability =
            apply_headless_identity_env(&mut cmd, "automation-bg-temporary-node", None, false);

        assert!(capability.is_none());
        let envs: Vec<_> = cmd.as_std().get_envs().collect();
        assert!(envs.iter().any(|(key, value)| {
            key.to_string_lossy() == "WARDIAN_SESSION_ID"
                && value.map(|value| value.to_string_lossy())
                    == Some("automation-bg-temporary-node".into())
        }));
        assert!(!envs.iter().any(|(key, _)| {
            key.to_string_lossy() == wardian_core::memory::MEMORY_CAPABILITY_ENV
        }));
        let connection = rusqlite::Connection::open(test_home._home.path().join("memory.db"))
            .expect("memory database");
        let count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM memory_process_capabilities",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);
        assert_eq!(count, 0);
    }

    #[test]
    fn headless_identity_env_uses_captured_memory_decision() {
        let test_home = TestWardianHome::new();
        std::fs::write(
            test_home._home.path().join("settings/app.json"),
            r#"{"schema_version":2,"overrides":{}}"#,
        )
        .expect("disable memory after launch decision was captured");
        let mut cmd = crate::utils::process::new_headless_command("node");

        let capability = apply_headless_identity_env(
            &mut cmd,
            "automation-bg-run-node",
            Some("wardian-session-123"),
            true,
        );

        assert!(capability.is_some());
        assert!(cmd.as_std().get_envs().any(|(key, value)| {
            key.to_string_lossy() == wardian_core::memory::MEMORY_CAPABILITY_ENV && value.is_some()
        }));
    }

    #[test]
    fn headless_identity_env_includes_resolved_wardian_home() {
        let _guard = crate::utils::wardian_test_env_lock();
        let previous_home = std::env::var_os("WARDIAN_HOME");
        let home = tempfile::tempdir().expect("temp dir");
        std::env::set_var("WARDIAN_HOME", home.path());
        let mut cmd = crate::utils::process::new_headless_command("node");

        apply_headless_identity_env(
            &mut cmd,
            "automation-bg-run-node",
            Some("wardian-session-123"),
            true,
        );

        let envs: Vec<_> = cmd.as_std().get_envs().collect();
        assert!(envs.iter().any(|(key, value)| {
            key.to_string_lossy() == "WARDIAN_HOME"
                && value.map(|value| value.to_string_lossy())
                    == Some(home.path().display().to_string().into())
        }));

        match previous_home {
            Some(value) => std::env::set_var("WARDIAN_HOME", value),
            None => std::env::remove_var("WARDIAN_HOME"),
        }
    }

    #[test]
    fn headless_identity_env_is_omitted_when_session_id_is_blank() {
        let mut cmd = crate::utils::process::new_headless_command("node");

        apply_headless_identity_env(&mut cmd, "  ", None, false);

        let envs: Vec<_> = cmd.as_std().get_envs().collect();
        assert!(!envs
            .iter()
            .any(|(key, _value)| key.to_string_lossy() == "WARDIAN_SESSION_ID"));
    }

    #[test]
    fn pi_headless_args_use_json_mode_and_exact_session() {
        let provider = ProviderFactory::resolve("pi").expect("Pi provider");
        let config = AgentConfig {
            provider: "pi".into(),
            session_id: "wardian-agent".into(),
            fresh_provider_session_id: Some("pi-session".into()),
            provider_config: wardian_core::models::ProviderConfig::Pi(
                wardian_core::models::PiProviderConfig {
                    reasoning_effort: Some("high".into()),
                    ..Default::default()
                },
            ),
            ..Default::default()
        };

        let args = headless_provider_args(
            "pi",
            provider.as_ref(),
            Path::new("/workspace"),
            "say hello",
            "json",
            None,
            Some(&config),
        );

        assert!(!args.contains(&"--tui-mode".to_string()));
        assert!(args.windows(2).any(|pair| pair == ["--mode", "json"]));
        assert!(args
            .windows(2)
            .any(|pair| pair == ["--session-id", "pi-session"]));
        assert_eq!(args.last().map(String::as_str), Some("say hello"));
    }

    #[test]
    fn pi_fresh_headless_config_rebinds_registered_agent_profile() {
        let test_wardian_home = TestWardianHome::new();
        let provider = ProviderFactory::resolve("pi").expect("Pi provider");
        let registered_config = AgentConfig {
            provider: "pi".into(),
            session_id: "visible-agent".into(),
            fresh_provider_session_id: None,
            provider_config: wardian_core::models::ProviderConfig::Pi(Default::default()),
            ..Default::default()
        };

        let config = effective_headless_provider_config(
            "pi",
            Path::new("/workspace"),
            "automation-fresh-run",
            None,
            Some(&registered_config),
            None,
        )
        .expect("fresh Pi automation config");
        let provider_session_id = config
            .fresh_provider_session_id
            .as_deref()
            .expect("fresh Pi identity");
        let args = headless_provider_args(
            "pi",
            provider.as_ref(),
            Path::new("/workspace"),
            "task",
            "json",
            None,
            Some(&config),
        );

        assert_eq!(config.session_id, "automation-fresh-run");
        assert!(uuid::Uuid::parse_str(provider_session_id).is_ok());
        assert!(args
            .windows(2)
            .any(|pair| pair == ["--session-id", provider_session_id]));
        let expected_session_dir = test_wardian_home
            ._home
            .path()
            .join("agents")
            .join("automation-fresh-run")
            .join("pi")
            .join("sessions")
            .to_string_lossy()
            .to_string();
        assert!(args
            .windows(2)
            .any(|pair| pair == ["--session-dir", expected_session_dir.as_str()]));
        assert!(!args.contains(&"--no-session".to_string()));
    }

    #[test]
    fn pi_blank_resume_id_keeps_explicit_fresh_session_policy() {
        let _test_wardian_home = TestWardianHome::new();
        let provider = ProviderFactory::resolve("pi").expect("Pi provider");
        let registered_config = AgentConfig {
            provider: "pi".into(),
            session_id: "visible-agent".into(),
            resume_session: Some("   ".into()),
            fresh_provider_session_id: None,
            provider_config: wardian_core::models::ProviderConfig::Pi(Default::default()),
            ..Default::default()
        };

        let config = effective_headless_provider_config(
            "pi",
            Path::new("/workspace"),
            "automation-fresh-run",
            Some("   "),
            Some(&registered_config),
            None,
        )
        .expect("fresh Pi automation config");
        let provider_session_id = config
            .fresh_provider_session_id
            .as_deref()
            .expect("fresh Pi identity");
        let args = headless_provider_args(
            "pi",
            provider.as_ref(),
            Path::new("/workspace"),
            "task",
            "json",
            Some("   "),
            Some(&config),
        );

        assert!(args
            .windows(2)
            .any(|pair| pair == ["--session-id", provider_session_id]));
        assert!(!args.contains(&"--session".to_string()));
    }

    #[test]
    fn pi_headless_output_returns_final_assistant_text() {
        let output = concat!(
            "{\"type\":\"session\",\"id\":\"pi-session\"}\n",
            "{\"type\":\"message_end\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"Pi answer\"}],\"stopReason\":\"stop\"}}\n",
            "{\"type\":\"agent_end\",\"messages\":[]}\n",
        );

        let normalized = normalize_pi_headless_output(output, "json").expect("Pi output");
        assert_eq!(normalized["session_id"], "pi-session");
        assert_eq!(normalized["response"], "Pi answer");
    }

    #[test]
    fn pi_headless_output_accepts_completed_message_event() {
        let output = concat!(
            "{\"type\":\"session\",\"id\":\"pi-session\"}\n",
            "{\"type\":\"message\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"Pi final answer\"}],\"stopReason\":\"stop\"}}\n",
            "{\"type\":\"agent_end\",\"messages\":[]}\n",
        );

        let normalized = normalize_pi_headless_output(output, "json").expect("Pi output");
        assert_eq!(normalized["session_id"], "pi-session");
        assert_eq!(normalized["response"], "Pi final answer");
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn sequential_bootstrap_attempts_launch_with_current_private_provider_configuration() {
        if !node_available() {
            return;
        }
        let home = TestWardianHome::new_async().await;
        struct SourceEnvironment(Vec<(&'static str, Option<OsString>)>);
        impl Drop for SourceEnvironment {
            fn drop(&mut self) {
                for (key, value) in &self.0 {
                    match value {
                        Some(value) => std::env::set_var(key, value),
                        None => std::env::remove_var(key),
                    }
                }
            }
        }
        let _source_environment = SourceEnvironment(
            ["CODEX_HOME", "OPENAI_API_KEY", "CODEX_API_KEY"]
                .into_iter()
                .map(|key| (key, std::env::var_os(key)))
                .collect(),
        );
        let upstream = home._home.path().join("selected-upstream");
        std::fs::create_dir(&upstream).unwrap();
        std::env::set_var("CODEX_HOME", &upstream);
        std::env::remove_var("OPENAI_API_KEY");
        std::env::remove_var("CODEX_API_KEY");
        let mut first_home = None;
        for endpoint in ["https://first.invalid", "https://second.invalid"] {
            std::fs::write(upstream.join("config.toml"), format!(
                "model_provider = 'fixture'\n[model_providers.fixture]\nname = 'fixture'\nbase_url = '{endpoint}'\n")).unwrap();
            let auth = serde_json::json!({"OPENAI_API_KEY":endpoint}).to_string();
            std::fs::write(upstream.join("auth.json"), &auth).unwrap();
            let context = crate::utils::codex_home::CodexLaunchContext::capture(&AgentConfig {
                provider: "codex".into(),
                ..Default::default()
            })
            .unwrap();
            let (_, projected) = codex_bootstrap_launch_context(
                home._home.path(),
                home._home.path(),
                &uuid::Uuid::new_v4().to_string(),
            )
            .unwrap();
            crate::utils::fs::sync_codex_agent_home_with_context(
                &context.upstream().path,
                &projected,
                Path::new(""),
                &context,
            )
            .unwrap();
            let capture = projected.join("captured.json");
            let mut credentials =
                super::super::codex::BootstrapCredentialRetirement::new(projected.clone());
            let mut command = bootstrap_fixture_command("capture-config");
            command
                .env("CODEX_HOME", &projected)
                .env("WARDIAN_BOOTSTRAP_TEST_CAPTURE", &capture);
            let receipt = run_owned_bootstrap_command(
                &mut command,
                "codex",
                Duration::from_secs(10),
                Duration::from_secs(3),
                Some(&context),
                None,
                Some(&mut credentials),
            )
            .await
            .unwrap();
            assert!(receipt.work_complete && receipt.exit_success && receipt.containment_empty);
            credentials
                .confirm_closed(receipt.containment_empty)
                .unwrap();
            assert!(!projected.join("auth.json").exists());
            assert!(!projected.join("config.toml").exists());
            let captured: Value = serde_json::from_slice(&std::fs::read(capture).unwrap()).unwrap();
            let configuration = captured["config"]
                .as_str()
                .unwrap()
                .parse::<toml_edit::DocumentMut>()
                .unwrap();
            assert_eq!(
                configuration["model_providers"]["fixture"]["base_url"].as_str(),
                Some(endpoint)
            );
            assert_eq!(captured["auth"].as_str(), Some(auth.as_str()));
            if let Some(first) = &first_home {
                assert_ne!(first, &projected);
            } else {
                first_home = Some(projected);
            }
        }
        wardian_core::db::init_db().unwrap();
        for fail_before_start in [true, false] {
            let context = crate::utils::codex_home::CodexLaunchContext::capture(&AgentConfig {
                provider: "codex".into(),
                ..Default::default()
            })
            .unwrap();
            let (_, projected) = codex_bootstrap_launch_context(
                home._home.path(),
                home._home.path(),
                &uuid::Uuid::new_v4().to_string(),
            )
            .unwrap();
            crate::utils::fs::sync_codex_agent_home_with_context(
                &context.upstream().path,
                &projected,
                Path::new(""),
                &context,
            )
            .unwrap();
            let mut credentials =
                super::super::codex::BootstrapCredentialRetirement::new(projected.clone());
            let pid = projected.join("child.pid");
            let mut command = bootstrap_fixture_command("capture-config");
            command
                .env("CODEX_HOME", &projected)
                .env(
                    "WARDIAN_BOOTSTRAP_TEST_CAPTURE",
                    projected.join("capture.json"),
                )
                .env("WARDIAN_BOOTSTRAP_TEST_PID_FILE", &pid);
            if fail_before_start {
                std::fs::write(
                    upstream.join("auth.json"),
                    r#"{"OPENAI_API_KEY":"rotated-fixture"}"#,
                )
                .unwrap();
            }
            let operation_id = uuid::Uuid::new_v4().to_string();
            let attempt = uuid::Uuid::new_v4().to_string();
            let result = run_owned_bootstrap_command(
                &mut command,
                "codex",
                Duration::from_secs(10),
                Duration::from_secs(3),
                Some(&context),
                Some((&operation_id, &attempt)),
                Some(&mut credentials),
            )
            .await;
            assert!(
                result.is_err(),
                "final validation or post-closure ledger write must fail"
            );
            drop(credentials);
            assert_eq!(pid.exists(), !fail_before_start);
            if !fail_before_start {
                let child = std::fs::read_to_string(pid)
                    .unwrap()
                    .parse::<u32>()
                    .unwrap();
                assert!(!bootstrap_fixture_process_alive(child));
            }
            for leaf in ["auth.json", "config.toml", "cap_sid"] {
                assert!(
                    !projected.join(leaf).exists(),
                    "known no-start/closure retires {leaf}"
                );
            }
        }
    }

    #[cfg(windows)]
    fn owned_bootstrap_fixture() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("src")
            .join("manager")
            .join("fixtures")
            .join("owned_bootstrap_child.js")
    }

    #[cfg(windows)]
    fn bootstrap_fixture_command(mode: &str) -> tokio::process::Command {
        let executable = if cfg!(windows) { "node.exe" } else { "node" };
        let mut command = crate::utils::process::new_headless_command(executable);
        command
            .arg(owned_bootstrap_fixture())
            .arg(mode)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        command
    }

    #[cfg(windows)]
    fn bootstrap_fixture_process_alive(pid: u32) -> bool {
        crate::utils::process::process_exists(pid)
    }

    #[tokio::test]
    #[cfg(windows)]
    async fn bootstrap_drains_stderr_flood_and_waits_for_child_exit_and_eof() {
        if !node_available() {
            return;
        }
        let mut command = bootstrap_fixture_command("stderr-flood");
        let receipt = run_owned_bootstrap_command(
            &mut command,
            "codex",
            Duration::from_secs(10),
            Duration::from_secs(3),
            None,
            None,
            None,
        )
        .await
        .expect("contained bootstrap should complete");

        assert!(receipt.work_complete);
        assert!(!receipt.timed_out);
        assert!(receipt.exit_success);
        assert!(receipt.stdout_eof);
        assert!(receipt.stderr_eof);
        assert!(receipt.containment_empty);
        assert_eq!(
            receipt.session_id.as_deref(),
            Some("thread-controlled-bootstrap-fixture")
        );
        assert!(receipt.stderr_bytes >= 1024 * 1024);
    }

    #[tokio::test]
    #[cfg(windows)]
    async fn bootstrap_marker_does_not_count_as_child_completion() {
        if !node_available() {
            return;
        }
        let mut command = bootstrap_fixture_command("early-marker");
        let receipt = run_owned_bootstrap_command(
            &mut command,
            "codex",
            Duration::from_secs(2),
            Duration::from_secs(3),
            None,
            None,
            None,
        )
        .await
        .expect("timed-out child cleanup should close its owned process group");

        assert!(receipt.timed_out);
        assert!(!receipt.work_complete);
        assert!(!receipt.exit_success);
        assert_eq!(
            receipt.session_id.as_deref(),
            Some("thread-controlled-bootstrap-fixture")
        );
        assert!(receipt.stdout_eof);
        assert!(receipt.stderr_eof);
        assert!(receipt.containment_empty);
    }

    #[tokio::test]
    #[cfg(windows)]
    async fn cancellation_drops_owned_bootstrap_containment() {
        if !node_available() {
            return;
        }
        for mode in ["silent", "leader-exited"] {
            let temp = tempfile::tempdir().expect("bootstrap pid marker directory");
            let pid_path = temp.path().join("bootstrap.pid");
            let leader_path = temp.path().join("leader.pid");
            let failure_path = temp.path().join("failure.txt");
            let task_pid_path = pid_path.clone();
            let task_leader_path = leader_path.clone();
            let task_failure_path = failure_path.clone();
            let secret_home = temp.path().join("credentials");
            std::fs::create_dir(&secret_home).unwrap();
            for leaf in ["auth.json", "config.toml", "cap_sid"] {
                std::fs::write(secret_home.join(leaf), "fixture").unwrap();
            }
            let task_secret_home = secret_home.clone();
            let task = tokio::spawn(async move {
                let mut command = bootstrap_fixture_command(mode);
                command
                    .env("WARDIAN_BOOTSTRAP_TEST_PID_FILE", task_pid_path)
                    .env("WARDIAN_BOOTSTRAP_TEST_LEADER", task_leader_path)
                    .env("WARDIAN_BOOTSTRAP_TEST_FAILURE", task_failure_path);
                let mut credentials =
                    super::super::codex::BootstrapCredentialRetirement::new(task_secret_home);
                run_owned_bootstrap_command(
                    &mut command,
                    "codex",
                    Duration::from_secs(30),
                    Duration::from_secs(3),
                    None,
                    None,
                    Some(&mut credentials),
                )
                .await
            });
            let observed = tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    if std::fs::read_to_string(&pid_path).is_ok() {
                        break true;
                    }
                    if task.is_finished() || failure_path.exists() {
                        break false;
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await;
            if !matches!(observed, Ok(true)) {
                task.abort();
                let outcome = task.await;
                panic!("bootstrap fixture {mode} failed before its PID barrier: {outcome:?}; detail={:?}", std::fs::read_to_string(&failure_path));
            }
            let pid = std::fs::read_to_string(&pid_path)
                .expect("fixture PID")
                .trim()
                .parse::<u32>()
                .expect("numeric fixture PID");

            if mode == "leader-exited" {
                let leader = std::fs::read_to_string(&leader_path)
                    .unwrap()
                    .parse::<u32>()
                    .unwrap();
                tokio::time::timeout(Duration::from_secs(5), async {
                    while bootstrap_fixture_process_alive(leader) {
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                })
                .await
                .expect("leader must exit before cancellation");
                assert!(
                    bootstrap_fixture_process_alive(pid),
                    "descendant must still be live"
                );
                assert!(
                    !task.is_finished(),
                    "retained descendant pipes must keep the observation pending"
                );
            }

            task.abort();
            let _ = task.await;
            for leaf in ["auth.json", "config.toml", "cap_sid"] {
                assert!(
                    secret_home.join(leaf).exists(),
                    "cancelled observation retains uncertain inputs"
                );
            }
            tokio::time::timeout(Duration::from_secs(5), async {
                while bootstrap_fixture_process_alive(pid) {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            })
            .await
            .expect("cancellation must terminate the retained bootstrap child");
        }
    }
}
