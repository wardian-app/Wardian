use crate::{automation::runs, state::AppState};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;
use tauri::{AppHandle, State};
use wardian_core::control::AutomationRunResponse;
use wardian_core::engine::store::{read_checkpoint, read_events};
use wardian_core::engine::RunStatus;
use wardian_core::limits::{MAX_AUTOMATION_BLUEPRINTS, MAX_AUTOMATION_RUNS};
use wardian_core::models::{
    AutomationAssignments, AutomationRoleAssignment, AutomationSchedule, InvocationKind,
};

#[tauri::command]
pub async fn session_close_invoker_list(
) -> Result<Vec<wardian_core::session_close::AutomationSessionCloseInvoker>, String> {
    Ok(wardian_core::session_close::load_invokers())
}

#[tauri::command]
pub async fn session_close_invoker_save(
    invoker: wardian_core::session_close::AutomationSessionCloseInvoker,
) -> Result<wardian_core::session_close::AutomationSessionCloseInvoker, String> {
    if invoker.id.trim().is_empty()
        || invoker.name.trim().is_empty()
        || invoker.blueprint_id.trim().is_empty()
    {
        return Err("session-close invoker id, name, and blueprint_id are required".into());
    }
    wardian_core::automation::resolve_blueprint_path(&invoker.blueprint_id)
        .ok_or_else(|| format!("automation blueprint not found: {}", invoker.blueprint_id))?;
    wardian_core::session_close::mutate_invokers(|invokers| {
        if let Some(existing) = invokers.iter_mut().find(|item| item.id == invoker.id) {
            *existing = invoker.clone();
        } else {
            invokers.push(invoker.clone());
        }
        Ok(())
    })
    .map_err(|error| error.to_string())?;
    Ok(invoker)
}

#[tauri::command]
pub async fn session_close_invoker_delete(id: String) -> Result<(), String> {
    wardian_core::session_close::mutate_invokers(|invokers| {
        let before = invokers.len();
        invokers.retain(|item| item.id != id);
        if invokers.len() == before {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("session-close invoker not found: {id}"),
            ));
        }
        Ok(())
    })
    .map_err(|error| error.to_string())
}
use wardian_core::automation::{self, Blueprint};
use wardian_core::schedule::{
    compute_next_run, load_schedules, resolve_workspace_path, save_schedules,
    validate_schedule_definition,
};

const AUTOMATION_EVENTS_FILE: &str = "events.jsonl";

/// Parse + validate a blueprint `.md` at `path`. Returns the structured graph
/// and any diagnostics (parse errors surface as an Err string).
#[tauri::command]
pub fn automation_parse(path: String) -> Result<serde_json::Value, String> {
    let blueprint =
        automation::parse_file(std::path::Path::new(&path)).map_err(|e| e.to_string())?;
    let report = automation::validate(&blueprint);
    Ok(serde_json::json!({ "blueprint": blueprint, "diagnostics": report.diagnostics }))
}

/// Validate an in-memory blueprint (debounced from the builder on edit).
#[tauri::command]
pub fn automation_validate(blueprint: Blueprint) -> Result<serde_json::Value, String> {
    let report = automation::validate(&blueprint);
    Ok(serde_json::json!({ "ok": report.is_valid(), "diagnostics": report.diagnostics }))
}

