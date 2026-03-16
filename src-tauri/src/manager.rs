use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};
use std::collections::HashMap;
use std::io::{Read, Write};
use tauri::{AppHandle, Emitter, Manager};
use crate::models::{AgentConfig, AgentTelemetry, AgentClassDefinition};
use crate::state::{ActiveAgent, AppState};

pub use crate::utils::logging::log_debug;
pub use crate::utils::fs::*;

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

pub fn resolve_gemini_binary() -> (String, Vec<String>) {
    if cfg!(target_os = "windows") {
        let appdata = std::env::var("APPDATA").unwrap_or_default();
        let target = std::path::Path::new(&appdata)
            .join("npm")
            .join("node_modules")
            .join("@google")
            .join("gemini-cli")
            .join("dist")
            .join("index.js");

        if target.exists() {
            ("node".to_string(), vec![target.to_string_lossy().to_string()])
        } else {
            ("gemini.cmd".to_string(), vec![])
        }
    } else {
        ("gemini".to_string(), vec![])
    }
}

pub async fn spawn_gemini_cli(
    app: AppHandle,
    config: AgentConfig,
    _is_restored: bool,
) -> Result<ActiveAgent, String> {
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

    let (bin, args) = resolve_gemini_binary();
    let mut cmd = CommandBuilder::new(bin);
    for arg in args {
        cmd.arg(arg);
    }
    cmd.cwd(&expected_folder);

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

    if !final_includes.is_empty() {
        cmd.arg("--include-directories");
        cmd.arg(final_includes.join(","));
    }

    if config.debug.unwrap_or(false) { cmd.arg("--debug"); }
    if let Some(ref model) = config.model { cmd.arg("--model"); cmd.arg(model); }
    if config.sandbox.unwrap_or(false) { cmd.arg("--sandbox"); }
    if config.yolo.unwrap_or(false) { cmd.arg("--yolo"); }
    if let Some(ref approval) = config.approval_mode {
        if !approval.trim().is_empty() {
            cmd.arg("--approval-mode");
            cmd.arg(approval);
        }
    }
    if let Some(ref policy) = config.policy {
        if !policy.is_empty() {
            cmd.arg("--policy");
            cmd.arg(policy.join(","));
        }
    }
    if config.experimental_acp.unwrap_or(false) { cmd.arg("--experimental-acp"); }
    if let Some(ref servers) = config.allowed_mcp_server_names {
        for s in servers { cmd.arg("--allowed-mcp-server-names"); cmd.arg(s); }
    }
    if let Some(ref extensions) = config.extensions {
        if !extensions.is_empty() {
            cmd.arg("--extensions");
            cmd.arg(extensions.join(","));
        }
    }
    if config.screen_reader.unwrap_or(false) { cmd.arg("--screen-reader"); }
    if let Some(ref format) = config.output_format { cmd.arg("--output-format"); cmd.arg(format); }

    if let Some(ref custom) = config.custom_args {
        if let Some(args) = shlex::split(custom) {
            for arg in args { cmd.arg(arg); }
        }
    }

    let resume_id = config.resume_session.as_deref().unwrap_or("");

    if !resume_id.is_empty() {
        cmd.arg("--resume");
        cmd.arg(resume_id);
    }

    log_debug(&format!(
        "[Wardian] Spawning gemini. Session: {}, Resume ID: {}",
        config.session_id, resume_id
    ));

    let child = pair.slave.spawn_command(cmd)
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
        } else { None }
    };
    #[cfg(not(windows))]
    let job_object = None;

    let mut reader = pair.master.try_clone_reader()
        .map_err(|e| format!("Failed to get pty reader: {}", e))?;
    let mut writer = pair.master.take_writer()
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

    std::thread::spawn(move || {
        let mut buf = [0; 4096];
        let mut current_line = String::new();
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    let text = String::from_utf8_lossy(&buf[0..n]).to_string();
                    if let Ok(mut h) = output_buffer_clone.lock() { h.push_str(&text); }
                    current_line.push_str(&text);
                    loop {
                        if let Some(start) = current_line.find('{') {
                            let slice = &current_line[start..];
                            let mut stream = serde_json::Deserializer::from_str(slice).into_iter::<serde_json::Value>();
                            match stream.next() {
                                Some(Ok(parsed)) => {
                                    if let Some(msg_type) = parsed.get("type").and_then(|v| v.as_str()) {
                                        if msg_type == "user" {
                                            if let Ok(mut count) = query_count_clone.lock() { *count += 1; }
                                            if let Ok(mut status) = current_status_clone.lock() { *status = "Processing...".to_string(); }
                                        } else if msg_type == "init" {
                                            if let Ok(mut ts) = init_timestamp_clone.lock() {
                                                if let Some(timestamp) = parsed.get("timestamp").and_then(|v| v.as_str()) {
                                                    *ts = Some(timestamp.to_string());
                                                }
                                            }
                                        } else if ["gemini", "model", "info"].contains(&msg_type) {
                                            if let Ok(mut status) = current_status_clone.lock() { *status = "Idle".to_string(); }
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
                    if current_line.len() > 10000 { current_line.clear(); }
                }
                Err(_) => break,
            }
        }
    });

    Ok(ActiveAgent {
        config: AgentConfig { folder: expected_folder, ..config },
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
    if cols < 10 { return Ok(()); }
    let master_arc = {
        let agents = state.agents.lock().await;
        if let Some(agent) = agents.get(&session_id) {
            agent.pty_master.clone().ok_or_else(|| format!("Agent {} is off", session_id))?
        } else { return Err(format!("Agent {} not found", session_id)); }
    };
    let _ = tokio::task::spawn_blocking(move || {
        if let Ok(master) = master_arc.try_lock() {
            let _ = master.resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 });
        }
    });
    Ok(())
}

pub async fn run_gemini_headless(cwd: &std::path::PathBuf, prompt: &str, session_id: &str, output_format: &str) -> Result<serde_json::Value, String> {
    use tokio::io::{AsyncBufReadExt, BufReader};
    let (bin, args) = resolve_gemini_binary();
    let mut cmd = if cfg!(target_os = "windows") && bin.ends_with(".cmd") {
        let mut c = tokio::process::Command::new("cmd");
        c.arg("/c").arg(&bin);
        c
    } else {
        tokio::process::Command::new(&bin)
    };
    for arg in args { cmd.arg(arg); }
    cmd.arg("-p").arg(prompt)
       .arg("--output-format").arg(output_format)
       .arg("--resume").arg(session_id)
       .current_dir(cwd)
       .stdout(std::process::Stdio::piped())
       .stderr(std::process::Stdio::null());

    let mut child = cmd.spawn().map_err(|e| e.to_string())?;
    let mut output = String::new();

    if let Some(stdout) = child.stdout.take() {
        let mut reader = BufReader::new(stdout);
        let mut line = String::new();
        while let Ok(n) = reader.read_line(&mut line).await {
            if n == 0 { break; }
            output.push_str(&line);
            line.clear();
        }
    }

    let _ = child.wait().await;
    
    if output_format == "json" {
        serde_json::from_str(&output).map_err(|e| format!("Failed to parse JSON output: {}. Raw: {}", e, output))
    } else {
        Ok(serde_json::json!({ "text": output }))
    }
}

pub async fn obtain_session_id_headless(cwd: &std::path::PathBuf) -> Option<String> {
    use tokio::io::{AsyncBufReadExt, BufReader};
    let (bin, args) = resolve_gemini_binary();
    let mut cmd = if cfg!(target_os = "windows") && bin.ends_with(".cmd") {
        let mut c = tokio::process::Command::new("cmd");
        c.arg("/c").arg(&bin);
        c
    } else {
        tokio::process::Command::new(&bin)
    };
    for arg in args {
        cmd.arg(arg);
    }
    cmd.arg("-p")
        .arg("Introduce yourself")
        .arg("-o")
        .arg("stream-json")
        .current_dir(cwd)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());

    if let Ok(mut child) = cmd.spawn() {
        let mut session_id_res = None;
        if let Some(stdout) = child.stdout.take() {
            let mut reader = BufReader::new(stdout);
            let mut line = String::new();
            while let Ok(n) = reader.read_line(&mut line).await {
                if n == 0 {
                    break;
                }
                if session_id_res.is_none() && line.trim().starts_with('{') {
                    if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&line) {
                        if parsed.get("type").and_then(|v| v.as_str()) == Some("init") {
                            if let Some(id) = parsed.get("session_id").and_then(|v| v.as_str()) {
                                session_id_res = Some(id.to_string());
                            }
                        }
                    }
                }
                line.clear();
            }
        }
        let _ = child.wait().await;
        return session_id_res;
    }
    None
}

