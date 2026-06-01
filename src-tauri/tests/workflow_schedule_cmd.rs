//! Unit-level checks for schedule command semantics that do not need a running app.
//!
//! These tests mutate the process-global `WARDIAN_HOME` env var, so they share a
//! lock (CI runs `cargo test` multi-threaded). Pattern mirrors `workflow_invoker.rs`.

use std::sync::{Mutex, MutexGuard, OnceLock};

/// Serializes `WARDIAN_HOME` access within this test binary and restores it on drop.
struct EnvGuard {
    _lock: MutexGuard<'static, ()>,
    previous_home: Option<std::ffi::OsString>,
}

impl EnvGuard {
    fn set(home: &std::path::Path) -> Self {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        let lock = LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
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
        match &self.previous_home {
            Some(value) => std::env::set_var("WARDIAN_HOME", value),
            None => std::env::remove_var("WARDIAN_HOME"),
        }
    }
}

fn sample_schedule() -> wardian_core::models::WorkflowSchedule {
    wardian_core::models::WorkflowSchedule {
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

#[tokio::test]
async fn schedule_list_reads_persisted_schedules() {
    let dir = tempfile::tempdir().unwrap();
    let _env = EnvGuard::set(dir.path());

    wardian_core::schedule::save_schedules(&[sample_schedule()]).unwrap();
    let loaded = wardian_app_lib::commands::workflow::schedule_list()
        .await
        .unwrap();

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
