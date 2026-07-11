use crate::providers::antigravity::AntigravityProvider;
use crate::providers::claude::{classify_claude_user_event, ClaudeUserEventKind};
use crate::providers::transcript::extract_transcript_message;
use crate::providers::ProviderFactory;
use crate::state::{ActiveAgent, AgentWatchState, AppState};
use crate::utils::fs::*;
use crate::utils::logging::{log_debug, log_terminal_trace_bytes, log_terminal_trace_note};
use crate::utils::PtyUtf8Decoder;
use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};
use std::io::{BufRead, Read, Seek, Write};
use tauri::{AppHandle, Emitter, Manager};
use wardian_core::control::ProviderInputReadiness;
use wardian_core::models::{AgentConfig, AgentEvent, ProviderConfig};

use super::claude::{claude_permission_hook_matches_session, claude_project_dir_name};
use super::codex::{
    codex_provider_session_is_excluded, codex_session_file_path, latest_codex_session_index_entry,
};
use super::opencode::{opencode_interactive_env, opencode_status_from_title};
use super::{
    apply_agent_event, apply_agent_event_with_policy, apply_agent_status_event,
    apply_agent_status_event_with_policy, apply_terminal_identity_env, debug_preview_bytes,
    extract_terminal_titles, finalize_interactive_spawn_args, interactive_provider_args,
    interactive_provider_cwd, interactive_provider_launch, provider_status_from_event,
    set_agent_status, ProviderStatusEventPolicy,
};
use crate::providers::gemini::gemini_status_from_title;

const OUTPUT_READY_EMIT_MIN_INTERVAL: std::time::Duration = std::time::Duration::from_millis(33);

#[derive(Default)]
struct OutputReadyEmitGate {
    last_emit_at: Option<std::time::Instant>,
    delayed_emit_scheduled: bool,
}

impl OutputReadyEmitGate {
    fn after_buffer_append(&mut self, now: std::time::Instant) -> OutputReadyEmitAction {
        let elapsed = self
            .last_emit_at
            .map(|last_emit_at| now.saturating_duration_since(last_emit_at));
        if elapsed.is_none_or(|elapsed| elapsed >= OUTPUT_READY_EMIT_MIN_INTERVAL) {
            self.last_emit_at = Some(now);
            self.delayed_emit_scheduled = false;
            return OutputReadyEmitAction::EmitNow;
        }

        if self.delayed_emit_scheduled {
            return OutputReadyEmitAction::Suppress;
        }

        self.delayed_emit_scheduled = true;
        OutputReadyEmitAction::ScheduleAfter(OUTPUT_READY_EMIT_MIN_INTERVAL - elapsed.unwrap())
    }

    fn finish_delayed_emit(&mut self, buffer_has_output: bool, now: std::time::Instant) -> bool {
        self.delayed_emit_scheduled = false;
        if !buffer_has_output {
            return false;
        }

        let elapsed = self
            .last_emit_at
            .map(|last_emit_at| now.saturating_duration_since(last_emit_at));
        if elapsed.is_none_or(|elapsed| elapsed >= OUTPUT_READY_EMIT_MIN_INTERVAL) {
            self.last_emit_at = Some(now);
            return true;
        }

        false
    }
}

#[derive(Debug, PartialEq, Eq)]
enum OutputReadyEmitAction {
    EmitNow,
    ScheduleAfter(std::time::Duration),
    Suppress,
}

#[derive(Default)]
struct CodexTerminalThemeProbeResponder {
    answered_light_dark: bool,
    answered_foreground: bool,
    answered_background: bool,
    answered_palette_zero: bool,
    tail: Vec<u8>,
}

impl CodexTerminalThemeProbeResponder {
    fn responses_for_chunk(
        &mut self,
        provider_name: &str,
        chunk: &[u8],
        theme: &str,
    ) -> Vec<Vec<u8>> {
        if provider_name != "codex" || chunk.is_empty() {
            self.remember_tail(chunk);
            return Vec::new();
        }

        let mut data = self.tail.clone();
        data.extend_from_slice(chunk);
        let terminal_theme = CodexTerminalTheme::from_wardian_theme(theme);
        let mut responses = Vec::new();

        if !self.answered_light_dark && contains_bytes(&data, b"\x1b[?996n") {
            self.answered_light_dark = true;
            responses.push(
                format!(
                    "\x1b[?997;{}n",
                    if terminal_theme.prefers_light { 2 } else { 1 }
                )
                .into_bytes(),
            );
        }

        if !self.answered_foreground
            && (contains_bytes(&data, b"\x1b]10;?\x07")
                || contains_bytes(&data, b"\x1b]10;?\x1b\\"))
        {
            self.answered_foreground = true;
            responses.push(format!("\x1b]10;rgb:{}\x1b\\", terminal_theme.foreground).into_bytes());
        }

        if !self.answered_background
            && (contains_bytes(&data, b"\x1b]11;?\x07")
                || contains_bytes(&data, b"\x1b]11;?\x1b\\"))
        {
            self.answered_background = true;
            responses.push(format!("\x1b]11;rgb:{}\x1b\\", terminal_theme.background).into_bytes());
        }

        if !self.answered_palette_zero && contains_bytes(&data, b"\x1b]4;0;?\x07") {
            self.answered_palette_zero = true;
            responses.push(format!("\x1b]4;0;rgb:{}\x07", terminal_theme.background).into_bytes());
        }

        self.remember_tail(&data);
        responses
    }

    fn remember_tail(&mut self, data: &[u8]) {
        const MAX_TERMINAL_PROBE_TAIL: usize = 32;
        let start = data.len().saturating_sub(MAX_TERMINAL_PROBE_TAIL);
        self.tail.clear();
        self.tail.extend_from_slice(&data[start..]);
    }
}

struct CodexTerminalTheme {
    foreground: &'static str,
    background: &'static str,
    prefers_light: bool,
}

impl CodexTerminalTheme {
    fn from_wardian_theme(theme: &str) -> Self {
        if theme.trim() == "light" {
            Self {
                foreground: "11/18/27",
                background: "fc/fa/f5",
                prefers_light: true,
            }
        } else {
            Self {
                foreground: "ee/f2/ee",
                background: "02/04/02",
                prefers_light: false,
            }
        }
    }
}

fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|candidate| candidate == needle)
}

fn codex_cleared_provider_sessions(config: &AgentConfig) -> Vec<String> {
    config.codex_config().cleared_provider_sessions
}

fn clear_codex_cleared_provider_sessions(config: &mut AgentConfig) {
    if config.provider == "codex" || matches!(config.provider_config, ProviderConfig::Codex(_)) {
        config
            .codex_config_mut_preserve_encoding()
            .cleared_provider_sessions
            .clear();
    }
    config.codex_cleared_provider_sessions.clear();
}

#[cfg(target_os = "macos")]
use super::macos_extended_path;
#[cfg(windows)]
use super::{
    app_process_supervisor_active, assign_pid_to_job, cleanup_stale_session_processes,
    create_kill_on_close_job,
};

pub(super) fn capture_codex_init_resume_session(
    provider_name: &str,
    session_id: &str,
    config: &mut AgentConfig,
) -> bool {
    let session_id = session_id.trim();
    if provider_name != "codex"
        || session_id.is_empty()
        || config
            .resume_session
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty())
        || codex_provider_session_is_excluded(session_id, &codex_cleared_provider_sessions(config))
    {
        return false;
    }

    config.resume_session = Some(session_id.to_string());
    clear_codex_cleared_provider_sessions(config);
    true
}

