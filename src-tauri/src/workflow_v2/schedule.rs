//! The v2 schedule invoker: a periodic tick loop that fires due schedules
//! through the 6a run path with the live executor.

use crate::workflow_v2::runs;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};
use wardian_core::schedule::{load_schedules, plan_tick, save_schedules, FireRequest};
use wardian_core::workflow::Blueprint;

const TICK_SECS: u64 = 5;

fn resolve_provider(req: &FireRequest) -> String {
    req.provider.clone().unwrap_or_else(|| {
        crate::utils::load_shell_settings()
            .map(|settings| settings.default_provider)
            .unwrap_or_else(|_| "codex".to_string())
    })
}

/// Everything needed to launch one scheduled run, resolved from a `FireRequest`.
pub struct ResolvedFire {
    pub blueprint: Blueprint,
    pub run_id: String,
    pub run_root: PathBuf,
    pub provider: String,
    pub workspace: PathBuf,
    pub input: serde_json::Value,
    pub bindings: HashMap<String, String>,
}

/// Resolve a `FireRequest` into launch parameters without touching Tauri state.
pub fn resolve_fire(req: &FireRequest) -> Result<ResolvedFire, String> {
    let path = wardian_core::paths::blueprint_path(&req.blueprint_id)
        .ok_or_else(|| "no wardian home".to_string())?;
    let blueprint =
        wardian_core::workflow::parse_file(&path).map_err(|err| format!("parse failed: {err}"))?;
    let run_id = wardian_core::engine::driver::new_run_id();
    let run_root = wardian_core::paths::workflow_run_dir(&blueprint.id, &run_id)
        .ok_or_else(|| "could not resolve run directory".to_string())?;
    let provider = resolve_provider(req);
    let workspace = req
        .workspace
        .clone()
        .map(PathBuf::from)
        .unwrap_or_else(|| run_root.clone());
    Ok(ResolvedFire {
        blueprint,
        run_id,
        run_root,
        provider,
        workspace,
        input: req.input.clone(),
        bindings: req.bindings.clone(),
    })
}

fn fire_request(_app: &AppHandle, req: &FireRequest) {
    let resolved = match resolve_fire(req) {
        Ok(resolved) => resolved,
        Err(message) => {
            mark_error(&req.schedule_id, &message);
            return;
        }
    };

    crate::utils::logging::log_debug(&format!(
        "[workflow-v2] scheduler firing '{}' -> blueprint {} run {}",
        req.name, resolved.blueprint.id, resolved.run_id
    ));

    tokio::spawn(async move {
        if let Err(error) = runs::drive_new_run(
            resolved.blueprint,
            resolved.run_id,
            resolved.run_root,
            resolved.workspace,
            resolved.provider,
            resolved.input,
            resolved.bindings,
        )
        .await
        {
            crate::utils::logging::log_debug(&format!(
                "[workflow-v2] scheduled run failed: {error}"
            ));
        }
    });
}

fn mark_error(schedule_id: &str, message: &str) {
    let mut schedules = load_schedules();
    if let Some(schedule) = schedules
        .iter_mut()
        .find(|schedule| schedule.id == schedule_id)
    {
        schedule.last_run_status = Some("failed".to_string());
        schedule.last_run_error = Some(message.to_string());
    }
    let _ = save_schedules(&schedules);
    crate::utils::logging::log_debug(&format!("[workflow-v2] scheduler: {message}"));
}

fn persist_runtime(
    processed: &[wardian_core::models::WorkflowSchedule],
    removed: &std::collections::HashSet<String>,
) {
    let mut fresh = load_schedules();
    fresh.retain(|schedule| !removed.contains(&schedule.id));
    for fresh_schedule in fresh.iter_mut() {
        if let Some(processed_schedule) = processed
            .iter()
            .find(|schedule| schedule.id == fresh_schedule.id)
        {
            fresh_schedule.next_run_epoch_ms = processed_schedule.next_run_epoch_ms;
            fresh_schedule.paused_remaining_ms = processed_schedule.paused_remaining_ms;
            fresh_schedule.schedule.occurrence_count = processed_schedule.schedule.occurrence_count;
            fresh_schedule.last_run_status = processed_schedule.last_run_status.clone();
            fresh_schedule.last_run_error = processed_schedule.last_run_error.clone();
            fresh_schedule.last_run_epoch_ms = processed_schedule.last_run_epoch_ms;
        }
    }
    let _ = save_schedules(&fresh);
}

pub async fn start_v2_scheduler(app: AppHandle) {
    let state = app.state::<crate::state::AppState>();
    {
        let mut handle = state.v2_scheduler_handle.lock().await;
        if let Some(existing) = handle.take() {
            existing.abort();
        }
    }

    let app_clone = app.clone();
    let handle = tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(TICK_SECS)).await;
            let state = app_clone.state::<crate::state::AppState>();
            if state.v2_schedules_paused.load(Ordering::SeqCst) {
                continue;
            }

            let mut schedules = load_schedules();
            if schedules.is_empty() {
                continue;
            }

            let before: std::collections::HashSet<String> = schedules
                .iter()
                .map(|schedule| schedule.id.clone())
                .collect();
            let now_ms = chrono::Utc::now().timestamp_millis() as u64;
            let fires = plan_tick(&mut schedules, now_ms);
            let after: std::collections::HashSet<String> = schedules
                .iter()
                .map(|schedule| schedule.id.clone())
                .collect();
            let removed: std::collections::HashSet<String> =
                before.difference(&after).cloned().collect();

            persist_runtime(&schedules, &removed);
            for req in &fires {
                fire_request(&app_clone, req);
            }
            if !fires.is_empty() {
                let _ = app_clone.emit("v2-schedules-updated", ());
            }
        }
    });

    let mut slot = state.v2_scheduler_handle.lock().await;
    *slot = Some(handle);
}
