// Included in telemetry::tests so these cases use the production parsing and publication steps.

/// Exercise the production log/status/readiness/drain steps without a
/// process inventory or a provider process. The caller owns the log and
/// supplies whether this is the initial read or a subsequent append.
pub(crate) async fn apply_opencode_startup_log_pass(
    state: &crate::state::AppState,
    session_id: &str,
    log_path: &std::path::Path,
    initial_replay: bool,
) {
    let mut snap = test_snapshot("Starting");
    let config = {
        let agents = state.agents.lock().await;
        let agent = agents.get(session_id).unwrap();
        snap.session_id = session_id.to_string();
        snap.current_status = agent.current_status.clone();
        snap.watch_state = agent.watch_state.clone();
        snap.last_status_at = agent.last_status_at.clone();
        agent.config.clone()
    };
    snap.resume_session = config.lock().unwrap().resume_session.clone();
    snap.provider_generation = state
        .interactions
        .current_provider_input_generation(session_id)
        .await
        .unwrap();
    let status_before_log = snap.current_status.lock().unwrap().clone();
    let mut status = status_before_log.clone();
    super::apply_opencode_log_metrics(
        &super::read_log_bounded(log_path).unwrap(),
        snap.resume_session.as_deref().unwrap(),
        &mut 0,
        &mut None,
        &mut None,
        &mut status,
    );
    let status = super::reconcile_live_opencode_log_status(
        "opencode",
        &status_before_log,
        status,
        Some(true),
        None,
    );
    super::set_snapshot_status_from_log(&snap, &status, initial_replay);
    let status = snap.current_status.lock().unwrap().clone();
    super::apply_provider_status_observations(
        state,
        &[super::TelemetryProviderStatus {
            session_id: session_id.to_string(),
            generation: snap.provider_generation,
            status,
            current_status: snap.current_status,
        }],
    )
    .await;
}

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
