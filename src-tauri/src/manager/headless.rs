use crate::providers::antigravity::{changed_workspace_conversation, AntigravityProvider};
use crate::providers::codex::CodexProvider;
use crate::providers::opencode::OpenCodeProvider;
use crate::providers::prime::PrimeProvider;
use crate::providers::ProviderFactory;
use crate::utils::fs::*;
use crate::utils::process::new_headless_command;
use crate::utils::shell::build_program_launch;
use std::time::Duration;
use wardian_core::conversation_lease::ConversationLeaseOwner;
use wardian_core::models::{AgentConfig, AgentEvent, AgentProvider};

use super::codex::{
    codex_bootstrap_launch_context, codex_session_file_path_in, migrate_codex_bootstrap_home,
};
use super::opencode::opencode_env;
use super::{
    interactive_provider_cwd, persisted_agent_config, session_bootstrap_prompt,
    strip_flag_value_pairs,
};
use crate::utils::logging::log_debug;

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

pub struct HeadlessRunOptions<'a> {
    pub cwd: &'a std::path::Path,
    pub prompt: &'a str,
    pub wardian_session_id: &'a str,
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
    config_override: Option<&AgentConfig>,
    persisted_config: Option<&AgentConfig>,
) -> Option<AgentConfig> {
    config_override
        .cloned()
        .or_else(|| persisted_config.cloned())
        .or_else(|| {
            // Provider-bound workflow workers do not have an agent profile,
            // but Codex still needs the persisted runtime policy flags.
            (provider_name == "codex").then(|| AgentConfig {
                provider: "codex".to_string(),
                folder: cwd.to_string_lossy().to_string(),
                provider_config: wardian_core::models::ProviderConfig::Codex(
                    wardian_core::models::CodexProviderConfig::default(),
                ),
                ..Default::default()
            })
        })
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
            provider_args
                .push(crate::utils::terminal_input::normalize_prompt_for_terminal_submit(prompt));
        }
        "prime" => {
            // Prime Agent emits the same structured event stream in print mode
            // as interactively, so headless runs reuse get_spawn_args verbatim
            // and only add the run mode. `output_format` is ignored: --mode json
            // is the only machine-readable print format Prime offers.
            if let Some(config) = config_override {
                let mut config = config.clone();
                config.resume_session = resume_session.map(str::to_string);
                provider_args.extend(provider.get_spawn_args(&config, resume_session.is_some()));
                PrimeProvider::append_autonomous_args(&mut provider_args, &config);
            } else if let Some(resume_id) = resume_session.filter(|s| !s.trim().is_empty()) {
                provider_args.push("--resume".to_string());
                provider_args.push(resume_id.to_string());
            }
            provider_args.push("--cwd".to_string());
            provider_args.push(provider_cwd.to_string_lossy().to_string());
            provider_args.push("--mode".to_string());
            provider_args.push("json".to_string());
            provider_args.push("--print".to_string());
            provider_args.push(prompt.to_string());
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
) -> Result<serde_json::Value, String> {
    use tokio::io::{AsyncBufReadExt, BufReader};
    super::validate_session_values_for_launch(options.wardian_session_id, options.resume_session)?;
    let cwd = options.cwd;
    let prompt = options.prompt;
    let wardian_session_id = options.wardian_session_id;
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
    let provider_context = headless_provider_context(
        provider_name,
        cwd,
        wardian_session_id,
        config_override,
        persisted_config.as_ref(),
    )?;
    let effective_provider_config = effective_headless_provider_config(
        provider_name,
        cwd,
        config_override,
        persisted_config.as_ref(),
    );
    let (bin, _) = provider.get_executable();
    let claude_hook = if provider_name == "claude" {
        ensure_claude_permission_hook(wardian_session_id).ok()
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
    apply_headless_identity_env(&mut cmd, wardian_session_id);
    super::apply_managed_cli_path_to_process(&mut cmd);
    super::apply_process_provider_runtime_env(provider_name, &mut cmd)?;
    if let Some(config) = effective_provider_config.as_ref() {
        for (key, value) in super::worktree_build_env(config) {
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
        cmd.stdin(std::process::Stdio::null());
    } else if provider_name == "antigravity" {
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
        let sandbox_mode = launch_spec
            .args
            .iter()
            .position(|arg| arg == "--sandbox")
            .and_then(|index| launch_spec.args.get(index + 1))
            .map(String::as_str)
            .unwrap_or("<bypassed>");
        let approval_policy = launch_spec
            .args
            .iter()
            .position(|arg| arg == "--ask-for-approval")
            .and_then(|index| launch_spec.args.get(index + 1))
            .map(String::as_str)
            .unwrap_or("<bypassed>");
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

    let status = wait_for_headless_child(
        &mut child,
        provider_name,
        options.timeout,
        options.lease_owner.as_ref(),
    )
    .await;
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
        return Err(format!(
            "Headless provider {provider_name} exited with status {}: {detail}",
            status.code().unwrap_or(-1)
        ));
    }

    if provider_name == "codex" {
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
                "thread_id": wardian_session_id,
                "response": last_message.unwrap_or_else(|| output.clone()),
                "raw": output,
            }))
        } else {
            Ok(serde_json::json!({ "text": last_message.unwrap_or(output) }))
        }
    } else if provider_name == "claude" {
        normalize_claude_headless_output(&output, output_format)
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
    } else if output_format == "json" {
        serde_json::from_str(&output)
            .map_err(|e| format!("Failed to parse JSON output: {}. Raw: {}", e, output))
    } else {
        Ok(serde_json::json!({ "text": output }))
    }
}