/// Normalize + serialize + write a blueprint to `path`. Refuses to write while
/// it has validation errors (returns them instead).
#[tauri::command]
pub fn automation_write(
    path: String,
    mut blueprint: Blueprint,
) -> Result<serde_json::Value, String> {
    automation::normalize(&mut blueprint);
    let report = automation::validate(&blueprint);
    if !report.is_valid() {
        return Ok(serde_json::json!({ "written": false, "diagnostics": report.diagnostics }));
    }
    let text = automation::to_string(&blueprint).map_err(|e| e.to_string())?;
    std::fs::write(&path, text).map_err(|e| e.to_string())?;
    Ok(serde_json::json!({ "written": true, "diagnostics": [] }))
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct AutomationBlueprintListResult {
    pub blueprints: Vec<serde_json::Value>,
    pub truncated: bool,
    pub next_offset: Option<usize>,
}

/// List at most 500 blueprint `.md` files under
/// `<wardian-home>/library/automations`.
#[tauri::command]
pub fn automation_list_blueprints(
    offset: Option<usize>,
) -> Result<AutomationBlueprintListResult, String> {
    let offset = offset.unwrap_or(0);
    let home = wardian_core::paths::wardian_home().ok_or("no wardian home")?;
    let dir = home.join("library").join("automations");
    let mut out = Vec::new();
    let mut truncated = false;
    if dir.exists() {
        let (entries, files_truncated) =
            automation::list_blueprint_files_page(&dir, offset, MAX_AUTOMATION_BLUEPRINTS);
        truncated = files_truncated;
        for entry in entries {
            if let Ok(bp) = automation::parse_file(&entry) {
                out.push(serde_json::json!({ "id": bp.id, "name": bp.name, "path": entry.to_string_lossy() }));
            }
        }
    }
    Ok(AutomationBlueprintListResult {
        blueprints: out,
        truncated,
        next_offset: truncated.then_some(offset + MAX_AUTOMATION_BLUEPRINTS),
    })
}

/// List the 200 newest automation runs under
/// `<home>/logs/automations/<id>/<run_id>/`.

#[derive(Debug, Clone, serde::Serialize)]
pub struct AutomationRunListResult {
    pub runs: Vec<serde_json::Value>,
    pub truncated: bool,
    pub next_offset: Option<usize>,
}

#[tauri::command]
pub async fn automation_list_runs(
    offset: Option<usize>,
) -> Result<AutomationRunListResult, String> {
    tokio::task::spawn_blocking(move || automation_list_runs_blocking(offset))
        .await
        .map_err(|error| format!("automation run listing task failed: {error}"))?
}

/// Execute the filesystem-heavy run listing away from Tauri's event-loop
/// thread. The desktop polls this command while rendering the workbench, and
/// a long-lived run history must never turn a routine refresh into a UI stall.
pub(crate) fn automation_list_runs_blocking(
    offset: Option<usize>,
) -> Result<AutomationRunListResult, String> {
    let root = wardian_core::paths::automation_runs_dir().ok_or("no wardian home")?;
    automation_list_runs_page_from_root(&root, resolve_blueprint_path, offset.unwrap_or(0))
}

pub(crate) fn automation_list_runs_matching_blocking<F>(
    offset: usize,
    include: F,
) -> Result<AutomationRunListResult, String>
where
    F: FnMut(&serde_json::Value) -> bool,
{
    let root = wardian_core::paths::automation_runs_dir().ok_or("no wardian home")?;
    automation_list_runs_page_from_root_matching(&root, resolve_blueprint_path, offset, include)
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct AutomationRunFileWatermark {
    len: u64,
    modified: Option<std::time::SystemTime>,
    created: Option<std::time::SystemTime>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct AutomationRunWatermark {
    state: Option<AutomationRunFileWatermark>,
}

#[derive(Clone)]
struct CachedAutomationRunSummary {
    watermark: AutomationRunWatermark,
    summary: serde_json::Value,
}

#[derive(Default)]
struct CachedAutomationRunRoot {
    runs: HashMap<PathBuf, CachedAutomationRunSummary>,
    scanned_at: Option<Instant>,
}

type AutomationRunSummaryRoots = HashMap<PathBuf, CachedAutomationRunRoot>;

static AUTOMATION_RUN_SUMMARY_CACHE: OnceLock<Mutex<AutomationRunSummaryRoots>> = OnceLock::new();

fn automation_run_file_watermark(path: &Path) -> Option<AutomationRunFileWatermark> {
    let metadata = std::fs::metadata(path).ok()?;
    Some(AutomationRunFileWatermark {
        len: metadata.len(),
        modified: metadata.modified().ok(),
        created: metadata.created().ok(),
    })
}

fn automation_run_watermark(dir: &Path) -> AutomationRunWatermark {
    AutomationRunWatermark {
        state: automation_run_file_watermark(&dir.join("state.json")),
    }
}

fn automation_list_runs_page_from_root<F>(
    root: &Path,
    resolve_blueprint_path: F,
    offset: usize,
) -> Result<AutomationRunListResult, String>
where
    F: FnMut(&str) -> Option<PathBuf>,
{
    automation_list_runs_page_from_root_matching(root, resolve_blueprint_path, offset, |_| true)
}

fn automation_list_runs_page_from_root_matching<F, I>(
    root: &Path,
    mut resolve_blueprint_path: F,
    offset: usize,
    mut include: I,
) -> Result<AutomationRunListResult, String>
where
    F: FnMut(&str) -> Option<PathBuf>,
    I: FnMut(&serde_json::Value) -> bool,
{
    let requested_at = Instant::now();
    let cache = AUTOMATION_RUN_SUMMARY_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut cache = cache
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let root_cache = cache.entry(root.to_path_buf()).or_default();
    let needs_scan = root_cache
        .scanned_at
        .is_none_or(|scanned_at| scanned_at < requested_at);
    if needs_scan {
        let mut blueprint_paths: HashMap<String, Option<PathBuf>> = HashMap::new();
        let mut observed_runs = HashSet::new();
        if root.exists() {
            for bp in std::fs::read_dir(root)
                .map_err(|e| e.to_string())?
                .flatten()
            {
                if !bp.file_type().is_ok_and(|file_type| file_type.is_dir()) {
                    continue;
                }
                for run in std::fs::read_dir(bp.path())
                    .map_err(|e| e.to_string())?
                    .flatten()
                {
                    let dir = run.path();
                    if !run.file_type().is_ok_and(|file_type| file_type.is_dir()) {
                        continue;
                    }
                    observed_runs.insert(dir.clone());
                    let cached_is_terminal = root_cache.runs.get(&dir).is_some_and(|cached| {
                        matches!(
                            cached
                                .summary
                                .get("status")
                                .and_then(serde_json::Value::as_str),
                            Some("completed" | "failed")
                        )
                    });
                    let watermark = (!cached_is_terminal).then(|| automation_run_watermark(&dir));
                    if watermark.as_ref().is_some_and(|watermark| {
                        !matches!(
                            root_cache.runs.get(&dir),
                            Some(cached) if &cached.watermark == watermark
                        )
                    }) {
                        let Some(state) = read_checkpoint(&dir).ok().flatten() else {
                            root_cache.runs.remove(&dir);
                            continue;
                        };
                        let blueprint_path = blueprint_paths
                            .entry(state.blueprint_id.clone())
                            .or_insert_with(|| resolve_blueprint_path(&state.blueprint_id));
                        let summary =
                            run_summary_from_state(&dir, state, blueprint_path.as_deref());
                        root_cache.runs.insert(
                            dir.clone(),
                            CachedAutomationRunSummary {
                                watermark: watermark.expect("non-terminal runs have a watermark"),
                                summary,
                            },
                        );
                    }

                    let Some(cached) = root_cache.runs.get_mut(&dir) else {
                        continue;
                    };
                    if let Some(blueprint_id) = cached
                        .summary
                        .get("blueprint_id")
                        .and_then(serde_json::Value::as_str)
                    {
                        let blueprint_path = blueprint_paths
                            .entry(blueprint_id.to_string())
                            .or_insert_with(|| resolve_blueprint_path(blueprint_id));
                        cached.summary["blueprint_path"] = blueprint_path
                            .as_ref()
                            .map(|path| {
                                serde_json::Value::String(path.to_string_lossy().into_owned())
                            })
                            .unwrap_or(serde_json::Value::Null);
                    }
                }
            }
        }
        root_cache.runs.retain(|dir, _| observed_runs.contains(dir));
        root_cache.scanned_at = Some(Instant::now());
    }

    let mut retained = root_cache
        .runs
        .values()
        .filter(|cached| include(&cached.summary))
        .map(|cached| cached.summary.clone())
        .collect::<Vec<_>>();
    retained.sort_by(compare_run_summaries);
    let page_end = offset.saturating_add(MAX_AUTOMATION_RUNS);
    let truncated = retained.len() > page_end;
    let mut runs = retained
        .into_iter()
        .skip(offset)
        .take(MAX_AUTOMATION_RUNS)
        .collect::<Vec<_>>();
    let worker_attention =
        wardian_core::temporary_workers::attention_counts_by_run().unwrap_or_default();
    for run in &mut runs {
        let blueprint_id = run
            .get("blueprint_id")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let run_id = run
            .get("run_id")
            .and_then(Value::as_str)
            .unwrap_or_default();
        run["worker_attention_count"] = serde_json::json!(worker_attention
            .get(&(blueprint_id.to_string(), run_id.to_string()))
            .copied()
            .unwrap_or(0));
    }
    Ok(AutomationRunListResult {
        runs,
        truncated,
        next_offset: truncated.then_some(page_end),
    })
}

fn compare_run_summaries(a: &serde_json::Value, b: &serde_json::Value) -> std::cmp::Ordering {
    let a_updated = a.get("updated_at").and_then(Value::as_str).unwrap_or("");
    let b_updated = b.get("updated_at").and_then(Value::as_str).unwrap_or("");
    b_updated.cmp(a_updated).then_with(|| {
        let a_run = a.get("run_id").and_then(Value::as_str).unwrap_or("");
        let b_run = b.get("run_id").and_then(Value::as_str).unwrap_or("");
        b_run.cmp(a_run)
    })
}

#[cfg(test)]
fn summarize_run_dir(dir: &Path) -> Option<serde_json::Value> {
    let state = read_checkpoint(dir).ok().flatten()?;
    let blueprint_path = resolve_blueprint_path(&state.blueprint_id);
    Some(run_summary_from_state(
        dir,
        state,
        blueprint_path.as_deref(),
    ))
}

fn run_summary_from_state(
    dir: &Path,
    state: wardian_core::engine::RunState,
    blueprint_path: Option<&Path>,
) -> serde_json::Value {
    let (started_at, updated_at) = event_log_timestamp_bounds(dir);
    let completed_at = match state.status {
        RunStatus::Completed => updated_at.clone(),
        RunStatus::Running | RunStatus::AwaitingApproval | RunStatus::Failed => None,
    };
    let blueprint_path = blueprint_path.map(|path| path.to_string_lossy().to_string());
    let invocation = runs::read_run_invocation(dir).ok().flatten();
    let schedule_id = invocation
        .as_ref()
        .and_then(|invocation| invocation.schedule_id.clone());
    // Carried alongside `schedule_id` so the monitor can collapse a busy
    // listener's runs the way it already collapses a schedule's; without it a
    // file listener would flood the run list.
    let listener_id = invocation.and_then(|invocation| invocation.listener_id);

    serde_json::json!({
        "run_id": state.run_id,
        "blueprint_id": state.blueprint_id,
        "schedule_id": schedule_id,
        "listener_id": listener_id,
        "status": state.status,
        "node_count": state.nodes.len(),
        "failure": state.failure,
        "path": dir.to_string_lossy(),
        "blueprint_path": blueprint_path,
        "started_at": started_at,
        "updated_at": updated_at,
        "completed_at": completed_at,
    })
}

pub(crate) fn event_log_timestamp_bounds(dir: &Path) -> (Option<String>, Option<String>) {
    let path = dir.join(AUTOMATION_EVENTS_FILE);
    if !path.exists() {
        return (None, None);
    }

    let started_at = first_event_timestamp(&path);
    let updated_at = last_event_timestamp(&path).or_else(|| started_at.clone());
    (started_at, updated_at)
}

fn first_event_timestamp(path: &Path) -> Option<String> {
    let file = File::open(path).ok()?;
    for line in BufReader::new(file).lines() {
        let line = line.ok()?;
        if let Some(timestamp) = timestamp_from_event_line(&line) {
            return Some(timestamp);
        }
    }
    None
}

fn last_event_timestamp(path: &Path) -> Option<String> {
    let mut file = File::open(path).ok()?;
    let mut remaining = file.metadata().ok()?.len();
    let mut tail = Vec::new();
    let mut chunk = [0_u8; 8192];

    while remaining > 0 {
        let read_len = remaining.min(chunk.len() as u64) as usize;
        remaining -= read_len as u64;
        file.seek(SeekFrom::Start(remaining)).ok()?;
        file.read_exact(&mut chunk[..read_len]).ok()?;

        let mut combined = Vec::with_capacity(read_len + tail.len());
        combined.extend_from_slice(&chunk[..read_len]);
        combined.extend_from_slice(&tail);
        tail = combined;

        while matches!(tail.last(), Some(b'\n' | b'\r' | b' ' | b'\t')) {
            tail.pop();
        }
        if let Some(newline_index) = tail.iter().rposition(|byte| *byte == b'\n') {
            let line = String::from_utf8_lossy(&tail[newline_index + 1..]);
            if let Some(timestamp) = timestamp_from_event_line(line.trim()) {
                return Some(timestamp);
            }
            tail.truncate(newline_index);
        }
    }

    let line = String::from_utf8_lossy(&tail);
    timestamp_from_event_line(line.trim())
}

fn timestamp_from_event_line(line: &str) -> Option<String> {
    let value = serde_json::from_str::<Value>(line).ok()?;
    value.get("ts").and_then(Value::as_str).map(str::to_string)
}

/// Read one run: its RunState checkpoint, full event trace, and optional blueprint.
#[tauri::command]
pub fn automation_read_run(
    blueprint_id: String,
    run_id: String,
) -> Result<serde_json::Value, String> {
    let dir =
        wardian_core::paths::automation_run_dir(&blueprint_id, &run_id).ok_or("no wardian home")?;
    let state = read_checkpoint(&dir).map_err(|e| e.to_string())?;
    let events = read_events(&dir).map_err(|e| e.to_string())?;
    let blueprint = if let Some(snapshot) =
        wardian_core::engine::store::read_blueprint_snapshot(&dir).map_err(|e| e.to_string())?
    {
        Some(serde_json::to_value(snapshot).map_err(|e| e.to_string())?)
    } else {
        resolve_blueprint(&blueprint_id)
    };
    let blueprint_path =
        resolve_blueprint_path(&blueprint_id).map(|path| path.to_string_lossy().to_string());
    let workers = wardian_core::temporary_workers::list_for_run(&blueprint_id, &run_id)
        .map_err(|error| error.to_string())?;
    let worker_telemetry =
        wardian_core::temporary_workers::telemetry_for_run(&blueprint_id, &run_id)
            .map_err(|error| error.to_string())?;

    Ok(serde_json::json!({
        "state": state,
        "events": events,
        "blueprint": blueprint,
        "blueprint_path": blueprint_path,
        "workers": workers,
        "worker_telemetry": worker_telemetry
    }))
}

/// Compact child-worker counts for owner-linked agent surfaces. Temporary
/// workers remain outside the permanent roster and watchlist.
#[tauri::command]
pub fn temporary_worker_root_summaries() -> Result<serde_json::Value, String> {
    let summaries = wardian_core::temporary_workers::root_agent_summaries()
        .map_err(|error| error.to_string())?;
    Ok(serde_json::json!({ "summaries": summaries }))
}

/// Detailed observe-only child-worker evidence for one permanent root agent.
/// This does not grant follow-up, interruption, or resume capabilities.
#[tauri::command]
pub fn temporary_worker_root_details(root_agent_id: String) -> Result<serde_json::Value, String> {
    if root_agent_id.trim().is_empty() {
        return Err("root_agent_id is required".to_string());
    }
    let workers = wardian_core::temporary_workers::list_for_root(&root_agent_id)
        .map_err(|error| error.to_string())?;
    let worker_telemetry = wardian_core::temporary_workers::telemetry_for_root(&root_agent_id)
        .map_err(|error| error.to_string())?;
    Ok(serde_json::json!({
        "root_agent_id": root_agent_id,
        "workers": workers,
        "worker_telemetry": worker_telemetry
    }))
}

/// Launch an automation blueprint run and write durable run artifacts. The default
/// live path routes execution through the running app; CLI mock execution is
/// reserved for automation-engine tests.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn automation_run(
    state: State<'_, AppState>,
    app: AppHandle,
    path: String,
    provider: Option<String>,
    workspace: Option<String>,
    input: Option<Value>,
    bindings: Option<HashMap<String, String>>,
    assignments: Option<AutomationAssignments>,
) -> Result<AutomationRunResponse, String> {
    automation_run_impl(
        state,
        app,
        path,
        provider,
        workspace,
        input,
        bindings,
        assignments,
        false,
        None,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub async fn automation_run_from_control(
    state: State<'_, AppState>,
    app: AppHandle,
    path: String,
    provider: Option<String>,
    workspace: Option<String>,
    input: Option<Value>,
    bindings: Option<HashMap<String, String>>,
    assignments: Option<AutomationAssignments>,
    memory_principal: Option<String>,
) -> Result<AutomationRunResponse, String> {
    automation_run_impl(
        state,
        app,
        path,
        provider,
        workspace,
        input,
        bindings,
        assignments,
        true,
        memory_principal,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn automation_run_impl(
    state: State<'_, AppState>,
    app: AppHandle,
    path: String,
    provider: Option<String>,
    workspace: Option<String>,
    input: Option<Value>,
    bindings: Option<HashMap<String, String>>,
    assignments: Option<AutomationAssignments>,
    control_origin: bool,
    memory_principal: Option<String>,
) -> Result<AutomationRunResponse, String> {
    let blueprint = if control_origin {
        parse_control_automation_blueprint(&path)?
    } else {
        wardian_core::automation::parse_file(std::path::Path::new(&path))
            .map_err(|e| e.to_string())?
    };
    let report = wardian_core::automation::validate(&blueprint);
    if !report.is_valid() {
        return Ok(AutomationRunResponse::validation_failed(
            "live",
            serde_json::to_value(report.diagnostics).map_err(|error| error.to_string())?,
        ));
    }

    let run_id = wardian_core::engine::driver::new_run_id();
    let run_root =
        wardian_core::paths::automation_run_dir(&blueprint.id, &run_id).ok_or_else(|| {
            format!(
                "invalid automation run path components for blueprint id `{}` and run id `{run_id}`",
                blueprint.id
            )
        })?;
    let provider = provider.unwrap_or_else(|| "codex".to_string());
    let workspace = workspace
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| run_root.clone());
    let input = input.unwrap_or_else(|| serde_json::json!({}));
    let bindings = bindings.unwrap_or_default();
    let assignments = wardian_core::automation::assignment::normalize_assignments(
        assignments,
        &bindings,
        InvocationKind::Manual,
    );
    let agent_catalog = runs::agent_catalog_from_state_with_assignments(
        &state,
        &bindings,
        &assignments,
        &workspace,
        &provider,
    )
    .await;
    let blueprint_for_run = blueprint.clone();
    let blueprint_for_inbox = blueprint.clone();
    let run_root_for_run = run_root.clone();
    let run_root_for_inbox = run_root.clone();
    let app_for_inbox = app.clone();
    let run_state = runs::prepare_new_run_with_assignments_and_memory_principal(
        &blueprint,
        &run_id,
        &run_root,
        &workspace,
        &provider,
        &bindings,
        &assignments,
        input,
        memory_principal.clone(),
    )?;

    tokio::spawn(async move {
        if let Err(error) = runs::drive_started_run_with_catalog_assignments_and_memory_principal(
            Some(app),
            blueprint_for_run,
            run_state,
            run_root_for_run,
            workspace,
            provider,
            bindings,
            assignments,
            agent_catalog,
            memory_principal,
        )
        .await
        {
            crate::utils::logging::log_debug(&format!("[automation] run failed: {error}"));
        }
        runs::emit_automation_inbox_update(
            &app_for_inbox,
            &blueprint_for_inbox,
            &run_root_for_inbox,
        );
    });

    Ok(AutomationRunResponse::started(
        "live",
        run_id,
        blueprint.id,
        run_root.to_string_lossy().to_string(),
    ))
}

fn parse_control_automation_blueprint(path: &str) -> Result<Blueprint, String> {
    let requested = std::path::Path::new(path);
    let requested = std::fs::canonicalize(requested)
        .map_err(|error| format!("automation path is not readable: {error}"))?;
    let automations_dir = wardian_core::paths::library_automations_dir()
        .ok_or_else(|| "no wardian home".to_string())?;
    let automations_dir = std::fs::canonicalize(&automations_dir).map_err(|error| {
        format!(
            "automation library is not readable at {}: {error}",
            automations_dir.to_string_lossy()
        )
    })?;
    if !requested.starts_with(&automations_dir) {
        return Err(format!(
            "control automation_run only accepts files under {}",
            automations_dir.to_string_lossy()
        ));
    }

    wardian_core::automation::parse_file(&requested).map_err(|error| error.to_string())
}

/// Resume an interrupted or parked automation run.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn automation_resume(
    state: State<'_, AppState>,
    app: AppHandle,
    blueprint_id: String,
    run_id: String,
    blueprint_path: String,
    provider: Option<String>,
    workspace: Option<String>,
    bindings: Option<HashMap<String, String>>,
    assignments: Option<AutomationAssignments>,
) -> Result<serde_json::Value, String> {
    let blueprint = parse_blueprint_for_run(&blueprint_id, &blueprint_path)?;
    validate_blueprint_for_execution(&blueprint)?;
    let run_root =
        wardian_core::paths::automation_run_dir(&blueprint_id, &run_id).ok_or("no wardian home")?;
    let invocation = runs::read_run_invocation(&run_root)?;
    let provider = provider
        .or_else(|| invocation.as_ref().map(|value| value.provider.clone()))
        .unwrap_or_else(|| "codex".to_string());
    let workspace = workspace
        .map(std::path::PathBuf::from)
        .or_else(|| {
            invocation
                .as_ref()
                .map(|value| std::path::PathBuf::from(&value.workspace))
        })
        .unwrap_or_else(|| run_root.clone());
    let bindings = bindings
        .or_else(|| invocation.as_ref().map(|value| value.bindings.clone()))
        .unwrap_or_default();
    let assignments = wardian_core::automation::assignment::normalize_assignments(
        assignments.or_else(|| invocation.as_ref().map(|value| value.assignments.clone())),
        &bindings,
        InvocationKind::Manual,
    );
    let memory_principal = invocation
        .as_ref()
        .and_then(|value| value.memory_principal.clone());
    let agent_catalog = runs::agent_catalog_from_state_with_assignments(
        &state,
        &bindings,
        &assignments,
        &workspace,
        &provider,
    )
    .await;
    let owner_id = format!("{}/{}", blueprint.id, run_id);
    let app_for_inbox = app.clone();
    let blueprint_for_inbox = blueprint.clone();
    let run_root_for_inbox = run_root.clone();

    tokio::spawn(async move {
        let _headless_execution =
            match wardian_core::automation_execution_lock::acquire_headless_execution_guard() {
                Ok(guard) => guard,
                Err(error) => {
                    let message = format!("automation resume could not start: {error}");
                    match runs::mark_run_failed(&run_root, &message) {
                        Ok(_) => crate::utils::logging::log_debug(&format!(
                            "[automation] resume failed: {message}"
                        )),
                        Err(persist_error) => crate::utils::logging::log_debug(&format!(
                            "[automation] resume failed: {message}; failed to persist terminal failure: {persist_error}"
                        )),
                    }
                    runs::emit_automation_inbox_update(
                        &app_for_inbox,
                        &blueprint_for_inbox,
                        &run_root_for_inbox,
                    );
                    return;
                }
            };
        let exec = runs::live_executor_with_catalog_assignments_and_app(
            app,
            workspace,
            provider,
            bindings,
            assignments,
            agent_catalog,
        )
        .with_owner_id(owner_id);
        let exec = match memory_principal {
            Some(agent_id) => exec.with_memory_principal(agent_id),
            None => exec,
        };
        if let Err(error) = wardian_core::engine::Engine::resume(&blueprint, &run_root, &exec)
            .await
            .map(|_| ())
            .map_err(|err| err.to_string())
        {
            crate::utils::logging::log_debug(&format!("[automation] resume failed: {error}"));
        }
        runs::emit_automation_inbox_update(
            &app_for_inbox,
            &blueprint_for_inbox,
            &run_root_for_inbox,
        );
    });

    Ok(serde_json::json!({ "ok": true, "run_id": run_id }))
}

/// Grant or reject an approval gate. Granting persists the decision before the
/// remaining automation continues in the background.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn automation_approve(
    state: State<'_, AppState>,
    app: AppHandle,
    blueprint_id: String,
    run_id: String,
    blueprint_path: String,
    node: String,
    granted: bool,
    actor: String,
    note: Option<String>,
    provider: Option<String>,
    workspace: Option<String>,
    bindings: Option<HashMap<String, String>>,
    assignments: Option<AutomationAssignments>,
) -> Result<serde_json::Value, String> {
    approve_automation_for_surface(
        &state,
        app,
        blueprint_id,
        run_id,
        blueprint_path,
        node,
        granted,
        actor,
        note,
        provider,
        workspace,
        bindings,
        assignments,
    )
    .await
}

/// Shared approval implementation for the desktop command and authenticated
/// remote Inbox actions.
#[allow(clippy::too_many_arguments)]
pub async fn approve_automation_for_surface(
    state: &AppState,
    app: AppHandle,
    blueprint_id: String,
    run_id: String,
    blueprint_path: String,
    node: String,
    granted: bool,
    actor: String,
    note: Option<String>,
    provider: Option<String>,
    workspace: Option<String>,
    bindings: Option<HashMap<String, String>>,
    assignments: Option<AutomationAssignments>,
) -> Result<serde_json::Value, String> {
    let blueprint = parse_blueprint_for_run(&blueprint_id, &blueprint_path)?;
    validate_blueprint_for_execution(&blueprint)?;
    let run_root =
        wardian_core::paths::automation_run_dir(&blueprint_id, &run_id).ok_or("no wardian home")?;

    if granted {
        let invocation = runs::read_run_invocation(&run_root)?;
        let provider = provider
            .or_else(|| invocation.as_ref().map(|value| value.provider.clone()))
            .unwrap_or_else(|| "codex".to_string());
        let workspace = workspace
            .map(std::path::PathBuf::from)
            .or_else(|| {
                invocation
                    .as_ref()
                    .map(|value| std::path::PathBuf::from(&value.workspace))
            })
            .unwrap_or_else(|| run_root.clone());
        let bindings = bindings
            .or_else(|| invocation.as_ref().map(|value| value.bindings.clone()))
            .unwrap_or_default();
        let assignments = wardian_core::automation::assignment::normalize_assignments(
            assignments.or_else(|| invocation.as_ref().map(|value| value.assignments.clone())),
            &bindings,
            InvocationKind::Manual,
        );
        let agent_catalog = runs::agent_catalog_from_state_with_assignments(
            state,
            &bindings,
            &assignments,
            &workspace,
            &provider,
        )
        .await;
        let headless_execution =
            wardian_core::automation_execution_lock::acquire_headless_execution_guard()?;
        let run_state = wardian_core::engine::Engine::record_approval_granted(
            &blueprint, &run_root, &node, &actor, note,
        )
        .map_err(|error| error.to_string())?;
        let owner_id = format!("{}/{}", blueprint.id, run_id);
        let app_for_inbox = app.clone();
        let blueprint_for_inbox = blueprint.clone();
        let run_root_for_inbox = run_root.clone();

        tokio::spawn(async move {
            let _headless_execution = headless_execution;
            let exec = runs::live_executor_with_catalog_assignments_and_app(
                app,
                workspace,
                provider,
                bindings,
                assignments,
                agent_catalog,
            )
            .with_owner_id(owner_id);
            let exec = match invocation.and_then(|value| value.memory_principal) {
                Some(agent_id) => exec.with_memory_principal(agent_id),
                None => exec,
            };
            if let Err(error) = wardian_core::engine::Engine::drive_from_state(
                &blueprint, run_state, &run_root, &exec,
            )
            .await
            {
                crate::utils::logging::log_debug(&format!(
                    "[automation] approved run failed: {error}"
                ));
            }
            runs::emit_automation_inbox_update(
                &app_for_inbox,
                &blueprint_for_inbox,
                &run_root_for_inbox,
            );
        });

        Ok(serde_json::json!({ "ok": true }))
    } else {
        wardian_core::engine::Engine::reject_approval(&blueprint, &run_root, &node, &actor, note)
            .await
            .map(|_| {
                runs::emit_automation_inbox_update(&app, &blueprint, &run_root);
                serde_json::json!({ "ok": true })
            })
            .map_err(|error| error.to_string())
    }
}

/// Record a durable cancel request. An active temporary worker observes this
/// run-owned marker, terminates and reaps its provider process tree, and writes
/// a guarded cancellation acknowledgement before the engine settles the run.
#[tauri::command]
pub async fn automation_cancel(
    blueprint_id: String,
    run_id: String,
) -> Result<serde_json::Value, String> {
    let run_root =
        wardian_core::paths::automation_run_dir(&blueprint_id, &run_id).ok_or("no wardian home")?;
    let checkpoint = read_checkpoint(&run_root).map_err(|error| error.to_string())?;
    if let Some(state) = checkpoint.as_ref() {
        if state.status == RunStatus::Running {
            std::fs::write(run_root.join("cancel.marker"), "cancelled")
                .map_err(|error| error.to_string())?;
            let (status, worker_cancellation_acknowledged) =
                await_owned_worker_cancellation(&blueprint_id, &run_id, &run_root).await?;
            return Ok(serde_json::json!({
                "ok": true,
                "status": status,
                "worker_cancellation_acknowledged": worker_cancellation_acknowledged
            }));
        }
        if matches!(state.status, RunStatus::Completed | RunStatus::Failed) {
            let _ = std::fs::remove_file(run_root.join("cancel.marker"));
            return Ok(serde_json::json!({ "ok": true, "status": state.status }));
        }
        if state.status == RunStatus::AwaitingApproval {
            let state = wardian_core::engine::Engine::cancel_awaiting(&run_root)
                .map_err(|error| error.to_string())?;
            return Ok(serde_json::json!({ "ok": true, "status": state.status }));
        }
    }
    let blueprint_path = resolve_blueprint_path(&blueprint_id)
        .ok_or_else(|| format!("automation blueprint not found: {blueprint_id}"))?;
    let blueprint = automation::parse_file(&blueprint_path).map_err(|error| error.to_string())?;
    std::fs::write(run_root.join("cancel.marker"), "cancelled").map_err(|e| e.to_string())?;
    let state = wardian_core::engine::Engine::cancel(&blueprint, &run_root)
        .map_err(|error| error.to_string())?;
    Ok(serde_json::json!({ "ok": true, "status": state.status }))
}

async fn await_owned_worker_cancellation(
    blueprint_id: &str,
    run_id: &str,
    run_root: &Path,
) -> Result<(RunStatus, bool), String> {
    let workers = wardian_core::temporary_workers::list_for_run(blueprint_id, run_id)
        .map_err(|error| error.to_string())?;
    let requested_worker_ids = workers
        .iter()
        .filter_map(|worker| {
            (worker.kind == wardian_core::temporary_workers::TemporaryWorkerKind::Automation
                && matches!(
                    worker.state,
                    wardian_core::temporary_workers::TemporaryWorkerState::Requested
                        | wardian_core::temporary_workers::TemporaryWorkerState::Running
                ))
            .then(|| worker.worker_id.clone())
        })
        .collect::<Vec<_>>();
    if requested_worker_ids.is_empty() {
        let status = read_checkpoint(run_root)
            .map_err(|error| error.to_string())?
            .map(|state| state.status)
            .unwrap_or(RunStatus::Running);
        return Ok((status, false));
    }

    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let workers = wardian_core::temporary_workers::list_for_run(blueprint_id, run_id)
            .map_err(|error| error.to_string())?;
        let acknowledged = requested_worker_ids.iter().all(|worker_id| {
            workers
                .iter()
                .find(|worker| worker.worker_id == *worker_id)
                .is_some_and(worker_has_cancellation_acknowledgement)
        });
        let active = requested_worker_ids.iter().any(|worker_id| {
            workers
                .iter()
                .find(|worker| worker.worker_id == *worker_id)
                .is_some_and(|worker| {
                    matches!(
                        worker.state,
                        wardian_core::temporary_workers::TemporaryWorkerState::Requested
                            | wardian_core::temporary_workers::TemporaryWorkerState::Running
                    )
                })
        });
        let status = read_checkpoint(run_root)
            .map_err(|error| error.to_string())?
            .map(|state| state.status)
            .unwrap_or(RunStatus::Running);
        if acknowledged {
            return Ok((status, true));
        }
        if !active {
            return Ok((status, false));
        }
        if tokio::time::Instant::now() >= deadline {
            return Ok((status, false));
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
}

fn worker_has_cancellation_acknowledgement(
    worker: &wardian_core::temporary_workers::TemporaryWorkerRecord,
) -> bool {
    is_cancellation_acknowledgement(worker.state, &worker.coverage)
}

fn is_cancellation_acknowledgement(
    state: wardian_core::temporary_workers::TemporaryWorkerState,
    coverage: &str,
) -> bool {
    state == wardian_core::temporary_workers::TemporaryWorkerState::Cancelled
        && coverage == "run_cancellation_acknowledged"
}

fn now_ms() -> u64 {
    chrono::Utc::now().timestamp_millis() as u64
}

fn emit_schedules_updated(app: &AppHandle) {
    use tauri::Emitter;
    let _ = app.emit("schedules-updated", ());
}

fn validate_schedule_blueprint(blueprint_id: &str) -> Result<(), String> {
    let path = resolve_blueprint_path(blueprint_id)
        .ok_or_else(|| format!("blueprint not found in library/automations: {blueprint_id}"))?;
    let blueprint = automation::parse_file(&path)
        .map_err(|error| format!("could not parse blueprint {blueprint_id}: {error}"))?;
    let report = automation::validate(&blueprint);
    if !report.is_valid() {
        let diagnostics =
            serde_json::to_string(&report.diagnostics).map_err(|error| error.to_string())?;
        return Err(format!(
            "blueprint {blueprint_id} is invalid: {diagnostics}"
        ));
    }
    Ok(())
}

fn validate_schedule_provider(provider: Option<&str>) -> Result<(), String> {
    if let Some(provider) = provider {
        if !wardian_core::automation::assignment::is_known_provider(provider) {
            return Err(format!("unsupported provider `{provider}`"));
        }
    }
    Ok(())
}

fn normalize_schedule_assignments(
    assignments: Option<AutomationAssignments>,
    bindings: &HashMap<String, String>,
) -> Result<(HashMap<String, String>, AutomationAssignments), String> {
    let mut assignments = wardian_core::automation::assignment::normalize_assignments(
        assignments,
        bindings,
        InvocationKind::Scheduled,
    );
    for assignment in assignments.values_mut() {
        if let AutomationRoleAssignment::TemporaryProvider {
            workspace: Some(workspace),
            ..
        } = assignment
        {
            let canonical = resolve_workspace_path(workspace)?;
            *workspace = canonical.to_string_lossy().into_owned();
        }
    }
    wardian_core::automation::assignment::validate_assignments(&assignments)?;
    let bindings = wardian_core::automation::assignment::legacy_bindings(&assignments);
    Ok((bindings, assignments))
}

/// Create an automation schedule. `schedule` is the cadence definition; runtime fields are seeded.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn schedule_create(
    app: AppHandle,
    blueprint_id: String,
    name: String,
    mut schedule: wardian_core::models::ScheduleDefinition,
    provider: Option<String>,
    workspace: String,
    input: Option<Value>,
    bindings: Option<HashMap<String, String>>,
    assignments: Option<wardian_core::models::AutomationAssignments>,
) -> Result<AutomationSchedule, String> {
    if name.trim().is_empty() {
        return Err("schedule name must not be empty".to_string());
    }
    validate_schedule_blueprint(&blueprint_id)?;
    validate_schedule_provider(provider.as_deref())?;
    if schedule.end_condition.trim().is_empty() {
        schedule.end_condition = "never".to_string();
    }
    validate_schedule_definition(&schedule)?;
    let workspace = resolve_workspace_path(&workspace)?
        .to_string_lossy()
        .into_owned();
    let mut schedules = load_schedules();
    let now = now_ms();
    let bindings = bindings.unwrap_or_default();
    let (bindings, assignments) = normalize_schedule_assignments(assignments, &bindings)?;
    let record = AutomationSchedule {
        id: wardian_core::engine::driver::new_run_id(),
        blueprint_id,
        name,
        provider,
        workspace: Some(workspace),
        input: input.unwrap_or_else(|| serde_json::json!({})),
        bindings,
        assignments,
        schedule,
        next_run_epoch_ms: None,
        paused_remaining_ms: None,
        is_paused: false,
        last_run_status: None,
        last_run_error: None,
        last_run_epoch_ms: None,
    };
    let mut record = record;
    record.next_run_epoch_ms = compute_next_run(&record.schedule, now);
    schedules.push(record.clone());
    save_schedules(&schedules).map_err(|error| error.to_string())?;
    emit_schedules_updated(&app);
    Ok(record)
}

/// Update an automation schedule in place, preserving its identity and runtime history.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn schedule_update(
    app: AppHandle,
    id: String,
    blueprint_id: Option<String>,
    name: Option<String>,
    mut schedule: wardian_core::models::ScheduleDefinition,
    provider: Option<String>,
    workspace: String,
    input: Option<Value>,
    bindings: Option<HashMap<String, String>>,
    assignments: Option<wardian_core::models::AutomationAssignments>,
) -> Result<AutomationSchedule, String> {
    let mut schedules = load_schedules();
    let existing = schedules
        .iter_mut()
        .find(|schedule| schedule.id == id)
        .ok_or_else(|| format!("schedule not found: {id}"))?;

    let next_blueprint_id = blueprint_id.unwrap_or_else(|| existing.blueprint_id.clone());
    validate_schedule_blueprint(&next_blueprint_id)?;
    if let Some(name) = name.as_deref() {
        if name.trim().is_empty() {
            return Err("schedule name must not be empty".to_string());
        }
    }
    validate_schedule_provider(provider.as_deref())?;
    if schedule.end_condition.trim().is_empty() {
        schedule.end_condition = "never".to_string();
    }
    validate_schedule_definition(&schedule)?;
    let workspace = resolve_workspace_path(&workspace)?
        .to_string_lossy()
        .into_owned();

    let (next_bindings, next_assignments) = match (bindings, assignments) {
        (None, None) => (existing.bindings.clone(), existing.assignments.clone()),
        (bindings, assignments) => {
            normalize_schedule_assignments(assignments, &bindings.unwrap_or_default())?
        }
    };
    let schedule_changed = {
        schedule.occurrence_count = existing.schedule.occurrence_count;
        serde_json::to_value(&schedule).map_err(|error| error.to_string())?
            != serde_json::to_value(&existing.schedule).map_err(|error| error.to_string())?
    };
    let now = now_ms();
    let was_paused = existing.is_paused;

    existing.blueprint_id = next_blueprint_id;
    if let Some(name) = name {
        existing.name = name;
    }
    if provider.is_some() {
        existing.provider = provider;
    }
    existing.workspace = Some(workspace);
    if let Some(input) = input {
        existing.input = input;
    }
    existing.bindings = next_bindings;
    existing.assignments = next_assignments;
    existing.schedule = schedule;

    if was_paused {
        existing.next_run_epoch_ms = None;
        if schedule_changed {
            existing.paused_remaining_ms = None;
        }
    } else if schedule_changed || existing.next_run_epoch_ms.is_none() {
        existing.paused_remaining_ms = None;
        existing.next_run_epoch_ms = compute_next_run(&existing.schedule, now);
    }

    let updated = existing.clone();
    save_schedules(&schedules).map_err(|error| error.to_string())?;
    emit_schedules_updated(&app);
    Ok(updated)
}

#[tauri::command]
pub async fn schedule_list() -> Result<Vec<AutomationSchedule>, String> {
    Ok(load_schedules())
}

#[tauri::command]
pub async fn schedule_pause(app: AppHandle, id: String) -> Result<(), String> {
    let mut schedules = load_schedules();
    let now = now_ms();
    if let Some(schedule) = schedules.iter_mut().find(|schedule| schedule.id == id) {
        schedule.is_paused = true;
        schedule.paused_remaining_ms = schedule
            .next_run_epoch_ms
            .map(|next_run| next_run.saturating_sub(now));
        schedule.next_run_epoch_ms = None;
    }
    save_schedules(&schedules).map_err(|error| error.to_string())?;
    emit_schedules_updated(&app);
    Ok(())
}

#[tauri::command]
pub async fn schedule_resume(app: AppHandle, id: String) -> Result<(), String> {
    let mut schedules = load_schedules();
    let now = now_ms();
    if let Some(schedule) = schedules.iter_mut().find(|schedule| schedule.id == id) {
        schedule.is_paused = false;
        schedule.next_run_epoch_ms = match schedule.paused_remaining_ms.take() {
            Some(remaining) => Some(now.saturating_add(remaining)),
            None => compute_next_run(&schedule.schedule, now),
        };
    }
    save_schedules(&schedules).map_err(|error| error.to_string())?;
    emit_schedules_updated(&app);
    Ok(())
}

#[tauri::command]
pub async fn schedule_remove(app: AppHandle, id: String) -> Result<(), String> {
    let mut schedules = load_schedules();
    schedules.retain(|schedule| schedule.id != id);
    save_schedules(&schedules).map_err(|error| error.to_string())?;
    emit_schedules_updated(&app);
    Ok(())
}

/// Fire ASAP: set next_run to now so the scheduler's next tick launches it live.
#[tauri::command]
pub async fn schedule_run_now(app: AppHandle, id: String) -> Result<(), String> {
    let mut schedules = load_schedules();
    let now = now_ms();
    if let Some(schedule) = schedules.iter_mut().find(|schedule| schedule.id == id) {
        schedule.is_paused = false;
        schedule.next_run_epoch_ms = Some(now);
    }
    save_schedules(&schedules).map_err(|error| error.to_string())?;
    emit_schedules_updated(&app);
    Ok(())
}

fn parse_blueprint_for_run(blueprint_id: &str, blueprint_path: &str) -> Result<Blueprint, String> {
    let provided = std::path::Path::new(blueprint_path);
    if !blueprint_path.trim().is_empty() && provided.is_file() {
        return parse_blueprint_file_for_id(provided, blueprint_id);
    }
    let resolved = resolve_blueprint_path(blueprint_id).ok_or_else(|| {
        if blueprint_path.trim().is_empty() {
            format!("could not resolve blueprint path for {blueprint_id}")
        } else {
            format!(
                "could not resolve blueprint path for {blueprint_id}; provided path is not a file: {blueprint_path}"
            )
        }
    })?;
    parse_blueprint_file_for_id(&resolved, blueprint_id)
}

fn validate_blueprint_for_execution(blueprint: &Blueprint) -> Result<(), String> {
    let report = automation::validate(blueprint);
    if report.is_valid() {
        return Ok(());
    }
    let diagnostics =
        serde_json::to_string(&report.diagnostics).map_err(|error| error.to_string())?;
    Err(format!("automation blueprint is invalid: {diagnostics}"))
}

fn parse_blueprint_file_for_id(
    path: &std::path::Path,
    blueprint_id: &str,
) -> Result<Blueprint, String> {
    let blueprint = wardian_core::automation::parse_file(path).map_err(|e| e.to_string())?;
    if blueprint.id != blueprint_id {
        return Err(format!(
            "blueprint path id mismatch: expected {blueprint_id}, found {}",
            blueprint.id
        ));
    }
    Ok(blueprint)
}

fn resolve_blueprint_path(id: &str) -> Option<std::path::PathBuf> {
    automation::resolve_blueprint_path(id)
}

fn resolve_blueprint(id: &str) -> Option<serde_json::Value> {
    let path = resolve_blueprint_path(id)?;
    let blueprint = wardian_core::automation::parse_file(&path).ok()?;
    serde_json::to_value(blueprint).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use wardian_core::engine::event::{Event, EventKind};
    use wardian_core::engine::store::{append_event, write_checkpoint};
    use wardian_core::engine::{RunState, RunStatus};

    struct EnvGuard {
        _lock: tokio::sync::MutexGuard<'static, ()>,
        previous_home: Option<std::ffi::OsString>,
    }

    impl EnvGuard {
        fn set(home: &std::path::Path) -> Self {
            Self::from_guard(home, crate::utils::wardian_test_env_lock())
        }

        async fn set_async(home: &std::path::Path) -> Self {
            Self::from_guard(home, crate::utils::wardian_test_env_lock_async().await)
        }

        fn from_guard(home: &std::path::Path, lock: tokio::sync::MutexGuard<'static, ()>) -> Self {
            let guard = Self {
                _lock: lock,
                previous_home: std::env::var_os("WARDIAN_HOME"),
            };
            std::env::set_var("WARDIAN_HOME", home);
            guard
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match self.previous_home.take() {
                Some(value) => std::env::set_var("WARDIAN_HOME", value),
                None => std::env::remove_var("WARDIAN_HOME"),
            }
        }
    }

    #[test]
    fn automation_run_bound_keeps_newest_runs_and_marks_partial_results() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("automation-runs");
        for index in 0..=MAX_AUTOMATION_RUNS {
            let run_root = root.join("wf").join(format!("run-{index:04}"));
            write_checkpoint(&run_root, &RunState::new(format!("run-{index:04}"), "wf")).unwrap();
        }

        let result = automation_list_runs_page_from_root(&root, |_| None, 0).unwrap();

        assert!(result.truncated);
        assert_eq!(result.runs.len(), MAX_AUTOMATION_RUNS);
        assert_eq!(result.runs[0]["run_id"], "run-0200");
    }

    #[test]
    fn automation_run_pages_continue_after_the_newest_page() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("automation-runs");
        for index in 0..=MAX_AUTOMATION_RUNS {
            let run_root = root.join("wf").join(format!("run-{index:04}"));
            write_checkpoint(&run_root, &RunState::new(format!("run-{index:04}"), "wf")).unwrap();
        }

        let page =
            automation_list_runs_page_from_root(&root, |_| None, MAX_AUTOMATION_RUNS).unwrap();
        assert_eq!(page.runs.len(), 1);
        assert!(!page.truncated);
        assert_eq!(page.next_offset, None);
    }

    #[test]
    fn automation_run_status_filter_applies_before_pagination() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("automation-runs");
        for index in 0..=MAX_AUTOMATION_RUNS {
            let run_root = root.join("wf").join(format!("run-z-{index:04}"));
            write_checkpoint(&run_root, &RunState::new(format!("run-z-{index:04}"), "wf")).unwrap();
        }
        let completed_root = root.join("wf").join("run-a-completed");
        let mut completed = RunState::new("run-a-completed", "wf");
        completed.status = RunStatus::Completed;
        write_checkpoint(&completed_root, &completed).unwrap();

        let result = automation_list_runs_page_from_root_matching(
            &root,
            |_| None,
            0,
            |run| run["status"] == "completed",
        )
        .unwrap();

        assert_eq!(result.runs.len(), 1);
        assert_eq!(result.runs[0]["run_id"], "run-a-completed");
        assert!(!result.truncated);
    }

    #[test]
    fn automation_run_summary_cache_refreshes_changed_checkpoints() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("automation-runs");
        let run_root = root.join("wf").join("run-changing");
        let state = RunState::new("run-changing", "wf");
        write_checkpoint(&run_root, &state).unwrap();
        let initial = automation_list_runs_page_from_root(&root, |_| None, 0).unwrap();
        assert_eq!(initial.runs[0]["status"], "running");

        let mut completed = state;
        completed.status = RunStatus::Completed;
        write_checkpoint(&run_root, &completed).unwrap();
        let refreshed = automation_list_runs_page_from_root(&root, |_| None, 0).unwrap();

        assert_eq!(refreshed.runs[0]["status"], "completed");
    }

    #[tokio::test]
    async fn automation_cancel_marks_running_run_without_loading_blueprint() {
        let temp = tempfile::tempdir().unwrap();
        let _guard = EnvGuard::set_async(temp.path()).await;
        wardian_core::db::init_db_at_path(&temp.path().join("state.db"))
            .expect("initialize automation command database");
        let blueprint_id = "missing-blueprint";
        let run_id = "run-cancel";
        let run_root = wardian_core::paths::automation_run_dir(blueprint_id, run_id).unwrap();
        write_checkpoint(&run_root, &RunState::new(run_id, blueprint_id)).unwrap();

        let result = automation_cancel(blueprint_id.into(), run_id.into())
            .await
            .unwrap();

        assert_eq!(result["status"], "running");
        assert_eq!(result["worker_cancellation_acknowledged"], false);
        assert!(run_root.join("cancel.marker").exists());
    }

    #[test]
    fn cancellation_acknowledgement_requires_cancelled_state_and_owned_coverage() {
        use wardian_core::temporary_workers::TemporaryWorkerState;

        assert!(is_cancellation_acknowledgement(
            TemporaryWorkerState::Cancelled,
            "run_cancellation_acknowledged",
        ));
        assert!(!is_cancellation_acknowledgement(
            TemporaryWorkerState::Succeeded,
            "run_cancellation_acknowledged",
        ));
        assert!(!is_cancellation_acknowledgement(
            TemporaryWorkerState::Cancelled,
            "provider_observation_adapter_unavailable",
        ));
    }

    #[test]
    fn automation_read_run_prefers_the_immutable_blueprint_snapshot() {
        let temp = tempfile::tempdir().unwrap();
        let _guard = EnvGuard::set(temp.path());
        wardian_core::db::init_db_at_path(&temp.path().join("state.db"))
            .expect("initialize automation command database");
        seed_automation_blueprint(temp.path());
        let run_root = wardian_core::paths::automation_run_dir("wf", "run-snapshot").unwrap();
        let snapshot = Blueprint {
            schema: 2,
            id: "wf".into(),
            name: "Immutable snapshot".into(),
            nodes: Vec::new(),
            edges: Vec::new(),
            body: String::new(),
        };
        wardian_core::engine::store::write_blueprint_snapshot(&run_root, &snapshot).unwrap();
        write_checkpoint(&run_root, &RunState::new("run-snapshot", "wf")).unwrap();

        let result = automation_read_run("wf".into(), "run-snapshot".into()).unwrap();

        assert_eq!(result["blueprint"]["name"], "Immutable snapshot");
        assert!(result["blueprint_path"].as_str().is_some());
    }

    fn sample_schedule() -> AutomationSchedule {
        AutomationSchedule {
            id: "s1".into(),
            blueprint_id: "heartbeat".into(),
            name: "HB".into(),
            provider: None,
            workspace: None,
            input: serde_json::json!({}),
            bindings: Default::default(),
            assignments: Default::default(),
            schedule: wardian_core::models::ScheduleDefinition {
                schedule_type: "interval".into(),
                interval_minutes: Some(60),
                active: true,
                ..Default::default()
            },
            next_run_epoch_ms: Some(9_999_999_999),
            paused_remaining_ms: None,
            is_paused: false,
            last_run_status: None,
            last_run_error: None,
            last_run_epoch_ms: None,
        }
    }

    const AUTOMATION_BLUEPRINT: &str = r#"---
