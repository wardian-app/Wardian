use super::AgentSnapshot;

pub(super) fn set_snapshot_status(snap: &AgentSnapshot, next_status: &str) {
    if snap.provider == "codex"
        && matches!(
            wardian_core::identity::normalize_status(next_status).as_str(),
            "idle" | "processing"
        )
        && !snap
            .watch_state
            .lock()
            .is_ok_and(|watch_state| watch_state.codex_attachment_ready())
    {
        return;
    }
    let mut status = snap.current_status.lock().unwrap();
    if *status == next_status {
        return;
    }
    *status = next_status.to_string();
    drop(status);

    let observed_at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let _ = wardian_core::db::update_agent_status(&snap.session_id, next_status, None);
    if let Ok(mut last_status_at) = snap.last_status_at.lock() {
        *last_status_at = Some(observed_at.clone());
    }
    if let Ok(mut watch_state) = snap.watch_state.lock() {
        watch_state.push_event(
            "status",
            serde_json::json!({
                "status": wardian_core::identity::normalize_status(next_status),
                "observed_at": observed_at,
            }),
        );
    }
}