async fn wait_for_headless_child(
    child: &mut tokio::process::Child,
    provider_name: &str,
    timeout: Duration,
    lease_owner: Option<&ConversationLeaseOwner>,
) -> Result<std::process::ExitStatus, String> {
    wait_for_headless_child_with_intervals(
        child,
        provider_name,
        timeout,
        lease_owner,
        HEADLESS_PROCESS_POLL_INTERVAL,
        HEADLESS_LEASE_HEARTBEAT_INTERVAL,
    )
    .await
}

async fn wait_for_headless_child_with_intervals(
    child: &mut tokio::process::Child,
    provider_name: &str,
    timeout: Duration,
    lease_owner: Option<&ConversationLeaseOwner>,
    process_poll_interval: Duration,
    lease_heartbeat_interval: Duration,
) -> Result<std::process::ExitStatus, String> {
    let deadline = tokio::time::Instant::now() + timeout;
    let mut process_poll = tokio::time::interval_at(
        tokio::time::Instant::now() + process_poll_interval,
        process_poll_interval,
    );
    let mut heartbeat = tokio::time::interval_at(
        tokio::time::Instant::now() + lease_heartbeat_interval,
        lease_heartbeat_interval,
    );

    loop {
        tokio::select! {
            _ = tokio::time::sleep_until(deadline) => {
                terminate_headless_child(child).await;
                return Err(format!(
                    "Headless provider {provider_name} exceeded its {} second execution limit",
                    timeout.as_secs()
                ));
            }
            _ = process_poll.tick() => {
                if let Some(status) = child.try_wait().map_err(|error| error.to_string())? {
                    return Ok(status);
                }
            }
            _ = heartbeat.tick(), if lease_owner.is_some() => {
                let owner = lease_owner.expect("lease owner checked by select guard");
                let now = chrono::Utc::now();
                let renewed = wardian_core::conversation_lease::renew_lease_owner_persisted(
                    owner,
                    &now.to_rfc3339(),
                    &(now + HEADLESS_LEASE_DURATION).to_rfc3339(),
                )?;
                if !renewed {
                    terminate_headless_child(child).await;
                    return Err(format!(
                        "Headless provider {provider_name} lost its conversation lease before completion"
                    ));
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
    let response = claude_headless_response(&parsed).unwrap_or_else(|| output.to_string());

    if output_format == "json" {
        Ok(serde_json::json!({
            "session_id": parsed.get("session_id").and_then(|value| value.as_str()),
            "response": response,
            "raw": output,
        }))
    } else {
        Ok(serde_json::json!({ "text": response }))
    }
}

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
) {
    provider_args.push("--cd".to_string());
    provider_args.push(provider_cwd.to_string_lossy().to_string());

    // Codex global options must precede `exec`; exec only accepts its own
    // subcommand options after this point.
    if let Some(config) = config {
        CodexProvider::new().append_headless_global_args(provider_args, config);
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
    use tokio::io::{AsyncBufReadExt, BufReader};
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
    let habitat_root =
        prepare_provider_habitat(provider_name, cwd, class_name, bootstrap_session_id)?;
    let codex_bootstrap = if provider_name == "codex" {
        let wardian_home = get_wardian_home().ok_or("Could not find Wardian home")?;
        Some(codex_bootstrap_launch_context(&wardian_home, cwd))
    } else {
        None
    };

    if provider_name == "codex" {
        if let Some(agent_habitat_root) = habitat_root.as_ref() {
            let agent_codex_home = habitat_codex_home(agent_habitat_root);
            let real_codex_home = dirs::home_dir()
                .ok_or("Could not find user home directory")?
                .join(".codex");
            sync_codex_agent_home(
                &real_codex_home,
                &agent_codex_home,
                std::path::Path::new(""),
            )?;

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
    }
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
        append_codex_bootstrap_args(&mut provider_args, &provider_cwd, config);
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
    if let Some(bootstrap_session_id) = bootstrap_session_id {
        apply_headless_identity_env(&mut cmd, bootstrap_session_id);
    }
    super::apply_managed_cli_path_to_process(&mut cmd);
    super::apply_process_provider_runtime_env(provider_name, &mut cmd)?;

    if provider_name == "codex" {
        if let Some((_, bootstrap_home)) = codex_bootstrap.as_ref() {
            let real_codex_home = dirs::home_dir()
                .ok_or("Could not find user home directory")?
                .join(".codex");
            sync_codex_agent_home(&real_codex_home, bootstrap_home, std::path::Path::new(""))?;
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
    match cmd.spawn() {
        Ok(mut child) => {
            log_debug("[WARDIAN-DEBUG] Spawned headless process. Reading stdout...");
            let mut session_id_res = None;
            let mut stderr_output = String::new();

            let timeout = tokio::time::Duration::from_secs(60);
            let read_future = async {
                let mut output = String::new();
                if let Some(stdout) = child.stdout.take() {
                    let mut reader = BufReader::new(stdout);
                    let mut line = String::new();
                    while let Ok(n) = reader.read_line(&mut line).await {
                        if n == 0 {
                            log_debug("[WARDIAN-DEBUG] Reached EOF on stdout.");
                            break;
                        }
                        let trimmed = line.trim();
                        output.push_str(trimmed);
                        output.push('\n');
                        if let Some(start) = trimmed.find('{') {
                            let json_part = &trimmed[start..];
                            if let Some(evt) = provider.parse_output(json_part) {
                                match evt {
                                    AgentEvent::Init { .. } => {
                                        log_debug(
                                            "[WARDIAN-DEBUG] Ignored provider initialization identifier.",
                                        );
                                    }
                                    // ModelResponse means the prompt completed and the session
                                    // has been persisted to disk — safe to stop reading.
                                    AgentEvent::ModelResponse => {
                                        log_debug(
                                            "[WARDIAN-DEBUG] Prompt complete, session saved.",
                                        );
                                        break;
                                    }
                                    _ => {}
                                }
                            }
                        }
                        line.clear();
                    }
                }
                bootstrap_output_session_id(provider_name, &output)
            };

            let timed_out = match tokio::time::timeout(timeout, read_future).await {
                Ok(sid) => {
                    session_id_res = sid;
                    false
                }
                Err(_) => {
                    log_debug("[WARDIAN-DEBUG] Timed out waiting for session_id.");
                    true
                }
            };

            // Only force-kill if we timed out; otherwise let the process exit naturally
            // so the session is fully flushed to disk before we attempt --resume.
            if timed_out {
                let _ = child.kill().await;
            }
            if let Some(stderr) = child.stderr.take() {
                let mut reader = BufReader::new(stderr);
                let mut line = String::new();
                while let Ok(n) = reader.read_line(&mut line).await {
                    if n == 0 {
                        break;
                    }
                    stderr_output.push_str(&line);
                    line.clear();
                }
            }
            let _ = child.wait().await;
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
            if session_id_res.is_none() && !stderr_output.trim().is_empty() {
                log_debug(&format!(
                    "[WARDIAN-DEBUG] obtain_session_id received {} stderr bytes.",
                    stderr_output.len()
                ));
            }
            if provider_name == "codex" {
                if let (Some((_, bootstrap_home)), Some(agent_habitat_root)) =
                    (codex_bootstrap.as_ref(), habitat_root.as_ref())
                {
                    // The interactive process uses a habitat keyed by Wardian's
                    // stable agent ID. Migrating under the freshly-created Codex
                    // thread ID makes `codex resume` look in the wrong home.
                    migrate_codex_bootstrap_home(
                        bootstrap_home,
                        &habitat_codex_home(agent_habitat_root),
                    )?;
                }
            }
            log_debug(&format!(
                "[WARDIAN-DEBUG] Returning session identifier: found={}",
                session_id_res.is_some()
            ));
            session_id_res.ok_or_else(|| {
                if stderr_output.trim().is_empty() {
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
        Err(e) => {
            log_debug(&format!("[WARDIAN-DEBUG] Failed to spawn cmd: {:?}", e));
            Err(format!(
                "Failed to spawn {} bootstrap command: {}",
                provider_name, e
            ))
        }
    }
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
            // Prime Agent's first stream line is its session header.
            "prime" if parsed.get("type").and_then(|value| value.as_str()) == Some("session") => {
                parsed
                    .get("id")
                    .and_then(|value| value.as_str())
                    .filter(|value| !value.trim().is_empty())
                    .map(str::to_string)
            }
            _ => None,
        }
    })
}

fn apply_headless_identity_env(cmd: &mut tokio::process::Command, wardian_session_id: &str) {
    if let Some(home) = crate::utils::fs::get_wardian_home() {
        cmd.env("WARDIAN_HOME", home);
    }
    if !wardian_session_id.trim().is_empty() {
        cmd.env("WARDIAN_SESSION_ID", wardian_session_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use std::ffi::OsString;
    use std::path::{Path, PathBuf};

    struct TestWardianHome {
        _lock: std::sync::MutexGuard<'static, ()>,
        previous_home: Option<OsString>,
        _home: tempfile::TempDir,
    }

    impl TestWardianHome {
        fn new() -> Self {
            let lock = crate::utils::wardian_test_env_lock();
            let home = tempfile::tempdir().expect("temp wardian home");
            let previous_home = std::env::var_os("WARDIAN_HOME");
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

    #[cfg(windows)]
    #[tokio::test]
    #[ignore = "requires a logged-in Codex CLI; run with cargo test -p Wardian -- --ignored real_codex_headless_projected_home_runs_shell_on_windows"]
    async fn real_codex_headless_projected_home_runs_shell_on_windows() {
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
                resume_session: None,
                output_format: "json",
                provider_name: "codex",
                config_override: Some(&config),
                timeout: Duration::from_secs(110),
                lease_owner: None,
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

    #[tokio::test]
    async fn headless_wait_heartbeats_an_active_conversation_lease() {
        if !node_available() {
            return;
        }
        let _home = TestWardianHome::new();
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
        let config = effective_headless_provider_config("codex", cwd, None, None)
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

        append_codex_bootstrap_args(&mut args, Path::new("/workspace"), Some(&config));

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
    fn claude_headless_output_exposes_result_text_for_workflows() {
        let output =
            r#"{"type":"result","session_id":"claude-session-1","result":"workflow complete"}"#;

        let normalized = normalize_claude_headless_output(output, "json").unwrap();

        assert_eq!(normalized["session_id"], "claude-session-1");
        assert_eq!(normalized["response"], "workflow complete");
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
        assert!(expected_habitat.join("GEMINI.md").is_file());
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
        let mut cmd = crate::utils::process::new_headless_command("node");

        apply_headless_identity_env(&mut cmd, "wardian-session-123");

        let envs: Vec<_> = cmd.as_std().get_envs().collect();
        assert!(envs.iter().any(|(key, value)| {
            key.to_string_lossy() == "WARDIAN_SESSION_ID"
                && value.map(|value| value.to_string_lossy()) == Some("wardian-session-123".into())
        }));
    }

    #[test]
    fn headless_identity_env_includes_resolved_wardian_home() {
        let _guard = crate::utils::wardian_test_env_lock();
        let previous_home = std::env::var_os("WARDIAN_HOME");
        let home = tempfile::tempdir().expect("temp dir");
        std::env::set_var("WARDIAN_HOME", home.path());
        let mut cmd = crate::utils::process::new_headless_command("node");

        apply_headless_identity_env(&mut cmd, "wardian-session-123");

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

        apply_headless_identity_env(&mut cmd, "  ");

        let envs: Vec<_> = cmd.as_std().get_envs().collect();
        assert!(!envs
            .iter()
            .any(|(key, _value)| key.to_string_lossy() == "WARDIAN_SESSION_ID"));
    }
}
