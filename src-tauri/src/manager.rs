use crate::models::{AgentClassDefinition, AgentConfig, AgentEvent, AgentTelemetry};
use crate::providers::ProviderFactory;
use crate::state::{ActiveAgent, AppState};
use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};
use std::collections::HashMap;
use std::io::{Read, Write};
use tauri::{AppHandle, Emitter, Manager};

pub use crate::utils::fs::*;
pub use crate::utils::logging::log_debug;

/// On macOS, GUI apps inherit a minimal PATH that excludes Homebrew, npm globals,
/// Volta, and other user-level tool installs. Prepend the common locations so that
/// `claude`, `gemini`, and similar CLIs can be found when spawning child processes.
#[cfg(target_os = "macos")]
fn macos_extended_path() -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    let existing = std::env::var("PATH").unwrap_or_default();
    let extra = format!(
        "{home}/.local/bin:/usr/local/bin:/opt/homebrew/bin:/opt/homebrew/sbin:{home}/.npm-global/bin:{home}/.volta/bin",
        home = home
    );
    if existing.is_empty() {
        format!("{}:/usr/bin:/bin:/usr/sbin:/sbin", extra)
    } else {
        format!("{}:{}", extra, existing)
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

    let cwd = crate::utils::fs::resolve_cwd(&config.folder, &config.session_id);

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
    cmd.cwd(&expected_folder);

    // Enable CLAUDE.md discovery from --add-dir directories so that
    // class/common/agent instruction files are loaded natively.
    if config.provider == "claude" {
        cmd.env("CLAUDE_CODE_ADDITIONAL_DIRECTORIES_CLAUDE_MD", "1");
    }
    #[cfg(target_os = "macos")]
    cmd.env("PATH", macos_extended_path());

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
            let _ = job.set_extended_limit_info(&mut info);
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
    let output_buffer = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let output_buffer_clone = output_buffer.clone();
    let query_count = std::sync::Arc::new(std::sync::Mutex::new(0));
    let query_count_clone = query_count.clone();
    let init_timestamp = std::sync::Arc::new(std::sync::Mutex::new(None));
    let init_timestamp_clone = init_timestamp.clone();
    let current_status = std::sync::Arc::new(std::sync::Mutex::new("Idle".to_string()));
    let current_status_clone = current_status.clone();

    // PTY reader thread: uses provider.parse_output() for event classification
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
                                    if let Some(event) = provider.parse_output(&raw_line) {
                                        match event {
                                            AgentEvent::UserQuery => {
                                                if let Ok(mut count) = query_count_clone.lock() {
                                                    *count += 1;
                                                }
                                                if let Ok(mut status) = current_status_clone.lock() {
                                                    *status = "Processing...".to_string();
                                                }
                                            }
                                            AgentEvent::Generating => {
                                                if let Ok(mut status) = current_status_clone.lock() {
                                                    *status = "Processing...".to_string();
                                                }
                                            }
                                            AgentEvent::Init { timestamp, .. } => {
                                                if let Ok(mut ts) = init_timestamp_clone.lock() {
                                                    *ts = timestamp;
                                                }
                                            }
                                            AgentEvent::ModelResponse => {
                                                if let Ok(mut status) = current_status_clone.lock() {
                                                    *status = "Idle".to_string();
                                                }
                                            }
                                            AgentEvent::ActionRequired { .. } => {
                                                if let Ok(mut status) = current_status_clone.lock() {
                                                    *status = "Action Needed".to_string();
                                                }
                                            }
                                            _ => {}
                                        }
                                    }
                                    let _ = app.emit("agent-json-event", serde_json::json!({ "session_id": sid_out, "data": parsed }));
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
        if let Ok(mut status) = current_status_clone.lock() {
            *status = "Off".to_string();
        }
    });

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
        log_path: std::sync::Arc::new(std::sync::Mutex::new(None)),
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
    let _ = tokio::task::spawn_blocking(move || {
        if let Ok(master) = master_arc.try_lock() {
            let _ = master.resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            });
        }
    });
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
        "claude" => {
            cmd.env("CLAUDE_CODE_ADDITIONAL_DIRECTORIES_CLAUDE_MD", "1")
                .arg("--print")
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

    if output_format == "json" {
        serde_json::from_str(&output)
            .map_err(|e| format!("Failed to parse JSON output: {}. Raw: {}", e, output))
    } else {
        Ok(serde_json::json!({ "text": output }))
    }
}

