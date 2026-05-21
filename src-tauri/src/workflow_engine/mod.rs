pub mod agent_execution;
mod migrate;

use crate::manager::log_debug;
use crate::utils::{
    build_shell_command, get_wardian_home, new_headless_command, validate_workspace_path,
};
use chrono::{Datelike, Local, TimeZone, Utc};
use notify::{Event, RecursiveMode, Watcher};
use regex::Regex;
use serde_json::Value;
use std::collections::{HashMap, VecDeque};
use std::fs::{self, OpenOptions};
use std::future::Future;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tauri::{AppHandle, Emitter, Listener, Manager};
use wardian_core::models::{WorkflowDefinition, WorkflowTelemetryEvent};

// ... (interpolate_string, get_registry_value, evaluate_logic remain above)

/// Flattens provider-specific response wrappers into a uniform output.
/// Gemini wraps in `{"response": "..."}`, Claude wraps in `{"result": "..."}`.
fn flatten_headless_response(mut data: Value) -> Value {
    let response_str = data
        .get("response")
        .and_then(|v| v.as_str())
        .or_else(|| data.get("result").and_then(|v| v.as_str()));

    if let Some(resp_str) = response_str {
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
    data
}

/// Resolves the current working directory for a node.
fn resolve_cwd(node_config: &Value, agent_id: &str) -> PathBuf {
    let folder = node_config
        .get("folder")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    crate::utils::fs::resolve_cwd(folder, agent_id)
}

/// Executes a command headlessly and returns the result in the mandatory schema.
fn resolve_command_node_launch<F>(
    command: &str,
    resolver: F,
) -> Result<crate::utils::ShellLaunchSpec, String>
where
    F: FnOnce(&str) -> Result<crate::utils::ShellLaunchSpec, String>,
{
    if command.trim().is_empty() {
        Err("Missing command string".to_string())
    } else {
        resolver(command)
    }
}

fn parse_optional_timeout_ms(node_config: &Value) -> Option<u64> {
    let raw = node_config.get("timeout_ms")?;
    let parsed = raw.as_u64().or_else(|| {
        raw.as_str()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .and_then(|value| value.parse::<u64>().ok())
    })?;
    (parsed > 0).then_some(parsed)
}

fn prepare_agent_for_headless_json_run(
    agent: &mut crate::state::ActiveAgent,
) -> Option<Box<dyn portable_pty::Child + Send>> {
    let child = agent.child_process.take();
    if let Ok(mut status) = agent.current_status.lock() {
        *status = "Headless".to_string();
    }
    child
}

async fn run_with_optional_timeout<T, F>(
    timeout_ms: Option<u64>,
    label: &str,
    future: F,
) -> Result<T, String>
where
    F: Future<Output = Result<T, String>>,
{
    match timeout_ms {
        Some(ms) => {
            match tokio::time::timeout(std::time::Duration::from_millis(ms), future).await {
                Ok(result) => result,
                Err(_) => Err(format!("{} timeout ({}ms)", label, ms)),
            }
        }
        None => future.await,
    }
}

fn record_restore_spawn_result<T>(
    agent_id: &str,
    result: Result<T, String>,
    node_error: &mut Option<String>,
) -> Option<T> {
    match result {
        Ok(value) => Some(value),
        Err(error) => {
            let message = format!(
                "Failed to restore agent {} after headless run: {}",
                agent_id, error
            );
            log_debug(&format!("[Wardian] {}", message));
            if node_error.is_none() {
                *node_error = Some(message);
            }
            None
        }
    }
}

async fn run_command_headless(
    executable: &str,
    args: Vec<String>,
    cwd: &Path,
    env: Option<&Value>,
    timeout_ms: Option<u64>,
) -> Result<Value, String> {
    let mut cmd = new_headless_command(executable);
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

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("Failed to spawn process: {}", e))?;

    // Take stdout/stderr before waiting so we retain mut access to child for kill.
    let stdout_pipe = child.stdout.take();
    let stderr_pipe = child.stderr.take();

    let stdout_task = tokio::spawn(async move {
        let mut buf = String::new();
        if let Some(pipe) = stdout_pipe {
            use tokio::io::AsyncReadExt;
            let mut reader = pipe;
            let _ = reader.read_to_string(&mut buf).await;
        }
        buf
    });
    let stderr_task = tokio::spawn(async move {
        let mut buf = String::new();
        if let Some(pipe) = stderr_pipe {
            use tokio::io::AsyncReadExt;
            let mut reader = pipe;
            let _ = reader.read_to_string(&mut buf).await;
        }
        buf
    });

    let status = match timeout_ms {
        Some(ms) => {
            match tokio::time::timeout(std::time::Duration::from_millis(ms), child.wait()).await {
                Ok(Ok(status)) => status,
                Ok(Err(e)) => return Err(format!("Command execution failed: {}", e)),
                Err(_) => {
                    // Kill the child on timeout to prevent zombie processes.
                    let _ = child.kill().await;
                    stdout_task.abort();
                    stderr_task.abort();
                    return Err(format!("Command timed out after {}ms", ms));
                }
            }
        }
        None => child
            .wait()
            .await
            .map_err(|e| format!("Command execution failed: {}", e))?,
    };

    let stdout = stdout_task.await.unwrap_or_default();
    let stderr = stderr_task.await.unwrap_or_default();
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

pub fn get_scheduled_runs_path() -> Option<PathBuf> {
    get_wardian_home().map(|h| h.join("scheduled_workflows.json"))
}

pub fn load_scheduled_runs() -> Vec<wardian_core::models::ScheduledRun> {
    if let Some(path) = get_scheduled_runs_path() {
        if let Ok(content) = fs::read_to_string(path) {
            return serde_json::from_str(&content).unwrap_or_default();
        }
    }
    Vec::new()
}

pub fn save_scheduled_runs(runs: &[wardian_core::models::ScheduledRun]) -> Result<(), String> {
    let path = get_scheduled_runs_path().ok_or("No home dir")?;
    let content = serde_json::to_string_pretty(runs).map_err(|e| e.to_string())?;
    fs::write(path, content).map_err(|e| e.to_string())?;
    Ok(())
}

pub fn hydrate_scheduled_runs() -> Result<Vec<wardian_core::models::ScheduledRun>, String> {
    let mut runs = load_scheduled_runs();
    let now_ms = Utc::now().timestamp_millis() as u64;
    let mut modified = false;

    for run in runs.iter_mut() {
        if run.is_paused || !run.schedule.active {
            continue;
        }

        if run.next_run_epoch_ms.is_none() {
            run.next_run_epoch_ms = match run.paused_remaining_ms.take() {
                Some(remaining_ms) => Some(now_ms.saturating_add(remaining_ms)),
                None => compute_next_run(&run.schedule, now_ms),
            };
            modified = true;
        }
    }

    if modified {
        save_scheduled_runs(&runs)?;
    }

    Ok(runs)
}

pub fn toggle_scheduled_run_state(
    run_id: &str,
) -> Result<Vec<wardian_core::models::ScheduledRun>, String> {
    let mut runs = load_scheduled_runs();
    let now_ms = Utc::now().timestamp_millis() as u64;

    if let Some(run) = runs.iter_mut().find(|r| r.id == run_id) {
        if run.is_paused {
            run.is_paused = false;

            if run.schedule.active {
                run.next_run_epoch_ms = match run.paused_remaining_ms.take() {
                    Some(remaining_ms) => Some(now_ms.saturating_add(remaining_ms)),
                    None => compute_next_run(&run.schedule, now_ms),
                };
            }
        } else {
            run.paused_remaining_ms = run
                .next_run_epoch_ms
                .map(|next_run| next_run.saturating_sub(now_ms));
            run.next_run_epoch_ms = None;
            run.is_paused = true;
        }
    }

    save_scheduled_runs(&runs)?;
    Ok(runs)
}

fn sync_scheduled_runs_for_workflow(
    wf: &WorkflowDefinition,
    existing_runs: &[wardian_core::models::ScheduledRun],
) -> Vec<wardian_core::models::ScheduledRun> {
    let mut synced_runs: Vec<wardian_core::models::ScheduledRun> = existing_runs
        .iter()
        .filter(|run| run.workflow_id != wf.id)
        .cloned()
        .collect();

    for node in &wf.nodes {
        if node.r#type != "trigger" || node.name.as_deref() != Some("Scheduled Trigger") {
            continue;
        }

        let config = &node.config;
        if config
            .get("status")
            .and_then(|value| value.as_str())
            .is_some_and(|status| status == "off")
        {
            continue;
        }

        // Read the nested schedule object from config
        let schedule: wardian_core::models::ScheduleDefinition =
            if let Some(sched_val) = config.get("schedule") {
                match serde_json::from_value(sched_val.clone()) {
                    Ok(s) => s,
                    Err(_) => continue,
                }
            } else {
                // Legacy fallback: read flat config keys
                let Some(sched_type) = config.get("schedule_type").and_then(|v| v.as_str()) else {
                    continue;
                };
                let interval = config
                    .get("interval")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let time = config
                    .get("time")
                    .and_then(|v| v.as_str())
                    .unwrap_or("00:00");
                let days = config.get("days").and_then(|v| v.as_str()).unwrap_or("");
                let datetime = config
                    .get("datetime")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");

                match sched_type {
                    "Minutes" => wardian_core::models::ScheduleDefinition {
                        schedule_type: "interval".to_string(),
                        interval_minutes: Some(interval.parse().unwrap_or(5)),
                        time_of_day: None,
                        days_of_week: None,
                        repeat_every: 1,
                        days_of_month: None,
                        specific_dates: None,
                        run_at: None,
                        end_condition: "never".to_string(),
                        end_date: None,
                        max_occurrences: None,
                        occurrence_count: 0,
                        active: true,
                    },
                    "Hours" => wardian_core::models::ScheduleDefinition {
                        schedule_type: "interval".to_string(),
                        interval_minutes: Some(interval.parse::<u32>().unwrap_or(1) * 60),
                        time_of_day: None,
                        days_of_week: None,
                        repeat_every: 1,
                        days_of_month: None,
                        specific_dates: None,
                        run_at: None,
                        end_condition: "never".to_string(),
                        end_date: None,
                        max_occurrences: None,
                        occurrence_count: 0,
                        active: true,
                    },
                    "Daily" => wardian_core::models::ScheduleDefinition {
                        schedule_type: "daily".to_string(),
                        interval_minutes: None,
                        time_of_day: Some(time.to_string()),
                        days_of_week: None,
                        repeat_every: 1,
                        days_of_month: None,
                        specific_dates: None,
                        run_at: None,
                        end_condition: "never".to_string(),
                        end_date: None,
                        max_occurrences: None,
                        occurrence_count: 0,
                        active: true,
                    },
                    "Weekly" => wardian_core::models::ScheduleDefinition {
                        schedule_type: "weekly".to_string(),
                        interval_minutes: None,
                        time_of_day: Some(time.to_string()),
                        days_of_week: Some(days.split(',').map(|s| s.trim().to_string()).collect()),
                        repeat_every: 1,
                        days_of_month: None,
                        specific_dates: None,
                        run_at: None,
                        end_condition: "never".to_string(),
                        end_date: None,
                        max_occurrences: None,
                        occurrence_count: 0,
                        active: true,
                    },
                    "One-Time" => wardian_core::models::ScheduleDefinition {
                        schedule_type: "one_time".to_string(),
                        interval_minutes: None,
                        time_of_day: None,
                        days_of_week: None,
                        repeat_every: 1,
                        days_of_month: None,
                        specific_dates: None,
                        run_at: Some(datetime.to_string()),
                        end_condition: "never".to_string(),
                        end_date: None,
                        max_occurrences: None,
                        occurrence_count: 0,
                        active: true,
                    },
                    _ => continue,
                }
            };

        let description = describe_schedule(&schedule);

        let run_id = format!("{}-{}", wf.id, node.id);
        let previous = existing_runs.iter().find(|run| run.id == run_id);

        // Detect if the schedule config actually changed
        let schedule_unchanged = previous.is_some_and(|run| {
            run.schedule.schedule_type == schedule.schedule_type
                && run.schedule.interval_minutes == schedule.interval_minutes
                && run.schedule.time_of_day == schedule.time_of_day
                && run.schedule.days_of_week == schedule.days_of_week
                && run.schedule.repeat_every == schedule.repeat_every
                && run.schedule.days_of_month == schedule.days_of_month
                && run.schedule.specific_dates == schedule.specific_dates
                && run.schedule.run_at == schedule.run_at
                && run.schedule.end_condition == schedule.end_condition
                && run.schedule.end_date == schedule.end_date
                && run.schedule.max_occurrences == schedule.max_occurrences
        });

        let mut final_schedule = schedule;
        if schedule_unchanged {
            if let Some(prev) = previous {
                final_schedule.active = prev.schedule.active;
                final_schedule.occurrence_count = prev.schedule.occurrence_count;
            }
        }

        synced_runs.push(wardian_core::models::ScheduledRun {
            id: run_id.clone(),
            workflow_id: wf.id.clone(),
            workflow_name: wf.name.clone(),
            schedule: final_schedule,
            role_mappings: wf.role_mappings.clone(),
            description,
            next_run_epoch_ms: if schedule_unchanged {
                previous.and_then(|run| run.next_run_epoch_ms)
            } else {
                None
            },
            paused_remaining_ms: if schedule_unchanged {
                previous.and_then(|run| run.paused_remaining_ms)
            } else {
                None
            },
            is_paused: previous.map(|run| run.is_paused).unwrap_or(false),
            last_run_status: previous.and_then(|run| run.last_run_status.clone()),
            last_run_error: previous.and_then(|run| run.last_run_error.clone()),
            last_run_completed_epoch_ms: previous.and_then(|run| run.last_run_completed_epoch_ms),
        });

        if previous.is_none() {
            log_debug(&format!(
                "[Wardian] Registered scheduled run {} for workflow '{}'",
                run_id, wf.name
            ));
        }
    }

    synced_runs
}

