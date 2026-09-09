//! No-child tests use the real terminal actor and native owner watch values.
use super::*;
use crate::control::{test_support::TestWardianHome, tests::insert_test_agent};
use crate::delivery::codex_shared::test_support::observation_with_activity;
use crate::state::terminal_session::TerminalRuntimeHandles;
use tokio::sync::mpsc;
use wardian_core::control::ProviderInputReadiness;
use wardian_core::models::TerminalGeometry;

const ID: &str = "choice-agent";
const COMPOSER: &str = "› Ask Codex to do anything";
const MENU: &str = include_str!("../delivery/fixtures/codex-rate-limit-menu.txt");

async fn fixture() -> (
    AppState,
    ProviderStartupObservation,
    mpsc::Receiver<Vec<u8>>,
) {
    let state = AppState::new();
    insert_test_agent(&state, ID, "Choice", "Coder").await;
    let (tx, rx) = mpsc::channel(4);
    let runtime_generation = state
        .terminal_sessions
        .start_or_replace_runtime(
            ID,
            TerminalRuntimeHandles::new(tx, |_| Ok(())),
            TerminalGeometry {
                cols: 190,
                rows: 51,
            },
        )
        .await
        .unwrap();
    let input_generation = state
        .interactions
        .start_provider_input_generation(ID, ProviderInputReadiness::Ready, None)
        .await
        .generation;
    let current_status = {
        let mut agents = state.agents.lock().await;
        let agent = agents.get_mut(ID).unwrap();
        agent.runtime_generation = Some(runtime_generation);
        agent.config.lock().unwrap().provider = "codex".into();
        *agent.current_status.lock().unwrap() = "Action Needed".into();
        agent.current_status.clone()
    };
    (
        state,
        ProviderStartupObservation {
            input_generation,
            runtime_generation,
            current_status,
        },
        rx,
    )
}

async fn paint(state: &AppState, generation: u64, text: &str) {
    let broker = state.terminal_sessions.clone();
    let bytes = format!("\x1b[2J\x1b[H{}", text.replace('\n', "\r\n")).into_bytes();
    tokio::task::spawn_blocking(move || broker.process_output_blocking(ID, generation, bytes))
        .await
        .unwrap()
        .unwrap();
}

fn status(observation: &ProviderStartupObservation) -> String {
    observation.current_status.lock().unwrap().clone()
}

#[tokio::test]
async fn native_completion_and_processing_cannot_publish_over_current_menu() {
    let _home = TestWardianHome::new_async().await;
    let (state, observation, mut input) = fixture().await;
    // Completion can arrive before or after the menu's PTY paint. Publication
    // must check the current screen, not the event's earlier activity snapshot.
    *observation.current_status.lock().unwrap() = "Idle".into();
    paint(&state, observation.runtime_generation, MENU).await;
    for requested in ["Idle", "Processing..."] {
        *observation.current_status.lock().unwrap() = requested.into();
        let _lifecycle = state.lock_agent_lifecycle(ID).await;
        assert_eq!(
            constrain_publication(&state, ID, &observation.current_status, requested).await,
            Some("Action Needed".into())
        );
        assert_eq!(status(&observation), "Action Needed");
    }
    assert!(
        input.try_recv().is_err(),
        "Status observation must never select a model"
    );
}

#[tokio::test]
async fn explicit_dismissal_restores_latest_owner_activity_without_input() {
    let _home = TestWardianHome::new_async().await;
    let (state, observation, mut input) = fixture().await;
    paint(&state, observation.runtime_generation, COMPOSER).await;
    for (activity, expected) in [
        (CodexTurnActivity::Idle("completed-turn".into()), "Idle"),
        (
            CodexTurnActivity::Processing("next-turn".into()),
            "Processing...",
        ),
    ] {
        *observation.current_status.lock().unwrap() = "Action Needed".into();
        let (tx, rx) = watch::channel(observation_with_activity(CodexTurnActivity::Pending));
        assert!(
            restore_with_owner(None, &state, ID, &observation, async {
                // The owner advances during lookup; restoration must read its latest value.
                tx.send_replace(observation_with_activity(activity));
                Ok(rx)
            })
            .await
        );
        assert_eq!(status(&observation), expected);
    }
    assert!(
        input.try_recv().is_err(),
        "Dismissal observation writes no terminal bytes"
    );
}

