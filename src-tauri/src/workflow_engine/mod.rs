use crate::models::{WorkflowDefinition, WorkflowTelemetryEvent};
use crate::utils::fs::{get_wardian_home, validate_workspace_path};
use crate::manager::log_debug;
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Emitter, Manager, Listener};
use chrono::Utc;
use regex::Regex;
use serde_json::Value;
use cron::Schedule;
use std::str::FromStr;
use notify::{Watcher, RecursiveMode, Event};
use tokio::process::Command;
use std::process::Stdio;

// ... (interpolate_string, get_registry_value, evaluate_logic remain above)

/// Resolves the current working directory for a node.
fn resolve_cwd(node_config: &Value, agent_id: &str) -> PathBuf {
    let cwd = std::env::var("USERPROFILE").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from("C:\\"));
    
    // Priority 1: Explicitly provided folder in node config
    if let Some(folder) = node_config.get("folder").and_then(|v| v.as_str()) {
        if !folder.is_empty() {
            let p = PathBuf::from(folder);
            if let Ok(validated) = validate_workspace_path(&p) {
                return validated;
            }
        }
    }

    // Priority 2: Persistent agent configuration (if agent_id is provided)
    if !agent_id.is_empty() {
        if let Some(home) = get_wardian_home() {
            if let Ok(data) = std::fs::read_to_string(home.join("wardian_state.json")) {
                if let Ok(configs) = serde_json::from_str::<Vec<crate::models::AgentConfig>>(&data) {
                    if let Some(cfg) = configs.iter().find(|c| c.session_id == agent_id) {
                        if !cfg.folder.is_empty() {
                            let p = PathBuf::from(&cfg.folder);
                            if let Ok(validated) = validate_workspace_path(&p) {
                                return validated;
                            }
                        }
                    }
                }
            }
        }
    }

    cwd
}

/// Executes a command headlessly and returns the result in the mandatory schema.
async fn run_command_headless(
    executable: &str, 
    args: Vec<String>, 
    cwd: &Path, 
    env: Option<&Value>,
    timeout_ms: u64
) -> Result<Value, String> {
    let mut cmd = Command::new(executable);
    cmd.args(args)
       .current_dir(cwd)
       .stdout(Stdio::piped())
       .stderr(Stdio::piped());

    // Inject environment variables
    if let Some(env_val) = env {
        if let Some(map) = env_val.as_object() {
            for (k, v) in map {
                if let Some(val_str) = v.as_str() {
                    cmd.env(k, val_str);
                } else {
                    cmd.env(k, v.to_string());
                }
            }
        }
    }

    let mut child = cmd.spawn().map_err(|e| format!("Failed to spawn process: {}", e))?;

    let timeout = std::time::Duration::from_millis(if timeout_ms > 0 { timeout_ms } else { 30000 });
    
    // We don't use wait_with_output because it consumes 'child', making kill() impossible on timeout.
    let status = match tokio::time::timeout(timeout, child.wait()).await {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => return Err(format!("Command execution failed: {}", e)),
        Err(_) => {
            let _ = child.kill().await;
            return Err(format!("Command timed out after {}ms", timeout.as_millis()));
        }
    };

    let output = child.wait_with_output().await.map_err(|e| format!("Failed to collect output: {}", e))?;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let exit_code = status.code().unwrap_or(-1);

    Ok(serde_json::json!({
        "exit_code": exit_code,
        "stdout": stdout,
        "stderr": stderr
    }))
}

pub fn get_workflows_dir() -> Option<PathBuf> {
    get_wardian_home().map(|h| h.join("workflows"))
}

pub fn get_logs_dir(workflow_id: &str) -> Option<PathBuf> {
    get_wardian_home().map(|h| h.join("workflow_logs").join(workflow_id))
}

