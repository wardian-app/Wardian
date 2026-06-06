use crate::providers::antigravity::AntigravityProvider;
use crate::providers::codex::CodexProvider;
use crate::providers::opencode::OpenCodeProvider;
use crate::providers::ProviderFactory;
use crate::utils::fs::*;
use crate::utils::process::new_headless_command;
use crate::utils::shell::build_program_launch;
use wardian_core::models::{AgentConfig, AgentEvent, AgentProvider};

use super::codex::{codex_bootstrap_launch_context, migrate_codex_bootstrap_home};
use super::opencode::opencode_env;
use super::{
    interactive_provider_cwd, persisted_agent_config, session_bootstrap_prompt,
    strip_flag_value_pairs, strip_standalone_flag,
};
use crate::utils::logging::log_debug;

#[cfg(target_os = "macos")]
use super::macos_extended_path;
#[cfg(windows)]
use super::{quote_cmd_arg, windows_cmd_host};

pub(crate) fn headless_provider_launch(
    provider_name: &str,
    bin: &str,
    provider_args: &[String],
) -> Result<crate::utils::shell::ShellLaunchSpec, String> {
    #[cfg(windows)]
    {
        let lower_bin = bin.to_ascii_lowercase();
        if provider_name == "opencode" && !lower_bin.ends_with(".exe") {
            let cmd_host = windows_cmd_host();
            let mut fragments = vec![quote_cmd_arg(bin)];
            fragments.extend(provider_args.iter().map(|arg| quote_cmd_arg(arg)));
            return Ok(crate::utils::shell::ShellLaunchSpec {
                executable: cmd_host,
                args: vec!["/d".to_string(), "/c".to_string(), fragments.join(" ")],
            });
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

pub struct HeadlessRunOptions<'a> {
    pub cwd: &'a std::path::Path,
    pub prompt: &'a str,
    pub wardian_session_id: &'a str,
    pub resume_session: Option<&'a str>,
    pub output_format: &'a str,
    pub provider_name: &'a str,
    pub config_override: Option<&'a AgentConfig>,
}

#[derive(Debug)]
struct HeadlessProviderContext {
    class_name: String,
    command_cwd: std::path::PathBuf,
    args_cwd: std::path::PathBuf,
    habitat_root: Option<std::path::PathBuf>,
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
                CodexProvider::new().append_common_args(&mut provider_args, config, true);
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

pub async fn run_headless(
    cwd: &std::path::Path,
    prompt: &str,
    session_id: &str,
    output_format: &str,
    provider_name: &str,
) -> Result<serde_json::Value, String> {
    run_headless_with_config(cwd, prompt, session_id, output_format, provider_name, None).await
}

pub async fn run_headless_with_config(
    cwd: &std::path::Path,
    prompt: &str,
    session_id: &str,
    output_format: &str,
    provider_name: &str,
    config_override: Option<&AgentConfig>,
) -> Result<serde_json::Value, String> {
    run_headless_with_options(HeadlessRunOptions {
        cwd,
        prompt,
        wardian_session_id: session_id,
        resume_session: (!session_id.trim().is_empty()).then_some(session_id),
        output_format,
        provider_name,
        config_override,
    })
    .await
}

pub async fn run_headless_with_options(
    options: HeadlessRunOptions<'_>,
) -> Result<serde_json::Value, String> {
    use tokio::io::{AsyncBufReadExt, BufReader};
    let cwd = options.cwd;
    let prompt = options.prompt;
    let wardian_session_id = options.wardian_session_id;
    let resume_session = options.resume_session;
    let output_format = options.output_format;
    let provider_name = options.provider_name;
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
    let effective_provider_config = config_override
        .cloned()
        .or_else(|| persisted_config.clone());
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
    } else if provider_name == "claude" {
        cmd.env("CLAUDE_CODE_ADDITIONAL_DIRECTORIES_CLAUDE_MD", "1");
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
        "[Wardian] run_headless args: exe={} args={:?}",
        launch_spec.executable, launch_spec.args
    ));

    let mut child = cmd.spawn().map_err(|e| e.to_string())?;

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

    let (output, err_output) = tokio::join!(stdout_handle, stderr_handle);
    let output = output.unwrap_or_default();
    let err_output = err_output.unwrap_or_default();

    let status = child.wait().await.map_err(|error| error.to_string())?;

    if !err_output.is_empty() {
        log_debug(&format!("[Wardian] Headless stderr: {}", err_output.trim()));
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
                "response": last_message.unwrap_or_default(),
                "raw": output,
            }))
        } else {
            Ok(serde_json::json!({ "text": last_message.unwrap_or(output) }))
        }
    } else if provider_name == "opencode" {
        let summary = OpenCodeProvider::summarize_run_output(&output);

        if output_format == "json" {
            Ok(serde_json::json!({
                "session_id": summary.session_id.unwrap_or_else(|| wardian_session_id.to_string()),
                "response": summary.last_text.clone().unwrap_or_default(),
                "raw": output,
            }))
        } else {
            Ok(serde_json::json!({ "text": summary.last_text.unwrap_or(output) }))
        }
    } else if provider_name == "antigravity" {
        let conversation_id = resume_session
            .filter(|value| !value.trim().is_empty())
            .map(str::to_string)
            .or_else(|| {
                AntigravityProvider::antigravity_home()
                    .and_then(|home| AntigravityProvider::conversation_for_workspace(&home, cwd))
            })
            .or_else(|| {
                AntigravityProvider::antigravity_home()
                    .and_then(|home| AntigravityProvider::latest_conversation_id(&home))
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
                "session_id": conversation_id.unwrap_or_else(|| wardian_session_id.to_string()),
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
    let provider_cwd = interactive_provider_cwd(
        provider_name,
        cwd,
        habitat_root.as_deref(),
        codex_bootstrap.as_ref(),
    );

    if provider_name == "codex" {
        provider_args.push("--cd".to_string());
        provider_args.push(provider_cwd.to_string_lossy().to_string());
        provider_args.push("exec".to_string());

        if let Some(config) = config {
            let spawn_args =
                strip_flag_value_pairs(provider.get_spawn_args(config, false), "--add-dir");
            provider_args.extend(strip_standalone_flag(spawn_args, "--no-alt-screen"));
            let codex = config.codex_config();
            if codex.skip_git_repo_check.unwrap_or(true) {
                provider_args.push("--skip-git-repo-check".to_string());
            }
            if codex.ephemeral.unwrap_or(false) {
                provider_args.push("--ephemeral".to_string());
            }
        }

        provider_args.push("--json".to_string());
        provider_args.push(session_bootstrap_prompt().to_string());
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
        // --print mode does not accept --input-format stream-json; strip it.
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
        cmd.stdin(std::process::Stdio::null());
    } else if provider_name == "claude" {
        cmd.env("CLAUDE_CODE_ADDITIONAL_DIRECTORIES_CLAUDE_MD", "1");
        cmd.stdin(std::process::Stdio::null());
    } else {
        cmd.stdin(std::process::Stdio::null());
    }

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
        "[WARDIAN-DEBUG] obtain_session_id launch: exe={} args={:?} cwd={}",
        launch_spec.executable,
        launch_spec.args,
        command_cwd.display()
    ));
    match cmd.spawn() {
        Ok(mut child) => {
            log_debug("[WARDIAN-DEBUG] Spawned headless process. Reading stdout...");
            let mut session_id_res = None;
            let mut stderr_output = String::new();

            let timeout = tokio::time::Duration::from_secs(60);
            let read_future = async {
                let mut session_id: Option<String> = None;
                if let Some(stdout) = child.stdout.take() {
                    let mut reader = BufReader::new(stdout);
                    let mut line = String::new();
                    while let Ok(n) = reader.read_line(&mut line).await {
                        if n == 0 {
                            log_debug("[WARDIAN-DEBUG] Reached EOF on stdout.");
                            break;
                        }
                        let trimmed = line.trim();
                        if let Some(start) = trimmed.find('{') {
                            let json_part = &trimmed[start..];
                            if provider_name == "opencode" {
                                if let Ok(parsed) =
                                    serde_json::from_str::<serde_json::Value>(json_part)
                                {
                                    if session_id.is_none() {
                                        session_id = parsed
                                            .get("sessionID")
                                            .and_then(|value| value.as_str())
                                            .map(|value| value.to_string());
                                    }
                                }
                            }
                            if let Some(evt) = provider.parse_output(json_part) {
                                match evt {
                                    AgentEvent::Init {
                                        session_id: sid, ..
                                    } if !sid.is_empty() => {
                                        log_debug(&format!(
                                            "[WARDIAN-DEBUG] Found session_id: {}",
                                            sid
                                        ));
                                        session_id = Some(sid);
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
                session_id
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
            if session_id_res.is_none() && !stderr_output.trim().is_empty() {
                log_debug(&format!(
                    "[WARDIAN-DEBUG] obtain_session_id stderr: {}",
                    stderr_output.trim()
                ));
            }
            if provider_name == "codex" {
                if let (Some(session_id), Some((_, bootstrap_home))) =
                    (session_id_res.as_ref(), codex_bootstrap.as_ref())
                {
                    if let Some(final_habitat_root) =
                        prepare_provider_habitat(provider_name, cwd, class_name, Some(session_id))?
                    {
                        migrate_codex_bootstrap_home(
                            bootstrap_home,
                            &habitat_codex_home(&final_habitat_root),
                        )?;
                    }
                }
            }
            log_debug(&format!(
                "[WARDIAN-DEBUG] Returning session_id: {:?}",
                session_id_res
            ));
            session_id_res.ok_or_else(|| {
                if stderr_output.trim().is_empty() {
                    format!(
                        "Provider {} did not return a session ID during initialization.",
                        provider_name
                    )
                } else {
                    stderr_output.trim().to_string()
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
    use std::path::{Path, PathBuf};
    #[test]
    fn bootstrap_session_prompt_uses_intro_prompt_for_providers_that_need_bootstrap() {
        assert_eq!(session_bootstrap_prompt(), "Introduce yourself");
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
    fn opencode_extensionless_headless_launch_uses_cmd_host_on_windows() {
        let args = vec![
            "run".to_string(),
            "--print-logs".to_string(),
            "first line\nsecond line".to_string(),
        ];

        let spec = headless_provider_launch("opencode", "C:/nvm4w/nodejs/opencode", &args)
            .expect("launch spec");

        assert!(spec.executable.ends_with("cmd.exe") || spec.executable == "cmd.exe");
        assert_eq!(spec.args[0], "/d");
        assert_eq!(spec.args[1], "/c");
        assert!(spec.args[2].contains("C:/nvm4w/nodejs/opencode"));
        assert!(spec.args[2].contains("first line\nsecond line"));
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