schema: 2
id: wf
name: Automation
nodes:
  - id: trigger
    type: manual_trigger
    fields: {}
edges: []
---

# Automation
"#;

    const SHELL_AUTOMATION_BLUEPRINT: &str = r#"---
schema: 2
id: shell-wf
name: Shell Automation
nodes:
  - id: trigger
    type: manual_trigger
    fields: {}
  - id: shell
    type: shell
    fields:
      command: echo unsafe
edges:
  - from: trigger
    to: shell
---

# Shell Automation
"#;

    fn seed_automation_blueprint(home: &std::path::Path) -> std::path::PathBuf {
        let automations_dir = home.join("library").join("automations");
        std::fs::create_dir_all(&automations_dir).unwrap();
        let path = automations_dir.join("wf.md");
        std::fs::write(&path, AUTOMATION_BLUEPRINT).unwrap();
        path
    }

    #[test]
    fn run_summary_uses_event_timestamps() {
        let dir = tempfile::tempdir().unwrap();
        let run_root = dir.path().join("wf").join("run-1");
        let mut state = RunState::new("run-1", "wf");
        state.status = RunStatus::Completed;
        write_checkpoint(&run_root, &state).unwrap();
        append_event(
            &run_root,
            &Event::at(
                0,
                "2026-05-31T12:00:00Z".into(),
                EventKind::RunStarted {
                    run_id: None,
                    blueprint_hash: None,
                    blueprint_id: "wf".into(),
                    schema: 2,
                    trigger: serde_json::json!({}),
                },
            ),
        )
        .unwrap();
        append_event(
            &run_root,
            &Event::at(1, "2026-05-31T12:01:00Z".into(), EventKind::RunCompleted),
        )
        .unwrap();

        let summary = summarize_run_dir(&run_root).unwrap();

        assert_eq!(summary["started_at"], "2026-05-31T12:00:00Z");
        assert_eq!(summary["updated_at"], "2026-05-31T12:01:00Z");
        assert_eq!(summary["completed_at"], "2026-05-31T12:01:00Z");
    }

    #[test]
    fn run_summary_reads_event_timestamp_bounds_without_full_event_payloads() {
        let dir = tempfile::tempdir().unwrap();
        let run_root = dir.path().join("wf").join("run-1");
        let mut state = RunState::new("run-1", "wf");
        state.status = RunStatus::Completed;
        write_checkpoint(&run_root, &state).unwrap();
        std::fs::write(
            run_root.join("events.jsonl"),
            r#"{"seq":0,"ts":"2026-05-31T12:00:00Z","kind":"run_started","blueprint_id":"wf","schema":2,"trigger":{}}
{"seq":1,"ts":"2026-05-31T12:00:30Z","kind":"node_completed","node":"large","output":
{"seq":2,"ts":"2026-05-31T12:01:00Z","kind":"run_completed"}
"#,
        )
        .unwrap();

        let summary = summarize_run_dir(&run_root).unwrap();

        assert_eq!(summary["started_at"], "2026-05-31T12:00:00Z");
        assert_eq!(summary["updated_at"], "2026-05-31T12:01:00Z");
        assert_eq!(summary["completed_at"], "2026-05-31T12:01:00Z");
    }

    #[test]
    fn automation_run_listing_resolves_blueprint_paths_once_per_blueprint() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("logs").join("automations");
        for index in 0..3 {
            let run_root = root.join("wf").join(format!("run-{index}"));
            let mut state = RunState::new(format!("run-{index}"), "wf");
            state.status = RunStatus::Completed;
            write_checkpoint(&run_root, &state).unwrap();
            append_event(
                &run_root,
                &Event::at(
                    0,
                    format!("2026-05-31T12:0{index}:00Z"),
                    EventKind::RunCompleted,
                ),
            )
            .unwrap();
        }
        let resolve_count = std::cell::Cell::new(0);

        let result = automation_list_runs_page_from_root(
            &root,
            |blueprint_id| {
                resolve_count.set(resolve_count.get() + 1);
                Some(
                    dir.path()
                        .join("library")
                        .join("automations")
                        .join(format!("{blueprint_id}.md")),
                )
            },
            0,
        )
        .unwrap();

        assert_eq!(result.runs.len(), 3);
        assert_eq!(resolve_count.get(), 1);
        assert!(result.runs.iter().all(|run| run["blueprint_path"]
            .as_str()
            .is_some_and(|path| path.ends_with("wf.md"))));
    }

    #[test]
    fn run_summary_carries_resolved_blueprint_path_separately_from_run_path() {
        let dir = tempfile::tempdir().unwrap();
        let _env = EnvGuard::set(dir.path());
        let blueprint_path = seed_automation_blueprint(dir.path());
        let run_root = dir
            .path()
            .join("logs")
            .join("automations")
            .join("wf")
            .join("run-1");
        let mut state = RunState::new("run-1", "wf");
        state.status = RunStatus::AwaitingApproval;
        write_checkpoint(&run_root, &state).unwrap();

        let summary = summarize_run_dir(&run_root).unwrap();

        assert_eq!(
            summary["path"].as_str(),
            Some(run_root.to_string_lossy().as_ref())
        );
        assert_eq!(
            summary["blueprint_path"].as_str(),
            Some(blueprint_path.to_string_lossy().as_ref())
        );
    }

    #[test]
    fn run_summary_carries_schedule_id_from_invocation() {
        let dir = tempfile::tempdir().unwrap();
        let _env = EnvGuard::set(dir.path());
        seed_automation_blueprint(dir.path());
        let run_root = dir
            .path()
            .join("logs")
            .join("automations")
            .join("wf")
            .join("run-1");
        let mut state = RunState::new("run-1", "wf");
        state.status = RunStatus::Running;
        write_checkpoint(&run_root, &state).unwrap();
        std::fs::write(
            run_root.join("invocation.json"),
            r#"{
  "schema": 1,
  "provider": "mock",
  "workspace": "<absolute-workspace-path>",
  "schedule_id": "schedule-trader"
}"#,
        )
        .unwrap();

        let summary = summarize_run_dir(&run_root).unwrap();

        assert_eq!(summary["schedule_id"], "schedule-trader");
    }

    #[test]
    fn parse_blueprint_for_run_falls_back_from_run_dir_to_blueprint_id() {
        let dir = tempfile::tempdir().unwrap();
        let _env = EnvGuard::set(dir.path());
        seed_automation_blueprint(dir.path());
        let stale_run_dir = dir
            .path()
            .join("logs")
            .join("automations")
            .join("wf")
            .join("run-1");
        std::fs::create_dir_all(&stale_run_dir).unwrap();

        let blueprint = parse_blueprint_for_run("wf", &stale_run_dir.to_string_lossy()).unwrap();

        assert_eq!(blueprint.id, "wf");
    }

    #[test]
    fn parse_blueprint_for_run_rejects_mismatched_provided_file() {
        let dir = tempfile::tempdir().unwrap();
        let _env = EnvGuard::set(dir.path());
        let other_path = dir.path().join("other.md");
        std::fs::write(
            &other_path,
            AUTOMATION_BLUEPRINT.replace("id: wf", "id: other"),
        )
        .unwrap();

        let error = parse_blueprint_for_run("wf", &other_path.to_string_lossy()).unwrap_err();

        assert!(error.contains("blueprint path id mismatch"));
    }

    #[test]
    fn control_automation_blueprint_must_live_under_library() {
        let dir = tempfile::tempdir().unwrap();
        let _env = EnvGuard::set(dir.path());
        std::fs::create_dir_all(dir.path().join("library").join("automations")).unwrap();
        let outside_path = dir.path().join("outside.md");
        std::fs::write(&outside_path, AUTOMATION_BLUEPRINT).unwrap();

        let error =
            parse_control_automation_blueprint(&outside_path.to_string_lossy()).unwrap_err();

        assert!(error.contains("only accepts files under"));
    }

    #[test]
    fn control_automation_blueprint_allows_library_shell_nodes() {
        let dir = tempfile::tempdir().unwrap();
        let _env = EnvGuard::set(dir.path());
        let automations_dir = dir.path().join("library").join("automations");
        std::fs::create_dir_all(&automations_dir).unwrap();
        let path = automations_dir.join("shell.md");
        std::fs::write(&path, SHELL_AUTOMATION_BLUEPRINT).unwrap();

        let blueprint = parse_control_automation_blueprint(&path.to_string_lossy()).unwrap();

        assert_eq!(blueprint.id, "shell-wf");
        assert!(blueprint.nodes.iter().any(|node| node.r#type == "shell"));
    }

    #[tokio::test]
    async fn schedule_list_reads_persisted_schedules() {
        let dir = tempfile::tempdir().unwrap();
        let _env = EnvGuard::set_async(dir.path()).await;

        wardian_core::schedule::save_schedules(&[sample_schedule()]).unwrap();
        let loaded = schedule_list().await.unwrap();

        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].id, "s1");
    }

    #[test]
    fn pause_then_resume_round_trips_via_core() {
        let dir = tempfile::tempdir().unwrap();
        let _env = EnvGuard::set(dir.path());

        let mut schedule = sample_schedule();
        schedule.is_paused = true;
        schedule.paused_remaining_ms = Some(1234);
        schedule.next_run_epoch_ms = None;
        wardian_core::schedule::save_schedules(&[schedule]).unwrap();

        let loaded = wardian_core::schedule::load_schedules();
        assert_eq!(loaded.len(), 1);
        assert!(loaded[0].is_paused);
        assert_eq!(loaded[0].paused_remaining_ms, Some(1234));
    }
}