pub async fn get_all_metrics(state: &AppState) -> Vec<AgentTelemetry> {
    struct AgentSnapshot {
        session_id: String,
        process_id: Option<u32>,
        query_count: std::sync::Arc<std::sync::Mutex<usize>>,
        init_timestamp: std::sync::Arc<std::sync::Mutex<Option<String>>>,
        current_status: std::sync::Arc<std::sync::Mutex<String>>,
        log_path: std::sync::Arc<std::sync::Mutex<Option<std::path::PathBuf>>>,
    }

    let snapshots: Vec<AgentSnapshot> = {
        let agents = state.agents.lock().await;
        agents.iter().map(|(sid, agent)| AgentSnapshot {
            session_id: sid.clone(),
            process_id: agent.process_id,
            query_count: agent.query_count.clone(),
            init_timestamp: agent.init_timestamp.clone(),
            current_status: agent.current_status.clone(),
            log_path: agent.log_path.clone(),
        }).collect()
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
                fn sum_tree(pid: sysinfo::Pid, sys: &sysinfo::System, cmap: &HashMap<sysinfo::Pid, Vec<sysinfo::Pid>>, cpu: &mut f32, mem: &mut f64, uptime: &mut u64) {
                    if let Some(p) = sys.process(pid) {
                        *cpu += p.cpu_usage();
                        *mem += p.memory() as f64 / 1_048_576.0;
                        *uptime = std::cmp::max(*uptime, p.run_time());
                    }
                    if let Some(children) = cmap.get(&pid) {
                        for &cpid in children { sum_tree(cpid, sys, cmap, cpu, mem, uptime); }
                    }
                }
                sum_tree(root_pid, &sys, &children_map, &mut cpu, &mut mem, &mut uptime);
            }

            let mut q_count = 0;
            let mut i_ts = None;
            let mut s_val = "Idle".to_string();
            let mut log_path_lock = snap.log_path.lock().unwrap_or_else(|e| e.into_inner());

            if log_path_lock.is_none() {
                if let Some(app_dir) = get_wardian_home() {
                    let tmp_dir = app_dir.join("../.gemini/tmp");
                    if let Ok(entries) = std::fs::read_dir(tmp_dir) {
                        for entry in entries.flatten() {
                            let chat_dir = entry.path().join("chats");
                            if let Ok(chat_files) = std::fs::read_dir(chat_dir) {
                                for chat_file in chat_files.flatten() {
                                    if let Ok(content) = std::fs::read_to_string(chat_file.path()) {
                                        if let Ok(p) = serde_json::from_str::<serde_json::Value>(&content) {
                                            if p.get("sessionId").and_then(|v| v.as_str()) == Some(&snap.session_id) {
                                                *log_path_lock = Some(chat_file.path());
                                                break;
                                            }
                                        }
                                    }
                                }
                            }
                            if log_path_lock.is_some() { break; }
                        }
                    }
                }
            }

            if let Some(ref path) = *log_path_lock {
                if let Ok(content) = std::fs::read_to_string(path) {
                    if let Ok(p) = serde_json::from_str::<serde_json::Value>(&content) {
                        if let Some(msgs) = p.get("messages").and_then(|v| v.as_array()) {
                            q_count = msgs.iter().filter(|m| m.get("type").and_then(|v| v.as_str()) == Some("user")).count();
                            if let Some(last) = msgs.last() {
                                let m_type = last.get("type").and_then(|v| v.as_str()).unwrap_or("");
                                s_val = if m_type == "user" { "Processing...".into() } else if ["gemini", "model", "info"].contains(&m_type) { "Idle".into() } else { "Action Needed".into() };
                            }
                        }
                        if let Some(st) = p.get("startTime").and_then(|v| v.as_str()) { i_ts = Some(st.to_string()); }
                    }
                }
            }

            if q_count > 0 { *snap.query_count.lock().unwrap() = q_count; }
            if let Some(ts) = i_ts { *snap.init_timestamp.lock().unwrap() = Some(ts); }
            if s_val != "Idle" || *snap.current_status.lock().unwrap() == "Idle" {
                *snap.current_status.lock().unwrap() = s_val;
            }

            results.push(AgentTelemetry {
                session_id: snap.session_id.clone(),
                cpu_usage: cpu,
                memory_mb: mem,
                uptime_seconds: uptime,
                query_count: *snap.query_count.lock().unwrap(),
                init_timestamp: snap.init_timestamp.lock().unwrap().clone(),
                current_status: snap.current_status.lock().unwrap().clone(),
                log_path: log_path_lock.as_ref().map(|p| p.to_string_lossy().to_string()),
            });
        }
        results
    }).await.unwrap_or_default()
}

