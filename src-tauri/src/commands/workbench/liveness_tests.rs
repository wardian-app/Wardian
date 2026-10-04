use super::*;

#[tokio::test(flavor = "current_thread")]
async fn workbench_save_keeps_the_async_executor_responsive_during_io() {
    let home = tempfile::tempdir().expect("temp home");
    let state = std::sync::Arc::new(AppState::new());
    let loaded = load_workbench_for_home(home.path()).expect("load default");
    let mut document = loaded.document.expect("default document");
    document.revision = 1;
    document.saved_at = "2026-10-04T00:00:00.000Z".to_string();
    let request = WorkbenchSaveRequest {
        document,
        expected_revision: 0,
        expected_token: loaded.durable_token.expect("default token"),
        request_id: "executor-liveness".to_string(),
    };
    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let (heartbeat_tx, heartbeat_rx) = std::sync::mpsc::channel();
    // A supervisor releases the controlled I/O even when the executor is blocked.
    // The deadline starts only after the actual save I/O section has been entered.
    let supervisor = std::thread::spawn(move || {
        let responsive = started_rx
            .recv_timeout(std::time::Duration::from_secs(3))
            .is_ok()
            && heartbeat_rx
                .recv_timeout(std::time::Duration::from_millis(500))
                .is_ok();
        let _ = release_tx.send(());
        responsive
    });
    let path = home.path().to_path_buf();
    let task_state = state.clone();
    let save = tokio::spawn(async move {
        WORKBENCH_IO_PROBE
            .scope(
                std::cell::RefCell::new(Some(WorkbenchIoProbe {
                    entered: entered_tx,
                    started: started_tx,
                    release: release_rx,
                })),
                save_workbench_state_for_home(&path, request, &task_state),
            )
            .await
    });
    entered_rx
        .await
        .expect("save enters its actual I/O section");
    let _ = heartbeat_tx.send(());
    let saved = save.await.expect("save task joins").expect("save succeeds");
    let responsive = supervisor.join().expect("supervisor joins");
    assert_eq!(saved.outcome, WorkbenchPersistenceOutcome::Saved);
    assert!(
        responsive,
        "workbench save blocked the async executor during I/O"
    );
    let persisted = load_workbench_for_home(home.path()).expect("read saved state");
    assert_eq!(persisted.document.expect("saved document").revision, 1);
}

#[tokio::test(flavor = "current_thread")]
async fn cancelled_workbench_save_retains_exclusion_until_io_finishes() {
    let home = tempfile::tempdir().expect("temp home");
    let state = std::sync::Arc::new(AppState::new());
    let loaded = load_workbench_for_home(home.path()).expect("load default");
    let mut document = loaded.document.expect("default document");
    document.revision = 1;
    document.saved_at = "2026-10-04T00:00:00.000Z".to_string();
    let request = WorkbenchSaveRequest {
        document,
        expected_revision: 0,
        expected_token: loaded.durable_token.expect("default token"),
        request_id: "cancelled-save".to_string(),
    };
    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let (release_requested_tx, release_requested_rx) = std::sync::mpsc::channel();
    let supervisor = std::thread::spawn(move || {
        let _ = started_rx.recv_timeout(std::time::Duration::from_secs(3));
        let _ = release_requested_rx.recv_timeout(std::time::Duration::from_secs(3));
        let _ = release_tx.send(());
    });
    let path = home.path().to_path_buf();
    let task_state = state.clone();
    let save = tokio::spawn(async move {
        WORKBENCH_IO_PROBE
            .scope(
                std::cell::RefCell::new(Some(WorkbenchIoProbe {
                    entered: entered_tx,
                    started: started_tx,
                    release: release_rx,
                })),
                save_workbench_state_for_home(&path, request, &task_state),
            )
            .await
    });
    entered_rx
        .await
        .expect("save enters I/O with exclusion held");
    save.abort();
    let cancelled = save.await.expect_err("caller was cancelled").is_cancelled();
    let exclusion_retained = state.workbench_io_lock.try_lock().is_err();
    let path = home.path().to_path_buf();
    let task_state = state.clone();
    let mut successor =
        tokio::spawn(async move { load_workbench_state_for_home(&path, &task_state).await });
    let early_result =
        tokio::time::timeout(std::time::Duration::from_millis(50), &mut successor).await;
    let successor_waited = early_result.is_err();
    // Always unblock and join before assertions; cancellation must not strand I/O.
    release_requested_tx.send(()).expect("release save I/O");
    let loaded = match early_result {
        Ok(result) => result,
        Err(_) => tokio::time::timeout(std::time::Duration::from_secs(3), successor)
            .await
            .expect("successor completes"),
    }
    .expect("successor joins")
    .expect("successor loads after cancelled save");
    supervisor.join().expect("release supervisor joins");
    assert!(cancelled);
    assert!(exclusion_retained);
    assert!(successor_waited, "successor overlapped unfinished save I/O");
    assert_eq!(loaded.document.expect("committed document").revision, 1);
}

#[tokio::test(flavor = "current_thread")]
async fn workbench_io_errors_and_panics_release_exclusion() {
    let home = tempfile::tempdir().expect("temp home");
    let state = AppState::new();
    let error = run_workbench_io::<()>(&state, || Err("controlled I/O failure".to_string()))
        .await
        .expect_err("operation reports error");
    assert_eq!(error, "controlled I/O failure");
    let panic = run_workbench_io::<()>(&state, || panic!("controlled I/O panic"))
        .await
        .expect_err("worker panic is returned as an error");
    assert!(panic.starts_with("Workbench I/O task failed:"));
    assert!(state.workbench_io_lock.try_lock().is_ok());
    load_workbench_state_for_home(home.path(), &state)
        .await
        .expect("subsequent load succeeds");
}