#[tokio::test]
async fn dismissal_without_live_owner_evidence_stays_action_needed() {
    let _home = TestWardianHome::new_async().await;
    let (state, observation, mut input) = fixture().await;
    paint(&state, observation.runtime_generation, COMPOSER).await;
    assert!(
        !restore_with_owner(None, &state, ID, &observation, async {
            Err("owner unavailable".into())
        })
        .await
    );
    for activity in [
        CodexTurnActivity::Pending,
        CodexTurnActivity::Closed,
        CodexTurnActivity::Stopped,
    ] {
        let (_tx, rx) = watch::channel(observation_with_activity(activity));
        assert!(!restore_with_owner(None, &state, ID, &observation, async { Ok(rx) }).await);
        assert_eq!(status(&observation), "Action Needed");
    }
    assert!(input.try_recv().is_err());
}

#[tokio::test]
async fn dismissal_rechecks_screen_before_restoring_completed_owner() {
    let _home = TestWardianHome::new_async().await;
    let (state, observation, mut input) = fixture().await;
    for screen in [MENU, "partial repaint"] {
        paint(&state, observation.runtime_generation, COMPOSER).await;
        let (_tx, rx) = watch::channel(observation_with_activity(CodexTurnActivity::Idle(
            "completed-turn".into(),
        )));
        assert!(
            !restore_with_owner(None, &state, ID, &observation, async {
                paint(&state, observation.runtime_generation, screen).await;
                Ok(rx)
            })
            .await
        );
        assert_eq!(status(&observation), "Action Needed");
    }
    assert!(input.try_recv().is_err());
}

#[tokio::test]
async fn dismissal_rechecks_input_generation_after_owner_lookup() {
    let _home = TestWardianHome::new_async().await;
    let (state, observation, mut input) = fixture().await;
    paint(&state, observation.runtime_generation, COMPOSER).await;
    let (_tx, rx) = watch::channel(observation_with_activity(CodexTurnActivity::Idle(
        "old".into(),
    )));
    // Inject invalidation at the await seam. Normal replacement additionally
    // holds the lifecycle lock; this proves the queued observation's own fence.
    assert!(
        !restore_with_owner(None, &state, ID, &observation, async {
            state
                .interactions
                .start_provider_input_generation(ID, ProviderInputReadiness::Booting, None)
                .await;
            Ok(rx)
        })
        .await
    );
    assert_eq!(status(&observation), "Action Needed");
    assert_eq!(
        state
            .interactions
            .provider_input_state(ID)
            .await
            .unwrap()
            .state,
        ProviderInputReadiness::Booting
    );
    assert!(input.try_recv().is_err());
}

#[tokio::test]
async fn queued_dismissal_cannot_restore_replacement_runtime_or_status_arc() {
    let _home = TestWardianHome::new_async().await;
    let (state, observation, mut old_input) = fixture().await;
    let state = Arc::new(state);
    paint(&state, observation.runtime_generation, COMPOSER).await;
    let lifecycle = state.lock_agent_lifecycle(ID).await;
    let queued_state = state.clone();
    let queued_observation = observation.clone();
    let (started, waiting) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        started.send(()).unwrap();
        restore_with_owner(None, &queued_state, ID, &queued_observation, async {
            panic!("A stale dismissal must not even query its owner")
        })
        .await
    });
    waiting.await.unwrap();
    let (tx, mut new_input) = mpsc::channel(4);
    let generation = state
        .terminal_sessions
        .start_or_replace_runtime(
            ID,
            TerminalRuntimeHandles::new(tx, |_| Ok(())),
            TerminalGeometry {
                cols: 190,
                rows: 51,
            },
        )
        .await
        .unwrap();
    let replacement_status = Arc::new(std::sync::Mutex::new("Starting".into()));
    {
        let mut agents = state.agents.lock().await;
        let agent = agents.get_mut(ID).unwrap();
        agent.runtime_generation = Some(generation);
        agent.current_status = replacement_status.clone();
    }
    drop(lifecycle);
    assert!(!task.await.unwrap());
    assert_eq!(*replacement_status.lock().unwrap(), "Starting");
    assert_eq!(status(&observation), "Action Needed");
    let _lifecycle = state.lock_agent_lifecycle(ID).await;
    assert_eq!(
        constrain_publication(&state, ID, &observation.current_status, "Action Needed").await,
        None
    );
    assert!(old_input.try_recv().is_err());
    assert!(new_input.try_recv().is_err());
}

