use crate::state::AppState;
use std::path::Path;
use tauri::State;
use wardian_core::{
    models::workbench::WorkbenchDocumentV1,
    workbench::{
        load_workbench_for_home, reset_workbench_for_home, save_workbench_for_home,
        WorkbenchLoadResult, WorkbenchResetRequest, WorkbenchResetResult, WorkbenchSaveRequest,
        WorkbenchSaveResult,
    },
};

#[tauri::command]
pub async fn load_workbench_state(
    state: State<'_, AppState>,
) -> Result<WorkbenchLoadResult, String> {
    let home = crate::utils::fs::get_wardian_home()
        .ok_or_else(|| "Could not resolve Wardian home".to_string())?;
    load_workbench_state_for_home(&home, state.inner()).await
}

#[tauri::command]
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

#[tauri::command]
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
    load_workbench_for_home(home).map_err(|error| error.to_string())
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
        models::workbench::MAX_SAFE_INTEGER,
        workbench::{WorkbenchLoadSource, WorkbenchPersistenceOutcome},
    };

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
}