pub async fn obtain_session_id(
    cwd: &std::path::PathBuf,
    provider_name: &str,
) -> Option<String> {
    use tokio::io::{AsyncBufReadExt, BufReader};
    let provider = ProviderFactory::resolve(provider_name).ok()?;
    let (bin, base_args) = provider.get_executable();
    let mut cmd = tokio::process::Command::new(&bin);
    for arg in base_args {
        cmd.arg(arg);
    }
    if provider_name == "claude" {
        cmd.env("CLAUDE_CODE_ADDITIONAL_DIRECTORIES_CLAUDE_MD", "1")
            .arg("--print")
            .arg("--verbose")
            .arg("--output-format")
            .arg("stream-json")
            .arg("Introduce yourself")
            .current_dir(cwd)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
    } else {
        cmd.arg("-p")
            .arg("Introduce yourself")
            .arg("-o")
            .arg("stream-json")
            .current_dir(cwd)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
    }

    #[cfg(target_os = "macos")]
    cmd.env("PATH", macos_extended_path());

    log_debug(&format!("[WARDIAN-DEBUG] Running obtain_session_id for provider {}", provider_name));
    match cmd.spawn() {
        Ok(mut child) => {
            log_debug("[WARDIAN-DEBUG] Spawned headless process. Reading stdout...");
            let mut session_id_res = None;

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
                            if let Some(evt) = provider.parse_output(json_part) {
                                match evt {
                                    AgentEvent::Init { session_id: sid, .. } if !sid.is_empty() => {
                                        log_debug(&format!("[WARDIAN-DEBUG] Found session_id: {}", sid));
                                        session_id = Some(sid);
                                    }
                                    // ModelResponse means the prompt completed and the session
                                    // has been persisted to disk — safe to stop reading.
                                    AgentEvent::ModelResponse => {
                                        log_debug("[WARDIAN-DEBUG] Prompt complete, session saved.");
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
                Ok(sid) => { session_id_res = sid; false }
                Err(_) => { log_debug("[WARDIAN-DEBUG] Timed out waiting for session_id."); true }
            };

            // Only force-kill if we timed out; otherwise let the process exit naturally
            // so the session is fully flushed to disk before we attempt --resume.
            if timed_out {
                let _ = child.kill().await;
            }
            let _ = child.wait().await;
            log_debug(&format!("[WARDIAN-DEBUG] Returning session_id: {:?}", session_id_res));
            session_id_res
        }
        Err(e) => {
            log_debug(&format!("[WARDIAN-DEBUG] Failed to spawn cmd: {:?}", e));
            None
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
                        "claude" => {
                            // Claude logs are JSONL — one JSON object per line
                            let lines: Vec<serde_json::Value> = content
                                .lines()
                                .filter_map(|l| serde_json::from_str(l).ok())
                                .collect();

                            q_count = lines.iter().filter(|l| {
                                l.get("type").and_then(|v| v.as_str()) == Some("user")
                            }).count();

                            if let Some(first) = lines.first() {
                                if let Some(ts) = first.get("timestamp").and_then(|v| v.as_str()) {
                                    i_ts = Some(ts.to_string());
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

            // 3. Create provider stub files
            for stub_name in &["GEMINI.md", "CLAUDE.md"] {
                let stub_path = role_dir.join(stub_name);
                if !stub_path.exists() {
                    let _ = std::fs::write(stub_path, "@AGENTS.md\n");
                }
            }
        }
    }
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
