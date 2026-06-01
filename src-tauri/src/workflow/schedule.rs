//! The workflow schedule invoker: a periodic tick loop that fires due schedules
//! through the 6a run path with the live executor.

use crate::workflow::runs;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};
use wardian_core::engine::{store::read_checkpoint, RunStatus};
use wardian_core::models::InvocationKind;
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
    pub assignments: wardian_core::models::WorkflowAssignments,
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
        assignments: wardian_core::workflow::assignment::normalize_assignments(
            Some(req.assignments.clone()),
            &req.bindings,
            InvocationKind::Scheduled,
        ),
    })
}

async fn fire_request(app: &AppHandle, req: &FireRequest) {
    let resolved = match resolve_fire(req) {
        Ok(resolved) => resolved,
        Err(message) => {
            mark_error(&req.schedule_id, &message);
            return;
        }
    };

    crate::utils::logging::log_debug(&format!(
        "[workflow] scheduler firing '{}' -> blueprint {} run {}",
        req.name, resolved.blueprint.id, resolved.run_id
    ));

    let state = app.state::<crate::state::AppState>();
    let agent_catalog = runs::agent_catalog_from_state_with_assignments(
        &state,
        &resolved.bindings,
        &resolved.assignments,
        &resolved.workspace,
        &resolved.provider,
    )
    .await;

    let app_for_run = app.clone();
    let app_for_emit = app.clone();
    let schedule_id = req.schedule_id.clone();
    let run_root = resolved.run_root.clone();
    tokio::spawn(async move {
        let result = runs::drive_new_run_with_catalog_and_assignments(
            Some(app_for_run),
            resolved.blueprint,
            resolved.run_id,
            resolved.run_root,
            resolved.workspace,
            resolved.provider,
            resolved.input,
            resolved.bindings,
            resolved.assignments,
            agent_catalog,
        )
        .await;

        match result {
            Ok(()) => mark_finished_from_checkpoint(&schedule_id, &run_root),
            Err(error) => {
                crate::utils::logging::log_debug(&format!(
                    "[workflow] scheduled run failed: {error}"
                ));
                mark_error(&schedule_id, &error);
            }
        };
        let _ = app_for_emit.emit("schedules-updated", ());
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
    crate::utils::logging::log_debug(&format!("[workflow] scheduler: {message}"));
}

fn mark_finished_from_checkpoint(schedule_id: &str, run_root: &std::path::Path) {
    match read_checkpoint(run_root) {
        Ok(Some(state)) => {
            let (status, error) = match state.status {
                RunStatus::Completed => ("completed".to_string(), None),
                RunStatus::AwaitingApproval => ("awaiting_approval".to_string(), None),
                RunStatus::Running => ("running".to_string(), None),
                RunStatus::Failed => ("failed".to_string(), state.failure),
            };
            mark_run_status(schedule_id, status, error);
        }
        Ok(None) => mark_error(schedule_id, "run completed without a checkpoint"),
        Err(error) => mark_error(
            schedule_id,
            &format!("could not read completed run checkpoint: {error}"),
        ),
    }
}

fn mark_run_status(schedule_id: &str, status: String, error: Option<String>) {
    let mut schedules = load_schedules();
    if let Some(schedule) = schedules
        .iter_mut()
        .find(|schedule| schedule.id == schedule_id)
    {
        schedule.last_run_status = Some(status);
        schedule.last_run_error = error;
    }
    let _ = save_schedules(&schedules);
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

pub async fn start_scheduler(app: AppHandle) {
    let state = app.state::<crate::state::AppState>();
    {
        let mut handle = state.workflow_scheduler_handle.lock().await;
        if let Some(existing) = handle.take() {
            existing.abort();
        }
    }

    let app_clone = app.clone();
    let handle = tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(TICK_SECS)).await;
            let state = app_clone.state::<crate::state::AppState>();
            if state.workflow_schedules_paused.load(Ordering::SeqCst) {
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
                fire_request(&app_clone, req).await;
            }
            if !fires.is_empty() {
                let _ = app_clone.emit("schedules-updated", ());
            }
        }
    });

    let mut slot = state.workflow_scheduler_handle.lock().await;
    *slot = Some(handle);
}

#[cfg(test)]
mod tests {
    use super::*;

    const BLUEPRINT: &str = r#"---
schema: 2
id: sched-normalize
name: Scheduled Normalize
nodes:
  - id: trigger
    type: manual_trigger
    fields: {}
  - id: analyze
    type: task
    fields:
      agent: role:analyst
      prompt: hi
edges:
  - from: trigger
    to: analyze
---

# Scheduled Normalize
"#;

    #[test]
    fn resolve_fire_normalizes_legacy_bindings_as_scheduled_assignments() {
        let _guard = crate::utils::wardian_test_env_lock();
        let home = tempfile::tempdir().unwrap();
        let previous_home = std::env::var_os("WARDIAN_HOME");
        std::env::set_var("WARDIAN_HOME", home.path());
        let workflows_dir = home.path().join("library").join("workflows");
        std::fs::create_dir_all(&workflows_dir).unwrap();
        std::fs::write(workflows_dir.join("sched-normalize.md"), BLUEPRINT).unwrap();
        let mut bindings = HashMap::new();
        bindings.insert("analyst".to_string(), "agent-123".to_string());
        let req = FireRequest {
            schedule_id: "s1".to_string(),
            blueprint_id: "sched-normalize".to_string(),
            name: "Schedule".to_string(),
            provider: Some("mock".to_string()),
            workspace: None,
            input: serde_json::json!({}),
            bindings,
            assignments: Default::default(),
        };

        let resolved = resolve_fire(&req).unwrap();

        assert_eq!(
            resolved.assignments.get("analyst"),
            Some(&wardian_core::models::WorkflowRoleAssignment::Agent {
                agent_id: "agent-123".to_string(),
                conversation: wardian_core::models::AgentConversationMode::Current,
                busy_policy: wardian_core::models::BusyPolicy::Skip,
            })
        );

        match previous_home {
            Some(value) => std::env::set_var("WARDIAN_HOME", value),
            None => std::env::remove_var("WARDIAN_HOME"),
        }
    }
}