/// Interpolates a string using the Shared Registry data.
/// Syntax: {{nodes.[id].output.[path]}}
fn interpolate_string(input: &str, registry: &HashMap<String, Value>) -> String {
    let re = Regex::new(r"\{\{nodes\.([^.]+)\.output\.?([^}]*)?\}\}").unwrap();
    
    re.replace_all(input, |caps: &regex::Captures| {
        let node_id = caps.get(1).map_or("", |m| m.as_str());
        let path = caps.get(2).map_or("", |m| m.as_str());

        if let Some(node_output) = registry.get(node_id) {
            if path.is_empty() {
                // If no path, return serialized JSON if object, or raw string if string
                if let Some(s) = node_output.as_str() {
                    s.to_string()
                } else {
                    node_output.to_string()
                }
            } else {
                // Traverse the JSON path (e.g., "data.user.name")
                let mut current = node_output;
                for part in path.split('.') {
                    if part.is_empty() { continue; }
                    if let Some(next) = current.get(part) {
                        current = next;
                    } else {
                        return format!("{{{{nodes.{}.output.{}}}}}", node_id, path);
                    }
                }
                if let Some(s) = current.as_str() {
                    s.to_string()
                } else {
                    current.to_string()
                }
            }
        } else {
            // Keep original if not found
            format!("{{{{nodes.{}.output.{}}}}}", node_id, path)
        }
    }).to_string()
}

/// Resolves a value from the registry given a dot-notated path.
/// Format: nodes.[id].output.[optional.subpath]
fn get_registry_value(path: &str, registry: &HashMap<String, Value>) -> Value {
    let parts: Vec<&str> = path.split('.').collect();
    if parts.len() < 3 || parts[0] != "nodes" || parts[2] != "output" {
        return Value::Null;
    }
    let node_id = parts[1];
    if let Some(node_output) = registry.get(node_id) {
        let mut current = node_output;
        for part in &parts[3..] {
            if let Some(next) = current.get(*part) {
                current = next;
            } else {
                return Value::Null;
            }
        }
        current.clone()
    } else {
        Value::Null
    }
}

/// Evaluates a basic logic condition (equality/truthiness).
fn evaluate_logic(condition: &str, registry: &HashMap<String, Value>) -> bool {
    let trimmed = condition.trim();
    if trimmed.is_empty() { return true; }

    // Regex for basic equality: nodes.gatekeeper.output.decision === 'ACTION_REQUIRED'
    // Supports ===, ==, !==, !=
    let re = Regex::new(r#"^(nodes\.[^\s!>=<]+)\s*(===|==|!==|!=)\s*['"]([^'"]*)['"]$"#).unwrap();
    
    if let Some(caps) = re.captures(trimmed) {
        let path = caps.get(1).unwrap().as_str();
        let op = caps.get(2).unwrap().as_str();
        let target = caps.get(3).unwrap().as_str();
        
        let actual = get_registry_value(path, registry);
        let actual_str = match &actual {
            Value::String(s) => s.clone(),
            Value::Null => "null".to_string(),
            _ => actual.to_string().replace("\"", ""), // fallback for numbers/bools
        };
        
        match op {
            "==" | "===" => actual_str == target,
            "!=" | "!==" => actual_str != target,
            _ => false
        }
    } else {
        // Fallback: check truthiness of the resolved path
        let val = get_registry_value(trimmed, registry);
        match val {
            Value::Bool(b) => b,
            Value::Null => false,
            Value::String(s) => !s.is_empty() && s != "false",
            _ => {
                if val.is_number() {
                    val.as_f64().unwrap_or(0.0) != 0.0
                } else {
                    !val.is_null()
                }
            }
        }
    }
}

pub fn list_workflows() -> Result<Vec<WorkflowDefinition>, String> {
    let dir = get_workflows_dir().ok_or("Could not find Wardian home")?;
    if !dir.exists() {
        fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    }

    let mut workflows = Vec::new();
    for entry in fs::read_dir(dir).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) == Some("json") {
            let content = fs::read_to_string(&path).map_err(|e| e.to_string())?;
            if let Ok(wf) = serde_json::from_str::<WorkflowDefinition>(&content) {
                workflows.push(wf);
            } else {
                log_debug(&format!("[Wardian] Failed to deserialize workflow at {:?}", path));
            }
        }
    }
    Ok(workflows)
}

pub async fn init_triggers(app: AppHandle) {
    let workflows = list_workflows().unwrap_or_default();
    for wf in workflows {
        start_workflow_triggers(app.clone(), wf).await;
    }
}