fn codex_status_log_session(
    config: &mut AgentConfig,
    latest_session: Option<String>,
) -> Option<String> {
    let mut cleared_provider_sessions = codex_cleared_provider_sessions(config);
    if config
        .resume_session
        .as_deref()
        .is_some_and(|value| codex_provider_session_is_excluded(value, &cleared_provider_sessions))
    {
        config.resume_session = None;
    }

    let candidate = config
        .resume_session
        .clone()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| latest_session.filter(|value| !value.trim().is_empty()));

    let candidate = candidate?;

    if codex_provider_session_is_excluded(&candidate, &cleared_provider_sessions) {
        return Some(candidate);
    }

    if config
        .resume_session
        .as_deref()
        .is_none_or(|value| value.trim().is_empty())
    {
        config.resume_session = Some(candidate.clone());
        clear_codex_cleared_provider_sessions(config);
        cleared_provider_sessions.clear();
    } else if config.resume_session.as_deref() == Some(candidate.as_str())
        && !cleared_provider_sessions.is_empty()
    {
        clear_codex_cleared_provider_sessions(config);
    }

    Some(candidate)
}

fn save_agent_state_after_session_capture(app: &AppHandle) {
    use tauri::Manager;

    let app_state = app.state::<crate::state::app_state::AppState>();
    let agents = app_state.agents.blocking_lock();
    let order = app_state.agent_order.blocking_lock();
    crate::manager::save_state(app, &agents, &order);
}

fn should_cleanup_stale_session_processes_before_spawn(is_restored: bool) -> bool {
    !is_restored
}

fn pty_status_event_policy_for_provider(provider_name: &str) -> ProviderStatusEventPolicy {
    if provider_name == "claude" || provider_name == "codex" {
        ProviderStatusEventPolicy::PreserveActionRequired
    } else {
        ProviderStatusEventPolicy::Normal
    }
}

fn line_event_status_for_pty_provider(
    provider_name: &str,
    current_status: &str,
    event: &AgentEvent,
) -> Option<&'static str> {
    provider_status_from_event(
        current_status,
        event,
        pty_status_event_policy_for_provider(provider_name),
    )
}

fn filter_ignored_conversation_id(
    detected: Option<String>,
    ignored: Option<&str>,
) -> Option<String> {
    if let Some(ref detected_id) = detected {
        if let Some(ignored_id) = ignored {
            if detected_id == ignored_id {
                return None;
            }
        }
    }
    detected
}