#[tokio::test]
async fn prewrite_rejects_terminal_from_another_runtime_without_input() {
    use crate::delivery::{submit_live_surface_prompt, LiveSurfacePromptRequest};
    let _home = TestWardianHome::new_async().await;
    let (state, observation, mut input) = fixture().await;
    paint(&state, observation.runtime_generation, COMPOSER).await;
    {
        let mut agents = state.agents.lock().await;
        agents.get_mut(ID).unwrap().runtime_generation = Some(observation.runtime_generation + 1);
    }
    let failure = submit_live_surface_prompt(
        None,
        &state,
        LiveSurfacePromptRequest::message(ID, "must not reach a different runtime"),
    )
    .await
    .unwrap_err();
    assert!(
        failure
            .message
            .contains("Codex terminal runtime changed before input"),
        "{}",
        failure.message
    );
    assert!(
        input.try_recv().is_err(),
        "Cached Ready cannot authorize a different runtime"
    );
}

#[tokio::test]
async fn review_correction_unavailable_screen_cannot_publish_ready_or_busy() {
    let _home = TestWardianHome::new_async().await;
    for unavailable in [
        "missing_generation",
        "mismatched_generation",
        "missing_snapshot",
    ] {
        let (state, observation, mut input) = fixture().await;
        paint(&state, observation.runtime_generation, COMPOSER).await;
        let _lifecycle = state.lock_agent_lifecycle(ID).await;
        match unavailable {
            "missing_generation" => {
                state
                    .agents
                    .lock()
                    .await
                    .get_mut(ID)
                    .unwrap()
                    .runtime_generation = None
            }
            "mismatched_generation" => {
                state
                    .agents
                    .lock()
                    .await
                    .get_mut(ID)
                    .unwrap()
                    .runtime_generation = Some(observation.runtime_generation + 1)
            }
            _ => {
                state
                    .terminal_sessions
                    .terminate_and_remove_runtime(ID, observation.runtime_generation)
                    .await
                    .unwrap();
            }
        }
        for requested in ["Idle", "Processing..."] {
            // Mirror set_agent_status: the Arc changes before publication is queued.
            *observation.current_status.lock().unwrap() = requested.into();
            assert_eq!(
                constrain_publication(&state, ID, &observation.current_status, requested).await,
                Some("Action Needed".into()),
                "{unavailable}: {requested}"
            );
            assert_eq!(status(&observation), "Action Needed");
        }
        assert!(
            input.try_recv().is_err(),
            "Observation must never write input"
        );
    }
}

#[tokio::test]
async fn review_correction_current_composer_and_non_codex_statuses_remain_publishable() {
    let _home = TestWardianHome::new_async().await;
    let (state, observation, mut input) = fixture().await;
    paint(&state, observation.runtime_generation, COMPOSER).await;
    let _lifecycle = state.lock_agent_lifecycle(ID).await;
    for requested in ["Idle", "Processing...", "Off", "Error"] {
        *observation.current_status.lock().unwrap() = requested.into();
        assert_eq!(
            constrain_publication(&state, ID, &observation.current_status, requested).await,
            Some(requested.into())
        );
    }
    state
        .agents
        .lock()
        .await
        .get_mut(ID)
        .unwrap()
        .config
        .lock()
        .unwrap()
        .provider = "claude".into();
    state
        .terminal_sessions
        .terminate_and_remove_runtime(ID, observation.runtime_generation)
        .await
        .unwrap();
    *observation.current_status.lock().unwrap() = "Idle".into();
    assert_eq!(
        constrain_publication(&state, ID, &observation.current_status, "Idle").await,
        Some("Idle".into())
    );
    assert!(input.try_recv().is_err());
}

#[test]
fn review_correction_retained_alpha6_footer_is_ready_without_model_choice() {
    // Retained candidate2 pre-prompt screen; only filesystem paths are sanitized.
    let screen = concat!(
        r#"╭────────────────────────────────────────────────╮
│ >_ OpenAI Codex (v0.154.0-alpha.6)             │
│                                                │
│ model:     gpt-5.6-luna low   /model to change │
│ directory: <scratch-workspace> │
╰────────────────────────────────────────────────╯

  Tip: When the composer is empty, press Esc to step back and edit your last
  message; Enter confirms.

• You have 1 usage limit reset available. Run /usage to use one.
"#,
        " \n",
        " \n",
        r#"› Ask Codex to do anything
"#,
        " \n",
        r#"  gpt-5.6-luna low · Context 100% left · <workspace-path>"#
    );
    assert!(crate::delivery::codex_composer::output_has_ready_prompt(
        screen
    ));
    assert!(crate::control::provider_output_has_startup_ready_prompt(
        "codex", screen
    ));
    assert!(!crate::delivery::codex_menu::current_screen_requires_choice(screen));
}