pub async fn stop_workflow_triggers(app: AppHandle, workflow_id: &str) {
    let state = app.state::<crate::state::AppState>();
    let mut triggers = state.workflow_triggers.lock().await;
    if let Some(handles) = triggers.remove(workflow_id) {
        for handle in handles {
            handle.abort();
        }
    }
}

pub async fn start_workflow_triggers(app: AppHandle, wf: WorkflowDefinition) {
    // 1. Clean up existing triggers for this workflow
    stop_workflow_triggers(app.clone(), &wf.id).await;

    let mut handles = Vec::new();

    // 2. Identify trigger nodes
    for node in &wf.nodes {
        if node.r#type == "trigger" {
            let app_clone = app.clone();
            let wf_id = wf.id.clone();
            let config = node.config.clone();

            // --- Cron Trigger ---
            if let Some(cron_expr) = config.get("cron").and_then(|v| v.as_str()) {
                let cron_str = cron_expr.to_string();
                let wf_id_cron = wf_id.clone();
                let app_cron = app_clone.clone();
                if let Ok(schedule) = Schedule::from_str(&cron_str) {
                    let handle = tokio::spawn(async move {
                        loop {
                            let now = Utc::now().with_timezone(&chrono::Local);
                            if let Some(next) = schedule.upcoming(chrono::Local).next() {
                                let duration = (next - now).to_std().unwrap_or(std::time::Duration::from_secs(1));
                                tokio::time::sleep(duration).await;
                                
                                log_debug(&format!("[Wardian] Triggering workflow {} via cron {}", wf_id_cron, cron_str));
                                let _ = run_workflow(app_cron.clone(), wf_id_cron.clone()).await;
                            } else {
                                break;
                            }
                        }
                    });
                    handles.push(handle);
                }
            }

            // --- File Watcher Trigger ---
            if let Some(path_str) = config.get("path").and_then(|v| v.as_str()) {
                let path = PathBuf::from(path_str);
                let path_clone = path.clone();
                let wf_id_file = wf_id.clone();
                let app_file = app_clone.clone();
                if path.exists() {
                    let (tx, mut rx) = tokio::sync::mpsc::channel(1);
                    
                    // The watcher needs to stay alive
                    let handle = tokio::spawn(async move {
                        let mut watcher = notify::recommended_watcher(move |res: notify::Result<Event>| {
                            if let Ok(event) = res {
                                if event.kind.is_modify() || event.kind.is_create() {
                                    let _ = tx.try_send(());
                                }
                            }
                        }).unwrap();

                        if let Ok(_) = watcher.watch(&path_clone, RecursiveMode::NonRecursive) {
                            while let Some(_) = rx.recv().await {
                                log_debug(&format!("[Wardian] Triggering workflow {} via file watcher on {:?}", wf_id_file, path_clone));
                                let _ = run_workflow(app_file.clone(), wf_id_file.clone()).await;
                                // Debounce: wait a bit before being ready for next trigger
                                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                            }
                        }
                    });
                    handles.push(handle);
                }
            }
        }
    }

    if !handles.is_empty() {
        let state = app.state::<crate::state::AppState>();
        let mut triggers = state.workflow_triggers.lock().await;
        triggers.insert(wf.id, handles);
    }
}

pub async fn save_workflow(app: AppHandle, wf: WorkflowDefinition) -> Result<(), String> {
    let dir = get_workflows_dir().ok_or("Could not find Wardian home")?;
    if !dir.exists() {
        fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    }

    let path = dir.join(format!("{}.json", wf.id));
    let content = serde_json::to_string_pretty(&wf).map_err(|e| e.to_string())?;
    fs::write(path, content).map_err(|e| e.to_string())?;

    // Restart triggers
    start_workflow_triggers(app, wf).await;
    Ok(())
}

pub async fn delete_workflow(app: AppHandle, id: String) -> Result<(), String> {
    let dir = get_workflows_dir().ok_or("Could not find Wardian home")?;
    let path = dir.join(format!("{}.json", id));
    if path.exists() {
        fs::remove_file(path).map_err(|e| e.to_string())?;
    }

    // Stop triggers
    stop_workflow_triggers(app, &id).await;
    Ok(())
}

