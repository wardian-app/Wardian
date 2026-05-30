//! Unit-level checks for schedule command semantics that do not need a running app.

fn sample_schedule() -> wardian_core::models::WorkflowSchedule {
    wardian_core::models::WorkflowSchedule {
        id: "s1".into(),
        blueprint_id: "heartbeat".into(),
        name: "HB".into(),
        provider: None,
        workspace: None,
        input: serde_json::json!({}),
        bindings: Default::default(),
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

#[tokio::test]
async fn schedule_list_v2_reads_persisted_schedules() {
    let dir = tempfile::tempdir().unwrap();
    std::env::set_var("WARDIAN_HOME", dir.path());

    wardian_core::schedule::save_schedules(&[sample_schedule()]).unwrap();
    let loaded = wardian_app_lib::commands::workflow::schedule_list_v2()
        .await
        .unwrap();

    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].id, "s1");
    std::env::remove_var("WARDIAN_HOME");
}

#[test]
fn pause_then_resume_round_trips_via_core() {
    let dir = tempfile::tempdir().unwrap();
    std::env::set_var("WARDIAN_HOME", dir.path());

    let mut schedule = sample_schedule();
    schedule.is_paused = true;
    schedule.paused_remaining_ms = Some(1234);
    schedule.next_run_epoch_ms = None;
    wardian_core::schedule::save_schedules(&[schedule]).unwrap();

    let loaded = wardian_core::schedule::load_schedules();
    assert_eq!(loaded.len(), 1);
    assert!(loaded[0].is_paused);
    assert_eq!(loaded[0].paused_remaining_ms, Some(1234));
    std::env::remove_var("WARDIAN_HOME");
}