pub async fn spawn_agent(
    app: AppHandle,
    config: AgentConfig,
    is_restored: bool,
    initial_timestamp: Option<String>,
) -> Result<ActiveAgent, String> {
    let provider = ProviderFactory::resolve(&config.provider)?;
    crate::providers::readiness::ensure_provider_available_for_launch(&config.provider)?;

    let cwd = crate::utils::fs::resolve_cwd(&config.folder, &config.session_id);

    let expected_folder = if config.folder.is_empty() {
        cwd.to_string_lossy().to_string()
    } else {
        config.folder.clone()
    };

    // Phase 2: Record/Update agent in SQLite with explicit ISO 8601 timestamp
    let born_to_save = initial_timestamp
        .clone()
        .unwrap_or_else(|| chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true));
    let project = wardian_core::db::project_name_from_workspace(&expected_folder);
    let _ = wardian_core::db::upsert_agent(&wardian_core::db::AgentUpsert {
        session_id: &config.session_id,
        session_name: &config.session_name,
        agent_class: &config.agent_class,
        provider: &config.provider,
        workspace: Some(&expected_folder),
        project: project.as_deref(),
        is_off: config.is_off,
        created_at: Some(&born_to_save),
    });

    let app_state = app.state::<AppState>();
    if config.is_off {
        app_state
            .interactions
            .start_provider_input_generation(
                &config.session_id,
                ProviderInputReadiness::Unavailable,
                None,
            )
            .await;
        let _ = wardian_core::db::update_agent_status(&config.session_id, "Off", None);
        let session_id = config.session_id.clone();

        return Ok(ActiveAgent {
            config: std::sync::Arc::new(std::sync::Mutex::new(config)),
            child_process: None,
            background_processes: Vec::new(),
            runtime_generation: None,
            process_id: None,
            query_count: std::sync::Arc::new(std::sync::Mutex::new(0)),
            init_timestamp: std::sync::Arc::new(std::sync::Mutex::new(Some(born_to_save))),
            current_status: std::sync::Arc::new(std::sync::Mutex::new("Off".to_string())),
            last_status_at: std::sync::Arc::new(std::sync::Mutex::new(None)),
            watch_state: std::sync::Arc::new(std::sync::Mutex::new(AgentWatchState::new(
                session_id, 4096, 262_144,
            ))),
            terminal_title: std::sync::Arc::new(std::sync::Mutex::new(String::new())),
            last_output_at: std::sync::Arc::new(std::sync::Mutex::new(None)),
            log_path: std::sync::Arc::new(std::sync::Mutex::new(None)),
            log_last_modified: std::sync::Arc::new(std::sync::Mutex::new(None)),
            #[cfg(windows)]
            job_object: None,
        });
    }

    app_state
        .interactions
        .start_provider_input_generation(&config.session_id, ProviderInputReadiness::Booting, None)
        .await;

    let config_lock = std::sync::Arc::new(std::sync::Mutex::new(config.clone()));

    let initial_ignored_conversation_id = if config.provider == "antigravity" && !is_restored {
        let home = AntigravityProvider::antigravity_home();
        home.as_ref()
            .and_then(|home| AntigravityProvider::conversation_for_workspace(home, &cwd))
            .or_else(|| {
                home.as_ref()
                    .and_then(|home| AntigravityProvider::latest_conversation_id(home))
            })
    } else {
        None
    };

    #[cfg(windows)]
    if should_cleanup_stale_session_processes_before_spawn(is_restored) {
        cleanup_stale_session_processes(&config.session_id, &config.provider);
    }

    crate::commands::terminal::log_terminal_runtime_diagnostics_once();

    let pty_system = NativePtySystem::default();

    let initial_geometry = app_state
        .terminal_sessions
        .spawn_geometry(&config.session_id)
        .await
        .map_err(|error| format!("Failed to read terminal spawn geometry: {error}"))?
        .unwrap_or(wardian_core::models::TerminalGeometry { cols: 80, rows: 24 });
    let (initial_cols, initial_rows) = (initial_geometry.cols, initial_geometry.rows);

    let pair = pty_system
        .openpty(PtySize {
            rows: initial_rows,
            cols: initial_cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| format!("Failed to open pty: {}", e))?;

    let (bin, mut provider_args) = provider.get_executable();
    let claude_hook = if config.provider == "claude" {
        Some(ensure_claude_permission_hook(&config.session_id)?)
    } else {
        None
    };
    let habitat_root = prepare_provider_habitat(
        &config.provider,
        &cwd,
        &config.agent_class,
        Some(&config.session_id),
    )?;
    let provider_cwd =
        interactive_provider_cwd(&config.provider, &cwd, habitat_root.as_deref(), None);

    if config.provider == "claude" {
        if let Some(hook) = claude_hook.as_ref() {
            provider_args.push("--settings".to_string());
            provider_args.push(hook.settings_arg.clone());
        }
    }

    let background_processes = Vec::new();
    let is_resume = config
        .resume_session
        .as_deref()
        .is_some_and(|s| !s.is_empty());
    let spawn_args = provider.get_spawn_args(&config, is_resume);
    let spawn_args = finalize_interactive_spawn_args(
        &config.provider,
        is_restored,
        &config.resume_session,
        spawn_args,
    );
    provider_args.extend(spawn_args);
    provider_args = interactive_provider_args(&config.provider, &provider_cwd, &cwd, provider_args);

    let launch_spec = interactive_provider_launch(&config.provider, &bin, &provider_args)?;
    log_debug(&format!(
        "[Wardian] PTY spawn: provider={} exe={} args={:?} cwd={}",
        config.provider,
        launch_spec.executable,
        launch_spec.args,
        provider_cwd.display()
    ));
    let mut cmd = CommandBuilder::new(&launch_spec.executable);
    for arg in &launch_spec.args {
        cmd.arg(arg);
    }
    cmd.cwd(&provider_cwd);
    apply_terminal_identity_env(&mut cmd);
    super::apply_managed_cli_path_to_pty(&mut cmd);
    super::apply_interactive_provider_runtime_env(&config.provider, &mut cmd)?;
    cmd.env("WARDIAN_SESSION_ID", &config.session_id);
    for (key, value) in super::worktree_build_env(&config) {
        cmd.env(key, value);
    }

    if config.provider == "codex" {
        if let Some(root) = habitat_root.as_ref() {
            cmd.env("CODEX_HOME", habitat_codex_home(root));
        }
    } else if config.provider == "opencode" {
        for (key, value) in opencode_interactive_env(&provider_cwd, &config)? {
            cmd.env(key, value);
        }
    } else if config.provider == "mock" {
        let mut has_config_scenario = false;
        let mut has_config_delay = false;
        if let ProviderConfig::Mock(mock) = &config.provider_config {
            if let Some(scenario) = mock.scenario.as_deref().filter(|value| !value.is_empty()) {
                cmd.env("WARDIAN_MOCK_SCENARIO", scenario);
                has_config_scenario = true;
            }
            if let Some(delay_ms) = mock.delay_ms {
                cmd.env("WARDIAN_MOCK_DELAY_MS", delay_ms.to_string());
                has_config_delay = true;
            }
        }
        for key in [
            "WARDIAN_MOCK_SCENARIO",
            "WARDIAN_MOCK_DELAY_MS",
            "WARDIAN_MOCK_SCRIPT",
        ] {
            if (key == "WARDIAN_MOCK_SCENARIO" && has_config_scenario)
                || (key == "WARDIAN_MOCK_DELAY_MS" && has_config_delay)
            {
                continue;
            }
            if let Ok(value) = std::env::var(key) {
                cmd.env(key, value);
            }
        }
    }
    #[cfg(target_os = "macos")]
    cmd.env("PATH", macos_extended_path());

    let resume_id = config.resume_session.as_deref().unwrap_or("");
    log_debug(&format!(
        "[Wardian] Spawning {} agent. Session: {}, Resume ID: {}, Restored: {}",
        provider.name(),
        config.session_id,
        resume_id,
        is_restored
    ));

    let child = pair
        .slave
        .spawn_command(cmd)
        .map_err(|e| format!("Failed to spawn command: {}", e))?;

    let process_id = child.process_id();

    // Phase 2: Record/Update status in SQLite with the real PID
    let _ = wardian_core::db::update_agent_status(
        &config.session_id,
        if config.is_off { "Off" } else { "Idle" },
        process_id,
    );

    #[cfg(windows)]
    let job_object = {
        if app_process_supervisor_active() {
            None
        } else if let Ok(job) = create_kill_on_close_job("agent fallback") {
            if let Some(pid) = process_id {
                if let Err(err) = assign_pid_to_job(&job, pid, "agent fallback") {
                    log_debug(&format!(
                        "[Wardian] Failed to assign session {} PID {} to fallback job: {}",
                        config.session_id, pid, err
                    ));
                }
            }
            Some(job)
        } else {
            None
        }
    };
    let mut reader = pair
        .master
        .try_clone_reader()
        .map_err(|e| format!("Failed to get pty reader: {}", e))?;
    let mut writer = pair
        .master
        .take_writer()
        .map_err(|e| format!("Failed to get pty writer: {}", e))?;
    let pty_master: crate::state::terminal_session::SharedPtyMaster =
        std::sync::Arc::new(std::sync::Mutex::new(pair.master));
    drop(pair.slave);

    let (tx, mut rx) = tokio::sync::mpsc::channel::<Vec<u8>>(256);
    let runtime_generation = app_state
        .terminal_sessions
        .start_or_replace_runtime(
            &config.session_id,
            crate::state::terminal_session::native_terminal_runtime(tx.clone(), pty_master),
            initial_geometry,
        )
        .await
        .map_err(|error| format!("Failed to start terminal session broker: {error}"))?;
    if let Ok(mut senders) = app_state.input_senders.write() {
        senders.insert(config.session_id.clone(), tx.clone());
    }
    let sid_for_input = config.session_id.clone();
    let provider_name_for_input = config.provider.clone();

    std::thread::spawn(move || {
        while let Some(input) = rx.blocking_recv() {
            if provider_name_for_input == "opencode" {
                log_debug(&format!(
                    "[Wardian] OpenCode PTY input for session {}: {}",
                    sid_for_input,
                    debug_preview_bytes(&input, 128)
                ));
            }
            log_terminal_trace_bytes(&sid_for_input, &provider_name_for_input, "IN", &input);
            let _ = writer.write_all(&input);
            let _ = writer.flush();
        }
        log_terminal_trace_note(
            &sid_for_input,
            &provider_name_for_input,
            "input channel closed",
        );
    });

    let sid_out = config.session_id.clone();
    let provider_name_for_pty = config.provider.clone();
    let query_count = std::sync::Arc::new(std::sync::Mutex::new(0));
    let query_count_clone = query_count.clone();
    let init_timestamp = std::sync::Arc::new(std::sync::Mutex::new(Some(born_to_save)));
    let init_timestamp_clone = init_timestamp.clone();
    let current_status = std::sync::Arc::new(std::sync::Mutex::new("Idle".to_string()));
    let current_status_clone = current_status.clone();
    let watch_state = std::sync::Arc::new(std::sync::Mutex::new(AgentWatchState::new(
        config.session_id.clone(),
        4096,
        262_144,
    )));
    let watch_state_clone = watch_state.clone();
    let terminal_title = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let terminal_title_clone = terminal_title.clone();
    let last_output_at = std::sync::Arc::new(std::sync::Mutex::new(None));
    let last_output_at_clone = last_output_at.clone();
    let log_path = std::sync::Arc::new(std::sync::Mutex::new(None::<std::path::PathBuf>));
    // PTY reader thread: uses provider.parse_output() for event classification
    let pty_app = app.clone();
    let pty_provider = provider.clone();
    let sid_for_pty = sid_out.clone();
    let pty_emit_app = app.clone();
    let terminal_attach = app.state::<AppState>().terminal_attach.clone();
    let terminal_theme_for_pty = app_state.terminal_theme();
    let tx_for_terminal_probe = tx.clone();
    let config_lock_clone = config_lock.clone();
    let terminal_sessions = app_state.terminal_sessions.clone();
    let reader_runtime_generation = runtime_generation;
    std::thread::spawn(move || {
        let mut buf = [0; 4096];
        let mut current_line = String::new();
        let mut had_pty_output = false;
        let mut opencode_chunks_logged = 0usize;
        let mut codex_terminal_theme_responder = CodexTerminalThemeProbeResponder::default();
        let mut pty_decoder = PtyUtf8Decoder::new();
        let output_ready_emit_gate =
            std::sync::Arc::new(std::sync::Mutex::new(OutputReadyEmitGate::default()));
        loop {
            match reader.read(&mut buf) {
                Ok(0) => {
                    log_terminal_trace_note(&sid_for_pty, &provider_name_for_pty, "pty EOF");
                    if provider_name_for_pty == "opencode" {
                        log_debug(&format!(
                            "[Wardian] OpenCode PTY EOF for session {} (had_output={})",
                            sid_for_pty, had_pty_output
                        ));
                    }
                    // If the process exited immediately with no output, surface a
                    // diagnostic message so the terminal is not silently blank.
                    if !had_pty_output && provider_name_for_pty == "opencode" {
                        let msg = concat!(
                            "\r\n[Wardian] OpenCode exited without producing any output.\r\n",
                            "Possible causes:\r\n",
                            "  - generated OpenCode runtime config is invalid (check ~/.wardian/wardian_debug.log)\r\n",
                            "  - OpenCode binary not found or failed to start\r\n",
                            "  - Authentication/config error in OpenCode\r\n",
                            "Check ~/.wardian/wardian_debug.log for the exact command and config used.\r\n",
                        );
                        let _ = crate::state::terminal_session::forward_terminal_output(
                            &terminal_sessions,
                            &sid_for_pty,
                            reader_runtime_generation,
                            msg.as_bytes(),
                        );
                        let _ = pty_emit_app.emit(
                            "agent-pty-output-ready",
                            serde_json::json!({ "session_id": sid_for_pty }),
                        );
                    }
                    break;
                }
                Ok(n) => {
                    if let Err(error) = crate::state::terminal_session::forward_terminal_output(
                        &terminal_sessions,
                        &sid_for_pty,
                        reader_runtime_generation,
                        &buf[..n],
                    ) {
                        log_terminal_trace_note(
                            &sid_for_pty,
                            &provider_name_for_pty,
                            &format!("broker rejected PTY reader output: {error}"),
                        );
                        break;
                    }
                    if provider_name_for_pty == "opencode" && opencode_chunks_logged < 40 {
                        log_debug(&format!(
                            "[Wardian] OpenCode PTY chunk {} for session {}: {}",
                            opencode_chunks_logged + 1,
                            sid_for_pty,
                            debug_preview_bytes(&buf[0..n], 256)
                        ));
                        opencode_chunks_logged += 1;
                    }
                    had_pty_output = true;
                    for response in codex_terminal_theme_responder.responses_for_chunk(
                        &provider_name_for_pty,
                        &buf[0..n],
                        &terminal_theme_for_pty,
                    ) {
                        let _ = tx_for_terminal_probe.blocking_send(response);
                    }
                    terminal_attach.process_output(&sid_for_pty, &buf[0..n]);
                    if let Ok(mut watch_state) = watch_state_clone.lock() {
                        watch_state.push_output(&buf[0..n]);
                    }
                    log_terminal_trace_bytes(
                        &sid_for_pty,
                        &provider_name_for_pty,
                        "OUT",
                        &buf[0..n],
                    );
                    let text = pty_decoder.decode_chunk(&buf[0..n]);
                    if let Ok(mut stamp) = last_output_at_clone.lock() {
                        *stamp = Some(std::time::SystemTime::now());
                    }

                    // Process stream events to capture Session ID / Status changes
                    // Use a simple line-based approach for stream-json events
                    for line in text.lines() {
                        if let Some(event) = pty_provider.parse_output(line) {
                            if provider_name_for_pty == "claude" {
                                if let AgentEvent::Init {
                                    session_id,
                                    timestamp,
                                } = &event
                                {
                                    if let Some(ts) = timestamp {
                                        let mut it = init_timestamp_clone.lock().unwrap();
                                        if it.is_none() {
                                            *it = Some(ts.clone());
                                        }
                                    }
                                    if !session_id.trim().is_empty() {
                                        let needs_save = {
                                            let mut config = config_lock_clone.lock().unwrap();
                                            if config.resume_session.as_deref()
                                                != Some(session_id.as_str())
                                            {
                                                config.resume_session = Some(session_id.clone());
                                                true
                                            } else {
                                                false
                                            }
                                        };
                                        if needs_save {
                                            log_debug(&format!(
                                                "[Wardian] Session ID mapped for {}: {}",
                                                sid_for_pty, session_id
                                            ));
                                            let _ = pty_emit_app.emit("agents-updated", ());
                                            let _ = pty_emit_app.emit(
                                                "agent-pty-output-ready",
                                                serde_json::json!({ "session_id": sid_for_pty }),
                                            );
                                            save_agent_state_after_session_capture(&pty_emit_app);
                                        }
                                    }
                                }
                                apply_agent_status_event_with_policy(
                                    &pty_app,
                                    &sid_for_pty,
                                    event,
                                    &current_status_clone,
                                    ProviderStatusEventPolicy::PreserveActionRequired,
                                );
                                continue;
                            }

                            if let AgentEvent::Init {
                                session_id,
                                timestamp,
                            } = &event
                            {
                                if let Some(ts) = timestamp {
                                    let mut it = init_timestamp_clone.lock().unwrap();
                                    if it.is_none() {
                                        *it = Some(ts.clone());
                                    }
                                }
                                if !session_id.trim().is_empty() {
                                    let needs_save = {
                                        let mut config = config_lock_clone.lock().unwrap();
                                        if capture_codex_init_resume_session(
                                            &provider_name_for_pty,
                                            session_id,
                                            &mut config,
                                        ) {
                                            true
                                        } else if provider_name_for_pty != "codex"
                                            && config.resume_session.as_deref()
                                                != Some(session_id.as_str())
                                        {
                                            config.resume_session = Some(session_id.clone());
                                            true
                                        } else {
                                            false
                                        }
                                    };
                                    if needs_save {
                                        log_debug(&format!(
                                            "[Wardian] Session ID mapped for {}: {}",
                                            sid_for_pty, session_id
                                        ));

                                        // Notify UI that metadata (resume_session ID) has changed
                                        let _ = pty_emit_app.emit("agents-updated", ());
                                        let _ = pty_emit_app.emit(
                                            "agent-pty-output-ready",
                                            serde_json::json!({ "session_id": sid_for_pty }),
                                        );
                                        save_agent_state_after_session_capture(&pty_emit_app);
                                    }
                                }
                            }
                            let current = current_status_clone
                                .lock()
                                .map(|status| status.clone())
                                .unwrap_or_default();
                            if let Some(next_status) = line_event_status_for_pty_provider(
                                &provider_name_for_pty,
                                &current,
                                &event,
                            ) {
                                set_agent_status(
                                    &pty_app,
                                    &sid_for_pty,
                                    &current_status_clone,
                                    next_status,
                                );
                            }
                        }
                    }

                    if let Some(title) = extract_terminal_titles(&text).into_iter().last() {
                        let _previous_title = terminal_title_clone
                            .lock()
                            .map(|value| value.clone())
                            .unwrap_or_default();
                        if provider_name_for_pty == "opencode" {
                            log_debug(&format!(
                                "[Wardian] OpenCode backend title for session {}: {}",
                                sid_for_pty, title
                            ));
                        }
                        if let Ok(mut current_title) = terminal_title_clone.lock() {
                            *current_title = title.clone();
                        }
                        if provider_name_for_pty == "opencode" {
                            if let Some(next_status) = opencode_status_from_title(&title) {
                                set_agent_status(
                                    &pty_emit_app,
                                    &sid_for_pty,
                                    &current_status_clone,
                                    next_status,
                                );
                            }
                        } else if provider_name_for_pty == "gemini" {
                            if let Some(next_status) = gemini_status_from_title(&title) {
                                set_agent_status(
                                    &pty_emit_app,
                                    &sid_for_pty,
                                    &current_status_clone,
                                    next_status,
                                );
                            }
                        }
                    }
                    let output_ready_action = output_ready_emit_gate
                        .lock()
                        .map(|mut gate| gate.after_buffer_append(std::time::Instant::now()))
                        .unwrap_or(OutputReadyEmitAction::Suppress);
                    match output_ready_action {
                        OutputReadyEmitAction::EmitNow => {
                            let _ = pty_emit_app.emit(
                                "agent-pty-output-ready",
                                serde_json::json!({ "session_id": sid_for_pty }),
                            );
                        }
                        OutputReadyEmitAction::ScheduleAfter(delay) => {
                            let delayed_app = pty_emit_app.clone();
                            let delayed_session_id = sid_for_pty.clone();
                            let delayed_gate = output_ready_emit_gate.clone();
                            tauri::async_runtime::spawn(async move {
                                tokio::time::sleep(delay).await;
                                let should_emit = delayed_gate
                                    .lock()
                                    .map(|mut gate| {
                                        gate.finish_delayed_emit(true, std::time::Instant::now())
                                    })
                                    .unwrap_or(false);
                                if should_emit {
                                    let _ = delayed_app.emit(
                                        "agent-pty-output-ready",
                                        serde_json::json!({ "session_id": delayed_session_id }),
                                    );
                                }
                            });
                        }
                        OutputReadyEmitAction::Suppress => {}
                    }
                    current_line.push_str(&text);
                    loop {
                        if let Some(start) = current_line.find('{') {
                            let slice = &current_line[start..];
                            let mut stream = serde_json::Deserializer::from_str(slice)
                                .into_iter::<serde_json::Value>();
                            match stream.next() {
                                Some(Ok(parsed)) => {
                                    // Use provider to classify the raw JSON into an AgentEvent
                                    let raw_line = parsed.to_string();
                                    if let Some(message) = extract_transcript_message(
                                        &provider_name_for_pty,
                                        &raw_line,
                                    ) {
                                        if let Ok(mut watch_state) = watch_state_clone.lock() {
                                            watch_state.push_transcript(message);
                                        }
                                    }
                                    if let Some(event) = pty_provider.parse_output(&raw_line) {
                                        if let AgentEvent::Init {
                                            ref session_id,
                                            ref timestamp,
                                        } = event
                                        {
                                            if let Some(ts) = timestamp {
                                                let mut it = init_timestamp_clone.lock().unwrap();
                                                if it.is_none() {
                                                    *it = Some(ts.clone());
                                                }
                                            }
                                            if !session_id.trim().is_empty() {
                                                let needs_save = {
                                                    let mut config =
                                                        config_lock_clone.lock().unwrap();
                                                    if capture_codex_init_resume_session(
                                                        &provider_name_for_pty,
                                                        session_id,
                                                        &mut config,
                                                    ) {
                                                        true
                                                    } else if provider_name_for_pty != "codex"
                                                        && config.resume_session.as_deref()
                                                            != Some(session_id.as_str())
                                                    {
                                                        config.resume_session =
                                                            Some(session_id.clone());
                                                        true
                                                    } else {
                                                        false
                                                    }
                                                };

                                                if needs_save {
                                                    log_debug(&format!(
                                                        "[Wardian] Session ID mapped for {}: {}",
                                                        sid_for_pty, session_id
                                                    ));
                                                    // Notify UI that metadata (resume_session ID) has changed
                                                    let _ = pty_emit_app.emit("agents-updated", ());
                                                    let _ = pty_emit_app.emit(
                                                        "agent-pty-output-ready",
                                                        serde_json::json!({ "session_id": sid_for_pty }),
                                                    );
                                                    save_agent_state_after_session_capture(
                                                        &pty_emit_app,
                                                    );
                                                }
                                            }
                                        }

                                        let status_policy = if provider_name_for_pty == "claude"
                                            || provider_name_for_pty == "codex"
                                        {
                                            ProviderStatusEventPolicy::PreserveActionRequired
                                        } else {
                                            ProviderStatusEventPolicy::Normal
                                        };
                                        apply_agent_event_with_policy(
                                            &pty_app,
                                            &sid_for_pty,
                                            event,
                                            &query_count_clone,
                                            &init_timestamp_clone,
                                            &current_status_clone,
                                            status_policy,
                                        );
                                    }
                                    let _ = pty_emit_app.emit("agent-json-event", serde_json::json!({ "session_id": sid_out, "data": parsed }));
                                    let consumed = stream.byte_offset();
                                    current_line = current_line[start + consumed..].to_string();
                                    continue;
                                }
                                _ => break,
                            }
                        }
                        break;
                    }
                    if current_line.len() > 10000 {
                        current_line.clear();
                    }
                }
                Err(err) => {
                    log_terminal_trace_note(
                        &sid_for_pty,
                        &provider_name_for_pty,
                        &format!("pty read error: {}", err),
                    );
                    break;
                }
            }
        }
        // Process terminated (EOF or error) — mark status as Off
        set_agent_status(&pty_app, &sid_for_pty, &current_status_clone, "Off");
    });

    if config.provider == "codex" {
        let watcher_app = app.clone();
        let watcher_provider = provider.clone();
        let watcher_session = config.session_id.clone();
        let watcher_query_count = query_count.clone();
        let watcher_init_timestamp = init_timestamp.clone();
        let watcher_current_status = current_status.clone();
        let watcher_log_path = log_path.clone();
        let watcher_config = config_lock.clone();
        let watcher_watch_state = watch_state.clone();
        let watcher_skip_existing_log = is_restored;
        let wardian_agent_dir = get_wardian_home()
            .map(|home| home.join("agents").join(&watcher_session))
            .filter(|path| path.exists())
            .map(|path| path.to_string_lossy().to_string());

        std::thread::spawn(move || {
            // Tail-follow the resolved session log every iteration, but run
            // session discovery (which walks and samples every rollout file in
            // the agent's codex home) only on this interval.
            const CODEX_SESSION_DISCOVERY_INTERVAL: std::time::Duration =
                std::time::Duration::from_secs(5);
            let mut offset: u64 = 0;
            let mut last_lookup_session = String::new();
            let mut positioned_initial_log = !watcher_skip_existing_log;
            let mut cached_latest_session: Option<String> = None;
            let mut last_discovery: Option<std::time::Instant> = None;
            loop {
                let current = watcher_current_status
                    .lock()
                    .map(|s| s.clone())
                    .unwrap_or_else(|e| e.into_inner().clone());
                if current == "Off" {
                    break;
                }

                let path = {
                    let discovery_due = last_discovery
                        .is_none_or(|at| at.elapsed() >= CODEX_SESSION_DISCOVERY_INTERVAL);
                    if discovery_due {
                        cached_latest_session = latest_codex_session_index_entry(&watcher_session)
                            .ok()
                            .flatten()
                            .map(|(session_id, _updated_at)| session_id);
                        last_discovery = Some(std::time::Instant::now());
                    }
                    let latest_session = cached_latest_session.clone();
                    let lookup_session = watcher_config.lock().ok().and_then(|mut cfg| {
                        let previous_resume = cfg.resume_session.clone();
                        let previous_cleared = codex_cleared_provider_sessions(&cfg);
                        let lookup = codex_status_log_session(&mut cfg, latest_session);
                        if cfg.resume_session != previous_resume
                            || codex_cleared_provider_sessions(&cfg) != previous_cleared
                        {
                            let _ = watcher_app.emit("agents-updated", ());
                        }
                        lookup
                    });
                    let mut lock = watcher_log_path.lock().unwrap_or_else(|e| e.into_inner());
                    if let Some(lookup_session) = lookup_session {
                        if last_lookup_session != lookup_session {
                            *lock = None;
                            offset = 0;
                            positioned_initial_log = !watcher_skip_existing_log;
                            last_lookup_session = lookup_session.clone();
                        }
                        if lock.is_none() {
                            *lock = codex_session_file_path(
                                &lookup_session,
                                wardian_agent_dir.as_deref(),
                            );
                        }
                        lock.clone()
                    } else {
                        *lock = None;
                        offset = 0;
                        last_lookup_session.clear();
                        None
                    }
                };

                if let Some(path) = path {
                    if let Ok(mut out) = watcher_log_path.lock() {
                        *out = Some(path.clone());
                    }
                    if let Ok(mut file) = std::fs::File::open(&path) {
                        if let Ok(metadata) = file.metadata() {
                            if metadata.len() < offset {
                                offset = 0;
                            }
                            if !positioned_initial_log {
                                offset = metadata.len();
                                positioned_initial_log = true;
                            }
                        }
                        if file.seek(std::io::SeekFrom::Start(offset)).is_ok() {
                            let mut reader = std::io::BufReader::new(file);
                            let mut line = String::new();
                            loop {
                                line.clear();
                                let read = reader.read_line(&mut line).unwrap_or(0);
                                if read == 0 {
                                    break;
                                }
                                offset += read as u64;
                                if let Ok(parsed) =
                                    serde_json::from_str::<serde_json::Value>(line.trim())
                                {
                                    let raw_line = parsed.to_string();
                                    if let Some(message) =
                                        extract_transcript_message("codex", &raw_line)
                                    {
                                        if let Ok(mut watch_state) = watcher_watch_state.lock() {
                                            watch_state.push_transcript(message);
                                        }
                                    }
                                    if let Some(event) = watcher_provider.parse_output(&raw_line) {
                                        apply_agent_event_with_policy(
                                            &watcher_app,
                                            &watcher_session,
                                            event,
                                            &watcher_query_count,
                                            &watcher_init_timestamp,
                                            &watcher_current_status,
                                            ProviderStatusEventPolicy::PreserveActionRequired,
                                        );
                                    }
                                    let _ = watcher_app.emit(
                                        "agent-json-event",
                                        serde_json::json!({ "session_id": watcher_session, "data": parsed }),
                                    );
                                }
                            }
                        }
                    }
                }

                std::thread::sleep(std::time::Duration::from_millis(250));
            }
        });
    } else if config.provider == "claude" {
        let watcher_app = app.clone();
        let watcher_provider = provider.clone();
        let watcher_session = config.session_id.clone();
        let watcher_current_status = current_status.clone();
        let watcher_log_path = log_path.clone();
        let watcher_folder = expected_folder.clone();
        let watcher_watch_state = watch_state.clone();
        let watcher_skip_existing_log = is_restored;
        let hook_event_log = claude_hook.as_ref().map(|hook| hook.event_log_path.clone());
        let waiting_for_permission = std::sync::Arc::new(std::sync::Mutex::new(false));
        let log_waiting_for_permission = waiting_for_permission.clone();

        std::thread::spawn(move || {
            let mut offset: u64 = 0;
            let mut positioned_initial_log = !watcher_skip_existing_log;
            loop {
                let current = watcher_current_status
                    .lock()
                    .map(|s| s.clone())
                    .unwrap_or_else(|e| e.into_inner().clone());
                if current == "Off" {
                    break;
                }

                let path = {
                    let mut lock = watcher_log_path.lock().unwrap_or_else(|e| e.into_inner());
                    if lock.is_none() {
                        if let Some(home) = dirs::home_dir() {
                            let candidate = home
                                .join(".claude")
                                .join("projects")
                                .join(claude_project_dir_name(&watcher_folder))
                                .join(format!("{}.jsonl", watcher_session));
                            if candidate.exists() {
                                *lock = Some(candidate);
                            }
                        }
                    }
                    lock.clone()
                };

                if let Some(path) = path {
                    if let Ok(mut out) = watcher_log_path.lock() {
                        *out = Some(path.clone());
                    }
                    if let Ok(mut file) = std::fs::File::open(&path) {
                        if let Ok(metadata) = file.metadata() {
                            if metadata.len() < offset {
                                offset = 0;
                                positioned_initial_log = true;
                            }
                            if !positioned_initial_log {
                                offset = metadata.len();
                                positioned_initial_log = true;
                            }
                        }
                        if file.seek(std::io::SeekFrom::Start(offset)).is_ok() {
                            let mut reader = std::io::BufReader::new(file);
                            let mut line = String::new();
                            loop {
                                line.clear();
                                let read = reader.read_line(&mut line).unwrap_or(0);
                                if read == 0 {
                                    break;
                                }
                                offset += read as u64;
                                if let Some(message) =
                                    extract_transcript_message("claude", line.trim())
                                {
                                    if let Ok(mut watch_state) = watcher_watch_state.lock() {
                                        watch_state.push_transcript(message);
                                    }
                                }
                                if let Some(event) = watcher_provider.parse_output(line.trim()) {
                                    let mut waiting = log_waiting_for_permission
                                        .lock()
                                        .unwrap_or_else(|e| e.into_inner());
                                    if *waiting {
                                        match event {
                                            AgentEvent::UserQuery | AgentEvent::Generating => {
                                                if let Ok(parsed) =
                                                    serde_json::from_str::<serde_json::Value>(
                                                        line.trim(),
                                                    )
                                                {
                                                    let is_tool_result =
                                                        parsed.get("type").and_then(|v| v.as_str())
                                                            == Some("user")
                                                            && classify_claude_user_event(&parsed)
                                                                == ClaudeUserEventKind::ToolResult;
                                                    if is_tool_result {
                                                        *waiting = false;
                                                        apply_agent_status_event(
                                                            &watcher_app,
                                                            &watcher_session,
                                                            event,
                                                            &watcher_current_status,
                                                        );
                                                    }
                                                }
                                            }
                                            AgentEvent::ModelResponse => {
                                                *waiting = false;
                                                apply_agent_status_event(
                                                    &watcher_app,
                                                    &watcher_session,
                                                    event,
                                                    &watcher_current_status,
                                                );
                                            }
                                            AgentEvent::ActionRequired { .. } => {
                                                apply_agent_status_event(
                                                    &watcher_app,
                                                    &watcher_session,
                                                    event,
                                                    &watcher_current_status,
                                                );
                                            }
                                            AgentEvent::TurnCompleted => {
                                                *waiting = false;
                                                apply_agent_status_event(
                                                    &watcher_app,
                                                    &watcher_session,
                                                    event,
                                                    &watcher_current_status,
                                                );
                                            }
                                            AgentEvent::Init { .. } | AgentEvent::Unknown => {}
                                        }
                                    } else {
                                        apply_agent_status_event(
                                            &watcher_app,
                                            &watcher_session,
                                            event,
                                            &watcher_current_status,
                                        );
                                    }
                                }
                            }
                        }
                    }
                }

                std::thread::sleep(std::time::Duration::from_millis(250));
            }
        });

        if let Some(hook_event_log) = hook_event_log {
            let hook_app = app.clone();
            let hook_session = config.session_id.clone();
            let hook_accepted_sessions = {
                let mut sessions = vec![config.session_id.clone()];
                if let Some(resume_session) = config
                    .resume_session
                    .as_ref()
                    .map(|sid| sid.trim())
                    .filter(|sid| !sid.is_empty() && *sid != config.session_id)
                {
                    sessions.push(resume_session.to_string());
                }
                if let Some(fresh_provider_session_id) = config
                    .fresh_provider_session_id
                    .as_ref()
                    .map(|sid| sid.trim())
                    .filter(|sid| !sid.is_empty() && *sid != config.session_id)
                {
                    sessions.push(fresh_provider_session_id.to_string());
                }
                sessions
            };
            let hook_current_status = current_status.clone();
            let hook_waiting_for_permission = waiting_for_permission.clone();

            std::thread::spawn(move || {
                let mut offset = 0;
                loop {
                    let current = hook_current_status
                        .lock()
                        .map(|s| s.clone())
                        .unwrap_or_else(|e| e.into_inner().clone());
                    if current == "Off" {
                        break;
                    }

                    if let Ok(mut file) = std::fs::File::open(&hook_event_log) {
                        if let Ok(metadata) = file.metadata() {
                            if metadata.len() < offset {
                                offset = 0;
                            }
                        }
                        if file.seek(std::io::SeekFrom::Start(offset)).is_ok() {
                            let mut reader = std::io::BufReader::new(file);
                            let mut line = String::new();
                            loop {
                                line.clear();
                                let read = reader.read_line(&mut line).unwrap_or(0);
                                if read == 0 {
                                    break;
                                }
                                offset += read as u64;
                                if let Ok(parsed) =
                                    serde_json::from_str::<serde_json::Value>(line.trim())
                                {
                                    if !hook_accepted_sessions.iter().any(|session_id| {
                                        claude_permission_hook_matches_session(&parsed, session_id)
                                    }) {
                                        continue;
                                    }
                                    if let Ok(mut waiting) = hook_waiting_for_permission.lock() {
                                        *waiting = true;
                                    }
                                    let tool_name = parsed
                                        .get("tool_name")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("Tool approval required")
                                        .to_string();
                                    apply_agent_status_event(
                                        &hook_app,
                                        &hook_session,
                                        AgentEvent::ActionRequired {
                                            message: tool_name.clone(),
                                        },
                                        &hook_current_status,
                                    );
                                    let _ = hook_app.emit(
                                        "agent-json-event",
                                        serde_json::json!({
                                            "session_id": hook_session,
                                            "data": {
                                                "type": "system",
                                                "subtype": "permission_request",
                                                "tool_name": tool_name,
                                            }
                                        }),
                                    );
                                }
                            }
                        }
                    }

                    std::thread::sleep(std::time::Duration::from_millis(250));
                }
            });
        }
    } else if config.provider == "antigravity" {
        let watcher_app = app.clone();
        let watcher_provider = provider.clone();
        let watcher_session = config.session_id.clone();
        let watcher_query_count = query_count.clone();
        let watcher_init_timestamp = init_timestamp.clone();
        let watcher_current_status = current_status.clone();
        let watcher_log_path = log_path.clone();
        let watcher_config = config_lock.clone();
        let watcher_watch_state = watch_state.clone();
        let watcher_skip_existing_log = is_restored;
        let watcher_workspace = cwd.clone();
        let initial_ignored = initial_ignored_conversation_id.clone();

        std::thread::spawn(move || {
            let mut offset: u64 = 0;
            let mut positioned_initial_log = !watcher_skip_existing_log;
            let mut last_conversation_id = String::new();
            loop {
                let current = watcher_current_status
                    .lock()
                    .map(|s| s.clone())
                    .unwrap_or_else(|e| e.into_inner().clone());
                if current == "Off" {
                    break;
                }

                let home = AntigravityProvider::antigravity_home();
                let conversation_id = {
                    let configured = {
                        let cfg = watcher_config.lock().unwrap_or_else(|e| e.into_inner());
                        cfg.resume_session
                            .as_ref()
                            .map(|value| value.trim().to_string())
                            .filter(|value| !value.is_empty())
                    };
                    configured.or_else(|| {
                        let detected = home
                            .as_ref()
                            .and_then(|home| {
                                AntigravityProvider::conversation_for_workspace(
                                    home,
                                    &watcher_workspace,
                                )
                            })
                            .or_else(|| {
                                home.as_ref().and_then(|home| {
                                    AntigravityProvider::latest_conversation_id(home)
                                })
                            });
                        filter_ignored_conversation_id(detected, initial_ignored.as_deref())
                    })
                };

                let path = home
                    .as_ref()
                    .zip(conversation_id.as_deref())
                    .map(|(home, conversation_id)| {
                        AntigravityProvider::transcript_path(home, conversation_id)
                    })
                    .filter(|path| path.exists());

                if let (Some(conversation_id), Some(path)) = (conversation_id, path) {
                    if last_conversation_id != conversation_id {
                        offset = 0;
                        positioned_initial_log = !watcher_skip_existing_log;
                        last_conversation_id = conversation_id.clone();
                    }

                    let needs_save = {
                        let mut cfg = watcher_config.lock().unwrap_or_else(|e| e.into_inner());
                        if cfg.resume_session.as_deref() != Some(conversation_id.as_str()) {
                            cfg.resume_session = Some(conversation_id.clone());
                            true
                        } else {
                            false
                        }
                    };
                    if needs_save {
                        let _ = watcher_app.emit("agents-updated", ());
                        save_agent_state_after_session_capture(&watcher_app);
                    }

                    if let Ok(mut out) = watcher_log_path.lock() {
                        *out = Some(path.clone());
                    }
                    if let Ok(mut file) = std::fs::File::open(&path) {
                        if let Ok(metadata) = file.metadata() {
                            if metadata.len() < offset {
                                offset = 0;
                                positioned_initial_log = true;
                            }
                            if !positioned_initial_log {
                                offset = metadata.len();
                                positioned_initial_log = true;
                            }
                        }
                        if file.seek(std::io::SeekFrom::Start(offset)).is_ok() {
                            let mut reader = std::io::BufReader::new(file);
                            let mut line = String::new();
                            loop {
                                line.clear();
                                let read = reader.read_line(&mut line).unwrap_or(0);
                                if read == 0 {
                                    break;
                                }
                                offset += read as u64;
                                let trimmed = line.trim();
                                if trimmed.is_empty() {
                                    continue;
                                }
                                if let Ok(parsed) =
                                    serde_json::from_str::<serde_json::Value>(trimmed)
                                {
                                    let raw_line = parsed.to_string();
                                    if let Some(message) =
                                        extract_transcript_message("antigravity", &raw_line)
                                    {
                                        if let Ok(mut watch_state) = watcher_watch_state.lock() {
                                            watch_state.push_transcript(message);
                                        }
                                    }
                                    if let Some(event) = watcher_provider.parse_output(&raw_line) {
                                        apply_agent_event(
                                            &watcher_app,
                                            &watcher_session,
                                            event,
                                            &watcher_query_count,
                                            &watcher_init_timestamp,
                                            &watcher_current_status,
                                        );
                                    }
                                    let _ = watcher_app.emit(
                                        "agent-json-event",
                                        serde_json::json!({ "session_id": watcher_session, "data": parsed }),
                                    );
                                }
                            }
                        }
                    }
                }

                std::thread::sleep(std::time::Duration::from_millis(250));
            }
        });
    }

    // ── OpenCode log-file watcher ─────────────────────────────────────────
    {
        let mut cfg = config_lock.lock().unwrap();
        cfg.folder = expected_folder;
    }

    Ok(ActiveAgent {
        config: config_lock,
        child_process: Some(child),
        background_processes,
        runtime_generation: Some(runtime_generation),
        process_id,
        query_count,
        init_timestamp,
        current_status,
        last_status_at: std::sync::Arc::new(std::sync::Mutex::new(None)),
        watch_state,
        terminal_title,
        last_output_at,
        log_path,
        log_last_modified: std::sync::Arc::new(std::sync::Mutex::new(None)),
        #[cfg(windows)]
        job_object,
    })
}

