use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::{Read, Write};

pub fn log_debug(msg: &str) {
    for _ in 0..5 {
        if let Ok(mut file) = OpenOptions::new()
            .create(true)
            .append(true)
            .open("wardian_debug.log")
        {
            let _ = writeln!(file, "{}", msg);
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}
use tauri::{AppHandle, Emitter};
use tokio::sync::Mutex;

pub fn save_state(app: &AppHandle, agents: &HashMap<String, ActiveAgent>, order: &[String]) {
    let mut configs: Vec<AgentConfig> = Vec::new();
    for id in order {
        if let Some(agent) = agents.get(id) {
            configs.push(agent.config.clone());
        }
    }

    if let Ok(json) = serde_json::to_string_pretty(&configs) {
        use tauri::Manager;
        if let Ok(app_dir) = app.path().app_data_dir() {
            let _ = std::fs::create_dir_all(&app_dir);
            let state_path = app_dir.join("wardian_state.json");
            let _ = std::fs::write(state_path, json);
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    pub session_id: String,
    pub session_name: String,
    pub agent_class: String,
    pub folder: String,
    pub resume_session: Option<String>,
}

pub struct ActiveAgent {
    pub config: AgentConfig,
    pub child_process: Box<dyn portable_pty::Child + Send>,
    pub pty_master: std::sync::Arc<std::sync::Mutex<Box<dyn portable_pty::MasterPty + Send>>>,
    pub stdin_tx: tokio::sync::mpsc::Sender<String>,
    pub output_history: std::sync::Arc<std::sync::Mutex<String>>,
    pub process_id: Option<u32>,
    pub query_count: std::sync::Arc<std::sync::Mutex<usize>>,
    pub init_timestamp: std::sync::Arc<std::sync::Mutex<Option<String>>>,
    pub current_status: std::sync::Arc<std::sync::Mutex<String>>,
    pub log_path: std::sync::Arc<std::sync::Mutex<Option<std::path::PathBuf>>>,
    #[cfg(windows)]
    pub job_object: Option<win32job::Job>,
}

pub struct AppState {
    // Map of session_id to ActiveAgent
    pub agents: Mutex<HashMap<String, ActiveAgent>>,
    pub system_metrics: std::sync::Arc<Mutex<sysinfo::System>>,
    pub agent_order: Mutex<Vec<String>>,
}

impl AppState {
    pub fn new() -> Self {
        let mut sys = sysinfo::System::new_all();
        sys.refresh_all();
        Self {
            agents: Mutex::new(HashMap::new()),
            system_metrics: std::sync::Arc::new(Mutex::new(sys)),
            agent_order: Mutex::new(Vec::new()),
        }
    }
}

pub async fn spawn_gemini_cli(
    app: AppHandle,
    config: AgentConfig,
    is_restored: bool,
) -> Result<ActiveAgent, String> {
    // Determine the working directory
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

    let pty_system = NativePtySystem::default();

    let pair = pty_system
        .openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| format!("Failed to open pty: {}", e))?;

    let mut cmd = if cfg!(target_os = "windows") {
        CommandBuilder::new("gemini.cmd")
    } else {
        CommandBuilder::new("gemini")
    };
    cmd.cwd(&expected_folder);

    // Apply strict classes constraints organically if the context map natively exists
    use tauri::Manager;
    if let Ok(app_dir) = app.path().app_data_dir() {
        let class_path = app_dir.join("classes").join(&config.agent_class);
        if class_path.exists() {
            cmd.arg("--include-directories");
            cmd.arg(class_path.to_string_lossy().to_string());
        }
    }

    // If resume_session is explicitly provided from the UI, use it unconditionally.
    // Otherwise, ONLY attempt to resume via the object's session_id if Wardian is restoring from state.
    // Newly generated UUIDs from fresh agents DO NOT exist in the gemini backend yet, so doing it blindly breaks the CLI.
    let mut resume_id = String::new();
    if let Some(ref ui_override) = config.resume_session {
        resume_id = ui_override.clone();
    } else if is_restored {
        resume_id = config.session_id.clone();
    };

    if !resume_id.is_empty() {
        cmd.arg("--resume");
        cmd.arg(&resume_id);
    }

    log_debug(&format!(
        "[Wardian] Spawning gemini. Session: {}, CWD: {}, Resume ID: {}",
        config.session_id,
        cwd.display(),
        resume_id
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

            // Assign the portable_pty child process to the job
            if let Some(pid) = process_id {
                // Open raw win32 process handle natively to bind it
                unsafe {
                    use winapi::um::processthreadsapi::OpenProcess;
                    use winapi::um::winnt::PROCESS_SET_QUOTA;
                    use winapi::um::winnt::PROCESS_TERMINATE;
                    let handle = OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, 0, pid);
                    if !handle.is_null() {
                        // Cast the winapi::um::winnt::HANDLE (*mut c_void) to an isize safely
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

    log_debug(&format!(
        "[Wardian] Process spawned for session {}",
        config.session_id
    ));

    let mut reader = pair
        .master
        .try_clone_reader()
        .map_err(|e| format!("Failed to get pty reader: {}", e))?;

    let mut writer = pair
        .master
        .take_writer()
        .map_err(|e| format!("Failed to get pty writer: {}", e))?;

    // Wrap master to allow isolated background resizing across threads
    let pty_master = std::sync::Arc::new(std::sync::Mutex::new(pair.master));

    // Drop the slave handle to prevent deadlock (standard ConPTY practice)
    drop(pair.slave);

    let (tx, mut rx) = tokio::sync::mpsc::channel::<String>(32);
    let sid_in = config.session_id.clone();

    // Stdin writer task
    tokio::spawn(async move {
        while let Some(input) = rx.recv().await {
            let mut w = WriterThreadStruct { writer };
            let Ok(res) = tokio::task::spawn_blocking(move || {
                let _ = w.writer.write_all(input.as_bytes());
                let _ = w.writer.flush();
                w
            })
            .await
            else {
                log_debug(&format!(
                    "[Wardian] [{}] Writer thread deadlock or panic",
                    sid_in
                ));
                break;
            };
            writer = res.writer;
        }
    });

    let sid_out = config.session_id.clone();
    let history = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let history_clone = history.clone();

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
                Ok(0) => {
                    log_debug(&format!("[Wardian] [{}] Reader EOF", sid_out));
                    break;
                }
                Ok(n) => {
                    let text = String::from_utf8_lossy(&buf[0..n]).to_string();

                    // prevent UI memory starvation, cap length safely across unicode boundaries
                    if let Ok(mut h) = history_clone.lock() {
                        h.push_str(&text);
                        let len = h.len();
                        if len > 20_000 {
                            let split_idx = len - 10_000;
                            let valid_idx = (split_idx..len)
                                .find(|&i| h.is_char_boundary(i))
                                .unwrap_or(split_idx);
                            let suffix = h.split_off(valid_idx);
                            *h = suffix; // keep last 10k
                        }
                    }

                    if let Err(e) = app.emit(
                        "agent-output",
                        serde_json::json!(
                            {
                                "session_id": sid_out,
                                "text": text,
                                "stream": "stdout"
                            }
                        ),
                    ) {
                        log_debug(&format!("[Wardian] [{}] Emit error: {}", sid_out, e));
                    }

                    // JSON parsing
                    current_line.push_str(&text);
                    loop {
                        if let Some(start) = current_line.find('{') {
                            let slice = &current_line[start..];
                            let mut stream = serde_json::Deserializer::from_str(slice)
                                .into_iter::<serde_json::Value>();

                            match stream.next() {
                                Some(Ok(parsed)) => {
                                    // Track metadata natively
                                    if let Some(msg_type) =
                                        parsed.get("type").and_then(|v| v.as_str())
                                    {
                                        if msg_type == "user" {
                                            if let Ok(mut count) = query_count_clone.lock() {
                                                *count += 1;
                                            }
                                            if let Ok(mut status) = current_status_clone.lock() {
                                                *status = "Processing...".to_string();
                                            }
                                        } else if msg_type == "init" {
                                            if let Ok(mut ts) = init_timestamp_clone.lock() {
                                                if ts.is_none() {
                                                    if let Some(timestamp) = parsed
                                                        .get("timestamp")
                                                        .and_then(|v| v.as_str())
                                                    {
                                                        *ts = Some(timestamp.to_string());
                                                    }
                                                }
                                            }
                                        } else if msg_type == "gemini"
                                            || msg_type == "model"
                                            || msg_type == "info"
                                        {
                                            if let Ok(mut status) = current_status_clone.lock() {
                                                *status = "Idle".to_string();
                                            }
                                        }
                                    }

                                    let _ = app.emit(
                                        "agent-json-event",
                                        serde_json::json!({
                                            "session_id": sid_out,
                                            "data": parsed
                                        }),
                                    );
                                    let consumed = stream.byte_offset();
                                    current_line = current_line[start + consumed..].to_string();
                                    continue;
                                }
                                Some(Err(_)) => {
                                    // Incomplete JSON or garbage, break and wait for more data.
                                    break;
                                }
                                None => break, // No more JSON
                            }
                        }
                        break;
                    }

                    if current_line.len() > 10000 {
                        current_line.clear();
                    }
                }
                Err(e) => {
                    log_debug(&format!("[Wardian] [{}] Reader error: {}", sid_out, e));
                    break;
                }
            }
        }
    });

    let final_config = AgentConfig {
        folder: expected_folder,
        ..config
    };

    Ok(ActiveAgent {
        config: final_config,
        child_process: child,
        pty_master: pty_master,
        stdin_tx: tx,
        output_history: history,
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
    let master_arc = {
        let agents = state.agents.lock().await;
        if let Some(agent) = agents.get(&session_id) {
            agent.pty_master.clone()
        } else {
            return Err(format!("Agent {} not found", session_id));
        }
    }; // Agents lock is explicitly dropped here

    // Execute synchronous resizing in a background thread.
    // Windows ConPTY frequently deadlocks when resized while waiting for input.
    // By decoupling it, we sacrifice future terminal resizes for this agent
    // but protect the entire application from an unrecoverable global deadlock.
    let _ = tokio::task::spawn_blocking(move || {
        if let Ok(master) = master_arc.lock() {
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

// Helper struct to pass writer back and forth to tokio::spawn_blocking
struct WriterThreadStruct {
    pub writer: Box<dyn Write + Send>,
}

pub async fn obtain_session_id_headless(cwd: &std::path::PathBuf) -> Option<String> {
    use tokio::io::{AsyncBufReadExt, BufReader};

    let mut cmd = tokio::process::Command::new(if cfg!(windows) {
        "gemini.cmd"
    } else {
        "gemini"
    });
    cmd.arg("-p")
        .arg("Introduce yourself")
        .arg("-o")
        .arg("stream-json")
        .current_dir(cwd)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());

    #[cfg(windows)]
    {
        // CREATE_NO_WINDOW is tricky with tokio traits, skipping for now to ensure build stability
    }

    if let Ok(mut child) = cmd.spawn() {
        if let Some(stdout) = child.stdout.take() {
            let mut reader = BufReader::new(stdout);
            let mut line = String::new();
            while let Ok(bytes_read) = reader.read_line(&mut line).await {
                if bytes_read == 0 {
                    break;
                }

                if line.trim().starts_with('{') {
                    if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&line) {
                        if parsed.get("type").and_then(|v| v.as_str()) == Some("init") {
                            if let Some(id) = parsed.get("session_id").and_then(|v| v.as_str()) {
                                let result = id.to_string();
                                let _ = child.kill().await;
                                return Some(result);
                            }
                        }
                    }
                }
                line.clear();
            }
        }
        let _ = child.kill().await;
    }
    None
}

#[derive(serde::Serialize)]
pub struct AgentTelemetry {
    pub session_id: String,
    pub cpu_usage: f32,
    pub memory_mb: f64,
    pub uptime_seconds: u64,
    pub query_count: usize,
    pub init_timestamp: Option<String>,
    pub current_status: String,
}

pub async fn get_all_metrics(state: &AppState) -> Vec<AgentTelemetry> {
    let mut results = Vec::new();
    let agents = state.agents.lock().await;

    let mut sys = state.system_metrics.lock().await;
    sys.refresh_all(); // Full refresh is more reliable for parent-child discovery in current sysinfo version

    // Index processes by parent ID for O(1) child lookup
    let mut children_map: HashMap<sysinfo::Pid, Vec<sysinfo::Pid>> = HashMap::new();
    for (pid, process) in sys.processes() {
        if let Some(parent) = process.parent() {
            children_map.entry(parent).or_default().push(*pid);
        }
    }

    for (sid, agent) in agents.iter() {
        let mut cpu_usage = 0.0;
        let mut memory_mb = 0.0;
        let mut uptime_seconds = 0;

        if let Some(pid) = agent.process_id {
            let root_pid = sysinfo::Pid::from_u32(pid);

            // Recursive function to sum up process tree
            fn sum_tree(
                pid: sysinfo::Pid,
                sys: &sysinfo::System,
                children_map: &HashMap<sysinfo::Pid, Vec<sysinfo::Pid>>,
                cpu: &mut f32,
                mem: &mut f64,
                uptime: &mut u64,
            ) {
                if let Some(proc) = sys.process(pid) {
                    *cpu += proc.cpu_usage();
                    *mem += proc.memory() as f64 / 1_048_576.0;
                    *uptime = std::cmp::max(*uptime, proc.run_time());
                }
                if let Some(children) = children_map.get(&pid) {
                    for &child_pid in children {
                        sum_tree(child_pid, sys, children_map, cpu, mem, uptime);
                    }
                }
            }

            sum_tree(
                root_pid,
                &sys,
                &children_map,
                &mut cpu_usage,
                &mut memory_mb,
                &mut uptime_seconds,
            );
        }

        let mut queries_val = 0;
        let mut init_ts_val = None;
        let mut status_val = "Idle".to_string();

        let mut log_path_lock = agent.log_path.lock().unwrap_or_else(|e| e.into_inner());
        if log_path_lock.is_none() {
            let home = if cfg!(windows) {
                std::env::var("USERPROFILE")
                    .map(std::path::PathBuf::from)
                    .unwrap_or_else(|_| std::path::PathBuf::from("C:\\"))
            } else {
                std::env::var("HOME")
                    .map(std::path::PathBuf::from)
                    .unwrap_or_else(|_| std::path::PathBuf::from("/"))
            };

            let tmp_dir = home.join(".gemini/tmp");
            if let Ok(entries) = std::fs::read_dir(tmp_dir) {
                for entry in entries.flatten() {
                    if entry.path().is_dir() {
                        let chats_dir = entry.path().join("chats");
                        if chats_dir.exists() {
                            if let Ok(chat_files) = std::fs::read_dir(chats_dir) {
                                for chat_file in chat_files.flatten() {
                                    if chat_file.path().is_file()
                                        && chat_file.path().extension().and_then(|s| s.to_str())
                                            == Some("json")
                                    {
                                        if let Ok(content) =
                                            std::fs::read_to_string(chat_file.path())
                                        {
                                            if let Ok(parsed) =
                                                serde_json::from_str::<serde_json::Value>(&content)
                                            {
                                                if parsed.get("sessionId").and_then(|v| v.as_str())
                                                    == Some(sid)
                                                {
                                                    *log_path_lock = Some(chat_file.path());
                                                    break;
                                                }
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

        if let Some(ref path) = *log_path_lock {
            if let Ok(content) = std::fs::read_to_string(path) {
                if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&content) {
                    if let Some(messages) = parsed.get("messages").and_then(|v| v.as_array()) {
                        for msg in messages {
                            if msg.get("type").and_then(|v| v.as_str()) == Some("user") {
                                queries_val += 1;
                            }
                        }
                        if let Some(last_msg) = messages.last() {
                            if let Some(msg_type) = last_msg.get("type").and_then(|v| v.as_str()) {
                                if msg_type == "user" {
                                    status_val = "Processing...".to_string();
                                } else if msg_type == "gemini"
                                    || msg_type == "model"
                                    || msg_type == "info"
                                {
                                    status_val = "Idle".to_string();
                                } else {
                                    status_val = "Action Needed".to_string();
                                }
                            }
                        }
                    }
                    if let Some(start_time) = parsed.get("startTime").and_then(|v| v.as_str()) {
                        init_ts_val = Some(start_time.to_string());
                    }
                }
            }
        }

        // Apply metadata directly to cached structs if we found better data in logs
        if queries_val > 0 {
            *agent.query_count.lock().unwrap_or_else(|e| e.into_inner()) = queries_val;
        }
        if let Some(ts) = init_ts_val {
            *agent
                .init_timestamp
                .lock()
                .unwrap_or_else(|e| e.into_inner()) = Some(ts);
        }
        if status_val != "Idle"
            || *agent
                .current_status
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                == "Idle"
        {
            *agent
                .current_status
                .lock()
                .unwrap_or_else(|e| e.into_inner()) = status_val;
        }

        let query_count = *agent.query_count.lock().unwrap_or_else(|e| e.into_inner());
        let init_timestamp = agent
            .init_timestamp
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let current_status = agent
            .current_status
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();

        results.push(AgentTelemetry {
            session_id: sid.clone(),
            cpu_usage,
            memory_mb,
            uptime_seconds,
            query_count,
            init_timestamp,
            current_status,
        });
    }

    results
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentClassDefinition {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub is_default: bool,
}

/// Returns all agent classes (defaults + custom) with `is_default` flag set.
pub fn get_all_agent_classes(app: &AppHandle) -> Vec<AgentClassDefinition> {
    use tauri::Manager;
    let default_classes_json = include_str!("default_classes.json");
    let mut defaults: Vec<AgentClassDefinition> =
        serde_json::from_str(default_classes_json).unwrap_or_default();
    for d in defaults.iter_mut() {
        d.is_default = true;
    }

    let mut custom: Vec<AgentClassDefinition> = Vec::new();
    if let Ok(app_dir) = app.path().app_data_dir() {
        let custom_path = app_dir.join("custom_classes.json");
        if let Ok(data) = std::fs::read_to_string(&custom_path) {
            if let Ok(parsed) = serde_json::from_str::<Vec<AgentClassDefinition>>(&data) {
                custom = parsed;
            }
        }
    }
    for c in custom.iter_mut() {
        c.is_default = false;
    }

    defaults.extend(custom);
    defaults
}

/// Saves the custom classes list to `<AppData>/custom_classes.json`.
pub fn save_custom_classes(
    app: &AppHandle,
    classes: &[AgentClassDefinition],
) -> Result<(), String> {
    use tauri::Manager;
    let app_dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    let _ = std::fs::create_dir_all(&app_dir);
    let custom_path = app_dir.join("custom_classes.json");
    let json = serde_json::to_string_pretty(classes).map_err(|e| e.to_string())?;
    std::fs::write(custom_path, json).map_err(|e| e.to_string())?;
    Ok(())
}

/// Scaffolds filesystem directories for a single agent class.
fn scaffold_class_dir(app: &AppHandle, class: &AgentClassDefinition) {
    use tauri::Manager;
    if let Ok(app_dir) = app.path().app_data_dir() {
        let classes_dir = app_dir.join("classes");
        let role_dir = classes_dir.join(&class.name);
        if !role_dir.exists() {
            let _ = std::fs::create_dir_all(&role_dir);
        }
        let gemini_md_path = role_dir.join("GEMINI.md");
        if !gemini_md_path.exists() {
            if class.is_default {
                // Read the corresponding markdown file from bundled resources
                let prompt_content = if let Ok(resource_path) = app.path().resolve(
                    format!("agent_prompts/{}.md", class.name),
                    tauri::path::BaseDirectory::Resource,
                ) {
                    std::fs::read_to_string(resource_path).unwrap_or_else(|_| String::new())
                } else {
                    String::from("Failed to load prompt from resources.")
                };
                let _ = std::fs::write(gemini_md_path, prompt_content);
            } else {
                // Custom class: scaffold a basic GEMINI.md
                let content = format!("# {} Agent\n\n{}\n", class.name, class.description);
                let _ = std::fs::write(gemini_md_path, content);
            }
        }
    }
}

pub fn init_agent_classes(app: &AppHandle) {
    use tauri::Manager;
    if let Ok(app_dir) = app.path().app_data_dir() {
        let classes_dir = app_dir.join("classes");
        if !classes_dir.exists() {
            let _ = std::fs::create_dir_all(&classes_dir);
        }

        let all_classes = get_all_agent_classes(app);

        let mut agents_registry = String::from("# Wardian Agent Registry\n\nThis file catalogs the active agent classes available natively within your local Wardian deployment. Altering the `GEMINI.md` within the `classes/<Role>` sub-directories will immediately evolve the agent's core context parameters upon spawning.\n\n");

        for class in &all_classes {
            scaffold_class_dir(app, class);
            agents_registry.push_str(&format!("## {}\n{}\n\n", class.name, class.description));
        }

        let gemini_dir = if cfg!(windows) {
            std::env::var("USERPROFILE")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|_| std::path::PathBuf::from("C:\\"))
                .join(".gemini")
        } else {
            std::env::var("HOME")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|_| std::path::PathBuf::from("/"))
                .join(".gemini")
        };

        if !gemini_dir.exists() {
            let _ = std::fs::create_dir_all(&gemini_dir);
        }

        let registry_path = gemini_dir.join("AGENTS.md");
        let registry_block = format!(
            "<!-- WARDIAN_REGISTRY_START -->\n{}\n<!-- WARDIAN_REGISTRY_END -->",
            agents_registry.trim_end()
        );

        if !registry_path.exists() {
            let _ = std::fs::write(&registry_path, &registry_block);
        } else if let Ok(content) = std::fs::read_to_string(&registry_path) {
            if let (Some(start), Some(end)) = (
                content.find("<!-- WARDIAN_REGISTRY_START -->"),
                content.find("<!-- WARDIAN_REGISTRY_END -->"),
            ) {
                // Replace existing block in-place, preserving surrounding user content
                let before = &content[..start];
                let after = &content[end + "<!-- WARDIAN_REGISTRY_END -->".len()..];
                let new_content = format!("{}{}{}", before, registry_block, after);
                let _ = std::fs::write(&registry_path, new_content);
            } else {
                // No existing block — append with separator
                let mut new_content = content;
                new_content.push_str("\n\n---\n\n");
                new_content.push_str(&registry_block);
                let _ = std::fs::write(&registry_path, new_content);
            }
        }
    }
}

pub async fn kill_all_agents(state: &AppState) {
    let mut agents = state.agents.lock().await;
    let mut order = state.agent_order.lock().await;

    log_debug(&format!(
        "[Wardian] Killing all {} agents on exit...",
        agents.len()
    ));

    for (sid, mut agent) in agents.drain() {
        log_debug(&format!("[Wardian] Killing agent session {}", sid));
        let _ = agent.child_process.kill();
        // Dropping the agent struct here will close pty_master and job_object handles
    }
    order.clear();
}

// ══════════════════════════════════════════════════════════════════════
// Pure functions extracted for testability
// ══════════════════════════════════════════════════════════════════════

/// Event effect produced by processing a streaming JSON event.
#[derive(Debug, PartialEq)]
pub enum JsonEventEffect {
    IncrementQueryAndSetProcessing,
    SetTimestamp(String),
    SetIdle,
    NoOp,
}

/// Determines the resume ID for spawning a Gemini CLI session.
/// - UI override (explicit resume_session) takes priority.
/// - If restored from state, uses the existing session_id.
/// - Otherwise returns empty (fresh agent, no resume).
pub fn determine_resume_id(config: &AgentConfig, is_restored: bool) -> String {
    if let Some(ref ui_override) = config.resume_session {
        if !ui_override.is_empty() {
            return ui_override.clone();
        }
    }
    if is_restored {
        return config.session_id.clone();
    }
    String::new()
}

/// Resolves the working directory from the config folder field.
pub fn resolve_working_directory(folder: &str) -> std::path::PathBuf {
    if folder.is_empty() {
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
        std::path::PathBuf::from(folder)
    }
}

/// Classifies a streaming JSON event and returns the effect it should have.
pub fn process_json_event(parsed: &serde_json::Value) -> JsonEventEffect {
    if let Some(msg_type) = parsed.get("type").and_then(|v| v.as_str()) {
        match msg_type {
            "user" => JsonEventEffect::IncrementQueryAndSetProcessing,
            "init" => {
                if let Some(ts) = parsed.get("timestamp").and_then(|v| v.as_str()) {
                    JsonEventEffect::SetTimestamp(ts.to_string())
                } else {
                    JsonEventEffect::NoOp
                }
            }
            "gemini" | "model" | "info" => JsonEventEffect::SetIdle,
            _ => JsonEventEffect::NoOp,
        }
    } else {
        JsonEventEffect::NoOp
    }
}

/// Caps the output history buffer to prevent memory starvation.
/// Unicode-safe: finds a valid char boundary before truncating.
pub fn cap_output_history(history: &mut String, max_len: usize, keep_len: usize) {
    let len = history.len();
    if len > max_len {
        let split_idx = len - keep_len;
        let valid_idx = (split_idx..len)
            .find(|&i| history.is_char_boundary(i))
            .unwrap_or(split_idx);
        let suffix = history.split_off(valid_idx);
        *history = suffix;
    }
}

/// Parses metadata from a Gemini CLI log file.
/// Returns (query_count, status_from_last_message, init_timestamp).
pub fn parse_log_metadata(content: &str) -> (u32, String, Option<String>) {
    let mut query_count = 0u32;
    let mut status = "Idle".to_string();
    let mut init_ts = None;

    if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(content) {
        if let Some(messages) = parsed.get("messages").and_then(|v| v.as_array()) {
            for msg in messages {
                if msg.get("type").and_then(|v| v.as_str()) == Some("user") {
                    query_count += 1;
                }
            }
            if let Some(last_msg) = messages.last() {
                if let Some(msg_type) = last_msg.get("type").and_then(|v| v.as_str()) {
                    if msg_type == "user" {
                        status = "Processing...".to_string();
                    } else if msg_type == "gemini" || msg_type == "model" || msg_type == "info" {
                        status = "Idle".to_string();
                    } else {
                        status = "Action Needed".to_string();
                    }
                }
            }
        }
        if let Some(start_time) = parsed.get("startTime").and_then(|v| v.as_str()) {
            init_ts = Some(start_time.to_string());
        }
    }

    (query_count, status, init_ts)
}

/// Finds a session log file within the mock `.gemini/tmp/*/chats/` directory tree.
pub fn find_session_log(tmp_dir: &std::path::Path, session_id: &str) -> Option<std::path::PathBuf> {
    if let Ok(entries) = std::fs::read_dir(tmp_dir) {
        for entry in entries.flatten() {
            if entry.path().is_dir() {
                let chats_dir = entry.path().join("chats");
                if chats_dir.exists() {
                    if let Ok(chat_files) = std::fs::read_dir(&chats_dir) {
                        for chat_file in chat_files.flatten() {
                            if chat_file.path().is_file()
                                && chat_file.path().extension().and_then(|s| s.to_str())
                                    == Some("json")
                            {
                                if let Ok(content) = std::fs::read_to_string(chat_file.path()) {
                                    if let Ok(parsed) =
                                        serde_json::from_str::<serde_json::Value>(&content)
                                    {
                                        if parsed.get("sessionId").and_then(|v| v.as_str())
                                            == Some(session_id)
                                        {
                                            return Some(chat_file.path());
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    None
}

/// Loads custom classes from a directory containing `custom_classes.json`.
pub fn load_custom_classes(app_dir: &std::path::Path) -> Vec<AgentClassDefinition> {
    let custom_path = app_dir.join("custom_classes.json");
    if let Ok(data) = std::fs::read_to_string(&custom_path) {
        serde_json::from_str::<Vec<AgentClassDefinition>>(&data).unwrap_or_default()
    } else {
        Vec::new()
    }
}

/// Saves custom classes to a path.
pub fn save_custom_classes_to_path(
    path: &std::path::Path,
    classes: &[AgentClassDefinition],
) -> Result<(), String> {
    let json = serde_json::to_string_pretty(classes).map_err(|e| e.to_string())?;
    std::fs::write(path, json).map_err(|e| e.to_string())?;
    Ok(())
}

/// Merges default and custom classes into a single list with is_default flags set.
pub fn merge_classes(
    defaults: &[AgentClassDefinition],
    custom: &[AgentClassDefinition],
) -> Vec<AgentClassDefinition> {
    let mut result: Vec<AgentClassDefinition> = defaults
        .iter()
        .map(|d| AgentClassDefinition {
            name: d.name.clone(),
            description: d.description.clone(),
            is_default: true,
        })
        .collect();
    result.extend(custom.iter().map(|c| AgentClassDefinition {
        name: c.name.clone(),
        description: c.description.clone(),
        is_default: false,
    }));
    result
}

// ══════════════════════════════════════════════════════════════════════
// Tests
// ══════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    // ── Serde Round-Trip Tests ──────────────────────────────────────

    #[test]
    fn agent_config_serde_roundtrip() {
        let config = AgentConfig {
            session_id: "abc-123".into(),
            session_name: "TestAgent".into(),
            agent_class: "Coder".into(),
            folder: "C:/project".into(),
            resume_session: Some("def-456".into()),
        };
        let json = serde_json::to_string(&config).unwrap();
        let deserialized: AgentConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(config.session_id, deserialized.session_id);
        assert_eq!(config.session_name, deserialized.session_name);
        assert_eq!(config.agent_class, deserialized.agent_class);
        assert_eq!(config.folder, deserialized.folder);
        assert_eq!(config.resume_session, deserialized.resume_session);
    }

    #[test]
    fn agent_config_optional_resume() {
        let config = AgentConfig {
            session_id: "abc".into(),
            session_name: "Test".into(),
            agent_class: "QA".into(),
            folder: "".into(),
            resume_session: None,
        };
        let json = serde_json::to_string(&config).unwrap();
        let deserialized: AgentConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.resume_session, None);
    }

    #[test]
    fn agent_class_definition_serde_roundtrip() {
        let cls = AgentClassDefinition {
            name: "DevOps".into(),
            description: "Manages CI/CD".into(),
            is_default: false,
        };
        let json = serde_json::to_string(&cls).unwrap();
        let deserialized: AgentClassDefinition = serde_json::from_str(&json).unwrap();
        assert_eq!(cls.name, deserialized.name);
        assert_eq!(cls.description, deserialized.description);
        assert_eq!(cls.is_default, deserialized.is_default);
    }

    #[test]
    fn agent_class_definition_is_default_defaults_to_false() {
        let json = r#"{"name":"Test","description":"A test class"}"#;
        let cls: AgentClassDefinition = serde_json::from_str(json).unwrap();
        assert_eq!(cls.is_default, false);
    }

    #[test]
    fn agent_telemetry_serializes() {
        let telemetry = AgentTelemetry {
            session_id: "abc".into(),
            cpu_usage: 15.5,
            memory_mb: 256.3,
            uptime_seconds: 3600,
            query_count: 42,
            init_timestamp: Some("2026-01-01T00:00:00Z".into()),
            current_status: "Idle".into(),
        };
        let json = serde_json::to_string(&telemetry).unwrap();
        assert!(json.contains("\"session_id\":\"abc\""));
        assert!(json.contains("\"query_count\":42"));
    }

    // ── Resume ID Tests ────────────────────────────────────────────

    #[test]
    fn resume_id_ui_override_takes_priority() {
        let config = AgentConfig {
            session_id: "original-id".into(),
            session_name: "Agent".into(),
            agent_class: "Coder".into(),
            folder: "".into(),
            resume_session: Some("ui-override-id".into()),
        };
        assert_eq!(determine_resume_id(&config, false), "ui-override-id");
        assert_eq!(determine_resume_id(&config, true), "ui-override-id");
    }

    #[test]
    fn resume_id_restored_uses_session_id() {
        let config = AgentConfig {
            session_id: "restored-id".into(),
            session_name: "Agent".into(),
            agent_class: "Coder".into(),
            folder: "".into(),
            resume_session: None,
        };
        assert_eq!(determine_resume_id(&config, true), "restored-id");
    }

    #[test]
    fn resume_id_fresh_returns_empty() {
        let config = AgentConfig {
            session_id: "fresh-uuid".into(),
            session_name: "Agent".into(),
            agent_class: "Coder".into(),
            folder: "".into(),
            resume_session: None,
        };
        assert_eq!(determine_resume_id(&config, false), "");
    }

    #[test]
    fn resume_id_empty_override_treated_as_none() {
        let config = AgentConfig {
            session_id: "some-id".into(),
            session_name: "Agent".into(),
            agent_class: "Coder".into(),
            folder: "".into(),
            resume_session: Some("".into()),
        };
        // Empty override falls through to is_restored check
        assert_eq!(determine_resume_id(&config, false), "");
        assert_eq!(determine_resume_id(&config, true), "some-id");
    }

    // ── CWD Resolution Tests ───────────────────────────────────────

    #[test]
    fn cwd_explicit_folder() {
        let cwd = resolve_working_directory("D:/MyProject");
        assert_eq!(cwd, std::path::PathBuf::from("D:/MyProject"));
    }

    #[test]
    fn cwd_empty_falls_back_to_home() {
        let cwd = resolve_working_directory("");
        // On Windows: USERPROFILE, on Unix: HOME, or fallback
        assert!(
            cwd.exists()
                || cwd == std::path::PathBuf::from("C:\\")
                || cwd == std::path::PathBuf::from("/")
        );
    }

    // ── JSON Event Processing Tests ────────────────────────────────

    #[test]
    fn json_event_user_increments_query() {
        let parsed: serde_json::Value = serde_json::json!({"type": "user", "content": "hello"});
        assert_eq!(
            process_json_event(&parsed),
            JsonEventEffect::IncrementQueryAndSetProcessing
        );
    }

    #[test]
    fn json_event_init_with_timestamp() {
        let parsed: serde_json::Value =
            serde_json::json!({"type": "init", "timestamp": "2026-01-01T00:00:00Z"});
        assert_eq!(
            process_json_event(&parsed),
            JsonEventEffect::SetTimestamp("2026-01-01T00:00:00Z".into())
        );
    }

    #[test]
    fn json_event_init_without_timestamp() {
        let parsed: serde_json::Value = serde_json::json!({"type": "init"});
        assert_eq!(process_json_event(&parsed), JsonEventEffect::NoOp);
    }

    #[test]
    fn json_event_gemini_sets_idle() {
        let parsed: serde_json::Value = serde_json::json!({"type": "gemini"});
        assert_eq!(process_json_event(&parsed), JsonEventEffect::SetIdle);
    }

    #[test]
    fn json_event_model_sets_idle() {
        let parsed: serde_json::Value = serde_json::json!({"type": "model"});
        assert_eq!(process_json_event(&parsed), JsonEventEffect::SetIdle);
    }

    #[test]
    fn json_event_info_sets_idle() {
        let parsed: serde_json::Value = serde_json::json!({"type": "info"});
        assert_eq!(process_json_event(&parsed), JsonEventEffect::SetIdle);
    }

    #[test]
    fn json_event_unknown_type_noop() {
        let parsed: serde_json::Value = serde_json::json!({"type": "unknown"});
        assert_eq!(process_json_event(&parsed), JsonEventEffect::NoOp);
    }

    #[test]
    fn json_event_no_type_field_noop() {
        let parsed: serde_json::Value = serde_json::json!({"content": "hello"});
        assert_eq!(process_json_event(&parsed), JsonEventEffect::NoOp);
    }

    // ── History Capping Tests ──────────────────────────────────────

    #[test]
    fn cap_history_under_limit_noop() {
        let mut h = "Short string".to_string();
        cap_output_history(&mut h, 20_000, 10_000);
        assert_eq!(h, "Short string");
    }

    #[test]
    fn cap_history_over_limit_truncates() {
        let mut h = "A".repeat(25_000);
        cap_output_history(&mut h, 20_000, 10_000);
        assert!(h.len() <= 10_001); // keeps approximately last 10k
        assert!(h.len() >= 9_999);
    }

    #[test]
    fn cap_history_respects_unicode_boundaries() {
        // Create a string with multi-byte chars
        let mut h = "こんにちは".repeat(5000); // each repeat is 15 bytes
        let orig_len = h.len();
        if orig_len > 20_000 {
            cap_output_history(&mut h, 20_000, 10_000);
            // Result should be valid UTF-8 (no panics)
            assert!(h.len() <= 10_003); // within 3 bytes of target
            assert!(h.is_char_boundary(0));
        }
    }

    // ── Log Metadata Parsing Tests ─────────────────────────────────

    #[test]
    fn parse_log_counts_user_messages() {
        let content = r#"{"sessionId":"abc","messages":[
            {"type":"user","content":"hello"},
            {"type":"gemini","content":"response"},
            {"type":"user","content":"another"}
        ]}"#;
        let (count, _, _) = parse_log_metadata(content);
        assert_eq!(count, 2);
    }

    #[test]
    fn parse_log_status_from_last_user() {
        let content = r#"{"sessionId":"abc","messages":[
            {"type":"gemini","content":"response"},
            {"type":"user","content":"question"}
        ]}"#;
        let (_, status, _) = parse_log_metadata(content);
        assert_eq!(status, "Processing...");
    }

    #[test]
    fn parse_log_status_from_last_gemini() {
        let content = r#"{"sessionId":"abc","messages":[
            {"type":"user","content":"question"},
            {"type":"gemini","content":"response"}
        ]}"#;
        let (_, status, _) = parse_log_metadata(content);
        assert_eq!(status, "Idle");
    }

    #[test]
    fn parse_log_status_action_needed() {
        let content = r#"{"sessionId":"abc","messages":[
            {"type":"tool_use","content":"running..."}
        ]}"#;
        let (_, status, _) = parse_log_metadata(content);
        assert_eq!(status, "Action Needed");
    }

    #[test]
    fn parse_log_extracts_start_time() {
        let content = r#"{"sessionId":"abc","startTime":"2026-01-15T10:30:00Z","messages":[]}"#;
        let (_, _, ts) = parse_log_metadata(content);
        assert_eq!(ts, Some("2026-01-15T10:30:00Z".to_string()));
    }

    #[test]
    fn parse_log_empty_messages() {
        let content = r#"{"sessionId":"abc","messages":[]}"#;
        let (count, status, ts) = parse_log_metadata(content);
        assert_eq!(count, 0);
        assert_eq!(status, "Idle");
        assert_eq!(ts, None);
    }

    #[test]
    fn parse_log_malformed_json() {
        let (count, status, _) = parse_log_metadata("not valid json{{{");
        assert_eq!(count, 0);
        assert_eq!(status, "Idle");
    }

    // ── Log Path Discovery Tests ───────────────────────────────────

    #[test]
    fn find_session_log_discovers_correct_file() {
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path().join("workspace1").join("chats");
        std::fs::create_dir_all(&workspace).unwrap();
        let log_content = r#"{"sessionId":"target-session","messages":[]}"#;
        std::fs::write(workspace.join("chat.json"), log_content).unwrap();

        let result = find_session_log(tmp.path(), "target-session");
        assert!(result.is_some());
        assert!(result.unwrap().to_string_lossy().contains("chat.json"));
    }

    #[test]
    fn find_session_log_returns_none_for_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path().join("workspace1").join("chats");
        std::fs::create_dir_all(&workspace).unwrap();
        let log_content = r#"{"sessionId":"other-session","messages":[]}"#;
        std::fs::write(workspace.join("chat.json"), log_content).unwrap();

        let result = find_session_log(tmp.path(), "target-session");
        assert!(result.is_none());
    }

    #[test]
    fn find_session_log_ignores_non_json_files() {
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path().join("workspace1").join("chats");
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::write(
            workspace.join("readme.txt"),
            r#"{"sessionId":"target","messages":[]}"#,
        )
        .unwrap();

        let result = find_session_log(tmp.path(), "target");
        assert!(result.is_none());
    }

    #[test]
    fn find_session_log_handles_empty_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let result = find_session_log(tmp.path(), "anything");
        assert!(result.is_none());
    }

    // ── Class CRUD Tests ───────────────────────────────────────────

    #[test]
    fn load_custom_classes_empty_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let classes = load_custom_classes(tmp.path());
        assert!(classes.is_empty());
    }

    #[test]
    fn load_custom_classes_valid_json() {
        let tmp = tempfile::tempdir().unwrap();
        let content = r#"[{"name":"DevOps","description":"CI/CD","is_default":false}]"#;
        std::fs::write(tmp.path().join("custom_classes.json"), content).unwrap();

        let classes = load_custom_classes(tmp.path());
        assert_eq!(classes.len(), 1);
        assert_eq!(classes[0].name, "DevOps");
        assert_eq!(classes[0].is_default, false);
    }

    #[test]
    fn load_custom_classes_malformed_json() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("custom_classes.json"), "not valid").unwrap();

        let classes = load_custom_classes(tmp.path());
        assert!(classes.is_empty());
    }

    #[test]
    fn save_and_load_custom_classes_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let classes = vec![
            AgentClassDefinition {
                name: "Alpha".into(),
                description: "Test A".into(),
                is_default: false,
            },
            AgentClassDefinition {
                name: "Beta".into(),
                description: "Test B".into(),
                is_default: false,
            },
        ];
        let path = tmp.path().join("custom_classes.json");
        save_custom_classes_to_path(&path, &classes).unwrap();

        let loaded = load_custom_classes(tmp.path());
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].name, "Alpha");
        assert_eq!(loaded[1].name, "Beta");
    }

    #[test]
    fn merge_classes_combines_with_flags() {
        let defaults = vec![AgentClassDefinition {
            name: "Coder".into(),
            description: "Writes code".into(),
            is_default: true,
        }];
        let custom = vec![AgentClassDefinition {
            name: "DevOps".into(),
            description: "CI/CD".into(),
            is_default: false,
        }];

        let merged = merge_classes(&defaults, &custom);
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].name, "Coder");
        assert!(merged[0].is_default);
        assert_eq!(merged[1].name, "DevOps");
        assert!(!merged[1].is_default);
    }

    #[test]
    fn merge_classes_empty_custom() {
        let defaults = vec![AgentClassDefinition {
            name: "QA".into(),
            description: "Test".into(),
            is_default: true,
        }];
        let merged = merge_classes(&defaults, &[]);
        assert_eq!(merged.len(), 1);
        assert!(merged[0].is_default);
    }

    // ── AppState Tests ─────────────────────────────────────────────

    #[test]
    fn app_state_constructs_without_panic() {
        let state = AppState::new();
        // If we get here, construction succeeded
        assert!(true, "AppState::new() succeeded");
        drop(state);
    }
}
