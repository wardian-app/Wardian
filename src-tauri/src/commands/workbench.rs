use crate::state::AppState;
use serde::Serialize;
use std::{ffi::OsStr, path::Path};
use tauri::State;
use wardian_core::{
    models::workbench::WorkbenchDocumentV1,
    workbench::{
        load_workbench_for_home, reset_workbench_for_home, save_workbench_for_home,
        WorkbenchLoadResult, WorkbenchResetRequest, WorkbenchResetResult, WorkbenchSaveRequest,
        WorkbenchSaveResult,
    },
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct WorkbenchBootConfig {
    pub safe_mode: bool,
}

fn workbench_safe_mode_from_value(value: Option<&OsStr>) -> bool {
    value == Some(OsStr::new("1"))
}

#[tauri::command]
pub fn get_workbench_boot_config() -> WorkbenchBootConfig {
    WorkbenchBootConfig {
        safe_mode: workbench_safe_mode_from_value(
            std::env::var_os("WARDIAN_WORKBENCH_SAFE_MODE").as_deref(),
        ),
    }
}

#[tauri::command]
pub async fn load_workbench_state(
    state: State<'_, AppState>,
) -> Result<WorkbenchLoadResult, String> {
    let home = crate::utils::fs::get_wardian_home()
        .ok_or_else(|| "Could not resolve Wardian home".to_string())?;
    load_workbench_state_for_home(&home, state.inner()).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn save_workbench_state(
    document: WorkbenchDocumentV1,
    expected_revision: u64,
    expected_token: String,
    request_id: String,
    state: State<'_, AppState>,
) -> Result<WorkbenchSaveResult, String> {
    let home = crate::utils::fs::get_wardian_home()
        .ok_or_else(|| "Could not resolve Wardian home".to_string())?;
    save_workbench_state_for_home(
        &home,
        WorkbenchSaveRequest {
            document,
            expected_revision,
            expected_token,
            request_id,
        },
        state.inner(),
    )
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn reset_workbench_state(
    expected_revision: u64,
    expected_token: String,
    request_id: String,
    state: State<'_, AppState>,
) -> Result<WorkbenchResetResult, String> {
    let home = crate::utils::fs::get_wardian_home()
        .ok_or_else(|| "Could not resolve Wardian home".to_string())?;
    reset_workbench_state_for_home(
        &home,
        WorkbenchResetRequest {
            expected_revision,
            expected_token,
            request_id,
        },
        state.inner(),
    )
    .await
}

async fn load_workbench_state_for_home(
    home: &Path,
    state: &AppState,
) -> Result<WorkbenchLoadResult, String> {
    let _io_guard = state.workbench_io_lock.lock().await;
    let mut loaded = load_workbench_for_home(home).map_err(|error| error.to_string())?;
    if let Some(document) = loaded.document.as_mut() {
        for surface in document.surfaces.values_mut() {
            if surface.surface_type == "queue" {
                surface.surface_type = "inbox".to_string();
            }
        }
        for closed in &mut document.recently_closed {
            if closed.surface.surface_type == "queue" {
                closed.surface.surface_type = "inbox".to_string();
            }
        }
    }
    Ok(loaded)
}

async fn save_workbench_state_for_home(
    home: &Path,
    request: WorkbenchSaveRequest,
    state: &AppState,
) -> Result<WorkbenchSaveResult, String> {
    let _io_guard = state.workbench_io_lock.lock().await;
    save_workbench_for_home(home, request).map_err(|error| error.to_string())
}

async fn reset_workbench_state_for_home(
    home: &Path,
    request: WorkbenchResetRequest,
    state: &AppState,
) -> Result<WorkbenchResetResult, String> {
    let _io_guard = state.workbench_io_lock.lock().await;
    reset_workbench_for_home(home, request).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use wardian_core::{
        models::workbench::{ClosedSurfaceV1, WorkbenchSurfaceV1, MAX_SAFE_INTEGER},
        workbench::{WorkbenchLoadSource, WorkbenchPersistenceOutcome},
    };

    #[test]
    fn workbench_safe_mode_requires_the_exact_value_one() {
        assert!(workbench_safe_mode_from_value(Some(OsStr::new("1"))));
        assert!(!workbench_safe_mode_from_value(None));
        assert!(!workbench_safe_mode_from_value(Some(OsStr::new("true"))));
        assert!(!workbench_safe_mode_from_value(Some(OsStr::new("01"))));
        assert!(!workbench_safe_mode_from_value(Some(OsStr::new("1 "))));
    }

    #[tokio::test]
    async fn workbench_command_helpers_return_structured_save_conflict_and_reset_results() {
        let home = tempfile::tempdir().expect("temp home");
        let state = AppState::new();

        let loaded = load_workbench_state_for_home(home.path(), &state)
            .await
            .expect("load default");
        assert_eq!(loaded.source, WorkbenchLoadSource::Default);
        let mut document = loaded.document.expect("default document");
        document.revision = 1;
        document.saved_at = "2026-07-10T12:34:56.789Z".to_string();

        let saved = save_workbench_state_for_home(
            home.path(),
            WorkbenchSaveRequest {
                document,
                expected_revision: 0,
                expected_token: loaded.durable_token.expect("default token"),
                request_id: "command-save".to_string(),
            },
            &state,
        )
        .await
        .expect("save result");
        assert_eq!(saved.outcome, WorkbenchPersistenceOutcome::Saved);
        assert_eq!(saved.request_id, "command-save");

        let conflict = reset_workbench_state_for_home(
            home.path(),
            WorkbenchResetRequest {
                expected_revision: 0,
                expected_token: "stale-token".to_string(),
                request_id: "command-conflict".to_string(),
            },
            &state,
        )
        .await
        .expect("structured conflict");
        assert_eq!(
            conflict.outcome,
            WorkbenchPersistenceOutcome::RevisionConflict
        );
        assert_eq!(conflict.durable_revision, Some(1));

        let reset = reset_workbench_state_for_home(
            home.path(),
            WorkbenchResetRequest {
                expected_revision: 1,
                expected_token: saved.durable_token.expect("saved token"),
                request_id: "command-reset".to_string(),
            },
            &state,
        )
        .await
        .expect("reset result");
        assert_eq!(reset.outcome, WorkbenchPersistenceOutcome::Saved);
        assert_eq!(reset.durable_revision, Some(2));
        assert_eq!(reset.request_id, "command-reset");
        assert_eq!(reset.document.expect("reset document").revision, 2);
    }

    #[tokio::test]
    async fn workbench_command_helper_rejects_semantically_invalid_input() {
        let home = tempfile::tempdir().expect("temp home");
        let state = AppState::new();
        let loaded = load_workbench_state_for_home(home.path(), &state)
            .await
            .expect("load default");
        let mut document = loaded.document.expect("default document");
        document.revision = MAX_SAFE_INTEGER + 1;

        let error = save_workbench_state_for_home(
            home.path(),
            WorkbenchSaveRequest {
                document,
                expected_revision: 0,
                expected_token: loaded.durable_token.expect("default token"),
                request_id: "command-invalid".to_string(),
            },
            &state,
        )
        .await
        .expect_err("invalid command input");

        assert!(error.contains("invalid workbench document"));
    }

    #[tokio::test]
    async fn loading_legacy_queue_surfaces_migrates_them_to_inbox() {
        let home = tempfile::tempdir().expect("temp home");
        let state = AppState::new();
        let loaded = load_workbench_state_for_home(home.path(), &state)
            .await
            .expect("load default");
        let mut document = loaded.document.expect("default document");
        let queue_surface = WorkbenchSurfaceV1 {
            surface_id: "surface-queue".to_string(),
            surface_type: "queue".to_string(),
            resource_key: None,
            presentation_provenance: None,
            state_schema_version: 1,
            state: serde_json::json!({}),
        };
        document
            .groups
            .get_mut("group-1")
            .expect("default group")
            .surface_ids
            .push(queue_surface.surface_id.clone());
        document
            .groups
            .get_mut("group-1")
            .expect("default group")
            .active_surface_id = Some(queue_surface.surface_id.clone());
        document
            .surfaces
            .insert(queue_surface.surface_id.clone(), queue_surface.clone());
        document.recently_closed.push(ClosedSurfaceV1 {
            surface: WorkbenchSurfaceV1 {
                surface_id: "closed-queue".to_string(),
                ..queue_surface
            },
            previous_group_id: "group-1".to_string(),
            previous_index: 0,
        });
        document.revision = 1;
        document.saved_at = "2026-07-21T12:00:00.000Z".to_string();

        save_workbench_state_for_home(
            home.path(),
            WorkbenchSaveRequest {
                document,
                expected_revision: 0,
                expected_token: loaded.durable_token.expect("default token"),
                request_id: "save-legacy-queue".to_string(),
            },
            &state,
        )
        .await
        .expect("save legacy document");

        let migrated = load_workbench_state_for_home(home.path(), &state)
            .await
            .expect("load migrated document")
            .document
            .expect("migrated document");
        assert_eq!(
            migrated.surfaces["surface-queue"].surface_type,
            "inbox"
        );
        assert_eq!(migrated.recently_closed[0].surface.surface_type, "inbox");
    }
}
