use raw_window_handle::{HasDisplayHandle, HasWindowHandle};
use std::{sync::Arc, time::Duration};
use tauri::Manager;
use tauri_plugin_dialog::{MessageDialogButtons, MessageDialogKind, MessageDialogResult};
use wardian_core::memory::{
    MemoryActor, MemoryMaintenancePlan, MemoryMaintenancePreview, MemoryMaintenanceReceipt,
    MemoryRecord, MemoryStore, RecallResult, MAX_PLAN_BYTES,
};

const MEMORY_MAINTENANCE_CONFIRMATION_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MaintenanceConfirmation {
    Accepted,
    Declined,
    Cancelled,
}

#[tauri::command]
pub async fn memory_list(
    agent_id: String,
    workspace: Option<String>,
) -> Result<Vec<MemoryRecord>, String> {
    MemoryStore::from_default_home()
        .and_then(|store| {
            store.list_active(&MemoryActor::Operator, &agent_id, workspace.as_deref())
        })
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn memory_get(memory_id: String) -> Result<MemoryRecord, String> {
    MemoryStore::from_default_home()
        .and_then(|store| store.get(&MemoryActor::Operator, &memory_id))
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn memory_history(memory_id: String) -> Result<Vec<MemoryRecord>, String> {
    MemoryStore::from_default_home()
        .and_then(|store| store.history(&MemoryActor::Operator, &memory_id))
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn memory_recall(
    agent_id: String,
    workspace: Option<String>,
) -> Result<RecallResult, String> {
    MemoryStore::from_default_home()
        .and_then(|store| store.recall(&MemoryActor::Operator, &agent_id, workspace.as_deref()))
        .map_err(|error| error.to_string())
}

#[tauri::command]
/// Parses a raw maintenance plan before JavaScript materializes its JSON objects.
pub fn memory_maintenance_parse(raw_json: String) -> Result<MemoryMaintenancePlan, String> {
    if raw_json.len() > MAX_PLAN_BYTES {
        return Err(format!(
            "plan exceeds 1 MiB limit: {} bytes",
            raw_json.len()
        ));
    }

    serde_json::from_str(&raw_json).map_err(|error| error.to_string())
}

#[tauri::command]
/// Returns an operator preview without changing memory records or receipts.
pub async fn memory_maintenance_preview(
    plan: MemoryMaintenancePlan,
) -> Result<MemoryMaintenancePreview, String> {
    with_memory_store("maintenance preview", move |store| {
        store
            .preview_maintenance(&MemoryActor::Operator, &plan)
            .map_err(|error| error.to_string())
    })
    .await
}

#[tauri::command]
/// Applies a reviewed plan after native confirmation, returning exact replays before prompting.
pub async fn memory_maintenance_apply(
    plan: MemoryMaintenancePlan,
    preview_digest: String,
    window: tauri::WebviewWindow,
) -> Result<MemoryMaintenanceReceipt, String> {
    let plan = Arc::new(plan);
    let replay_plan = Arc::clone(&plan);
    let replay = with_memory_store("maintenance replay", move |store| {
        store
            .maintenance_replay(&MemoryActor::Operator, &replay_plan)
            .map_err(|error| error.to_string())
    })
    .await;

    let apply_plan = Arc::clone(&plan);
    let confirm_plan = Arc::clone(&plan);
    let confirm_digest = preview_digest.clone();
    apply_after_replay(
        async move { replay },
        move || async move {
            confirm_memory_maintenance(&window, &confirm_plan, &confirm_digest).await
        },
        move || async move {
            with_memory_store("maintenance apply", move |store| {
                store
                    .apply_maintenance(&MemoryActor::Operator, &apply_plan, &preview_digest)
                    .map_err(|error| error.to_string())
            })
            .await
        },
    )
    .await
}

#[tauri::command]
/// Resolves an uncertain apply response using its owner and idempotency key.
pub async fn memory_maintenance_receipt(
    agent_id: String,
    idempotency_key: String,
) -> Result<Option<MemoryMaintenanceReceipt>, String> {
    with_memory_store("maintenance receipt", move |store| {
        store
            .maintenance_receipt(&MemoryActor::Operator, &agent_id, &idempotency_key)
            .map_err(|error| error.to_string())
    })
    .await
}

async fn with_memory_store<T, F>(label: &'static str, operation: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce(MemoryStore) -> Result<T, String> + Send + 'static,
{
    // SQLite work is synchronous and must not occupy a Tauri async runtime worker.
    tokio::task::spawn_blocking(move || {
        let store = MemoryStore::from_default_home().map_err(|error| error.to_string())?;
        operation(store)
    })
    .await
    .map_err(|error| format!("memory {label} worker failed: {error}"))?
}

async fn confirm_memory_maintenance(
    window: &tauri::WebviewWindow,
    plan: &MemoryMaintenancePlan,
    preview_digest: &str,
) -> Result<MaintenanceConfirmation, String> {
    let dialog = window
        .try_state::<tauri_plugin_dialog::Dialog<tauri::Wry>>()
        .ok_or_else(|| "native memory maintenance confirmation is unavailable".to_string())?;
    let message =
        maintenance_confirmation_message(&plan.agent_id, plan.operations.len(), preview_digest);
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let builder = dialog
        .message(message)
        .title("Wardian memory maintenance")
        .buttons(MessageDialogButtons::YesNoCancel)
        .kind(MessageDialogKind::Warning);
    with_dialog_parent_handles(
        || {
            window
                .window_handle()
                .map(|_| ())
                .map_err(|error| error.to_string())
        },
        || {
            window
                .display_handle()
                .map(|_| ())
                .map_err(|error| error.to_string())
        },
        move || {
            builder.parent(window).show_with_result(move |result| {
                let _ = sender.send(maintenance_confirmation_from_dialog(result));
            });
        },
    )?;

    receive_native_confirmation(receiver, MEMORY_MAINTENANCE_CONFIRMATION_TIMEOUT).await
}

fn with_dialog_parent_handles(
    window_handle: impl FnOnce() -> Result<(), String>,
    display_handle: impl FnOnce() -> Result<(), String>,
    show_dialog: impl FnOnce(),
) -> Result<(), String> {
    window_handle().map_err(|error| {
        format!("native memory maintenance window handle is unavailable: {error}")
    })?;
    display_handle().map_err(|error| {
        format!("native memory maintenance display handle is unavailable: {error}")
    })?;
    show_dialog();
    Ok(())
}

fn maintenance_confirmation_message(
    agent_id: &str,
    operation_count: usize,
    preview_digest: &str,
) -> String {
    let owner = agent_id.escape_debug().to_string();
    let digest_prefix_chars = "sha256:".len() + 12;
    let digest_chars = preview_digest.chars().count();
    let digest_prefix = preview_digest
        .chars()
        .take(digest_prefix_chars)
        .collect::<String>()
        .escape_debug()
        .to_string();
    let digest_suffix = if digest_chars > digest_prefix_chars {
        "…"
    } else {
        ""
    };

    format!(
        "Apply this reviewed memory maintenance plan?\n\nOwner ID: {owner}\nOperation count: {operation_count}\nPreview digest prefix: {digest_prefix}{digest_suffix}\n\nChoose Yes to apply. No or Cancel leaves memory unchanged."
    )
}

fn maintenance_confirmation_from_dialog(
    result: MessageDialogResult,
) -> Result<MaintenanceConfirmation, String> {
    match result {
        MessageDialogResult::Yes => Ok(MaintenanceConfirmation::Accepted),
        MessageDialogResult::No => Ok(MaintenanceConfirmation::Declined),
        MessageDialogResult::Cancel => Ok(MaintenanceConfirmation::Cancelled),
        _ => Err("native memory maintenance confirmation is unavailable".into()),
    }
}

async fn receive_native_confirmation(
    receiver: tokio::sync::oneshot::Receiver<Result<MaintenanceConfirmation, String>>,
    timeout: Duration,
) -> Result<MaintenanceConfirmation, String> {
    match tokio::time::timeout(timeout, receiver).await {
        Ok(Ok(result)) => result,
        Ok(Err(_)) => Err("native memory maintenance confirmation is unavailable".into()),
        Err(_) => Err("native memory maintenance confirmation timed out".into()),
    }
}

async fn apply_after_replay<T, R, C, CF, A, AF>(
    replay: R,
    confirm: C,
    apply: A,
) -> Result<T, String>
where
    R: std::future::Future<Output = Result<Option<T>, String>>,
    C: FnOnce() -> CF,
    CF: std::future::Future<Output = Result<MaintenanceConfirmation, String>>,
    A: FnOnce() -> AF,
    AF: std::future::Future<Output = Result<T, String>>,
{
    // Construct the dialog only after replay lookup proves this is a new apply.
    match replay.await? {
        Some(receipt) => Ok(receipt),
        None => apply_after_confirmation(confirm(), apply).await,
    }
}

async fn apply_after_confirmation<T, C, A, AF>(confirmation: C, apply: A) -> Result<T, String>
where
    C: std::future::Future<Output = Result<MaintenanceConfirmation, String>>,
    A: FnOnce() -> AF,
    AF: std::future::Future<Output = Result<T, String>>,
{
    match confirmation.await? {
        MaintenanceConfirmation::Accepted => apply().await,
        MaintenanceConfirmation::Declined => {
            Err("native memory maintenance confirmation was declined".into())
        }
        MaintenanceConfirmation::Cancelled => {
            Err("native memory maintenance confirmation was cancelled".into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        apply_after_confirmation, apply_after_replay, maintenance_confirmation_from_dialog,
        maintenance_confirmation_message, memory_maintenance_parse, receive_native_confirmation,
        with_dialog_parent_handles, MaintenanceConfirmation, MAX_PLAN_BYTES,
    };
    use std::cell::Cell;
    use std::future::{ready, Future};
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
    use std::time::Duration;
    use tauri_plugin_dialog::MessageDialogResult;

    #[test]
    fn confirmation_names_owner_operation_count_and_digest_prefix() {
        let digest = format!("sha256:{}", "0123456789abcdef".repeat(4));
        let message = maintenance_confirmation_message("agent-123", 4, &digest);

        assert!(message.contains("Owner ID: agent-123"));
        assert!(message.contains("Operation count: 4"));
        assert!(message.contains("Preview digest prefix: sha256:0123456789ab…"));
        assert!(!message.contains(&digest));
    }

    #[tokio::test]
    async fn exact_replay_returns_receipt_without_confirmation_or_apply() {
        let confirmation_calls = Arc::new(AtomicUsize::new(0));
        let confirm_calls = Arc::clone(&confirmation_calls);
        let apply_calls = Arc::clone(&confirmation_calls);

        let result = apply_after_replay(
            ready(Ok(Some("existing receipt"))),
            move || {
                confirm_calls.fetch_add(1, Ordering::SeqCst);
                ready(Ok(MaintenanceConfirmation::Accepted))
            },
            move || async move {
                apply_calls.fetch_add(1, Ordering::SeqCst);
                Ok("new receipt")
            },
        )
        .await;

        assert_eq!(result.unwrap(), "existing receipt");
        assert_eq!(confirmation_calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn replay_rejects_key_reuse_for_another_plan_without_confirmation() {
        let confirmation_calls = Arc::new(AtomicUsize::new(0));
        let confirm_calls = Arc::clone(&confirmation_calls);
        let apply_calls = Arc::clone(&confirmation_calls);

        let result = apply_after_replay(
            ready(Err(
                "idempotency key is already bound to another plan".into()
            )),
            move || {
                confirm_calls.fetch_add(1, Ordering::SeqCst);
                ready(Ok(MaintenanceConfirmation::Accepted))
            },
            move || async move {
                apply_calls.fetch_add(1, Ordering::SeqCst);
                Ok("new receipt")
            },
        )
        .await;

        assert!(result.unwrap_err().contains("another plan"));
        assert_eq!(confirmation_calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn accepted_confirmation_runs_apply_exactly_once() {
        let calls = Arc::new(AtomicUsize::new(0));
        let apply_calls = Arc::clone(&calls);
        let result = apply_after_confirmation(
            ready(Ok(MaintenanceConfirmation::Accepted)),
            move || async move {
                apply_calls.fetch_add(1, Ordering::SeqCst);
                Ok("receipt")
            },
        )
        .await;

        assert_eq!(result.unwrap(), "receipt");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn declined_cancelled_and_unavailable_confirmation_never_run_apply() {
        assert!(
            assert_apply_not_called(ready(Ok(MaintenanceConfirmation::Declined)))
                .await
                .unwrap_err()
                .contains("declined")
        );
        assert!(
            assert_apply_not_called(ready(Ok(MaintenanceConfirmation::Cancelled)))
                .await
                .unwrap_err()
                .contains("cancelled")
        );
        assert!(
            assert_apply_not_called(ready(Err("native dialog unavailable".into())))
                .await
                .unwrap_err()
                .contains("unavailable")
        );
    }

    #[test]
    fn native_dialog_results_keep_decline_and_cancel_distinct() {
        assert_eq!(
            maintenance_confirmation_from_dialog(MessageDialogResult::Yes),
            Ok(MaintenanceConfirmation::Accepted)
        );
        assert_eq!(
            maintenance_confirmation_from_dialog(MessageDialogResult::No),
            Ok(MaintenanceConfirmation::Declined)
        );
        assert_eq!(
            maintenance_confirmation_from_dialog(MessageDialogResult::Cancel),
            Ok(MaintenanceConfirmation::Cancelled)
        );
        assert!(maintenance_confirmation_from_dialog(MessageDialogResult::Ok).is_err());
    }

    #[test]
    fn unavailable_parent_handle_fails_closed_before_showing_dialog() {
        let shown = Cell::new(false);
        let result = with_dialog_parent_handles(
            || Err("raw window handle unavailable".into()),
            || Ok(()),
            || shown.set(true),
        );
        assert!(result.unwrap_err().contains("window handle is unavailable"));
        assert!(!shown.get());

        let shown = Cell::new(false);
        let result = with_dialog_parent_handles(
            || Ok(()),
            || Err("raw display handle unavailable".into()),
            || shown.set(true),
        );
        assert!(result
            .unwrap_err()
            .contains("display handle is unavailable"));
        assert!(!shown.get());
    }

    #[tokio::test(start_paused = true)]
    async fn unavailable_callback_and_timeout_never_run_apply() {
        let (sender, receiver) = tokio::sync::oneshot::channel();
        drop(sender);
        assert!(assert_apply_not_called(receive_native_confirmation(
            receiver,
            Duration::from_secs(1),
        ))
        .await
        .unwrap_err()
        .contains("unavailable"));

        let (sender, receiver) = tokio::sync::oneshot::channel();
        let result = assert_apply_not_called(receive_native_confirmation(
            receiver,
            Duration::from_secs(120),
        ))
        .await;
        drop(sender);
        assert!(result.unwrap_err().contains("timed out"));
    }

    async fn assert_apply_not_called<C>(confirmation: C) -> Result<(), String>
    where
        C: Future<Output = Result<MaintenanceConfirmation, String>>,
    {
        let calls = Arc::new(AtomicUsize::new(0));
        let apply_calls = Arc::clone(&calls);
        let result = apply_after_confirmation(confirmation, move || async move {
            apply_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
        .await;

        assert_eq!(calls.load(Ordering::SeqCst), 0);
        result
    }

    #[test]
    fn raw_maintenance_parse_rejects_duplicate_scope_keys() {
        let duplicate_kind = r#"{"schema_version":1,"plan_id":"plan-1","agent_id":"agent-a","idempotency_key":"key-1","operations":[{"op":"create","client_key":"new-1","text":"text","kind":"stable","scope":{"kind":"agent","kind":"workspace","path":"/workspace"},"evidence_excerpt":"evidence"}]}"#;
        let err_kind = memory_maintenance_parse(duplicate_kind.into()).unwrap_err();
        assert!(err_kind.contains("duplicate field `kind`"), "{err_kind}");

        let duplicate_path = r#"{"schema_version":1,"plan_id":"plan-1","agent_id":"agent-a","idempotency_key":"key-1","operations":[{"op":"create","client_key":"new-1","text":"text","kind":"stable","scope":{"kind":"workspace","path":"/first","path":"/second"},"evidence_excerpt":"evidence"}]}"#;
        let err_path = memory_maintenance_parse(duplicate_path.into()).unwrap_err();
        assert!(err_path.contains("duplicate field `path`"), "{err_path}");
    }

    #[test]
    fn raw_maintenance_parse_preserves_scope_wire_shapes() {
        let raw_plan = r#"{"schema_version":1,"plan_id":"plan-1","agent_id":"agent-a","idempotency_key":"key-1","operations":[{"op":"create","client_key":"agent-scope","text":"text","kind":"stable","scope":{"kind":"agent"},"evidence_excerpt":"evidence"},{"op":"create","client_key":"workspace-scope","text":"text","kind":"stable","scope":{"kind":"workspace","path":"/workspace"},"evidence_excerpt":"evidence"}]}"#;
        let plan = memory_maintenance_parse(raw_plan.into()).unwrap();
        let serialized = serde_json::to_value(plan).unwrap();

        assert_eq!(
            serialized["operations"][0]["scope"],
            serde_json::json!({ "kind": "agent" })
        );
        assert_eq!(
            serialized["operations"][1]["scope"],
            serde_json::json!({ "kind": "workspace", "path": "/workspace" })
        );
    }

    #[test]
    fn raw_maintenance_parse_enforces_the_byte_limit() {
        let oversized = " ".repeat(MAX_PLAN_BYTES + 1);
        let error = memory_maintenance_parse(oversized).unwrap_err();
        assert!(error.contains("plan exceeds 1 MiB limit"), "{error}");
    }
}
