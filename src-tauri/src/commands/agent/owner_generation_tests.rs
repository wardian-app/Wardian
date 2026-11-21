//! Isolated mock-Codex lifecycle coverage using the real generation stores and stop path.
use super::*;
use crate::delivery::codex_shared::test_support::{stop_fixture, WriterCase};
use crate::manager::codex_shared::StartingCodexTui;
use wardian_core::control::ProviderInputReadiness;
use wardian_core::models::TerminalGeometry;

#[tokio::test]
async fn off_restore_resume_pause_resume_stop_keep_independent_generations() {
    restored_owner_lifecycle(true).await;
}

#[tokio::test]
async fn saved_active_restore_pause_resume_stop_keeps_working() {
    restored_owner_lifecycle(false).await;
}

async fn restored_owner_lifecycle(saved_off: bool) {
    let _environment = crate::utils::wardian_test_env_lock_async().await;
    let fixture = Fixture::new();
    let state = AppState::new();
    let id = "restored-codex-owner";
    if saved_off {
        // Saved-Off restore publishes unavailable input, without allocating a PTY.
        let restored = state
            .interactions
            .start_provider_input_generation(id, ProviderInputReadiness::Unavailable, None)
            .await;
        assert_eq!(restored.generation, 1);
        assert!(matches!(
            state.terminal_sessions.broker_state(id).await,
            Err(TerminalBrokerError::SessionNotFound)
        ));
    }
    for (incarnation, remove_terminal) in [(1, false), (2, true)] {
        let provider_generation = state
            .interactions
            .start_provider_input_generation(id, ProviderInputReadiness::Booting, None)
            .await
            .generation;
        assert_eq!(provider_generation, incarnation + u64::from(saved_off));
        let (tx, _rx) = tokio::sync::mpsc::channel(1);
        let runtime_generation = state
            .terminal_sessions
            .start_or_replace_runtime(
                id,
                TerminalRuntimeHandles::new(tx, |_| Ok(())),
                TerminalGeometry { cols: 80, rows: 24 },
            )
            .await
            .unwrap();
        assert_eq!(runtime_generation, incarnation);

        let directory = fixture._temp.path().join(format!("owner-{incarnation}"));
        std::fs::create_dir(&directory).unwrap();
        let owner = stop_fixture(
            &directory,
            id,
            provider_generation,
            "restored-thread",
            WriterCase::Owned,
        )
        .await;
        state
            .native_delivery
            .install_stop_test_owner(id, provider_generation, None, owner.owner.clone())
            .await;
        let child = Arc::new(ChildState::default());
        child.exited.store(true, Ordering::SeqCst);
        let mut runtime = fixture.runtime(id, &child);
        let mut tui = StartingCodexTui::new(
            runtime.child_process.take().unwrap(),
            Some((
                state.native_delivery.clone(),
                id.into(),
                provider_generation,
            )),
        );
        tui.runtime(state.terminal_sessions.clone(), runtime_generation)
            .await
            .unwrap();
        runtime.child_process = Some(tui.attached());
        runtime.runtime_generation = Some(runtime_generation);
        runtime.config.lock().unwrap().is_off = false;
        state.agents.lock().await.insert(id.into(), runtime);

        let _lifecycle = state.lock_agent_lifecycle(id).await;
        stop_native_owner(&state, id, remove_terminal)
            .await
            .expect("the captured TUI must stop its own Codex owner across generation domains");
        await_quiescent(&fixture.home, id).await.unwrap();
        assert!(
            !state
                .native_delivery
                .codex_owner_selected(id, provider_generation)
                .await
        );
        let agents = state.agents.lock().await;
        assert!(agents[id].child_process.is_none());
        assert_eq!(agents[id].runtime_generation, None);
        assert_eq!(child.kills.load(Ordering::SeqCst), 0);
        drop(agents);
        if remove_terminal {
            assert!(matches!(
                state.terminal_sessions.broker_state(id).await,
                Err(TerminalBrokerError::SessionNotFound)
            ));
        } else {
            assert_eq!(
                state
                    .terminal_sessions
                    .broker_state(id)
                    .await
                    .unwrap()
                    .runtime_generation,
                runtime_generation
            );
        }
        owner.cleanup().await;
    }
}

#[tokio::test]
async fn stop_rejects_another_tui_even_when_its_number_matches_the_owner_generation() {
    let _environment = crate::utils::wardian_test_env_lock_async().await;
    let fixture = Fixture::new();
    let state = AppState::new();
    let id = "bound-codex-owner";
    let owner = stop_fixture(fixture._temp.path(), id, 2, "thread", WriterCase::Owned).await;
    state
        .native_delivery
        .install_stop_test_owner(id, 2, None, owner.owner.clone())
        .await;
    assert!(state
        .native_delivery
        .capture_codex_stop(id, Some(2))
        .await
        .is_err());
    // An owner with no attached TUI remains eligible for its own cleanup.
    drop(
        state
            .native_delivery
            .capture_codex_stop(id, None)
            .await
            .unwrap(),
    );
    state
        .native_delivery
        .bind_codex_tui_runtime(id, 2, 1)
        .await
        .unwrap();
    for wrong_tui in [None, Some(2), Some(3)] {
        let error = state
            .native_delivery
            .capture_codex_stop(id, wrong_tui)
            .await
            .err()
            .expect("a different or absent TUI cannot capture this owner");
        assert!(error.to_string().contains("not bound to captured TUI"));
        assert!(state.native_delivery.codex_owner_selected(id, 2).await);
    }
    for (provider_generation, runtime_generation) in [(1, 1), (2, 0), (2, 2)] {
        assert!(state
            .native_delivery
            .bind_codex_tui_runtime(id, provider_generation, runtime_generation)
            .await
            .is_err());
    }
    let captured = state
        .native_delivery
        .capture_codex_stop(id, Some(1))
        .await
        .expect("rejected captures and rebinding must preserve the original association");
    drop(captured);
    assert!(state.native_delivery.codex_owner_selected(id, 2).await);
    state.native_delivery.dispose_agent(id).await.unwrap();
    owner.cleanup().await;
}
