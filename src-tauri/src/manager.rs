use crate::models::{AgentClassDefinition, AgentConfig, AgentEvent, AgentTelemetry};
use crate::providers::ProviderFactory;
use crate::state::{ActiveAgent, AppState};
use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};
use std::collections::HashMap;
use std::io::{BufRead, Read, Seek, Write};
use tauri::{AppHandle, Emitter, Manager};

pub use crate::utils::fs::*;
pub use crate::utils::logging::log_debug;

fn set_agent_status(
    app: &AppHandle,
    session_id: &str,
    current_status: &std::sync::Arc<std::sync::Mutex<String>>,
    next_status: &str,
) {
    if let Ok(mut status) = current_status.lock() {
        if *status != next_status {
            *status = next_status.to_string();
            let _ = app.emit(
                "agent-status-updated",
                serde_json::json!({
                    "session_id": session_id,
                    "current_status": next_status,
                }),
            );
        }
    }
}

fn apply_agent_event(
    app: &AppHandle,
    session_id: &str,
    event: AgentEvent,
    query_count: &std::sync::Arc<std::sync::Mutex<usize>>,
    init_timestamp: &std::sync::Arc<std::sync::Mutex<Option<String>>>,
    current_status: &std::sync::Arc<std::sync::Mutex<String>>,
) {
    match event {
        AgentEvent::UserQuery => {
            if let Ok(mut count) = query_count.lock() {
                *count += 1;
            }
            set_agent_status(app, session_id, current_status, "Processing...");
        }
        AgentEvent::Generating => {
            set_agent_status(app, session_id, current_status, "Processing...");
        }
        AgentEvent::Init { timestamp, .. } => {
            if let Ok(mut ts) = init_timestamp.lock() {
                *ts = timestamp;
            }
        }
        AgentEvent::ModelResponse => {
            set_agent_status(app, session_id, current_status, "Idle");
        }
        AgentEvent::ActionRequired { .. } => {
            set_agent_status(app, session_id, current_status, "Action Needed");
        }
        AgentEvent::Unknown => {}
    }
}

fn apply_agent_status_event(
    app: &AppHandle,
    session_id: &str,
    event: AgentEvent,
    current_status: &std::sync::Arc<std::sync::Mutex<String>>,
) {
    match event {
        AgentEvent::UserQuery | AgentEvent::Generating => {
            set_agent_status(app, session_id, current_status, "Processing...");
        }
        AgentEvent::ModelResponse => {
            set_agent_status(app, session_id, current_status, "Idle");
        }
        AgentEvent::ActionRequired { .. } => {
            set_agent_status(app, session_id, current_status, "Action Needed");
        }
        AgentEvent::Init { .. } | AgentEvent::Unknown => {}
    }
}

pub fn save_state(_app: &AppHandle, agents: &HashMap<String, ActiveAgent>, order: &[String]) {
    let mut configs: Vec<AgentConfig> = Vec::new();
    for id in order {
        if let Some(agent) = agents.get(id) {
            configs.push(agent.config.clone());
        }
    }

    if let Ok(json) = serde_json::to_string_pretty(&configs) {
        if let Some(app_dir) = get_wardian_home() {
            let _ = std::fs::create_dir_all(&app_dir);
            let state_path = app_dir.join("wardian_state.json");
            let _ = std::fs::write(state_path, json);
        }
    }
}

