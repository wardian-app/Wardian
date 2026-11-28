use super::super::*;

fn live_sample(
    _: Arc<tokio::sync::Mutex<sysinfo::System>>,
    _: Vec<String>,
    roots: Vec<(String, Option<u32>)>,
) -> Option<SystemProcessSnapshot> {
    Some(SystemProcessSnapshot {
        logical_cpu_count: 2,
        children_map: Arc::new(HashMap::new()),
        processes: Arc::new(
            roots
                .into_iter()
                .filter_map(|(_, pid)| {
                    pid.map(|pid| {
                        (
                            pid,
                            ProcessSample {
                                cpu_usage: 24.0,
                                memory: 64 * 1024 * 1024,
                                run_time: 80,
                            },
                        )
                    })
                })
                .collect(),
        ),
        sys_refresh: std::time::Duration::ZERO,
        #[cfg(windows)]
        session_roots: HashMap::new(),
    })
}

fn missing_sample(
    _: Arc<tokio::sync::Mutex<sysinfo::System>>,
    _: Vec<String>,
    _: Vec<(String, Option<u32>)>,
) -> Option<SystemProcessSnapshot> {
    None
}

fn values(metric: &AgentTelemetry) -> (f32, f64, u64) {
    (metric.cpu_usage, metric.memory_mb, metric.uptime_seconds)
}

#[tokio::test]
async fn ordinary_collection_publication_retains_resources_and_resumes_after_gap() {
    let _home = crate::control::test_support::TestWardianHome::new_async().await;
    let state = AppState::new();
    let agent = super::super::tests::test_active_agent("resource-gap", "mock", "Idle", Some(1234));
    let owner = agent.current_status.clone();
    state
        .agents
        .lock()
        .await
        .insert("resource-gap".into(), agent);

    let (first, _) = collect_agent_metrics_with_sampler(&state, live_sample).await;
    assert_eq!(values(&first[0]), (12.0, 64.0, 80));
    assert_eq!(first[0].query_count, 0);
    for _ in 0..2 {
        let (gap, _) = collect_agent_metrics_with_sampler(&state, missing_sample).await;
        assert_eq!(values(&gap[0]), values(&first[0]));
        assert_eq!(gap[0].current_status, "Idle");
        assert_eq!(
            *owner.lock().unwrap(),
            "Idle",
            "unknown liveness cannot force Off"
        );
        assert!(
            gap[0].uptime_seconds > 4,
            "normal frontend fallback remains Ready, never Booting"
        );
        if let Ok(path) = std::env::var("WARDIAN_RESOURCE_GAP_EVIDENCE") {
            std::fs::write(path, serde_json::to_vec_pretty(&gap).unwrap()).unwrap();
        }
    }
    let (resumed, _) = collect_agent_metrics_with_sampler(&state, |sys, ids, roots| {
        let mut snapshot = live_sample(sys, ids, roots).unwrap();
        snapshot.processes = Arc::new(HashMap::from([(
            1234,
            ProcessSample {
                cpu_usage: 8.0,
                memory: 32 * 1024 * 1024,
                run_time: 90,
            },
        )]));
        Some(snapshot)
    })
    .await;
    assert_eq!(values(&resumed[0]), (4.0, 32.0, 90));
    let (gap, _) = collect_agent_metrics_with_sampler(&state, missing_sample).await;
    assert_eq!(values(&gap[0]), values(&resumed[0]));
    let (dead, _) = collect_agent_metrics_with_sampler(&state, |_, _, _| {
        Some(SystemProcessSnapshot {
            logical_cpu_count: 2,
            children_map: Arc::new(HashMap::new()),
            processes: Arc::new(HashMap::new()),
            sys_refresh: std::time::Duration::ZERO,
            #[cfg(windows)]
            session_roots: HashMap::new(),
        })
    })
    .await;
    assert_eq!(
        dead[0].current_status, "Off",
        "fresh absence still proves death"
    );
}

#[tokio::test]
async fn replacement_and_other_state_cannot_inherit_resource_samples() {
    let _home = crate::control::test_support::TestWardianHome::new_async().await;
    let state = AppState::new();
    state.agents.lock().await.insert(
        "same-session".into(),
        super::super::tests::test_active_agent("same-session", "mock", "Idle", Some(1234)),
    );
    collect_agent_metrics_with_sampler(&state, live_sample).await;
    // Keep the retired owner alive, just as an overlapping collection pass would.
    let retired = state.agents.lock().await.remove("same-session").unwrap();
    state.agents.lock().await.insert(
        "same-session".into(),
        super::super::tests::test_active_agent("same-session", "mock", "Idle", Some(1234)),
    );
    let (replacement, _) = collect_agent_metrics_with_sampler(&state, missing_sample).await;
    assert_eq!(values(&replacement[0]), (0.0, 0.0, 0));
    assert_eq!(replacement[0].current_status, "Idle");
    let other_state = AppState::new();
    other_state.agents.lock().await.insert(
        "same-session".into(),
        super::super::tests::test_active_agent("same-session", "mock", "Idle", Some(1234)),
    );
    let (other, _) = collect_agent_metrics_with_sampler(&other_state, missing_sample).await;
    assert_eq!(values(&other[0]), (0.0, 0.0, 0));
    drop(retired);
}

#[tokio::test]
async fn retired_gap_pass_cannot_publish_for_replaced_or_deleted_runtime() {
    let _home = crate::control::test_support::TestWardianHome::new_async().await;
    let state = Arc::new(AppState::new());
    for replace in [true, false] {
        state.agents.lock().await.insert(
            "overlap".into(),
            super::super::tests::test_active_agent("overlap", "mock", "Idle", Some(1234)),
        );
        collect_agent_metrics_with_sampler(&state, live_sample).await;
        let (ready, ready_rx) = tokio::sync::oneshot::channel();
        let (release, release_rx) = std::sync::mpsc::channel();
        let old_state = state.clone();
        let old_pass = tokio::spawn(async move {
            collect_agent_metrics_with_sampler(&old_state, move |_, _, _| {
                ready.send(()).unwrap();
                release_rx.recv().unwrap();
                None
            })
            .await
        });
        ready_rx.await.unwrap();
        state.agents.lock().await.remove("overlap");
        if replace {
            state.agents.lock().await.insert(
                "overlap".into(),
                super::super::tests::test_active_agent("overlap", "mock", "Idle", Some(1234)),
            );
        }
        release.send(()).unwrap();
        let (retired, _) = old_pass.await.unwrap();
        assert!(
            retired.is_empty(),
            "retired runtime row must be withheld at publication"
        );
        let (current, _) = collect_agent_metrics_with_sampler(&state, missing_sample).await;
        if replace {
            assert_eq!(values(&current[0]), (0.0, 0.0, 0));
            assert_eq!(current[0].current_status, "Idle");
        } else {
            assert!(current.is_empty());
        }
    }
}