pub async fn run_workflow(app: AppHandle, wf_id: String) -> Result<(), String> {
    let workflows = list_workflows()?;
    let wf = workflows.into_iter().find(|w| w.id == wf_id)
        .ok_or_else(|| format!("Workflow {} not found", wf_id))?;

    let log_dir = get_logs_dir(&wf_id).ok_or("Could not find log path")?;
    fs::create_dir_all(&log_dir).map_err(|e| e.to_string())?;
    
    let timestamp = Utc::now().format("%Y%m%d_%H%M%S").to_string();
    let log_path = log_dir.join(format!("{}.json", timestamp));

    tauri::async_runtime::spawn(async move {
        let mut trace = Vec::new();
        let mut pulsed_ports = HashSet::new(); // Set of (node_id, port_id)
        let mut executed_nodes = HashSet::new();
        let mut queue = VecDeque::new();
        let mut registry: HashMap<String, Value> = HashMap::new();

        // 1. Identify Entry Points (nodes with no dependencies)
        for node in &wf.nodes {
            if node.dependencies.as_ref().map_or(true, |d| d.is_empty()) {
                queue.push_back(node.id.clone());
            }
        }

        // 2. Execution Loop
        while let Some(current_node_id) = queue.pop_front() {
            if executed_nodes.contains(&current_node_id) { continue; }
            
            let node = match wf.nodes.iter().find(|n| n.id == current_node_id) {
                Some(n) => n,
                None => continue,
            };

            // Check if dependencies are satisfied
            if let Some(deps) = &node.dependencies {
                if !deps.is_empty() {
                    let satisfied = if node.r#type == "wait" {
                        deps.iter().all(|d| pulsed_ports.contains(&(d.node_id.clone(), d.port.clone())))
                    } else {
                        deps.iter().any(|d| pulsed_ports.contains(&(d.node_id.clone(), d.port.clone())))
                    };
                    if !satisfied { continue; }
                }
            }

            // --- Execute Node ---
            executed_nodes.insert(current_node_id.clone());
            
            let _ = app.emit("workflow-telemetry", &WorkflowTelemetryEvent {
                workflow_id: wf.id.clone(),
                node_id: node.id.clone(),
                status: "processing".to_string(),
                output: None,
                error: None,
            });

            // --- NODE LOGIC ---
            let mut result_ports = vec!["default".to_string()];
            let mut output_payload = Value::Null;
            let mut node_error = None;

            match node.r#type.as_str() {
                "agent" => {
                    let agent_id = node.config.get("agent_id").and_then(|v| v.as_str()).unwrap_or("");
                    let mut prompt = node.config.get("prompt").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    prompt = interpolate_string(&prompt, &registry);

                    if agent_id.is_empty() {
                        node_error = Some("Missing agent_id".to_string());
                    } else {
                        let output_format = node.config.get("output_format")
                            .or_else(|| node.config.get("mode"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("text");

                        // Apply JSON Schema constraint if provided
                        if output_format == "json" {
                            if let Some(schema) = node.config.get("json_schema").and_then(|v| v.as_str()) {
                                if !schema.trim().is_empty() && schema.trim() != "{}" {
                                    let interpolated_schema = interpolate_string(schema, &registry);
                                    prompt.push_str("\n\nGive your answer as JSON in the format:\n");
                                    prompt.push_str(&interpolated_schema);
                                }
                            }
                        }

                        log_debug(&format!("[Wardian] Agent node output_format: {}. Final Prompt Length: {}", output_format, prompt.len()));
                        
                        // 1. Resolve CWD logic (Shared between modes)
                        let mut cwd = std::env::var("USERPROFILE").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from("C:\\"));
                        
                        // Priority 1: Explicitly provided folder in node config
                        if let Some(folder) = node.config.get("folder").and_then(|v| v.as_str()) {
                            if !folder.is_empty() {
                                cwd = PathBuf::from(folder);
                            }
                        } else {
                            // Priority 2: Persistent agent configuration
                            if let Some(home) = get_wardian_home() {
                                if let Ok(data) = std::fs::read_to_string(home.join("wardian_state.json")) {
                                    if let Ok(configs) = serde_json::from_str::<Vec<crate::models::AgentConfig>>(&data) {
                                        if let Some(cfg) = configs.iter().find(|c| c.session_id == agent_id) {
                                            if !cfg.folder.is_empty() {
                                                cwd = PathBuf::from(&cfg.folder);
                                            }
                                        }
                                    }
                                }
                            }
                        }

                        if output_format == "json" {
                            // --- HEADLESS JSON MODE (Always kills PTY for clean structured output) ---
                            let state = app.state::<crate::state::AppState>();
                            let mut was_online = false;
                            let mut agent_cfg = None;

                            {
                                let mut agents_map = state.agents.lock().await;
                                if let Some(agent) = agents_map.get_mut(agent_id) {
                                    agent_cfg = Some(agent.config.clone());
                                    let _ = app.emit("agents-updated", ()); // Notify UI early
                                    if let Some(mut child) = agent.child_process.take() {
                                        was_online = true;
                                        let _ = child.kill();
                                        let _ = child.wait();
                                    }
                                    agent.config.is_off = true; // Mark persistent config as off
                                    if let Ok(mut status) = agent.current_status.lock() {
                                        *status = "Off".to_string();
                                    }
                                    if let Ok(mut senders) = state.input_senders.write() {
                                        senders.remove(agent_id);
                                    }
                                    let order = state.agent_order.lock().await;
                                    crate::manager::save_state(&app, &agents_map, &order);
                                    log_debug(&format!("[Wardian] Killed active PTY and persisted Off state for {} transition (was_online={})", agent_id, was_online));
                                }
                            }

                            let run_result = crate::manager::run_gemini_headless(&cwd, &prompt, agent_id, output_format).await;
                            
                            match run_result {
                                Ok(mut data) => {
                                    // Flatten Gemini CLI structured response if it contains a JSON response string
                                    if let Some(resp_str) = data.get("response").and_then(|v| v.as_str()) {
                                        if let Ok(inner) = serde_json::from_str::<Value>(resp_str) {
                                            if let Some(obj) = inner.as_object() {
                                                if let Some(out_obj) = data.as_object_mut() {
                                                    for (k, v) in obj {
                                                        out_obj.insert(k.clone(), v.clone());
                                                    }
                                                }
                                            }
                                        }
                                    }
                                    output_payload = data;
                                },
                                Err(e) => {
                                    log_debug(&format!("[Wardian] Headless JSON agent failed: {}", e));
                                    node_error = Some(e);
                                }
                            }

                            // Restore state if it was online
                            if was_online {
                                if let Some(mut cfg) = agent_cfg {
                                    cfg.is_off = false;
                                    log_debug(&format!("[Wardian] Restoring agent {} to Online state after headless run", agent_id));
                                    if let Ok(agent) = crate::manager::spawn_gemini_cli(app.clone(), cfg.clone(), false).await {
                                        let mut agents_map = state.agents.lock().await;
                                        if let Some(ref tx) = agent.stdin_tx {
                                            if let Ok(mut senders) = state.input_senders.write() {
                                                senders.insert(agent_id.to_string(), tx.clone());
                                            }
                                        }
                                        agents_map.insert(agent_id.to_string(), agent);
                                        let order = state.agent_order.lock().await;
                                        crate::manager::save_state(&app, &agents_map, &order);
                                        let _ = app.emit("agents-updated", ());
                                    }
                                }
                            }
                        } else {
                            // --- PTY TEXT MODE (With Headless Fallback) ---
                            let state = app.state::<crate::state::AppState>();
                            let sender = {
                                let senders = state.input_senders.read().map_err(|e| e.to_string()).unwrap();
                                senders.get(agent_id).cloned()
                            };

                            if let Some(tx) = sender {
                                // Agent is ONLINE: Use PTY injection
                                {
                                    let agents = state.agents.lock().await;
                                    if let Some(agent) = agents.get(agent_id) {
                                        if let Ok(mut buf) = agent.output_buffer.lock() {
                                            buf.clear();
                                        }
                                    }
                                }

                                let _ = tx.send(prompt.trim().to_string()).await;
                                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                                let _ = tx.send("\r".to_string()).await;

                                let (completion_tx, mut completion_rx) = tokio::sync::mpsc::channel::<Value>(1);
                                let agent_id_clone = agent_id.to_string();
                                
                                let handler_id = app.listen_any("agent-json-event", move |event| {
                                    if let Ok(parsed) = serde_json::from_str::<Value>(event.payload()) {
                                        if parsed.get("session_id").and_then(|v| v.as_str()) == Some(&agent_id_clone) {
                                            let data = parsed.get("data").cloned().unwrap_or(Value::Null);
                                            let msg_type = data.get("type").and_then(|v| v.as_str()).unwrap_or("");
                                            if ["gemini", "model", "info"].contains(&msg_type) {
                                                let _ = completion_tx.try_send(data);
                                            }
                                        }
                                    }
                                });

                                match tokio::time::timeout(std::time::Duration::from_secs(60), completion_rx.recv()).await {
                                    Ok(_) => {
                                        let agents = state.agents.lock().await;
                                        if let Some(agent) = agents.get(agent_id) {
                                            if let Ok(buf) = agent.output_buffer.lock() {
                                                output_payload = serde_json::json!({ "text": buf.clone() });
                                            }
                                        }
                                    },
                                    Err(_) => {
                                        node_error = Some("Agent timeout (60s)".to_string());
                                    }
                                }
                                app.unlisten(handler_id);
                            } else {
                                // Agent is OFFLINE: Fallback to Headless Execution
                                log_debug(&format!("[Wardian] Agent {} offline, falling back to headless execution with format: {}", agent_id, output_format));
                                match crate::manager::run_gemini_headless(&cwd, &prompt, agent_id, output_format).await {
                                    Ok(mut data) => {
                                        // Flatten Gemini CLI structured response
                                        if let Some(resp_str) = data.get("response").and_then(|v| v.as_str()) {
                                            if let Ok(inner) = serde_json::from_str::<Value>(resp_str) {
                                                if let Some(obj) = inner.as_object() {
                                                    if let Some(out_obj) = data.as_object_mut() {
                                                        for (k, v) in obj {
                                                            out_obj.insert(k.clone(), v.clone());
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                        output_payload = data;
                                    },
                                    Err(e) => {
                                        log_debug(&format!("[Wardian] Headless fallback failed: {}", e));
                                        node_error = Some(e);
                                    }
                                }
                            }
                        }
                    }
                },
                "communication" | "notify" => {
                    log_debug(&format!("[Wardian] Communication node {} starting", node.id));
                    if let Some(message) = node.config.get("message").and_then(|v| v.as_str()) {
                        let interpolated = interpolate_string(message, &registry);
                        log_debug(&format!("[Wardian] Sending notification: {}", interpolated));
                        
                        use tauri_plugin_notification::NotificationExt;
                        app.notification()
                            .builder()
                            .title("Wardian")
                            .body(interpolated)
                            .show()
                            .unwrap();

                        output_payload = serde_json::json!({ "delivered": true, "type": "notification" });
                    } else if let Some(prompt) = node.config.get("prompt").and_then(|v| v.as_str()) {
                         log_debug(&format!("[Wardian] Broadcast prompt: {}", prompt));
                         // TODO: Implement actual broadcast to all agents
                         output_payload = serde_json::json!({ "delivered": true, "type": "broadcast" });
                    } else {
                        log_debug("[Wardian] Communication node missing message or prompt");
                        output_payload = serde_json::json!({ "delivered": false, "error": "Missing message or prompt" });
                    }
                },
                "logic" => {
                    let condition = node.config.get("condition").and_then(|v| v.as_str()).unwrap_or("");
                    let is_true = evaluate_logic(condition, &registry);
                    
                    result_ports = if is_true { vec!["on_true".to_string()] } else { vec!["on_false".to_string()] };
                    output_payload = serde_json::json!({ "condition_met": is_true, "condition": condition });
                    log_debug(&format!("[Wardian] Logic node {} evaluated {} to {}", node.id, condition, is_true));
                },
                "command" => {
                    log_debug(&format!("[Wardian] Shell Command node {} starting", node.id));
                    let cmd_str = node.config.get("cmd").and_then(|v| v.as_str()).unwrap_or("");
                    let interpolated_cmd = interpolate_string(cmd_str, &registry);
                    let cwd = resolve_cwd(&node.config, "");
                    let env = node.config.get("env");
                    let timeout = node.config.get("timeout_ms").and_then(|v| v.as_u64()).unwrap_or(30000);

                    if interpolated_cmd.is_empty() {
                        node_error = Some("Missing command string".to_string());
                    } else {
                        // On Windows, we use cmd /C for better compatibility with built-ins
                        #[cfg(windows)]
                        let (executable, args) = ("cmd", vec!["/C".to_string(), interpolated_cmd]);
                        #[cfg(not(windows))]
                        let (executable, args) = ("sh", vec!["-c".to_string(), interpolated_cmd]);

                        match run_command_headless(executable, args, &cwd, env, timeout).await {
                            Ok(res) => output_payload = res,
                            Err(e) => node_error = Some(e),
                        }
                    }
                },
                "script" => {
                    log_debug(&format!("[Wardian] Script node {} starting", node.id));
                    let runtime = node.config.get("runtime").and_then(|v| v.as_str()).unwrap_or("python");
                    let file_path_str = node.config.get("file_path").and_then(|v| v.as_str()).unwrap_or("");
                    let args_str = node.config.get("args").and_then(|v| v.as_str()).unwrap_or("");
                    let interpolated_args = interpolate_string(args_str, &registry);
                    let env = node.config.get("env");
                    let timeout = node.config.get("timeout_ms").and_then(|v| v.as_u64()).unwrap_or(30000);
                    let cwd = resolve_cwd(&node.config, "");

                    if file_path_str.is_empty() {
                        node_error = Some("Missing file_path".to_string());
                    } else {
                        let script_path = cwd.join(file_path_str);
                        match validate_workspace_path(&script_path) {
                            Ok(validated_path) => {
                                let executable = match runtime {
                                    "python" => "python",
                                    "node" => "node",
                                    "sh" => "sh",
                                    _ => "python"
                                };

                                let mut args = vec![validated_path.to_string_lossy().to_string()];
                                if !interpolated_args.is_empty() {
                                    args.extend(interpolated_args.split_whitespace().map(|s| s.to_string()));
                                }

                                match run_command_headless(executable, args, &cwd, env, timeout).await {
                                    Ok(res) => output_payload = res,
                                    Err(e) => node_error = Some(e),
                                }
                            },
                            Err(e) => node_error = Some(format!("Security Violation: {}", e)),
                        }
                    }
                },
                _ => {
                    log_debug(&format!("[Wardian] Unknown node type: {}", node.r#type));
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                    output_payload = serde_json::json!({ "status": "unknown_type" });
                }
            }

            // Inject fired_ports into output for telemetry
            if !output_payload.is_object() {
                // If it's a primitive or null, wrap it in an object
                output_payload = serde_json::json!({
                    "data": output_payload,
                    "fired_ports": result_ports
                });
            } else {
                // If it's already an object, just insert the field
                if let Some(obj) = output_payload.as_object_mut() {
                    obj.insert("fired_ports".to_string(), serde_json::json!(result_ports));
                }
            }

            // Update Registry
            registry.insert(node.id.clone(), output_payload.clone());

            let event_done = WorkflowTelemetryEvent {
                workflow_id: wf.id.clone(),
                node_id: node.id.clone(),
                status: if node_error.is_some() { "failed".to_string() } else { "completed".to_string() },
                output: Some(output_payload.clone()),
                error: node_error.clone(),
            };
            let _ = app.emit("workflow-telemetry", &event_done);
            trace.push(event_done.clone());

            // 5. Execution Persistence: Append to persistent log
            if let Ok(json_inner) = serde_json::to_string(&event_done) {
                if let Ok(mut file) = OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(log_path.with_extension("log")) {
                        let _ = writeln!(file, "{}", json_inner);
                    }
            }

            // HALT if error
            if node_error.is_some() {
                break;
            }

            // Record pulsed ports
            for port in &result_ports {
                pulsed_ports.insert((current_node_id.clone(), port.clone()));
            }

            // Queue downstream nodes
            for candidate in &wf.nodes {
                if let Some(deps) = &candidate.dependencies {
                    if deps.iter().any(|d| d.node_id == current_node_id) {
                        queue.push_back(candidate.id.clone());
                    }
                }
            }
        }

        // Save the execution log
        if let Ok(json) = serde_json::to_string_pretty(&trace) {
            let _ = fs::write(log_path, json);
        }
    });

    Ok(())
}
