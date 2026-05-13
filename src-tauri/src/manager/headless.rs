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
use super::quote_cmd_arg;

pub(crate) fn headless_provider_launch(
    provider_name: &str,
    bin: &str,
    provider_args: &[String],
) -> Result<crate::utils::shell::ShellLaunchSpec, String> {
    #[cfg(windows)]
    if provider_name == "opencode" {
        let cmd_host = std::env::var("ComSpec").unwrap_or_else(|_| "cmd.exe".to_string());
        let mut fragments = vec![quote_cmd_arg(bin)];
        fragments.extend(provider_args.iter().map(|arg| quote_cmd_arg(arg)));
        return Ok(crate::utils::shell::ShellLaunchSpec {
            executable: cmd_host,
            args: vec!["/d".to_string(), "/c".to_string(), fragments.join(" ")],
        });
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
            provider_args.push("exec".to_string());
            if let Some(resume_id) = resume_session.filter(|s| !s.trim().is_empty()) {
                provider_args.push("resume".to_string());
                provider_args.push(resume_id.to_string());
            }
            provider_args.push("--json".to_string());
            provider_args.push(prompt.to_string());
        }
        "claude" => {
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
    let habitat_root = prepare_provider_habitat(provider_name, cwd, "", Some(wardian_session_id))?;
    let provider_cwd = cwd.to_path_buf();
    let persisted_opencode_config = if provider_name == "opencode" {
        config_override
            .cloned()
            .or_else(|| persisted_agent_config(wardian_session_id))
    } else {
        None
    };
    let (bin, _) = provider.get_executable();
    let claude_hook = if provider_name == "claude" {
        ensure_claude_permission_hook(wardian_session_id).ok()
    } else {
        None
    };

    let mut provider_args = headless_provider_args(
        provider_name,
        provider.as_ref(),
        &provider_cwd,
        prompt,
        output_format,
        resume_session,
        persisted_opencode_config.as_ref(),
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
    if provider_name == "codex" {
        if let Some(root) = habitat_root.as_ref() {
            cmd.env("CODEX_HOME", habitat_codex_home(root));
        }
    } else if provider_name == "claude" {
        cmd.env("CLAUDE_CODE_ADDITIONAL_DIRECTORIES_CLAUDE_MD", "1");
    } else if provider_name == "opencode" {
        let class_name = persisted_opencode_config
            .as_ref()
            .map(|config| config.agent_class.as_str())
            .unwrap_or("");
        let opencode_scope_session = if resume_session.is_some() {
            resume_session
        } else {
            (!wardian_session_id.trim().is_empty()).then_some(wardian_session_id)
        };
        for (key, value) in opencode_env(
            &provider_cwd,
            class_name,
            opencode_scope_session,
            persisted_opencode_config.as_ref(),
        )? {
            cmd.env(key, value);
        }
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

    cmd.current_dir(&provider_cwd)
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
        cwd.display(),
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

    let _ = child.wait().await;

    if !err_output.is_empty() {
        log_debug(&format!("[Wardian] Headless stderr: {}", err_output.trim()));
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
    if !wardian_session_id.trim().is_empty() {
        cmd.env("WARDIAN_SESSION_ID", wardian_session_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
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
    fn headless_identity_env_is_omitted_when_session_id_is_blank() {
        let mut cmd = crate::utils::process::new_headless_command("node");

        apply_headless_identity_env(&mut cmd, "  ");

        let envs: Vec<_> = cmd.as_std().get_envs().collect();
        assert!(!envs
            .iter()
            .any(|(key, _value)| key.to_string_lossy() == "WARDIAN_SESSION_ID"));
    }
}