pub async fn spawn_agent(
    app: AppHandle,
    config: AgentConfig,
    is_restored: bool,
) -> Result<ActiveAgent, String> {
    let provider = ProviderFactory::resolve(&config.provider)?;

    let cwd = if config.folder.is_empty() {
        if cfg!(windows) {
            std::env::var("USERPROFILE")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|_| std::path::PathBuf::from("C:\\"))
        } else {
            std::env::var("HOME")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|_| std::path::PathBuf::from("/"))
        }
    } else {
        std::path::PathBuf::from(&config.folder)
    };

    let expected_folder = if config.folder.is_empty() {
        cwd.to_string_lossy().to_string()
    } else {
        config.folder.clone()
    };

    if config.is_off {
        return Ok(ActiveAgent {
            config,
            child_process: None,
            pty_master: None,
            stdin_tx: None,
            output_buffer: std::sync::Arc::new(std::sync::Mutex::new(String::new())),
            process_id: None,
            query_count: std::sync::Arc::new(std::sync::Mutex::new(0)),
            init_timestamp: std::sync::Arc::new(std::sync::Mutex::new(None)),
            current_status: std::sync::Arc::new(std::sync::Mutex::new("Off".to_string())),
            log_path: std::sync::Arc::new(std::sync::Mutex::new(None)),
            #[cfg(windows)]
            job_object: None,
        });
    }

    let pty_system = NativePtySystem::default();

    let pair = pty_system
        .openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| format!("Failed to open pty: {}", e))?;

    let (bin, base_args) = provider.get_executable();
    let mut cmd = CommandBuilder::new(bin);
    for arg in base_args {
        cmd.arg(arg);
    }
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
    let provider_cwd = habitat_root
        .as_ref()
        .map(|root| habitat_workspace_cwd(root))
        .unwrap_or_else(|| cwd.clone());
    cmd.cwd(&provider_cwd);

    // Enable CLAUDE.md discovery from --add-dir directories so that
    // class/common/agent instruction files are loaded natively.
    if config.provider == "claude" {
        cmd.env("CLAUDE_CODE_ADDITIONAL_DIRECTORIES_CLAUDE_MD", "1");
        if let Some(hook) = claude_hook.as_ref() {
            cmd.arg("--settings");
            cmd.arg(&hook.settings_arg);
        }
    } else if config.provider == "codex" {
        if let Some(root) = habitat_root.as_ref() {
            cmd.env("CODEX_HOME", habitat_codex_home(root));
        }
    }

    let is_resume = config.resume_session.as_deref().is_some_and(|s| !s.is_empty());
    let spawn_args = provider.get_spawn_args(&config, is_resume);
    for arg in &spawn_args {
        cmd.arg(arg);
    }

    let resume_id = config.resume_session.as_deref().unwrap_or("");
    log_debug(&format!(
        "[Wardian] Spawning {} agent. Session: {}, Resume ID: {}, Restored: {}",
        provider.name(), config.session_id, resume_id, is_restored
    ));

    let child = pair
        .slave
        .spawn_command(cmd)
        .map_err(|e| format!("Failed to spawn command: {}", e))?;

    let process_id = child.process_id();

    #[cfg(windows)]
    let job_object = {
        if let Ok(job) = win32job::Job::create() {
            let mut info = job.query_extended_limit_info().unwrap_or_default();
            info.limit_kill_on_job_close();
            let _ = job.set_extended_limit_info(&info);
            if let Some(pid) = process_id {
                unsafe {
                    use winapi::um::processthreadsapi::OpenProcess;
                    use winapi::um::winnt::{PROCESS_SET_QUOTA, PROCESS_TERMINATE};
                    let handle = OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, 0, pid);
                    if !handle.is_null() {
                        let _ = job.assign_process(handle as isize);
                        winapi::um::handleapi::CloseHandle(handle);
                    }
                }
            }
            Some(job)
        } else {
            None
        }
    };
    #[cfg(not(windows))]
    let job_object = None;

    let mut reader = pair
        .master
        .try_clone_reader()
        .map_err(|e| format!("Failed to get pty reader: {}", e))?;
    let mut writer = pair
        .master
        .take_writer()
        .map_err(|e| format!("Failed to get pty writer: {}", e))?;
    let pty_master = std::sync::Arc::new(std::sync::Mutex::new(pair.master));
    drop(pair.slave);

    let (tx, mut rx) = tokio::sync::mpsc::channel::<String>(256);

    std::thread::spawn(move || {
        while let Some(input) = rx.blocking_recv() {
            let _ = writer.write_all(input.as_bytes());
            let _ = writer.flush();
        }
    });

    let sid_out = config.session_id.clone();
    let provider_name_for_pty = config.provider.clone();
    let output_buffer = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let output_buffer_clone = output_buffer.clone();
    let query_count = std::sync::Arc::new(std::sync::Mutex::new(0));
    let query_count_clone = query_count.clone();
    let init_timestamp = std::sync::Arc::new(std::sync::Mutex::new(None));
    let init_timestamp_clone = init_timestamp.clone();
    let current_status = std::sync::Arc::new(std::sync::Mutex::new("Idle".to_string()));
    let current_status_clone = current_status.clone();
    let log_path = std::sync::Arc::new(std::sync::Mutex::new(None::<std::path::PathBuf>));

    // PTY reader thread: uses provider.parse_output() for event classification
    let pty_app = app.clone();
    let pty_provider = provider.clone();
    let sid_for_pty = sid_out.clone();
    let pty_emit_app = app.clone();
    std::thread::spawn(move || {
        let mut buf = [0; 4096];
        let mut current_line = String::new();
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    let text = String::from_utf8_lossy(&buf[0..n]).to_string();
                    if let Ok(mut h) = output_buffer_clone.lock() {
                        h.push_str(&text);
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
                                    if let Some(event) = pty_provider.parse_output(&raw_line) {
                                        if provider_name_for_pty == "claude" {
                                            if let AgentEvent::Init { timestamp, .. } = event {
                                                if let Ok(mut ts) = init_timestamp_clone.lock() {
                                                    *ts = timestamp;
                                                }
                                            }
                                        } else {
                                            apply_agent_event(
                                                &pty_app,
                                                &sid_for_pty,
                                                event,
                                                &query_count_clone,
                                                &init_timestamp_clone,
                                                &current_status_clone,
                                            );
                                        }
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
                Err(_) => break,
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
        let wardian_agent_dir = get_wardian_home()
            .map(|home| home.join("agents").join(&watcher_session))
            .filter(|path| path.exists())
            .map(|path| path.to_string_lossy().to_string());

        std::thread::spawn(move || {
            let mut offset: u64 = 0;
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
                        *lock = codex_session_file_path(&watcher_session, wardian_agent_dir.as_deref());
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
                                if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(line.trim()) {
                                    let raw_line = parsed.to_string();
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
    } else if config.provider == "claude" {
        let watcher_app = app.clone();
        let watcher_provider = provider.clone();
        let watcher_session = config.session_id.clone();
        let watcher_current_status = current_status.clone();
        let watcher_log_path = log_path.clone();
        let watcher_folder = expected_folder.clone();
        let hook_event_log = claude_hook.as_ref().map(|hook| hook.event_log_path.clone());
        let waiting_for_permission = std::sync::Arc::new(std::sync::Mutex::new(false));
        let log_waiting_for_permission = waiting_for_permission.clone();

        std::thread::spawn(move || {
            let mut offset: u64 = 0;
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
                                                    let is_tool_result = parsed
                                                        .get("type")
                                                        .and_then(|v| v.as_str())
                                                        == Some("user")
                                                        && !claude_is_real_user_query(&parsed);
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
                                if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(line.trim()) {
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
    }

    Ok(ActiveAgent {
        config: AgentConfig {
            folder: expected_folder,
            ..config
        },
        child_process: Some(child),
        pty_master: Some(pty_master),
        stdin_tx: Some(tx),
        output_buffer,
        process_id,
        query_count,
        init_timestamp,
        current_status,
        log_path,
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
    let master_arc = {
        let agents = state.agents.lock().await;
        if let Some(agent) = agents.get(&session_id) {
            agent
                .pty_master
                .clone()
                .ok_or_else(|| format!("Agent {} is off", session_id))?
        } else {
            return Err(format!("Agent {} not found", session_id));
        }
    };
    tokio::task::spawn_blocking(move || {
        let master = match master_arc.lock() {
            Ok(master) => master,
            Err(poisoned) => poisoned.into_inner(),
        };
        master.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
    })
    .await
    .map_err(|e| format!("Failed to join PTY resize task: {}", e))?
    .map_err(|e| format!("Failed to resize PTY: {}", e))?;
    Ok(())
}

pub async fn run_headless(
    cwd: &std::path::PathBuf,
    prompt: &str,
    session_id: &str,
    output_format: &str,
    provider_name: &str,
) -> Result<serde_json::Value, String> {
    use tokio::io::{AsyncBufReadExt, BufReader};
    let provider = ProviderFactory::resolve(provider_name)?;
    let (bin, base_args) = provider.get_executable();
    let mut cmd = if cfg!(target_os = "windows") && bin.ends_with(".cmd") {
        let mut c = tokio::process::Command::new("cmd");
        c.arg("/c").arg(&bin);
        c
    } else {
        tokio::process::Command::new(&bin)
    };
    for arg in base_args {
        cmd.arg(arg);
    }
    match provider_name {
        "codex" => {
            cmd.arg("exec")
                .arg("resume")
                .arg(session_id)
                .arg("--json")
                .arg(prompt);
        }
        "claude" => {
            cmd.env("CLAUDE_CODE_ADDITIONAL_DIRECTORIES_CLAUDE_MD", "1");
            if let Ok(hook) = ensure_claude_permission_hook(session_id) {
                cmd.arg("--settings").arg(hook.settings_arg);
            }
            cmd.arg("--print")
                .arg("--output-format")
                .arg(output_format)
                .arg("--resume")
                .arg(session_id)
                .arg(prompt);
        }
        _ => {
            cmd.arg("-p")
                .arg(prompt)
                .arg("--output-format")
                .arg(output_format)
                .arg("--resume")
                .arg(session_id);
        }
    }
    cmd.current_dir(cwd)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());

    let mut child = cmd.spawn().map_err(|e| e.to_string())?;
    let mut output = String::new();

    if let Some(stdout) = child.stdout.take() {
        let mut reader = BufReader::new(stdout);
        let mut line = String::new();
        while let Ok(n) = reader.read_line(&mut line).await {
            if n == 0 {
                break;
            }
            output.push_str(&line);
            line.clear();
        }
    }

    let _ = child.wait().await;

    if provider_name == "codex" {
        let mut last_message = None;
        for line in output.lines() {
            if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(line) {
                match parsed.get("type").and_then(|v| v.as_str()).unwrap_or("") {
                    "item.completed" => {
                        if parsed
                            .get("item")
                            .and_then(|v| v.get("type"))
                            .and_then(|v| v.as_str())
                            == Some("agent_message")
                        {
                            last_message = parsed
                                .get("item")
                                .and_then(|v| v.get("text"))
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string());
                        }
                    }
                    "event_msg" => {
                        if parsed
                            .get("payload")
                            .and_then(|v| v.get("type"))
                            .and_then(|v| v.as_str())
                            == Some("agent_message")
                        {
                            last_message = parsed
                                .get("payload")
                                .and_then(|v| v.get("message"))
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string());
                        }
                    }
                    _ => {}
                }
            }
        }

        if output_format == "json" {
            Ok(serde_json::json!({
                "thread_id": session_id,
                "response": last_message.unwrap_or_default(),
                "raw": output,
            }))
        } else {
            Ok(serde_json::json!({ "text": last_message.unwrap_or(output) }))
        }
    } else if output_format == "json" {
        serde_json::from_str(&output)
            .map_err(|e| format!("Failed to parse JSON output: {}. Raw: {}", e, output))
    } else {
        Ok(serde_json::json!({ "text": output }))
    }
}

pub async fn obtain_session_id(
    cwd: &std::path::PathBuf,
    agent_class: Option<&str>,
    config: Option<&AgentConfig>,
) -> Result<String, String> {
    use tokio::io::{AsyncBufReadExt, BufReader};
    let provider_name = config.map(|c| c.provider.as_str()).unwrap_or("claude");
    let provider = ProviderFactory::resolve(provider_name)?;
    let (bin, base_args) = provider.get_executable();
    let mut cmd = if cfg!(target_os = "windows") && bin.ends_with(".cmd") {
        let mut c = tokio::process::Command::new("cmd");
        c.arg("/c").arg(&bin);
        c
    } else {
        tokio::process::Command::new(&bin)
    };
    let class_name = agent_class
        .filter(|name| !name.trim().is_empty())
        .or_else(|| {
            config
                .and_then(|cfg| (!cfg.agent_class.trim().is_empty()).then_some(cfg.agent_class.as_str()))
        })
        .unwrap_or("");
    let habitat_root = prepare_provider_habitat(provider_name, cwd, class_name, None)?;
    let provider_cwd = habitat_root
        .as_ref()
        .map(|root| habitat_workspace_cwd(root))
        .unwrap_or_else(|| cwd.clone());
    for arg in base_args {
        cmd.arg(arg);
    }
    if provider_name == "codex" {
        if let Some(root) = habitat_root.as_ref() {
            cmd.env("CODEX_HOME", habitat_codex_home(root));
        }
        cmd.arg("exec");

        if let Some(config) = config {
            if let Some(ref model) = config.model {
                cmd.arg("--model").arg(model);
            }
            if let Some(ref profile) = config.codex_profile {
                if !profile.trim().is_empty() {
                    cmd.arg("--profile").arg(profile);
                }
            }
            if let Some(ref sandbox_mode) = config.codex_sandbox_mode {
                if !sandbox_mode.trim().is_empty() {
                    cmd.arg("--sandbox").arg(sandbox_mode);
                }
            }
            if config.codex_full_auto.unwrap_or(false) {
                cmd.arg("--full-auto");
            }
            if config.codex_search.unwrap_or(false) {
                cmd.arg("--search");
            }
            if config.codex_skip_git_repo_check.unwrap_or(true) {
                cmd.arg("--skip-git-repo-check");
            }
            if config.codex_ephemeral.unwrap_or(false) {
                cmd.arg("--ephemeral");
            }

            let mut final_includes = config
                .system_include_directories
                .clone()
                .unwrap_or_default();
            if let Some(ref user_dirs) = config.include_directories {
                for dir in user_dirs {
                    if !final_includes.contains(dir) {
                        final_includes.push(dir.clone());
                    }
                }
            }
            for dir in final_includes {
                cmd.arg("--add-dir").arg(dir);
            }

            if let Some(ref custom) = config.custom_args {
                if let Some(parsed) = shlex::split(custom) {
                    for arg in parsed {
                        cmd.arg(arg);
                    }
                }
            }
        }

        cmd.arg("--json")
            .arg("Introduce yourself")
            .current_dir(&provider_cwd)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
    } else if provider_name == "claude" {
        cmd.env("CLAUDE_CODE_ADDITIONAL_DIRECTORIES_CLAUDE_MD", "1")
            .arg("--print")
            .arg("--verbose")
            .arg("--output-format")
            .arg("stream-json")
            .arg("Introduce yourself")
            .current_dir(cwd)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
    } else {
        cmd.arg("-p")
            .arg("Introduce yourself")
            .arg("-o")
            .arg("stream-json")
            .current_dir(&provider_cwd)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
    }

    log_debug(&format!("[WARDIAN-DEBUG] Running obtain_session_id for provider {}", provider_name));
    match cmd.spawn() {
        Ok(mut child) => {
            log_debug("[WARDIAN-DEBUG] Spawned headless process. Reading stdout...");
            let mut session_id_res = None;
            let mut stderr_output = String::new();
            
            if let Some(stdout) = child.stdout.take() {
                let mut reader = BufReader::new(stdout);
                let mut line = String::new();
                while let Ok(n) = reader.read_line(&mut line).await {
                    if n == 0 {
                        log_debug("[WARDIAN-DEBUG] Reached EOF on stdout.");
                        break;
                    }
                    let trimmed = line.trim();
                    if session_id_res.is_none() {
                        if let Some(start) = trimmed.find('{') {
                            let json_part = &trimmed[start..];
                            if let Some(AgentEvent::Init { session_id, .. }) = provider.parse_output(json_part) {
                                if !session_id.is_empty() {
                                    log_debug(&format!("[WARDIAN-DEBUG] Found session_id: {}", session_id));
                                    session_id_res = Some(session_id);
                                }
                            }
                        }
                    }
                    line.clear();
                }
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
            log_debug(&format!("[WARDIAN-DEBUG] Returning session_id: {:?}", session_id_res));
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
            Err(format!("Failed to spawn {} bootstrap command: {}", provider_name, e))
        }
    }
}

/// Converts a workspace absolute path into Claude Code's project directory name.
/// Claude replaces each of `:`, `\`, `/`, `.` with `-`.
/// e.g. `D:\Development\Wardian` → `D--Development-Wardian`
fn claude_project_dir_name(workspace: &str) -> String {
    workspace
        .chars()
        .map(|c| match c {
            ':' | '\\' | '/' | '.' => '-',
            _ => c,
        })
        .collect()
}

fn codex_session_file_path_in(base: &std::path::Path, session_id: &str) -> Option<std::path::PathBuf> {
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

fn codex_session_file_path(session_id: &str, wardian_agent_dir: Option<&str>) -> Option<std::path::PathBuf> {
    if let Some(agent_dir) = wardian_agent_dir {
        let projected_home = std::path::Path::new(agent_dir).join("habitat").join(".codex");
        if let Some(path) = codex_session_file_path_in(&projected_home, session_id) {
            return Some(path);
        }
    }

    let global_home = dirs::home_dir()?.join(".codex");
    codex_session_file_path_in(&global_home, session_id)
}

fn codex_status_from_log(lines: &[serde_json::Value]) -> Option<String> {
    for line in lines.iter().rev() {
        let payload = line.get("payload")?;
        let payload_type = payload.get("type").and_then(|v| v.as_str())?;
        match payload_type {
            "exec_approval_request" => return Some("Action Needed".to_string()),
            "task_started" | "agent_message" | "exec_command_begin" | "exec_command_start" => {
                return Some("Processing...".to_string())
            }
            "task_complete" => return Some("Idle".to_string()),
            _ => {}
        }
    }
    None
}

fn claude_is_real_user_query(line: &serde_json::Value) -> bool {
    let Some(message) = line.get("message") else {
        return true;
    };
    let Some(content) = message.get("content") else {
        return true;
    };

    let Some(items) = content.as_array() else {
        return true;
    };

    !items.iter().any(|item| item.get("type").and_then(|v| v.as_str()) == Some("tool_result"))
}

fn claude_status_from_log(lines: &[serde_json::Value]) -> Option<String> {
    for line in lines.iter().rev() {
        match line.get("type").and_then(|v| v.as_str()) {
            Some("system") => {
                if let Some("permission_request") =
                    line.get("subtype").and_then(|v| v.as_str())
                {
                    return Some("Action Needed".to_string());
                }
            }
            Some("assistant") => {
                let stop_reason = line
                    .get("message")
                    .and_then(|v| v.get("stop_reason"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if stop_reason == "end_turn" || stop_reason == "stop_sequence" {
                    return Some("Idle".to_string());
                }
                return Some("Processing...".to_string());
            }
            Some("progress") => return Some("Processing...".to_string()),
            Some("user") => {
                if claude_is_real_user_query(line) {
                    return Some("Processing...".to_string());
                }
            }
            _ => {}
        }
    }

    None
}

pub async fn get_all_metrics(state: &AppState) -> Vec<AgentTelemetry> {
    struct AgentSnapshot {
        session_id: String,
        provider: String,
        folder: String,
        process_id: Option<u32>,
        query_count: std::sync::Arc<std::sync::Mutex<usize>>,
        init_timestamp: std::sync::Arc<std::sync::Mutex<Option<String>>>,
        current_status: std::sync::Arc<std::sync::Mutex<String>>,
        log_path: std::sync::Arc<std::sync::Mutex<Option<std::path::PathBuf>>>,
    }

    let snapshots: Vec<AgentSnapshot> = {
        let agents = state.agents.lock().await;
        agents
            .iter()
            .map(|(sid, agent)| AgentSnapshot {
                session_id: sid.clone(),
                provider: agent.config.provider.clone(),
                folder: agent.config.folder.clone(),
                process_id: agent.process_id,
                query_count: agent.query_count.clone(),
                init_timestamp: agent.init_timestamp.clone(),
                current_status: agent.current_status.clone(),
                log_path: agent.log_path.clone(),
            })
            .collect()
    };

    let sys_metrics = state.system_metrics.clone();
    tokio::task::spawn_blocking(move || {
        let mut results = Vec::new();
        let mut sys = sys_metrics.blocking_lock();
        sys.refresh_all();

        let mut children_map: HashMap<sysinfo::Pid, Vec<sysinfo::Pid>> = HashMap::new();
        for (pid, process) in sys.processes() {
            if let Some(parent) = process.parent() {
                children_map.entry(parent).or_default().push(*pid);
            }
        }

        for snap in &snapshots {
            let mut cpu = 0.0;
            let mut mem = 0.0;
            let mut uptime = 0;

            if let Some(pid) = snap.process_id {
                let root_pid = sysinfo::Pid::from_u32(pid);
                fn sum_tree(
                    pid: sysinfo::Pid,
                    sys: &sysinfo::System,
                    cmap: &HashMap<sysinfo::Pid, Vec<sysinfo::Pid>>,
                    cpu: &mut f32,
                    mem: &mut f64,
                    uptime: &mut u64,
                ) {
                    if let Some(p) = sys.process(pid) {
                        *cpu += p.cpu_usage();
                        *mem += p.memory() as f64 / 1_048_576.0;
                        *uptime = std::cmp::max(*uptime, p.run_time());
                    }
                    if let Some(children) = cmap.get(&pid) {
                        for &cpid in children {
                            sum_tree(cpid, sys, cmap, cpu, mem, uptime);
                        }
                    }
                }
                sum_tree(
                    root_pid,
                    &sys,
                    &children_map,
                    &mut cpu,
                    &mut mem,
                    &mut uptime,
                );
            }

            // Detect whether the agent process is still alive
            let process_alive = snap
                .process_id
                .map(|pid| sys.process(sysinfo::Pid::from_u32(pid)).is_some())
                .unwrap_or(false);

            let mut q_count = 0;
            let mut i_ts = None;
            let mut log_path_lock = snap.log_path.lock().unwrap_or_else(|e| e.into_inner());

            // Provider-aware log discovery
            if log_path_lock.is_none() {
                match snap.provider.as_str() {
                    "codex" => {
                        let agent_home = get_wardian_home()
                            .map(|home| home.join("agents").join(&snap.session_id))
                            .filter(|path| path.exists())
                            .map(|path| path.to_string_lossy().to_string());
                        if let Some(path) = codex_session_file_path(&snap.session_id, agent_home.as_deref()) {
                            *log_path_lock = Some(path);
                        }
                    }
                    "claude" => {
                        // Claude Code stores sessions at:
                        // ~/.claude/projects/<project_dir>/<session_id>.jsonl
                        // where <project_dir> is the workspace path with :\/. replaced by -
                        if let Some(home) = dirs::home_dir() {
                            let project_dir = claude_project_dir_name(&snap.folder);
                            let candidate = home
                                .join(".claude")
                                .join("projects")
                                .join(&project_dir)
                                .join(format!("{}.jsonl", snap.session_id));
                            if candidate.exists() {
                                *log_path_lock = Some(candidate);
                            }
                        }
                    }
                    _ => {
                        // Gemini: scan .gemini/tmp for chat log files
                        if let Some(app_dir) = get_wardian_home() {
                            let tmp_dir = app_dir.join("../.gemini/tmp");
                            if let Ok(entries) = std::fs::read_dir(tmp_dir) {
                                for entry in entries.flatten() {
                                    let chat_dir = entry.path().join("chats");
                                    if let Ok(chat_files) = std::fs::read_dir(chat_dir) {
                                        for chat_file in chat_files.flatten() {
                                            if let Ok(content) = std::fs::read_to_string(chat_file.path()) {
                                                if let Ok(p) =
                                                    serde_json::from_str::<serde_json::Value>(&content)
                                                {
                                                    if p.get("sessionId").and_then(|v| v.as_str())
                                                        == Some(&snap.session_id)
                                                    {
                                                        *log_path_lock = Some(chat_file.path());
                                                        break;
                                                    }
                                                }
                                            }
                                        }
                                    }
                                    if log_path_lock.is_some() {
                                        break;
                                    }
                                }
                            }
                        }
                    }
                }
            }

            // Provider-aware log parsing for status/query enrichment
            if let Some(ref path) = *log_path_lock {
                if let Ok(content) = std::fs::read_to_string(path) {
                    match snap.provider.as_str() {
                        "codex" => {
                            let lines: Vec<serde_json::Value> = content
                                .lines()
                                .filter_map(|l| serde_json::from_str(l).ok())
                                .collect();

                            q_count = lines.iter().filter(|l| {
                                l.get("type").and_then(|v| v.as_str()) == Some("event_msg")
                                    && l.get("payload")
                                        .and_then(|v| v.get("type"))
                                        .and_then(|v| v.as_str())
                                        == Some("user_message")
                            }).count();

                            if let Some(meta) = lines.iter().find(|l| {
                                l.get("type").and_then(|v| v.as_str()) == Some("session_meta")
                            }) {
                                if let Some(ts) = meta
                                    .get("payload")
                                    .and_then(|v| v.get("timestamp"))
                                    .and_then(|v| v.as_str())
                                {
                                    i_ts = Some(ts.to_string());
                                }
                            }

                            if let Some(status) = codex_status_from_log(&lines) {
                                *snap.current_status.lock().unwrap() = status;
                            }
                        }
                        "claude" => {
                            // Claude logs are JSONL — one JSON object per line
                            let lines: Vec<serde_json::Value> = content
                                .lines()
                                .filter_map(|l| serde_json::from_str(l).ok())
                                .collect();

                            q_count = lines.iter().filter(|l| {
                                l.get("type").and_then(|v| v.as_str()) == Some("user")
                                    && claude_is_real_user_query(l)
                            }).count();

                            if let Some(first) = lines.first() {
                                if let Some(ts) = first.get("timestamp").and_then(|v| v.as_str()) {
                                    i_ts = Some(ts.to_string());
                                }
                            }

                            let current_status =
                                snap.current_status.lock().unwrap().clone();
                            if current_status != "Action Needed" {
                                if let Some(status) = claude_status_from_log(&lines) {
                                    *snap.current_status.lock().unwrap() = status;
                                }
                            }
                        }
                        _ => {
                            // Gemini logs are a single JSON object with a messages array
                            if let Ok(p) = serde_json::from_str::<serde_json::Value>(&content) {
                                if let Some(msgs) = p.get("messages").and_then(|v| v.as_array()) {
                                    q_count = msgs.iter().filter(|m| {
                                        m.get("type").and_then(|v| v.as_str()) == Some("user") || m.get("role").and_then(|v| v.as_str()) == Some("user")
                                    }).count();
                                }
                                if let Some(st) = p.get("startTime").and_then(|v| v.as_str()) {
                                    i_ts = Some(st.to_string());
                                }
                            }
                        }
                    }
                }
            }

            if q_count > 0 {
                *snap.query_count.lock().unwrap() = q_count;
            }
            if let Some(ts) = i_ts {
                *snap.init_timestamp.lock().unwrap() = Some(ts);
            }

            // If the process has terminated, force status to "Off" so the UI
            // doesn't stay stuck on "Processing..." or "Action Needed".
            if !process_alive && snap.process_id.is_some() {
                *snap.current_status.lock().unwrap() = "Off".to_string();
            }

            results.push(AgentTelemetry {
                session_id: snap.session_id.clone(),
                cpu_usage: cpu,
                memory_mb: mem,
                uptime_seconds: uptime,
                query_count: *snap.query_count.lock().unwrap(),
                init_timestamp: snap.init_timestamp.lock().unwrap().clone(),
                current_status: snap.current_status.lock().unwrap().clone(),
                log_path: log_path_lock
                    .as_ref()
                    .map(|p| p.to_string_lossy().to_string()),
            });
        }
        results
    })
    .await
    .unwrap_or_default()
}

pub fn get_all_agent_classes(_app: &AppHandle) -> Vec<AgentClassDefinition> {
    if let Some(app_dir) = get_wardian_home() {
        let classes_path = app_dir.join("classes.json");
        if let Ok(data) = std::fs::read_to_string(&classes_path) {
            return serde_json::from_str::<Vec<AgentClassDefinition>>(&data).unwrap_or_default();
        }
    }
    Vec::new()
}

pub fn save_classes(
    _app: &AppHandle,
    classes: &[AgentClassDefinition],
) -> Result<(), String> {
    let app_dir = get_wardian_home().ok_or("No home dir")?;
    let json = serde_json::to_string_pretty(classes).map_err(|e| e.to_string())?;
    std::fs::write(app_dir.join("classes.json"), json).map_err(|e| e.to_string())?;
    Ok(())
}

pub fn init_agent_classes(app: &AppHandle) {
    if let Some(app_dir) = get_wardian_home() {
        let classes_dir = app_dir.join("classes");
        let _ = std::fs::create_dir_all(&classes_dir);
        let _ = std::fs::create_dir_all(app_dir.join("common/desk"));
        let _ = std::fs::create_dir_all(app_dir.join("common/lineages"));

        // Ensure Claude can discover skills from the canonical .agents/skills/ location
        ensure_claude_skills_link(&app_dir.join("common"));

        let classes_path = app_dir.join("classes.json");
        
        // Migration and Initialization
        if !classes_path.exists() {
            let mut defaults: Vec<AgentClassDefinition> =
                serde_json::from_str(include_str!("default_classes.json")).unwrap_or_default();
            for d in defaults.iter_mut() {
                d.is_default = true;
            }

            let custom_path = app_dir.join("custom_classes.json");
            if custom_path.exists() {
                if let Ok(data) = std::fs::read_to_string(&custom_path) {
                    let mut custom = serde_json::from_str::<Vec<AgentClassDefinition>>(&data).unwrap_or_default();
                    for c in custom.iter_mut() {
                        c.is_default = false;
                    }
                    defaults.extend(custom);
                }
                // We've successfully merged. We could delete custom_classes.json here.
                let _ = std::fs::remove_file(&custom_path);
            }

            let _ = save_classes(app, &defaults);
        }

        let classes = get_all_agent_classes(app);
        for cls in &classes {
            let role_dir = classes_dir.join(&cls.name);
            let _ = std::fs::create_dir_all(&role_dir);
            
            // 1. Create AGENTS.md master file
            let agents_md_path = role_dir.join("AGENTS.md");
            if !agents_md_path.exists() {
                let content = if cls.is_default {
                    app.path()
                        .resolve(
                            format!("agent_prompts/{}.md", cls.name),
                            tauri::path::BaseDirectory::Resource,
                        )
                        .ok()
                        .and_then(|p| std::fs::read_to_string(p).ok())
                        .unwrap_or_default()
                } else {
                    format!("# {} Agent\n\n{}\n", cls.name, cls.description)
                };
                let _ = std::fs::write(agents_md_path, content);
            }

            // 2. Symlink .claude/skills/ → .agents/skills/ for Claude discovery
            ensure_claude_skills_link(&role_dir);

            // 3. Create provider stub files for providers that do not read AGENTS.md directly
            for stub_name in &["GEMINI.md", "CLAUDE.md"] {
                let stub_path = role_dir.join(stub_name);
                if !stub_path.exists() {
                    let _ = std::fs::write(stub_path, "@AGENTS.md\n");
                }
            }
        }
    }
}

pub fn get_agent_class_default_instruction(app: &AppHandle, class_name: &str) -> Option<String> {
    app.path()
        .resolve(
            format!("agent_prompts/{}.md", class_name),
            tauri::path::BaseDirectory::Resource,
        )
        .ok()
        .and_then(|p| std::fs::read_to_string(p).ok())
}

pub async fn kill_all_agents(state: &AppState) {
    let mut agents = state.agents.lock().await;
    for (sid, mut agent) in agents.drain() {
        log_debug(&format!("[Wardian] Killing session {}", sid));
        if let Some(mut child) = agent.child_process {
            let _ = child.kill();
        }
        #[cfg(windows)]
        {
            let _ = agent.job_object.take();
        }
    }
    state.agent_order.lock().await.clear();
}