fn record_scheduled_run_outcome(
    runs: &mut [wardian_core::models::ScheduledRun],
    scheduled_run_id: &str,
    status: &str,
    error: Option<String>,
    completed_epoch_ms: u64,
) {
    if let Some(run) = runs
        .iter_mut()
        .find(|scheduled_run| scheduled_run.id == scheduled_run_id)
    {
        run.last_run_status = Some(status.to_string());
        run.last_run_error = error;
        run.last_run_completed_epoch_ms = Some(completed_epoch_ms);
    }
}

fn describe_schedule(schedule: &wardian_core::models::ScheduleDefinition) -> String {
    match schedule.schedule_type.as_str() {
        "interval" => {
            let mins = schedule.interval_minutes.unwrap_or(0);
            if mins >= 60 && mins.is_multiple_of(60) {
                format!("Every {}h", mins / 60)
            } else {
                format!("Every {}m", mins)
            }
        }
        "daily" => {
            let time = schedule.time_of_day.as_deref().unwrap_or("00:00");
            format!("Daily at {}", time)
        }
        "weekly" => {
            let days = schedule
                .days_of_week
                .as_ref()
                .map(|d| d.join(", "))
                .unwrap_or_default();
            let time = schedule.time_of_day.as_deref().unwrap_or("00:00");
            if schedule.repeat_every > 1 {
                format!(
                    "Every {} weeks on {} at {}",
                    schedule.repeat_every, days, time
                )
            } else {
                format!("{} at {}", days, time)
            }
        }
        "monthly" => {
            let days = schedule
                .days_of_month
                .as_ref()
                .map(|d| {
                    d.iter()
                        .map(|n| n.to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_default();
            let time = schedule.time_of_day.as_deref().unwrap_or("00:00");
            format!("Monthly on day(s) {} at {}", days, time)
        }
        "specific_dates" => {
            let count = schedule
                .specific_dates
                .as_ref()
                .map(|d| d.len())
                .unwrap_or(0);
            let time = schedule.time_of_day.as_deref().unwrap_or("00:00");
            format!("{} specific date(s) at {}", count, time)
        }
        "one_time" => {
            let run_at = schedule.run_at.as_deref().unwrap_or("?");
            format!("Once at {}", run_at)
        }
        _ => "Unknown schedule".to_string(),
    }
}

pub fn get_logs_dir(workflow_id: &str) -> Option<PathBuf> {
    get_wardian_home().map(|h| h.join("logs/workflows").join(workflow_id))
}

pub fn get_library_path() -> Option<PathBuf> {
    get_wardian_home().map(|h| h.join("workflows.json"))
}

pub fn load_workflow_library() -> Value {
    if let Some(path) = get_library_path() {
        if let Ok(content) = fs::read_to_string(path) {
            return serde_json::from_str(&content).unwrap_or(serde_json::json!({
                "folders": [],
                "rootWorkflowIds": []
            }));
        }
    }
    serde_json::json!({
        "folders": [],
        "rootWorkflowIds": []
    })
}

pub fn save_workflow_library(state: &Value) -> Result<(), String> {
    if let Some(path) = get_library_path() {
        let content = serde_json::to_string_pretty(state).map_err(|e| e.to_string())?;
        fs::write(path, content).map_err(|e| e.to_string())?;
        Ok(())
    } else {
        Err("Could not find Wardian home".to_string())
    }
}

pub fn load_shared_storage() -> Value {
    if let Some(home) = get_wardian_home() {
        let path = home.join("shared_storage.json");
        if let Ok(content) = fs::read_to_string(path) {
            return serde_json::from_str(&content).unwrap_or(serde_json::json!({}));
        }
    }
    serde_json::json!({})
}

pub fn save_shared_storage(storage: &Value) {
    if let Some(home) = get_wardian_home() {
        let path = home.join("shared_storage.json");
        let content = serde_json::to_string_pretty(storage).unwrap_or("{}".to_string());
        let _ = fs::write(path, content);
    }
}

fn interpolate_string(input: &str, registry: &HashMap<String, Value>) -> String {
    // Matches {{nodes.id.output.path}}, {{trigger.payload.path}}, or {{storage.path}}
    let re = Regex::new(r"\{\{([^}]+)\}\}").unwrap();

    re.replace_all(input, |caps: &regex::Captures| {
        let full_path = caps.get(1).map_or("", |m| m.as_str());
        let val = get_registry_value(full_path, registry);

        if val.is_null() {
            format!("{{{{{}}}}}", full_path)
        } else if let Some(s) = val.as_str() {
            s.to_string()
        } else {
            val.to_string()
        }
    })
    .to_string()
}

/// Resolves a value from the registry given a dot-notated path.
/// Supported Formats:
/// 1. nodes.[id].output.[path]
/// 2. trigger.payload.[path]
/// 3. storage.[path]
fn get_registry_value(path: &str, registry: &HashMap<String, Value>) -> Value {
    let parts: Vec<&str> = path.split('.').collect();
    if parts.is_empty() {
        return Value::Null;
    }

    let (root_key, sub_path_start) =
        if parts[0] == "nodes" && parts.len() >= 3 && parts[2] == "output" {
            (parts[1], 3)
        } else if parts[0] == "trigger" && parts.len() >= 2 && parts[1] == "payload" {
            ("trigger", 2)
        } else if parts[0] == "storage" {
            ("storage", 1)
        } else {
            // Fallback: try direct lookup if it's just a single key
            return registry.get(path).cloned().unwrap_or(Value::Null);
        };

    if let Some(root_data) = registry.get(root_key) {
        let mut current = root_data;
        for part in &parts[sub_path_start..] {
            if part.is_empty() {
                continue;
            }
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
    if trimmed.is_empty() {
        return true;
    }

    // Regex for basic comparisons: nodes.gatekeeper.output.decision === 'ACTION_REQUIRED' or nodes.loop.output.i < 5
    // Supports ===, ==, !==, !=, <, >, <=, >=
    let re =
        Regex::new(r#"^(nodes\.[^\s!>=<]+)\s*(===|==|!==|!=|<=|>=|<|>)\s*(['"]?)([^'"]*)(['"]?)$"#)
            .unwrap();

    if let Some(caps) = re.captures(trimmed) {
        let path = caps.get(1).unwrap().as_str();
        let op = caps.get(2).unwrap().as_str();
        let target_val_str = caps.get(4).unwrap().as_str();

        let actual = get_registry_value(path, registry);

        match op {
            "==" | "===" | "!=" | "!==" => {
                let actual_str = match &actual {
                    Value::String(s) => s.clone(),
                    Value::Null => "null".to_string(),
                    _ => actual.to_string().replace("\"", ""), // fallback for numbers/bools
                };
                if op == "==" || op == "===" {
                    actual_str == target_val_str
                } else {
                    actual_str != target_val_str
                }
            }
            "<" | ">" | "<=" | ">=" => {
                // Numeric comparison
                if let (Some(actual_num), Ok(target_num)) =
                    (actual.as_f64(), target_val_str.parse::<f64>())
                {
                    match op {
                        "<" => actual_num < target_num,
                        ">" => actual_num > target_num,
                        "<=" => actual_num <= target_num,
                        ">=" => actual_num >= target_num,
                        _ => false,
                    }
                } else {
                    false
                }
            }
            _ => false,
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
                log_debug(&format!(
                    "[Wardian] Failed to deserialize workflow at {:?}",
                    path
                ));
            }
        }
    }
    Ok(workflows)
}

pub async fn init_triggers(app: AppHandle) {
    migrate::migrate_workflows_if_needed();
    let workflows = list_workflows().unwrap_or_default();
    for wf in workflows {
        start_workflow_triggers(app.clone(), wf, false).await;
    }
    // Start the unified scheduler
    start_scheduler(app).await;
}

pub async fn start_scheduler(app: AppHandle) {
    let state = app.state::<crate::state::AppState>();

    // Cancel any existing scheduler
    {
        let mut handle = state.scheduler_handle.lock().await;
        if let Some(h) = handle.take() {
            h.abort();
        }
    }

    let app_clone = app.clone();
    let handle = tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;

            let state = app_clone.state::<crate::state::AppState>();
            if state
                .triggers_paused
                .load(std::sync::atomic::Ordering::SeqCst)
            {
                continue;
            }

            let mut runs = load_scheduled_runs();
            let now_ms = Utc::now().timestamp_millis() as u64;
            let mut modified = false;
            let mut runs_to_delete = Vec::new();

            for run in runs.iter_mut() {
                if run.is_paused || !run.schedule.active {
                    continue;
                }

                // --- Aggressive End Condition Checks ---
                let end_cond = run.schedule.end_condition.as_str();
                let mut should_delete = false;

                if end_cond == "on_date" {
                    if let Some(ref end_date_str) = run.schedule.end_date {
                        let today = chrono::Local::now().date_naive();
                        if let Ok(end_date) =
                            chrono::NaiveDate::parse_from_str(end_date_str, "%Y-%m-%d")
                        {
                            if today > end_date {
                                log_debug(&format!(
                                    "[Wardian] Schedule {} expired (end_date {})",
                                    run.id, end_date_str
                                ));
                                should_delete = true;
                            }
                        }
                    }
                } else if end_cond == "after_occurrences" {
                    if let Some(max) = run.schedule.max_occurrences {
                        if run.schedule.occurrence_count >= max {
                            log_debug(&format!(
                                "[Wardian] Schedule {} completed all {} occurrences",
                                run.id, max
                            ));
                            should_delete = true;
                        }
                    }
                }

                if should_delete {
                    runs_to_delete.push(run.id.clone());
                    modified = true;
                    continue;
                }

                if run.next_run_epoch_ms.is_none() {
                    run.next_run_epoch_ms = match run.paused_remaining_ms.take() {
                        Some(remaining_ms) => Some(now_ms.saturating_add(remaining_ms)),
                        None => compute_next_run(&run.schedule, now_ms),
                    };
                    modified = true;
                }

                if let Some(next) = run.next_run_epoch_ms {
                    if now_ms >= next {
                        log_debug(&format!(
                            "[Wardian] Scheduler firing run {} for workflow {}",
                            run.id, run.workflow_id
                        ));

                        let is_one_time = run.schedule.schedule_type == "one_time";
                        let is_specific_dates = run.schedule.schedule_type == "specific_dates";
                        let completed_run_id = run.id.clone();

                        // Increment occurrence count
                        run.schedule.occurrence_count += 1;

                        run.next_run_epoch_ms = compute_next_run(&run.schedule, now_ms);
                        run.paused_remaining_ms = None;

                        if is_one_time || (is_specific_dates && run.next_run_epoch_ms.is_none()) {
                            runs_to_delete.push(completed_run_id);
                        }
                        modified = true;

                        let payload = scheduled_trigger_payload(&run.id, &run.role_mappings);

                        let _ =
                            run_workflow(app_clone.clone(), run.workflow_id.clone(), Some(payload))
                                .await;
                    }
                }
            }

            if modified {
                let mut fresh = load_scheduled_runs();
                if !runs_to_delete.is_empty() {
                    fresh.retain(|run| !runs_to_delete.iter().any(|id| id == &run.id));
                }
                for fresh_run in fresh.iter_mut() {
                    if let Some(updated) = runs.iter().find(|r| r.id == fresh_run.id) {
                        fresh_run.next_run_epoch_ms = updated.next_run_epoch_ms;
                        fresh_run.paused_remaining_ms = updated.paused_remaining_ms;
                        fresh_run.schedule.active = updated.schedule.active;
                        fresh_run.schedule.occurrence_count = updated.schedule.occurrence_count;
                        fresh_run.last_run_status = updated.last_run_status.clone();
                        fresh_run.last_run_error = updated.last_run_error.clone();
                        fresh_run.last_run_completed_epoch_ms = updated.last_run_completed_epoch_ms;
                    }
                }
                let _ = save_scheduled_runs(&fresh);
                let _ = app_clone.emit("scheduled-runs-updated", ());
            }
        }
    });

    let mut scheduler = state.scheduler_handle.lock().await;
    *scheduler = Some(handle);
}

fn compute_next_run(
    schedule: &wardian_core::models::ScheduleDefinition,
    now_ms: u64,
) -> Option<u64> {
    match schedule.schedule_type.as_str() {
        "interval" => {
            let mins = schedule.interval_minutes.unwrap_or(0) as u64;
            if mins > 0 {
                Some(now_ms + mins * 60_000)
            } else {
                None
            }
        }
        "daily" => {
            let time_str = schedule.time_of_day.as_deref().unwrap_or("00:00");
            let parts: Vec<&str> = time_str.split(':').collect();
            if parts.len() != 2 {
                return None;
            }
            let hour: u32 = parts[0].parse().unwrap_or(0);
            let minute: u32 = parts[1].parse().unwrap_or(0);

            let now_local = chrono::Local::now();
            let today = now_local.date_naive();
            let target_time = chrono::NaiveTime::from_hms_opt(hour, minute, 0)?;
            let target_naive = today.and_time(target_time);
            let target_local = chrono::Local
                .from_local_datetime(&target_naive)
                .earliest()?;

            let target_ms = target_local.timestamp_millis() as u64;
            if target_ms > now_ms {
                Some(target_ms)
            } else {
                Some(target_ms + 86_400_000)
            }
        }
        "weekly" => {
            let time_str = schedule.time_of_day.as_deref().unwrap_or("00:00");
            let time_parts: Vec<&str> = time_str.split(':').collect();
            if time_parts.len() != 2 {
                return None;
            }
            let hour: u32 = time_parts[0].parse().unwrap_or(0);
            let minute: u32 = time_parts[1].parse().unwrap_or(0);

            let day_names = match &schedule.days_of_week {
                Some(d) if !d.is_empty() => d.clone(),
                _ => return None,
            };

            let day_map = |name: &str| -> Option<chrono::Weekday> {
                match name.to_lowercase().as_str() {
                    "mon" => Some(chrono::Weekday::Mon),
                    "tue" => Some(chrono::Weekday::Tue),
                    "wed" => Some(chrono::Weekday::Wed),
                    "thu" => Some(chrono::Weekday::Thu),
                    "fri" => Some(chrono::Weekday::Fri),
                    "sat" => Some(chrono::Weekday::Sat),
                    "sun" => Some(chrono::Weekday::Sun),
                    _ => None,
                }
            };

            let repeat_weeks = schedule.repeat_every.max(1) as i64;
            let now_local = chrono::Local::now();
            let mut best: Option<u64> = None;

            // Search up to repeat_weeks * 7 + 7 days ahead
            let search_days = (repeat_weeks * 7 + 7) as u32;
            for day_name in &day_names {
                if let Some(target_day) = day_map(day_name) {
                    for offset in 0..search_days {
                        let candidate_date =
                            (now_local + chrono::Duration::days(offset as i64)).date_naive();
                        if candidate_date.weekday() == target_day {
                            // For repeat_every > 1, check week alignment
                            if repeat_weeks > 1 {
                                let epoch = chrono::NaiveDate::from_ymd_opt(2000, 1, 3).unwrap(); // a known Monday in the past
                                let weeks_since = (candidate_date - epoch).num_weeks();
                                if weeks_since.rem_euclid(repeat_weeks) != 0 {
                                    continue;
                                }
                            }
                            let target_time = chrono::NaiveTime::from_hms_opt(hour, minute, 0)?;
                            let candidate_naive = candidate_date.and_time(target_time);
                            if let Some(candidate_local) = chrono::Local
                                .from_local_datetime(&candidate_naive)
                                .earliest()
                            {
                                let candidate_ms = candidate_local.timestamp_millis() as u64;
                                if candidate_ms > now_ms {
                                    best = Some(
                                        best.map_or(candidate_ms, |b: u64| b.min(candidate_ms)),
                                    );
                                    break;
                                }
                            }
                        }
                    }
                }
            }

            best
        }
        "monthly" => {
            let time_str = schedule.time_of_day.as_deref().unwrap_or("00:00");
            let time_parts: Vec<&str> = time_str.split(':').collect();
            if time_parts.len() != 2 {
                return None;
            }
            let hour: u32 = time_parts[0].parse().unwrap_or(0);
            let minute: u32 = time_parts[1].parse().unwrap_or(0);

            let target_days = match &schedule.days_of_month {
                Some(d) if !d.is_empty() => d.clone(),
                _ => return None,
            };

            let now_local = chrono::Local::now();
            let target_time = chrono::NaiveTime::from_hms_opt(hour, minute, 0)?;
            let mut best: Option<u64> = None;

            // Check current month and next 2 months
            for month_offset in 0..3i32 {
                let candidate_month = now_local.month() as i32 + month_offset;
                let candidate_year = now_local.year() + (candidate_month - 1) / 12;
                let candidate_month_norm = ((candidate_month - 1) % 12 + 1) as u32;

                for &day in &target_days {
                    if let Some(candidate_date) =
                        chrono::NaiveDate::from_ymd_opt(candidate_year, candidate_month_norm, day)
                    {
                        let candidate_naive = candidate_date.and_time(target_time);
                        if let Some(candidate_local) = chrono::Local
                            .from_local_datetime(&candidate_naive)
                            .earliest()
                        {
                            let candidate_ms = candidate_local.timestamp_millis() as u64;
                            if candidate_ms > now_ms {
                                best =
                                    Some(best.map_or(candidate_ms, |b: u64| b.min(candidate_ms)));
                            }
                        }
                    }
                }

                if best.is_some() {
                    break;
                }
            }

            best
        }
        "specific_dates" => {
            let time_str = schedule.time_of_day.as_deref().unwrap_or("00:00");
            let time_parts: Vec<&str> = time_str.split(':').collect();
            let hour: u32 = time_parts.first().and_then(|p| p.parse().ok()).unwrap_or(0);
            let minute: u32 = time_parts.get(1).and_then(|p| p.parse().ok()).unwrap_or(0);

            let dates = match &schedule.specific_dates {
                Some(d) if !d.is_empty() => d.clone(),
                _ => return None,
            };

            let target_time = chrono::NaiveTime::from_hms_opt(hour, minute, 0)?;
            let mut best: Option<u64> = None;

            for date_str in &dates {
                if let Ok(date) = chrono::NaiveDate::parse_from_str(date_str, "%Y-%m-%d") {
                    let candidate_naive = date.and_time(target_time);
                    if let Some(candidate_local) = chrono::Local
                        .from_local_datetime(&candidate_naive)
                        .earliest()
                    {
                        let candidate_ms = candidate_local.timestamp_millis() as u64;
                        if candidate_ms > now_ms {
                            best = Some(best.map_or(candidate_ms, |b: u64| b.min(candidate_ms)));
                        }
                    }
                }
            }

            best
        }
        "one_time" => {
            let run_at = schedule.run_at.as_deref().unwrap_or("");
            if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(run_at) {
                let ms = dt.timestamp_millis() as u64;
                if ms > now_ms {
                    Some(ms)
                } else {
                    None
                }
            } else if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(run_at, "%Y-%m-%dT%H:%M") {
                let local = chrono::Local.from_local_datetime(&dt).earliest()?;
                let ms = local.timestamp_millis() as u64;
                if ms > now_ms {
                    Some(ms)
                } else {
                    None
                }
            } else {
                None
            }
        }
        _ => None,
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

pub async fn stop_workflow_run(app: AppHandle, run_instance_id: &str) {
    let state = app.state::<crate::state::AppState>();
    let mut runs = state.workflow_runs.lock().await;
    if let Some(handles) = runs.remove(run_instance_id) {
        for handle in handles {
            handle.abort();
        }
    }
}

pub async fn stop_all_triggers(app: AppHandle) {
    let state = app.state::<crate::state::AppState>();
    let mut triggers = state.workflow_triggers.lock().await;
    for (_wf_id, handles) in triggers.drain() {
        for handle in handles {
            handle.abort();
        }
    }
}

pub fn pause_all_triggers(app: AppHandle) {
    let state = app.state::<crate::state::AppState>();
    state
        .triggers_paused
        .store(true, std::sync::atomic::Ordering::SeqCst);
}

pub fn resume_all_triggers(app: AppHandle) {
    let state = app.state::<crate::state::AppState>();
    state
        .triggers_paused
        .store(false, std::sync::atomic::Ordering::SeqCst);
}

pub async fn start_workflow_triggers(
    app: AppHandle,
    wf: WorkflowDefinition,
    register_schedules: bool,
) {
    // 1. Clean up existing triggers for this workflow
    stop_workflow_triggers(app.clone(), &wf.id).await;

    if register_schedules {
        let runs = load_scheduled_runs();
        let synced_runs = sync_scheduled_runs_for_workflow(&wf, &runs);
        let _ = save_scheduled_runs(&synced_runs);
    }

    let mut handles = Vec::new();

    // 2. Identify trigger nodes
    for node in &wf.nodes {
        if node.r#type == "trigger" {
            let app_clone = app.clone();
            let wf_id = wf.id.clone();
            let config = node.config.clone();

            // --- Scheduled Trigger ---
            // Register a ScheduledRun entry. The unified heartbeat scheduler
            // (5s loop) handles all timed execution from scheduled_runs.json.
            if register_schedules
                && config
                    .get("status")
                    .and_then(|value| value.as_str())
                    .is_some_and(|status| status == "off")
            {
                continue;
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
                        let mut watcher =
                            notify::recommended_watcher(move |res: notify::Result<Event>| {
                                if let Ok(Event { paths, kind, .. }) = res {
                                    if !paths.is_empty() {
                                        let path = paths[0].to_string_lossy().to_string();
                                        let event_type = format!("{:?}", kind);
                                        let payload = serde_json::json!({
                                            "path": path,
                                            "event": event_type,
                                            "timestamp": Utc::now().to_rfc3339()
                                        });
                                        let _ = tx.try_send(Some(payload)); // Send payload through channel
                                    }
                                }
                            })
                            .unwrap();

                        if watcher
                            .watch(&path_clone, RecursiveMode::NonRecursive)
                            .is_ok()
                        {
                            while let Some(payload) = rx.recv().await {
                                let state = app_file.state::<crate::state::AppState>();
                                if state
                                    .triggers_paused
                                    .load(std::sync::atomic::Ordering::SeqCst)
                                {
                                    log_debug(&format!(
                                        "[Wardian] File trigger for {} skipped (Paused)",
                                        wf_id_file
                                    ));
                                    continue;
                                }

                                log_debug(&format!(
                                    "[Wardian] Triggering workflow {} via file watcher on {:?}",
                                    wf_id_file, path_clone
                                ));
                                let _ = run_workflow(app_file.clone(), wf_id_file.clone(), payload)
                                    .await;
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

pub async fn run_scheduled_workflow_now(app: AppHandle, run_id: String) -> Result<(), String> {
    let mut runs = load_scheduled_runs();
    let now_ms = Utc::now().timestamp_millis() as u64;

    let Some(run) = runs
        .iter_mut()
        .find(|scheduled_run| scheduled_run.id == run_id)
    else {
        return Err(format!("Scheduled run {} not found", run_id));
    };

    if run.schedule.active && !run.is_paused {
        run.next_run_epoch_ms = compute_next_run(&run.schedule, now_ms);
        run.paused_remaining_ms = None;
    }

    let payload = scheduled_trigger_payload(&run.id, &run.role_mappings);
    let workflow_id = run.workflow_id.clone();

    save_scheduled_runs(&runs)?;
    run_workflow(app, workflow_id, Some(payload)).await
}

fn scheduled_trigger_payload(
    scheduled_run_id: &str,
    role_mappings: &HashMap<String, String>,
) -> Value {
    serde_json::json!({
        "timestamp": Local::now().to_rfc3339(),
        "scheduled_run_id": scheduled_run_id,
        "role_mappings": role_mappings,
    })
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
    start_workflow_triggers(app, wf, false).await;
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

pub fn disable_scheduled_trigger(run_id: &str) -> Result<(), String> {
    let Some(run) = load_scheduled_runs()
        .into_iter()
        .find(|scheduled_run| scheduled_run.id == run_id)
    else {
        return Ok(());
    };

    let dir = get_workflows_dir().ok_or("Could not find Wardian home")?;
    let path = dir.join(format!("{}.json", run.workflow_id));
    if !path.exists() {
        return Ok(());
    }

    let content = fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let mut workflow =
        serde_json::from_str::<WorkflowDefinition>(&content).map_err(|e| e.to_string())?;

    for node in workflow.nodes.iter_mut() {
        if node.r#type != "trigger" || node.name.as_deref() != Some("Scheduled Trigger") {
            continue;
        }

        let expected_run_id = format!("{}-{}", workflow.id, node.id);
        if expected_run_id != run_id {
            continue;
        }

        match node.config.as_object_mut() {
            Some(config) => {
                config.insert(
                    "status".to_string(),
                    serde_json::Value::String("off".to_string()),
                );
            }
            None => {
                node.config = serde_json::json!({ "status": "off" });
            }
        }
    }

    let content = serde_json::to_string_pretty(&workflow).map_err(|e| e.to_string())?;
    fs::write(path, content).map_err(|e| e.to_string())
}

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
mod tests {
    use super::{
        parse_optional_timeout_ms, prepare_agent_for_headless_json_run,
        record_restore_spawn_result, record_scheduled_run_outcome, resolve_command_node_launch,
        scheduled_trigger_payload, sync_scheduled_runs_for_workflow,
    };
    use crate::utils::ShellLaunchSpec;
    use chrono::Local;
    use serde_json::json;
    use std::collections::HashMap;
    use wardian_core::models::{
        ScheduleDefinition, ScheduledRun, WorkflowDefinition, WorkflowNode, WorkflowSettings,
    };

    fn workflow_with_schedule(status: &str) -> WorkflowDefinition {
        WorkflowDefinition {
            id: "wf-1".to_string(),
            name: "Morning Sync".to_string(),
            settings: WorkflowSettings {
                max_iterations: 10,
                on_limit_reached: "pause".to_string(),
            },
            nodes: vec![WorkflowNode {
                id: "trigger-1".to_string(),
                r#type: "trigger".to_string(),
                name: Some("Scheduled Trigger".to_string()),
                config: json!({
                    "schedule_type": "Daily",
                    "time": "09:00",
                    "status": status,
                }),
                parameter_schema: None,
                dependencies: None,
                position: None,
            }],
            role_mappings: HashMap::from([("analyst".to_string(), "agent-1".to_string())]),
        }
    }

    #[test]
    fn compute_next_run_weekly_epoch_alignment() {
        use super::compute_next_run;
        use chrono::TimeZone;
        let schedule = ScheduleDefinition {
            schedule_type: "weekly".to_string(),
            time_of_day: Some("09:00".to_string()),
            days_of_week: Some(vec!["Mon".to_string(), "Wed".to_string()]),
            repeat_every: 2, // Every 2 weeks
            ..Default::default()
        };
        // Use a test date: 2024-01-01 was a Monday
        let now = chrono::NaiveDate::from_ymd_opt(2024, 1, 1)
            .unwrap()
            .and_time(chrono::NaiveTime::from_hms_opt(8, 0, 0).unwrap());
        let now_ms = chrono::Local
            .from_local_datetime(&now)
            .earliest()
            .unwrap()
            .timestamp_millis() as u64;

        // This will verify `rem_euclid` doesn't panic and returns a valid timestamp in the future.
        let next = compute_next_run(&schedule, now_ms);
        assert!(next.is_some());
        assert!(next.unwrap() > now_ms);
    }

    #[test]
    fn resolve_command_node_launch_returns_shell_spec() {
        let spec = resolve_command_node_launch("echo hi", |_| {
            Ok(ShellLaunchSpec {
                executable: "pwsh".to_string(),
                args: vec![
                    "-NoProfile".to_string(),
                    "-Command".to_string(),
                    "echo hi".to_string(),
                ],
            })
        })
        .expect("shell spec");

        assert_eq!(spec.executable, "pwsh");
        assert_eq!(spec.args[2], "echo hi");
    }

    #[test]
    fn resolve_command_node_launch_surfaces_shell_resolution_errors() {
        let err =
            resolve_command_node_launch("echo hi", |_| Err("No compatible shell".to_string()))
                .expect_err("shell resolution should fail");

        assert_eq!(err, "No compatible shell");
    }

    #[test]
    fn resolve_command_node_launch_rejects_empty_commands() {
        let err = resolve_command_node_launch("   ", |_| unreachable!("resolver should not run"))
            .expect_err("empty commands should fail");

        assert_eq!(err, "Missing command string");
    }

    #[test]
    fn record_restore_spawn_result_surfaces_failed_re_spawn() {
        let mut node_error = None;
        let restored = record_restore_spawn_result::<()>(
            "agent-1",
            Err("Codex provider unavailable".to_string()),
            &mut node_error,
        );

        assert!(restored.is_none());
        assert_eq!(
            node_error.as_deref(),
            Some("Failed to restore agent agent-1 after headless run: Codex provider unavailable")
        );
    }

    #[test]
    fn prepare_agent_for_headless_json_run_marks_status_and_detaches_child() {
        let mut active = crate::state::ActiveAgent {
            config: std::sync::Arc::new(std::sync::Mutex::new(
                wardian_core::models::AgentConfig::default(),
            )),
            child_process: None,
            background_processes: Vec::new(),
            pty_master: None,
            stdin_tx: None,
            output_buffer: std::sync::Arc::new(std::sync::Mutex::new(String::new())),
            process_id: None,
            query_count: std::sync::Arc::new(std::sync::Mutex::new(0)),
            init_timestamp: std::sync::Arc::new(std::sync::Mutex::new(None)),
            current_status: std::sync::Arc::new(std::sync::Mutex::new("Idle".to_string())),
            last_status_at: std::sync::Arc::new(std::sync::Mutex::new(None)),
            watch_state: std::sync::Arc::new(std::sync::Mutex::new(
                crate::state::AgentWatchState::new("agent-1".to_string(), 4096, 262_144),
            )),
            terminal_title: std::sync::Arc::new(std::sync::Mutex::new(String::new())),
            last_output_at: std::sync::Arc::new(std::sync::Mutex::new(None)),
            log_path: std::sync::Arc::new(std::sync::Mutex::new(None)),
            log_last_modified: std::sync::Arc::new(std::sync::Mutex::new(None)),
            #[cfg(windows)]
            job_object: None,
        };

        let child = prepare_agent_for_headless_json_run(&mut active);

        assert!(child.is_none());
        assert_eq!(active.current_status.lock().unwrap().as_str(), "Headless");
    }

    #[test]
    fn parse_optional_timeout_treats_absent_blank_null_and_zero_as_unlimited() {
        assert_eq!(parse_optional_timeout_ms(&json!({})), None);
        assert_eq!(
            parse_optional_timeout_ms(&json!({ "timeout_ms": null })),
            None
        );
        assert_eq!(
            parse_optional_timeout_ms(&json!({ "timeout_ms": "" })),
            None
        );
        assert_eq!(
            parse_optional_timeout_ms(&json!({ "timeout_ms": "   " })),
            None
        );
        assert_eq!(parse_optional_timeout_ms(&json!({ "timeout_ms": 0 })), None);
        assert_eq!(
            parse_optional_timeout_ms(&json!({ "timeout_ms": "0" })),
            None
        );
    }

    #[test]
    fn parse_optional_timeout_accepts_positive_numeric_values() {
        assert_eq!(
            parse_optional_timeout_ms(&json!({ "timeout_ms": 5000 })),
            Some(5000)
        );
        assert_eq!(
            parse_optional_timeout_ms(&json!({ "timeout_ms": "60000" })),
            Some(60000)
        );
    }

    #[test]
    fn scheduled_trigger_payload_uses_local_timestamp_offset() {
        let payload = scheduled_trigger_payload(
            "sched-1",
            &HashMap::from([("analyst".to_string(), "agent-1".to_string())]),
        );

        let timestamp = payload
            .get("timestamp")
            .and_then(|value| value.as_str())
            .expect("timestamp string");
        let parsed = chrono::DateTime::parse_from_rfc3339(timestamp).expect("valid rfc3339");

        assert_eq!(
            parsed.offset().local_minus_utc(),
            Local::now().offset().local_minus_utc()
        );
        assert_eq!(
            payload
                .get("scheduled_run_id")
                .and_then(|value| value.as_str()),
            Some("sched-1")
        );
    }

    #[test]
    fn sync_scheduled_runs_preserves_pause_state_for_active_schedule() {
        let workflow = workflow_with_schedule("active");
        let existing_runs = vec![ScheduledRun {
            id: "wf-1-trigger-1".to_string(),
            workflow_id: "wf-1".to_string(),
            workflow_name: "Morning Sync".to_string(),
            schedule: ScheduleDefinition {
                schedule_type: "daily".to_string(),
                interval_minutes: None,
                time_of_day: Some("09:00".to_string()),
                days_of_week: None,
                repeat_every: 1,
                days_of_month: None,
                specific_dates: None,
                run_at: None,
                end_condition: "never".to_string(),
                end_date: None,
                max_occurrences: None,
                occurrence_count: 0,
                active: true,
            },
            role_mappings: HashMap::new(),
            description: "Daily at 09:00".to_string(),
            next_run_epoch_ms: Some(1234),
            paused_remaining_ms: None,
            is_paused: true,
            last_run_status: None,
            last_run_error: None,
            last_run_completed_epoch_ms: None,
        }];

        let runs = sync_scheduled_runs_for_workflow(&workflow, &existing_runs);

        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].id, "wf-1-trigger-1");
        assert_eq!(runs[0].workflow_name, "Morning Sync");
        assert_eq!(runs[0].schedule.schedule_type, "daily");
        assert_eq!(runs[0].schedule.time_of_day.as_deref(), Some("09:00"));
        assert!(runs[0].is_paused);
        assert_eq!(runs[0].next_run_epoch_ms, Some(1234));
    }

    #[test]
    fn sync_scheduled_runs_resets_next_run_when_schedule_changes() {
        let workflow = workflow_with_schedule("active");
        let existing_runs = vec![ScheduledRun {
            id: "wf-1-trigger-1".to_string(),
            workflow_id: "wf-1".to_string(),
            workflow_name: "Morning Sync".to_string(),
            schedule: ScheduleDefinition {
                schedule_type: "daily".to_string(),
                interval_minutes: None,
                time_of_day: Some("08:00".to_string()),
                days_of_week: None,
                repeat_every: 1,
                days_of_month: None,
                specific_dates: None,
                run_at: None,
                end_condition: "never".to_string(),
                end_date: None,
                max_occurrences: None,
                occurrence_count: 0,
                active: true,
            },
            role_mappings: HashMap::new(),
            description: "Daily at 08:00".to_string(),
            next_run_epoch_ms: Some(1234),
            paused_remaining_ms: None,
            is_paused: false,
            last_run_status: None,
            last_run_error: None,
            last_run_completed_epoch_ms: None,
        }];

        let runs = sync_scheduled_runs_for_workflow(&workflow, &existing_runs);

        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].schedule.time_of_day.as_deref(), Some("09:00"));
        assert_eq!(runs[0].next_run_epoch_ms, None);
    }

    #[test]
    fn sync_scheduled_runs_removes_disabled_schedule_entries() {
        let workflow = workflow_with_schedule("off");
        let existing_runs = vec![ScheduledRun {
            id: "wf-1-trigger-1".to_string(),
            workflow_id: "wf-1".to_string(),
            workflow_name: "Morning Sync".to_string(),
            schedule: ScheduleDefinition {
                schedule_type: "daily".to_string(),
                interval_minutes: None,
                time_of_day: Some("09:00".to_string()),
                days_of_week: None,
                repeat_every: 1,
                days_of_month: None,
                specific_dates: None,
                run_at: None,
                end_condition: "never".to_string(),
                end_date: None,
                max_occurrences: None,
                occurrence_count: 0,
                active: true,
            },
            role_mappings: HashMap::new(),
            description: "Daily at 09:00".to_string(),
            next_run_epoch_ms: Some(1234),
            paused_remaining_ms: None,
            is_paused: false,
            last_run_status: None,
            last_run_error: None,
            last_run_completed_epoch_ms: None,
        }];

        let runs = sync_scheduled_runs_for_workflow(&workflow, &existing_runs);

        assert!(runs.is_empty());
    }

    #[test]
    fn record_scheduled_run_outcome_marks_failed_run_with_error() {
        let mut runs = vec![ScheduledRun {
            id: "wf-1-trigger-1".to_string(),
            workflow_id: "wf-1".to_string(),
            workflow_name: "Morning Sync".to_string(),
            schedule: ScheduleDefinition {
                schedule_type: "daily".to_string(),
                interval_minutes: None,
                time_of_day: Some("09:00".to_string()),
                days_of_week: None,
                repeat_every: 1,
                days_of_month: None,
                specific_dates: None,
                run_at: None,
                end_condition: "never".to_string(),
                end_date: None,
                max_occurrences: None,
                occurrence_count: 0,
                active: true,
            },
            role_mappings: HashMap::new(),
            description: "Daily at 09:00".to_string(),
            next_run_epoch_ms: Some(1234),
            paused_remaining_ms: None,
            is_paused: false,
            last_run_status: None,
            last_run_error: None,
            last_run_completed_epoch_ms: None,
        }];

        record_scheduled_run_outcome(
            &mut runs,
            "wf-1-trigger-1",
            "failed",
            Some("Agent timeout (60000ms)".to_string()),
            42,
        );

        assert_eq!(runs[0].last_run_status.as_deref(), Some("failed"));
        assert_eq!(
            runs[0].last_run_error.as_deref(),
            Some("Agent timeout (60000ms)")
        );
        assert_eq!(runs[0].last_run_completed_epoch_ms, Some(42));
    }
}

pub async fn run_workflow(
    app: AppHandle,
    wf_id: String,
    initial_payload: Option<Value>,
) -> Result<(), String> {
    let workflows = list_workflows()?;
    let mut wf = workflows
        .into_iter()
        .find(|w| w.id == wf_id)
        .ok_or_else(|| format!("Workflow {} not found", wf_id))?;

    let run_salt = Utc::now()
        .timestamp_nanos_opt()
        .unwrap_or_else(|| Utc::now().timestamp_micros() * 1000);

    let scheduled_run_id = initial_payload
        .as_ref()
        .and_then(|p| p.get("scheduled_run_id").and_then(|v| v.as_str()))
        .map(|s| s.to_string());

    // Generate unique run_instance_id per execution.
    let run_instance_id = if let Some(scheduled_id) = scheduled_run_id.as_ref() {
        format!("run-{}-{}", scheduled_id, run_salt)
    } else {
        format!("manual-{}-{}", wf_id, run_salt)
    };

    // Merge role_mappings from payload (e.g. from scheduler) into the workflow
    if let Some(ref payload) = initial_payload {
        if let Some(mappings) = payload.get("role_mappings").and_then(|v| v.as_object()) {
            for (k, v) in mappings {
                if let Some(s) = v.as_str() {
                    wf.role_mappings.insert(k.clone(), s.to_string());
                }
            }
        }
    }

    let log_dir = get_logs_dir(&wf_id).ok_or("Could not find log path")?;
    fs::create_dir_all(&log_dir).map_err(|e| e.to_string())?;

    let _ = app.emit(
        "workflow-status-updated",
        serde_json::json!({
            "workflow_id": wf_id,
            "run_instance_id": run_instance_id,
            "scheduled_run_id": scheduled_run_id.clone(),
            "status": "running",
        }),
    );

    let timestamp = Utc::now().format("%Y%m%d_%H%M%S").to_string();
    let log_path = log_dir.join(format!("{}.json", timestamp));

    // Store the run handle for cancellation support
    let app_for_handle = app.clone();
    let run_instance_id_for_completion = run_instance_id.clone();
    let run_instance_id_for_cleanup = run_instance_id.clone();
    let run_instance_id_for_handle = run_instance_id.clone();
    let scheduled_run_id_for_completion = scheduled_run_id.clone();
    let handle = tauri::async_runtime::spawn(async move {
        let mut trace = Vec::new();
        let mut pulsed_ports: HashMap<(String, String), u32> = HashMap::new(); // (node_id, port_id) -> count
        let mut consumed_pulses: HashMap<String, HashMap<(String, String), u32>> = HashMap::new(); // node_id -> dependency -> count
        let mut queue = VecDeque::new();
        let mut registry: HashMap<String, Value> = HashMap::new();
        let initial_payload_captured = initial_payload.clone();

        // Initialize storage in registry
        let mut storage = load_shared_storage();
        registry.insert("storage".to_string(), storage.clone());

        let mut global_step_count: usize = 0;
        let mut total_enqueued: usize = 0;
        let hard_limit = wf.settings.max_iterations.max(100) as usize;

        // 1. Identify Entry Points (nodes with no dependencies)
        for node in &wf.nodes {
            if node.dependencies.as_ref().is_none_or(|d| d.is_empty()) {
                queue.push_back(node.id.clone());
                total_enqueued += 1;
            }
        }

        // 2. Execution Loop
        while let Some(current_node_id) = queue.pop_front() {
            global_step_count += 1;
            if global_step_count > hard_limit {
                log_debug(&format!(
                    "[Wardian] Workflow {} aborted: Global step limit ({}) reached",
                    wf.id, hard_limit
                ));
                break;
            }

            let mut node = match wf.nodes.iter().find(|n| n.id == current_node_id) {
                Some(n) => n.clone(),
                None => continue,
            };

            // --- PARAMETER SCHEMA MERGE & VALIDATION ---
            let mut merged_config = match node.config.clone() {
                Value::Object(m) => m,
                _ => serde_json::Map::new(),
            };

            let mut node_validation_error: Option<String> = None;
            if let Some(Value::Object(schema_props)) = &node.parameter_schema {
                for (param_name, param_def) in schema_props {
                    if let Some(def_obj) = param_def.as_object() {
                        let is_required = def_obj
                            .get("required")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false);

                        let is_missing = match merged_config.get(param_name) {
                            None | Some(Value::Null) => true,
                            Some(Value::String(s)) if s.is_empty() => true,
                            _ => false,
                        };

                        if is_missing {
                            if let Some(default_val) = def_obj.get("default") {
                                let final_val = if let Some(s) = default_val.as_str() {
                                    Value::String(interpolate_string(s, &registry))
                                } else {
                                    default_val.clone()
                                };
                                merged_config.insert(param_name.clone(), final_val);
                            } else if is_required {
                                node_validation_error =
                                    Some(format!("Missing required parameter: {}", param_name));
                                break;
                            }
                        }
                    }
                }
            }
            node.config = Value::Object(merged_config);

            // --- 2A. Transactional Consumption Logic ---
            let mut triggered_by_dep = None;
            if let Some(deps) = &node.dependencies {
                if !deps.is_empty() {
                    // Check if there is AT LEAST ONE unconsumed pulse
                    let mut is_satisfied = false;
                    let node_consumed = consumed_pulses.entry(node.id.clone()).or_default();

                    if node.r#type == "wait" {
                        // Wait nodes: ALL dependent ports must have pulsed_count > consumed_count
                        is_satisfied = deps.iter().all(|d| {
                            let key = (d.node_id.clone(), d.port.clone());
                            let p_count = pulsed_ports.get(&key).cloned().unwrap_or(0);
                            let c_count = node_consumed.get(&key).cloned().unwrap_or(0);
                            p_count > c_count
                        });
                        if is_satisfied {
                            // Atomically consume ONE pulse from EVERY dependency
                            for d in deps {
                                let key = (d.node_id.clone(), d.port.clone());
                                *node_consumed.entry(key).or_insert(0) += 1;
                            }
                        }
                    } else {
                        // Standard nodes: ANY dependent port with pulsed_count > consumed_count triggers it
                        for d in deps {
                            let key = (d.node_id.clone(), d.port.clone());
                            let p_count = pulsed_ports.get(&key).cloned().unwrap_or(0);
                            let c_count = node_consumed.get(&key).cloned().unwrap_or(0);
                            if p_count > c_count {
                                triggered_by_dep = Some(key.clone());
                                is_satisfied = true;
                                break;
                            }
                        }
                        if is_satisfied {
                            // Atomically consume ONE pulse from the triggering dependency
                            if let Some(key) = triggered_by_dep {
                                *node_consumed.entry(key).or_insert(0) += 1;
                            }
                        }
                    }
                    if !is_satisfied {
                        continue;
                    }
                }
            }

            let _ = app.emit(
                "workflow-telemetry",
                &WorkflowTelemetryEvent {
                    workflow_id: wf.id.clone(),
                    node_id: node.id.clone(),
                    status: "processing".to_string(),
                    output: None,
                    error: None,
                },
            );

            let _ = app.emit(
                "workflow-progress",
                serde_json::json!({
                    "workflow_id": wf.id,
                    "run_instance_id": run_instance_id,
                    "scheduled_run_id": scheduled_run_id.clone(),
                    "workflow_name": wf.name,
                    "current_step": global_step_count,
                    "total_steps": total_enqueued + queue.len(),
                    "active_node_name": node.name.clone().unwrap_or_else(|| node.id.clone()),
                }),
            );

            // --- NODE LOGIC ---
            let mut result_ports = vec!["default".to_string()];
            let mut output_payload = Value::Null;
            let mut node_error = node_validation_error;

            if node_error.is_none() {
                match node.r#type.as_str() {
                    "trigger" => {
                        log_debug(&format!("[Wardian] Trigger node {} pulsing", node.id));

                        let mut payload_map = match node.config.as_object() {
                            Some(m) => m.clone(),
                            None => serde_json::Map::new(),
                        };

                        if let Some(Value::Object(initial)) = initial_payload_captured.as_ref() {
                            for (k, v) in initial {
                                payload_map.insert(k.clone(), v.clone());
                            }
                        }

                        // Clean up metadata that shouldn't be in the runtime payload
                        payload_map.remove("input_schema");
                        payload_map.remove("json_schema");

                        output_payload = Value::Object(payload_map);

                        // Alias this to the global 'trigger' key
                        registry.insert("trigger".to_string(), output_payload.clone());
                    }
                    "memory" => {
                        let op = node
                            .config
                            .get("operation")
                            .and_then(|v| v.as_str())
                            .unwrap_or("get");
                        let key_path = node
                            .config
                            .get("key")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        let value_to_set = node
                            .config
                            .get("value")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");

                        // Interpolate key and value
                        let interpolated_key = interpolate_string(key_path, &registry);
                        let interpolated_val = interpolate_string(value_to_set, &registry);

                        match op {
                            "set" => {
                                // Deep set logic or flat key? For now, let's stick to flat or dot-notated objects
                                if let Some(obj) = storage.as_object_mut() {
                                    // Simple dot notation support for set (key.subKey)
                                    let parts: Vec<&str> = interpolated_key.split('.').collect();
                                    if parts.len() == 1 {
                                        obj.insert(
                                            interpolated_key.clone(),
                                            serde_json::Value::String(interpolated_val.clone()),
                                        );
                                    } else {
                                        // Complex nested set - keeping it simple for now
                                        obj.insert(
                                            interpolated_key.clone(),
                                            serde_json::Value::String(interpolated_val.clone()),
                                        );
                                    }
                                }
                                save_shared_storage(&storage);
                                registry.insert("storage".to_string(), storage.clone());
                                output_payload = serde_json::json!({ "status": "success", "op": "set", "key": interpolated_key, "value": interpolated_val });
                            }
                            "delete" => {
                                if let Some(obj) = storage.as_object_mut() {
                                    obj.remove(&interpolated_key);
                                }
                                save_shared_storage(&storage);
                                registry.insert("storage".to_string(), storage.clone());
                                output_payload = serde_json::json!({ "status": "success", "op": "delete", "key": interpolated_key });
                            }
                            _ => {
                                // Already in registry as 'storage', but we return the specific value as node output too
                                let val = if let Some(obj) = storage.as_object() {
                                    obj.get(&interpolated_key).cloned().unwrap_or(Value::Null)
                                } else {
                                    Value::Null
                                };
                                output_payload = serde_json::json!({ "status": "success", "op": "get", "key": interpolated_key, "value": val });
                            }
                        }
                    }
                    "agent" => {
                        // Role-based resolution with agent_id fallback
                        let role = node
                            .config
                            .get("role")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        let direct_id = node
                            .config
                            .get("agent_id")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");

                        let agent_id = if !role.is_empty() {
                            wf.role_mappings.get(role).map(|s| s.as_str()).unwrap_or("")
                        } else {
                            direct_id
                        };
                        let mode =
                            wardian_core::models::AgentExecutionPolicy::from_legacy_session_type(
                                node.config.get("session_type").and_then(|v| v.as_str()),
                                node.config.get("mode").and_then(|v| v.as_str()),
                            )
                            .mode;

                        let mut prompt = node
                            .config
                            .get("prompt")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        prompt = interpolate_string(&prompt, &registry);

                        if agent_id.is_empty()
                            && mode != wardian_core::models::WorkflowAgentMode::Ephemeral
                        {
                            node_error = Some(if !role.is_empty() {
                                format!("Role '{}' not mapped to an agent. Set role_mappings before running.", role)
                            } else {
                                "Missing agent_id or role".to_string()
                            });
                        } else {
                            let target_agent_config = if agent_id.is_empty() {
                                None
                            } else {
                                let state = app.state::<crate::state::AppState>();
                                let agents_map = state.agents.lock().await;
                                let existing_config = if let Some(agent) = agents_map.get(agent_id)
                                {
                                    let config = agent.config.lock().unwrap().clone();
                                    Some(config)
                                } else if let Some(home) = get_wardian_home() {
                                    if let Ok(data) =
                                        std::fs::read_to_string(home.join("settings/state.json"))
                                    {
                                        if let Ok(configs) =
                                            serde_json::from_str::<
                                                Vec<wardian_core::models::AgentConfig>,
                                            >(&data)
                                        {
                                            configs.into_iter().find(|c| c.session_id == agent_id)
                                        } else {
                                            None
                                        }
                                    } else {
                                        None
                                    }
                                } else {
                                    None
                                };
                                existing_config
                            };
                            let node_config = node.config.as_object().cloned().unwrap_or_default();
                            let exec_ctx = match agent_execution::resolve_agent_execution_context(
                                &node.id,
                                &node_config,
                                target_agent_config.as_ref(),
                                &run_instance_id,
                            ) {
                                Ok(ctx) => Some(ctx),
                                Err(err) => {
                                    node_error = Some(err);
                                    None
                                }
                            };
                            if let Some(exec_ctx) = exec_ctx {
                                let provider_name = exec_ctx.config.provider.clone();
                                let output_format = node
                                    .config
                                    .get("output_format")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("text");

                                // Apply JSON Schema constraint if provided
                                if output_format == "json" {
                                    if let Some(schema) =
                                        node.config.get("json_schema").and_then(|v| v.as_str())
                                    {
                                        if !schema.trim().is_empty() && schema.trim() != "{}" {
                                            let interpolated_schema =
                                                interpolate_string(schema, &registry);
                                            prompt.push_str(
                                                "\n\nGive your answer as JSON in the format:\n",
                                            );
                                            prompt.push_str(&interpolated_schema);
                                        }
                                    }
                                }

                                let timeout_ms = parse_optional_timeout_ms(&node.config);

                                log_debug(&format!(
                            "[Wardian] Agent node: agent_id={}, provider={}, output_format={}, prompt_len={}, prompt_preview='{}'",
                            agent_id,
                            provider_name,
                            output_format,
                            prompt.len(),
                            &prompt[..prompt.len().min(200)]
                        ));

                                // 1. Resolve CWD logic (Shared between modes)
                                let cwd = crate::utils::fs::resolve_cwd(
                                    &exec_ctx.config.folder,
                                    &exec_ctx.execution_session_id,
                                );

                                if exec_ctx.mode
                                    != wardian_core::models::WorkflowAgentMode::InheritResume
                                {
                                    let run_result = run_with_optional_timeout(
                                        timeout_ms,
                                        "Agent",
                                        crate::manager::run_headless_with_options(
                                            crate::manager::HeadlessRunOptions {
                                                cwd: &cwd,
                                                prompt: &prompt,
                                                wardian_session_id: &exec_ctx.execution_session_id,
                                                resume_session: None,
                                                output_format,
                                                provider_name: &provider_name,
                                                config_override: Some(&exec_ctx.config),
                                            },
                                        ),
                                    )
                                    .await;

                                    match run_result {
                                        Ok(data) => {
                                            output_payload = flatten_headless_response(data);
                                        }
                                        Err(e) => {
                                            log_debug(&format!(
                                                "[Wardian] Fresh workflow agent run failed: {}",
                                                e
                                            ));
                                            node_error = Some(e);
                                        }
                                    }
                                } else if output_format == "json" {
                                    // --- HEADLESS JSON MODE (Always kills PTY for clean structured output) ---
                                    let state = app.state::<crate::state::AppState>();
                                    let mut child_to_stop = None;
                                    let mut agent_cfg = None;
                                    let mut state_snapshot = None;
                                    let mut should_emit_agents_updated = false;

                                    {
                                        let mut agents_map = state.agents.lock().await;
                                        if let Some(agent) = agents_map.get_mut(agent_id) {
                                            agent_cfg = Some(agent.config.clone());
                                            child_to_stop =
                                                prepare_agent_for_headless_json_run(agent);
                                            if let Ok(mut senders) = state.input_senders.write() {
                                                senders.remove(agent_id);
                                            }
                                            let order = state.agent_order.lock().await;
                                            state_snapshot =
                                                Some(crate::manager::state_configs_snapshot(
                                                    &agents_map,
                                                    &order,
                                                ));
                                            should_emit_agents_updated = true;
                                        }
                                    }
                                    if let Some(snapshot) = state_snapshot {
                                        crate::manager::save_state_snapshot(&app, &snapshot);
                                    }
                                    if should_emit_agents_updated {
                                        let _ = app.emit("agents-updated", ());
                                    }

                                    let was_online = child_to_stop.is_some();
                                    if let Some(mut child) = child_to_stop {
                                        let _ = child.kill();
                                        let _ = child.wait();
                                    }
                                    log_debug(&format!("[Wardian] Killed active PTY and set Headless status for {} (was_online={})", agent_id, was_online));

                                    let run_result = run_with_optional_timeout(
                                        timeout_ms,
                                        "Agent",
                                        crate::manager::run_headless_with_options(
                                            crate::manager::HeadlessRunOptions {
                                                cwd: &cwd,
                                                prompt: &prompt,
                                                wardian_session_id: &exec_ctx.execution_session_id,
                                                resume_session: exec_ctx.resume_session.as_deref(),
                                                output_format,
                                                provider_name: &provider_name,
                                                config_override: Some(&exec_ctx.config),
                                            },
                                        ),
                                    )
                                    .await;

                                    match run_result {
                                        Ok(data) => {
                                            output_payload = flatten_headless_response(data);
                                        }
                                        Err(e) => {
                                            log_debug(&format!(
                                                "[Wardian] Headless JSON agent failed: {}",
                                                e
                                            ));
                                            node_error = Some(e);
                                        }
                                    }

                                    // Restore state after headless run
                                    if was_online {
                                        if let Some(cfg_lock) = agent_cfg {
                                            let (cfg, born) = {
                                                let config_guard = cfg_lock.lock().unwrap();
                                                // Since we don't have direct access to the ActiveAgent struct here (it's being restored),
                                                // we'll pass None for the timestamp if it's not in the config, letting spawn_agent
                                                // use the existing one if it finds it, or Now() if not.
                                                // Actually, let's just pass None here as this path is for RE-SPAWNING after a headless run failure.
                                                (config_guard.clone(), None)
                                            };
                                            let mut cfg = cfg;
                                            cfg.is_off = false;
                                            log_debug(&format!("[Wardian] Restoring agent {} to Online state after headless run", agent_id));
                                            if let Some(new_agent) = record_restore_spawn_result(
                                                agent_id,
                                                crate::manager::spawn_agent(
                                                    app.clone(),
                                                    cfg,
                                                    false,
                                                    born,
                                                )
                                                .await,
                                                &mut node_error,
                                            ) {
                                                let new_stdin_tx = new_agent.stdin_tx.clone();
                                                let (state_snapshot, displaced_agent) = {
                                                    let mut agents_map = state.agents.lock().await;
                                                    if let Ok(mut senders) =
                                                        state.input_senders.write()
                                                    {
                                                        match new_stdin_tx {
                                                            Some(tx) => {
                                                                senders.insert(
                                                                    agent_id.to_string(),
                                                                    tx,
                                                                );
                                                            }
                                                            None => {
                                                                senders.remove(agent_id);
                                                            }
                                                        }
                                                    }
                                                    let displaced_agent = agents_map
                                                        .insert(agent_id.to_string(), new_agent);
                                                    let order = state.agent_order.lock().await;
                                                    (
                                                        crate::manager::state_configs_snapshot(
                                                            &agents_map,
                                                            &order,
                                                        ),
                                                        displaced_agent,
                                                    )
                                                };
                                                if let Some(mut displaced) = displaced_agent {
                                                    crate::manager::terminate_active_agent_process(
                                                        &mut displaced,
                                                    );
                                                }
                                                crate::manager::save_state_snapshot(
                                                    &app,
                                                    &state_snapshot,
                                                );
                                                let _ = app.emit("agents-updated", ());
                                            }
                                        }
                                    } else {
                                        // Agent was already off — reset from Headless back to Off
                                        let agents_map = state.agents.lock().await;
                                        if let Some(agent) = agents_map.get(agent_id) {
                                            if let Ok(mut status) = agent.current_status.lock() {
                                                *status = "Off".to_string();
                                            }
                                        }
                                        let _ = app.emit("agents-updated", ());
                                    }
                                } else {
                                    // --- PTY TEXT MODE (With Headless Fallback) ---
                                    let state = app.state::<crate::state::AppState>();
                                    let sender = {
                                        let senders = state
                                            .input_senders
                                            .read()
                                            .map_err(|e| e.to_string())
                                            .unwrap();
                                        senders.get(agent_id).cloned()
                                    };

                                    if let Some(tx) = sender {
                                        // Agent is ONLINE: Use provider-native automation when PTY submit
                                        // is unreliable, otherwise inject through the live terminal.
                                        {
                                            let agents = state.agents.lock().await;
                                            if let Some(agent) = agents.get(agent_id) {
                                                if let Ok(mut buf) = agent.output_buffer.lock() {
                                                    buf.clear();
                                                }
                                            }
                                        }

                                        if provider_name == "opencode" {
                                            let run_result = run_with_optional_timeout(
                                                timeout_ms,
                                                "Agent",
                                                crate::manager::run_headless_with_options(
                                                    crate::manager::HeadlessRunOptions {
                                                        cwd: &cwd,
                                                        prompt: &prompt,
                                                        wardian_session_id: &exec_ctx
                                                            .execution_session_id,
                                                        resume_session: exec_ctx
                                                            .resume_session
                                                            .as_deref(),
                                                        output_format,
                                                        provider_name: &provider_name,
                                                        config_override: Some(&exec_ctx.config),
                                                    },
                                                ),
                                            )
                                            .await;

                                            match run_result {
                                                Ok(data) => {
                                                    output_payload =
                                                        flatten_headless_response(data);
                                                }
                                                Err(e) => {
                                                    log_debug(&format!(
                                                "[Wardian] OpenCode online automation failed: {}",
                                                e
                                            ));
                                                    node_error = Some(e);
                                                }
                                            }
                                        } else {
                                            let (completion_tx, mut completion_rx) =
                                                tokio::sync::mpsc::channel::<Value>(1);
                                            let agent_id_clone = agent_id.to_string();

                                            let handler_id =
                                                app.listen_any(
                                                    "agent-turn-completed",
                                                    move |event| {
                                                        if let Ok(parsed) =
                                                            serde_json::from_str::<Value>(
                                                                event.payload(),
                                                            )
                                                        {
                                                            if parsed
                                                                .get("session_id")
                                                                .and_then(|v| v.as_str())
                                                                == Some(&agent_id_clone)
                                                            {
                                                                let _ =
                                                                    completion_tx.try_send(parsed);
                                                            }
                                                        }
                                                    },
                                                );

                                            if let Err(err) =
                                                crate::utils::terminal_input::submit_prompt_via_sender(
                                                    &tx,
                                                    &prompt,
                                                    &provider_name,
                                                )
                                                .await
                                            {
                                                node_error = Some(err);
                                            }

                                            if node_error.is_none() {
                                                let completion = match timeout_ms {
                                                    Some(ms) => match tokio::time::timeout(
                                                        std::time::Duration::from_millis(ms),
                                                        completion_rx.recv(),
                                                    )
                                                    .await
                                                    {
                                                        Ok(value) => Ok(value),
                                                        Err(_) => {
                                                            Err(format!("Agent timeout ({}ms)", ms))
                                                        }
                                                    },
                                                    None => Ok(completion_rx.recv().await),
                                                };

                                                match completion {
                                                    Ok(_) => {
                                                        let agents = state.agents.lock().await;
                                                        if let Some(agent) = agents.get(agent_id) {
                                                            if let Ok(buf) =
                                                                agent.output_buffer.lock()
                                                            {
                                                                output_payload = serde_json::json!({ "text": buf.clone() });
                                                            }
                                                        }
                                                    }
                                                    Err(err) => node_error = Some(err),
                                                }
                                            }
                                            app.unlisten(handler_id);
                                        }
                                    } else {
                                        // Agent is OFFLINE: Fallback to Headless Execution
                                        log_debug(&format!("[Wardian] Agent {} offline, falling back to headless execution with format: {}", agent_id, output_format));
                                        // Set Headless status for UI indicator
                                        {
                                            let agents_map = state.agents.lock().await;
                                            if let Some(agent) = agents_map.get(agent_id) {
                                                if let Ok(mut status) = agent.current_status.lock()
                                                {
                                                    *status = "Headless".to_string();
                                                }
                                            }
                                        }
                                        let _ = app.emit("agents-updated", ());
                                        let run_result = run_with_optional_timeout(
                                            timeout_ms,
                                            "Agent",
                                            crate::manager::run_headless_with_options(
                                                crate::manager::HeadlessRunOptions {
                                                    cwd: &cwd,
                                                    prompt: &prompt,
                                                    wardian_session_id: &exec_ctx
                                                        .execution_session_id,
                                                    resume_session: exec_ctx
                                                        .resume_session
                                                        .as_deref(),
                                                    output_format,
                                                    provider_name: &provider_name,
                                                    config_override: Some(&exec_ctx.config),
                                                },
                                            ),
                                        )
                                        .await;

                                        match run_result {
                                            Ok(data) => {
                                                output_payload = flatten_headless_response(data);
                                            }
                                            Err(e) => {
                                                log_debug(&format!(
                                                    "[Wardian] Headless fallback failed: {}",
                                                    e
                                                ));
                                                node_error = Some(e);
                                            }
                                        }
                                        // Reset from Headless back to Off
                                        {
                                            let agents_map = state.agents.lock().await;
                                            if let Some(agent) = agents_map.get(agent_id) {
                                                if let Ok(mut status) = agent.current_status.lock()
                                                {
                                                    *status = "Off".to_string();
                                                }
                                            }
                                        }
                                        let _ = app.emit("agents-updated", ());
                                    }
                                }
                            }
                        }
                    }
                    "communication" | "notify" => {
                        log_debug(&format!(
                            "[Wardian] Communication node {} starting",
                            node.id
                        ));
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

                            output_payload =
                                serde_json::json!({ "delivered": true, "type": "notification" });
                        } else if let Some(prompt) =
                            node.config.get("prompt").and_then(|v| v.as_str())
                        {
                            log_debug(&format!("[Wardian] Broadcast prompt: {}", prompt));
                            // TODO: Implement actual broadcast to all agents
                            output_payload =
                                serde_json::json!({ "delivered": true, "type": "broadcast" });
                        } else {
                            log_debug("[Wardian] Communication node missing message or prompt");
                            output_payload = serde_json::json!({ "delivered": false, "error": "Missing message or prompt" });
                        }
                    }
                    "logic" => {
                        let condition = node
                            .config
                            .get("condition")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        let is_true = evaluate_logic(condition, &registry);

                        result_ports = if is_true {
                            vec!["on_true".to_string()]
                        } else {
                            vec!["on_false".to_string()]
                        };
                        output_payload =
                            serde_json::json!({ "condition_met": is_true, "condition": condition });
                        log_debug(&format!(
                            "[Wardian] Logic node {} evaluated {} to {}",
                            node.id, condition, is_true
                        ));
                    }
                    "command" => {
                        log_debug(&format!(
                            "[Wardian] Shell Command node {} starting",
                            node.id
                        ));
                        let cmd_str = node
                            .config
                            .get("cmd")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        let interpolated_cmd = interpolate_string(cmd_str, &registry);
                        let cwd = resolve_cwd(&node.config, "");
                        let env = node.config.get("env");
                        let timeout_ms = parse_optional_timeout_ms(&node.config);

                        match resolve_command_node_launch(&interpolated_cmd, build_shell_command) {
                            Ok(spec) => match run_command_headless(
                                &spec.executable,
                                spec.args,
                                &cwd,
                                env,
                                timeout_ms,
                            )
                            .await
                            {
                                Ok(res) => {
                                    let exit_code =
                                        res.get("exit_code").and_then(|v| v.as_i64()).unwrap_or(0);
                                    if exit_code != 0 {
                                        node_error = Some(format!(
                                            "Exit code {}: {}",
                                            exit_code,
                                            res.get("stderr")
                                                .and_then(|v| v.as_str())
                                                .unwrap_or("Unknown error")
                                        ));
                                    }
                                    output_payload = res;
                                }
                                Err(e) => node_error = Some(e),
                            },
                            Err(e) => node_error = Some(e),
                        }
                    }
                    "script" => {
                        log_debug(&format!("[Wardian] Script node {} starting", node.id));
                        let runtime = node
                            .config
                            .get("runtime")
                            .and_then(|v| v.as_str())
                            .unwrap_or("python");
                        let file_path_str = node
                            .config
                            .get("file_path")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        let args_str = node
                            .config
                            .get("args")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        let interpolated_args = interpolate_string(args_str, &registry);
                        let env = node.config.get("env");
                        let timeout_ms = parse_optional_timeout_ms(&node.config);
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
                                        _ => "python",
                                    };

                                    let mut args =
                                        vec![validated_path.to_string_lossy().to_string()];
                                    if !interpolated_args.is_empty() {
                                        if let Some(parsed_args) = shlex::split(&interpolated_args)
                                        {
                                            args.extend(parsed_args);
                                        } else {
                                            // Fallback to naive split if shlex fails (unlikely)
                                            args.extend(
                                                interpolated_args
                                                    .split_whitespace()
                                                    .map(|s| s.to_string()),
                                            );
                                        }
                                    }

                                    match run_command_headless(
                                        executable, args, &cwd, env, timeout_ms,
                                    )
                                    .await
                                    {
                                        Ok(res) => {
                                            let exit_code = res
                                                .get("exit_code")
                                                .and_then(|v| v.as_i64())
                                                .unwrap_or(0);
                                            if exit_code != 0 {
                                                node_error = Some(format!(
                                                    "Exit code {}: {}",
                                                    exit_code,
                                                    res.get("stderr")
                                                        .and_then(|v| v.as_str())
                                                        .unwrap_or("Unknown error")
                                                ));
                                            }
                                            output_payload = res;
                                        }
                                        Err(e) => node_error = Some(e),
                                    }
                                }
                                Err(e) => node_error = Some(format!("Security Violation: {}", e)),
                            }
                        }
                    }
                    "loop" => {
                        let mode = node
                            .config
                            .get("mode")
                            .and_then(|v| v.as_str())
                            .unwrap_or("count");
                        let max_local = node
                            .config
                            .get("max_iterations")
                            .and_then(|v| v.as_str())
                            .and_then(|v| v.parse::<u32>().ok())
                            .unwrap_or(10);

                        let iterator_key = node
                            .config
                            .get("iterator_name")
                            .and_then(|v| v.as_str())
                            .unwrap_or("i");

                        // State Namespace: nodes.[id].output.[iterator_key]
                        let current_i = registry
                            .get(&node.id)
                            .and_then(|v| v.get(iterator_key))
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0) as u32;

                        let should_continue = if mode == "count" {
                            current_i < max_local
                        } else {
                            let condition = node
                                .config
                                .get("condition")
                                .and_then(|v| v.as_str())
                                .unwrap_or("false");
                            evaluate_logic(condition, &registry)
                        };

                        if should_continue {
                            result_ports = vec!["body".to_string()];
                            output_payload = serde_json::json!({ iterator_key: current_i + 1, "status": "pulsing_body" });
                        } else {
                            result_ports = vec!["done".to_string()];
                            output_payload =
                                serde_json::json!({ iterator_key: 0, "status": "pulsing_done" });
                        }
                        log_debug(&format!(
                            "[Wardian] Loop node {}: i={} -> continue={}",
                            node.id, current_i, should_continue
                        ));
                    }
                    _ => {
                        log_debug(&format!("[Wardian] Unknown node type: {}", node.r#type));
                        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                        output_payload = serde_json::json!({ "status": "unknown_type" });
                    }
                }
            } // end if node_error.is_none()

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
                status: if node_error.is_some() {
                    "failed".to_string()
                } else {
                    "completed".to_string()
                },
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
                    .open(log_path.with_extension("log"))
                {
                    let _ = writeln!(file, "{}", json_inner);
                }
            }

            // HALT if error
            if node_error.is_some() {
                break;
            }

            // Record pulsed ports: Increment counter in pulse map
            for port in &result_ports {
                *pulsed_ports
                    .entry((current_node_id.clone(), port.clone()))
                    .or_insert(0) += 1;
            }

            // Queue downstream nodes
            for candidate in &wf.nodes {
                if let Some(deps) = &candidate.dependencies {
                    if deps.iter().any(|d| d.node_id == current_node_id) {
                        queue.push_back(candidate.id.clone());
                        total_enqueued += 1;
                    }
                }
            }
        }

        // Emit workflow completion status
        let had_error = trace.iter().any(|e| e.status == "failed");
        let final_status = if had_error { "failed" } else { "completed" };
        let final_error = trace.iter().find_map(|event| event.error.clone());
        if let Some(scheduled_run_id) = scheduled_run_id_for_completion.as_deref() {
            let mut runs = load_scheduled_runs();
            record_scheduled_run_outcome(
                &mut runs,
                scheduled_run_id,
                final_status,
                final_error.clone(),
                Utc::now().timestamp_millis() as u64,
            );
            if save_scheduled_runs(&runs).is_ok() {
                let _ = app.emit("scheduled-runs-updated", ());
            }
        }
        let _ = app.emit(
            "workflow-status-updated",
            serde_json::json!({
                "workflow_id": wf.id,
                "run_instance_id": run_instance_id_for_completion,
                "scheduled_run_id": scheduled_run_id_for_completion,
                "status": final_status,
                "error": final_error,
            }),
        );

        // Save the execution log
        if let Ok(json) = serde_json::to_string_pretty(&trace) {
            let _ = fs::write(log_path, json);
        }

        let state = app.state::<crate::state::AppState>();
        let mut runs = state.workflow_runs.lock().await;
        runs.remove(&run_instance_id_for_cleanup);
    });

    // Store handle for cancellation
    {
        let state = app_for_handle.state::<crate::state::AppState>();
        let mut runs = state.workflow_runs.lock().await;
        let handles = runs.entry(run_instance_id_for_handle).or_default();
        handles.push(handle);
    }

    Ok(())
}