pub async fn resize_pty(
    session_id: String,
    cols: u16,
    rows: u16,
    state: &AppState,
) -> Result<(), String> {
    if cols < 10 {
        return Ok(());
    }
    let geometry = wardian_core::models::TerminalGeometry { cols, rows };
    match state
        .terminal_sessions
        .resize_legacy(&session_id, geometry)
        .await
    {
        Ok(result)
            if result.decision.status
                == wardian_core::models::TerminalLeaseDecisionStatus::Accepted =>
        {
            Ok(())
        }
        Ok(result) => Err(format!(
            "Terminal resize lease rejected: {}",
            result
                .decision
                .reason
                .map(|reason| format!("{reason:?}"))
                .unwrap_or_else(|| "unknown".to_string())
        )),
        Err(crate::state::terminal_session::TerminalBrokerError::SessionNotFound) => {
            let agents = state.agents.lock().await;
            if !agents.contains_key(&session_id) {
                return Err(format!("Agent {} not found", session_id));
            }
            drop(agents);
            state
                .terminal_sessions
                .remember_deferred_geometry(&session_id, "legacy-resize-adapter", geometry)
                .await
                .map(|_| ())
                .map_err(|error| error.to_string())
        }
        Err(error) => Err(error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wardian_core::models::{CodexProviderConfig, ProviderConfig};

    #[test]
    fn codex_status_log_session_tracks_excluded_latest_without_adopting_resume() {
        let mut config = AgentConfig {
            provider: "codex".to_string(),
            resume_session: None,
            provider_config: ProviderConfig::Codex(CodexProviderConfig {
                cleared_provider_sessions: vec!["provider-session-1".to_string()],
                ..Default::default()
            }),
            ..Default::default()
        };

        let log_session =
            codex_status_log_session(&mut config, Some("provider-session-1".to_string()));

        assert_eq!(log_session.as_deref(), Some("provider-session-1"));
        assert_eq!(config.resume_session, None);
        assert_eq!(
            config.codex_config().cleared_provider_sessions,
            vec!["provider-session-1".to_string()]
        );
    }

    #[test]
    fn restored_spawns_skip_stale_process_scan() {
        assert!(!should_cleanup_stale_session_processes_before_spawn(true));
        assert!(should_cleanup_stale_session_processes_before_spawn(false));
    }

    fn agent_without_pty() -> crate::state::ActiveAgent {
        crate::state::ActiveAgent {
            config: std::sync::Arc::new(std::sync::Mutex::new(AgentConfig::default())),
            child_process: None,
            background_processes: Vec::new(),
            runtime_generation: None,
            process_id: None,
            query_count: std::sync::Arc::new(std::sync::Mutex::new(0)),
            init_timestamp: std::sync::Arc::new(std::sync::Mutex::new(None)),
            current_status: std::sync::Arc::new(std::sync::Mutex::new("Restoring".to_string())),
            last_status_at: std::sync::Arc::new(std::sync::Mutex::new(None)),
            watch_state: std::sync::Arc::new(std::sync::Mutex::new(
                crate::state::AgentWatchState::new("restoring-agent".to_string(), 4096, 262_144),
            )),
            terminal_title: std::sync::Arc::new(std::sync::Mutex::new(String::new())),
            last_output_at: std::sync::Arc::new(std::sync::Mutex::new(None)),
            log_path: std::sync::Arc::new(std::sync::Mutex::new(None)),
            log_last_modified: std::sync::Arc::new(std::sync::Mutex::new(None)),
            #[cfg(windows)]
            job_object: None,
        }
    }

    // A resize that arrives while the agent is still a "Restoring" placeholder
    // is retained by the broker and seeds the native runtime when spawn begins.
    #[tokio::test]
    async fn resize_without_pty_records_size_for_spawn() {
        let state = AppState::new();
        state
            .agents
            .lock()
            .await
            .insert("restoring-agent".to_string(), agent_without_pty());

        let result = resize_pty("restoring-agent".to_string(), 124, 30, &state).await;

        assert!(result.is_ok());
        assert_eq!(
            state
                .terminal_sessions
                .spawn_geometry("restoring-agent")
                .await
                .expect("spawn geometry"),
            Some(wardian_core::models::TerminalGeometry {
                cols: 124,
                rows: 30
            })
        );
    }

    #[tokio::test]
    async fn resize_unknown_agent_still_errors() {
        let state = AppState::new();
        let result = resize_pty("missing".to_string(), 124, 30, &state).await;
        assert!(result.is_err());
        assert_eq!(
            state
                .terminal_sessions
                .spawn_geometry("missing")
                .await
                .expect("missing geometry"),
            None
        );
    }

    #[test]
    fn codex_line_status_preserves_action_needed_until_completion() {
        assert_eq!(
            line_event_status_for_pty_provider(
                "codex",
                "Idle",
                &AgentEvent::ActionRequired {
                    message: "approve command".to_string(),
                },
            ),
            Some("Action Needed")
        );
        assert_eq!(
            line_event_status_for_pty_provider("codex", "Action Needed", &AgentEvent::Generating),
            None
        );
        assert_eq!(
            line_event_status_for_pty_provider(
                "codex",
                "Action Needed",
                &AgentEvent::TurnCompleted,
            ),
            Some("Idle")
        );
    }

    #[test]
    fn output_ready_emit_gate_coalesces_repeats_after_throttle() {
        let mut gate = OutputReadyEmitGate::default();
        let start = std::time::Instant::now();

        assert_eq!(
            gate.after_buffer_append(start),
            OutputReadyEmitAction::EmitNow
        );
        assert_eq!(
            gate.after_buffer_append(start + OUTPUT_READY_EMIT_MIN_INTERVAL / 2),
            OutputReadyEmitAction::ScheduleAfter(OUTPUT_READY_EMIT_MIN_INTERVAL / 2)
        );
        assert_eq!(
            gate.after_buffer_append(start + OUTPUT_READY_EMIT_MIN_INTERVAL / 2),
            OutputReadyEmitAction::Suppress
        );
        assert!(gate.finish_delayed_emit(true, start + OUTPUT_READY_EMIT_MIN_INTERVAL));
    }

    #[test]
    fn codex_terminal_theme_probe_responder_answers_light_theme_queries() {
        let mut responder = CodexTerminalThemeProbeResponder::default();

        let responses = responder.responses_for_chunk(
            "codex",
            b"\x1b[?996n\x1b]10;?\x1b\\\x1b]11;?\x1b\\",
            "light",
        );

        let responses: Vec<String> = responses
            .into_iter()
            .map(|response| String::from_utf8(response).expect("utf8 response"))
            .collect();
        assert_eq!(
            responses,
            vec![
                "\x1b[?997;2n".to_string(),
                "\x1b]10;rgb:11/18/27\x1b\\".to_string(),
                "\x1b]11;rgb:fc/fa/f5\x1b\\".to_string(),
            ]
        );
    }

    #[test]
    fn codex_terminal_theme_probe_responder_handles_split_background_query() {
        let mut responder = CodexTerminalThemeProbeResponder::default();

        assert!(responder
            .responses_for_chunk("codex", b"\x1b]11", "dark")
            .is_empty());
        let responses = responder.responses_for_chunk("codex", b";?\x1b\\", "dark");

        assert_eq!(responses, vec![b"\x1b]11;rgb:02/04/02\x1b\\".to_vec()]);
        assert!(responder
            .responses_for_chunk("codex", b"\x1b]11;?\x1b\\", "dark")
            .is_empty());
    }

    #[test]
    fn codex_terminal_theme_probe_responder_ignores_other_providers() {
        let mut responder = CodexTerminalThemeProbeResponder::default();

        let responses = responder.responses_for_chunk("opencode", b"\x1b]11;?\x1b\\", "light");

        assert!(responses.is_empty());
    }

    #[test]
    fn test_filter_ignored_conversation_id() {
        assert_eq!(
            filter_ignored_conversation_id(Some("conv_abc".to_string()), Some("conv_abc")),
            None
        );
        assert_eq!(
            filter_ignored_conversation_id(Some("conv_xyz".to_string()), Some("conv_abc")),
            Some("conv_xyz".to_string())
        );
        assert_eq!(
            filter_ignored_conversation_id(Some("conv_abc".to_string()), None),
            Some("conv_abc".to_string())
        );
        assert_eq!(filter_ignored_conversation_id(None, Some("conv_abc")), None);
    }
}
