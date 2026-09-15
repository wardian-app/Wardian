// Included in telemetry::tests so these cases use the production parsing and publication steps.

#[test]
fn initial_log_replay_does_not_record_status_transition() {
    let snap = test_snapshot("Off");

    super::set_snapshot_status_from_log(&snap, "Idle", true);

    assert_eq!(*snap.current_status.lock().unwrap(), "Off");
    assert!(snap.last_status_at.lock().unwrap().is_none());
    let snapshot = snap
        .watch_state
        .lock()
        .unwrap()
        .snapshot_since(None, None)
        .unwrap();
    assert!(snapshot.events.is_empty());
}

#[test]
fn live_log_update_records_status_transition() {
    let snap = test_snapshot("Processing...");

    super::set_snapshot_status_from_log(&snap, "Idle", false);

    assert_eq!(*snap.current_status.lock().unwrap(), "Idle");
    assert!(snap.last_status_at.lock().unwrap().is_some());
    let snapshot = snap
        .watch_state
        .lock()
        .unwrap()
        .snapshot_since(None, None)
        .unwrap();
    assert_eq!(snapshot.events.len(), 1);
    assert_eq!(
        snapshot.events[0]
            .payload
            .get("status")
            .and_then(|value| value.as_str()),
        Some("idle")
    );
}

#[test]
fn opencode_starting_ignores_retained_log_status_until_composer_ready() {
    let snap = test_snapshot("Starting");
    for retained in ["Idle", "Processing...", "Error"] {
        super::set_snapshot_status_from_log(&snap, retained, false);
        assert_eq!(*snap.current_status.lock().unwrap(), "Starting");
    }
    // A provider transition since the log read must remain authoritative;
    // the startup fence must not write a captured Starting value back.
    *snap.current_status.lock().unwrap() = "Processing...".to_string();
    super::set_snapshot_status_from_log(&snap, "Idle", false);
    assert_eq!(
        *snap.current_status.lock().unwrap(),
        "Idle",
        "normal turn completion remains observable after startup"
    );
}