pub fn get_all_agent_classes(_app: &AppHandle) -> Vec<AgentClassDefinition> {
    let mut defaults: Vec<AgentClassDefinition> = serde_json::from_str(include_str!("default_classes.json")).unwrap_or_default();
    for d in defaults.iter_mut() { d.is_default = true; }
    let mut custom = Vec::new();
    if let Some(app_dir) = get_wardian_home() {
        if let Ok(data) = std::fs::read_to_string(app_dir.join("custom_classes.json")) {
            custom = serde_json::from_str::<Vec<AgentClassDefinition>>(&data).unwrap_or_default();
        }
    }
    for c in custom.iter_mut() { c.is_default = false; }
    defaults.extend(custom);
    defaults
}

pub fn save_custom_classes(_app: &AppHandle, classes: &[AgentClassDefinition]) -> Result<(), String> {
    let app_dir = get_wardian_home().ok_or("No home dir")?;
    let json = serde_json::to_string_pretty(classes).map_err(|e| e.to_string())?;
    std::fs::write(app_dir.join("custom_classes.json"), json).map_err(|e| e.to_string())?;
    Ok(())
}

pub fn init_agent_classes(app: &AppHandle) {
    if let Some(app_dir) = get_wardian_home() {
        let classes_dir = app_dir.join("classes");
        let _ = std::fs::create_dir_all(&classes_dir);
        let _ = std::fs::create_dir_all(app_dir.join("common/desk"));
        let _ = std::fs::create_dir_all(app_dir.join("common/lineages"));

        let classes = get_all_agent_classes(app);
        for cls in &classes {
            let role_dir = classes_dir.join(&cls.name);
            let _ = std::fs::create_dir_all(&role_dir);
            let md_path = role_dir.join("GEMINI.md");
            if !md_path.exists() {
                let content = if cls.is_default {
                    app.path().resolve(format!("agent_prompts/{}.md", cls.name), tauri::path::BaseDirectory::Resource)
                       .ok()
                       .and_then(|p| std::fs::read_to_string(p).ok())
                       .unwrap_or_default()
                } else {
                    format!("# {} Agent\n\n{}\n", cls.name, cls.description)
                };
                let _ = std::fs::write(md_path, content);
            }
        }
    }
}

pub async fn kill_all_agents(state: &AppState) {
    let mut agents = state.agents.lock().await;
    for (sid, mut agent) in agents.drain() {
        log_debug(&format!("[Wardian] Killing session {}", sid));
        if let Some(mut child) = agent.child_process { let _ = child.kill(); }
        #[cfg(windows)] { let _ = agent.job_object.take(); }
    }
    state.agent_order.lock().await.clear();
}
