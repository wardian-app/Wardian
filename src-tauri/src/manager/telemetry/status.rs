use super::{AgentSnapshot, TelemetryProviderStatus};
use crate::state::AppState;
use wardian_core::control::{ProviderInputReadiness, ProviderReadyEvidence};
use wardian_core::models::AgentTelemetry;

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
    let Ok(mut observation) = snap.status_observation.lock() else {
        return;
    };
    let observed_at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    if observation.current_status == next_status {
        return;
    }
    let previous_status = observation.current_status.clone();
    observation
        .transitions
        .push(super::TelemetryStatusTransition {
            previous_status,
            status: next_status.to_string(),
            observed_at,
        });
    observation.current_status = next_status.to_string();
}

pub(super) fn commit_snapshot_status_observation(
    state: &crate::state::AppState,
    observation: &super::TelemetryProviderStatus,
    current_status: &std::sync::Arc<std::sync::Mutex<String>>,
    last_status_at: &std::sync::Arc<std::sync::Mutex<Option<String>>>,
    watch_state: &std::sync::Arc<std::sync::Mutex<crate::state::AgentWatchState>>,
    codex_attachment_ready: bool,
) -> Option<(String, u64, u64)> {
    if !std::sync::Arc::ptr_eq(&observation.current_status, current_status) {
        return None;
    }

    let Ok(mut status) = current_status.lock() else {
        return None;
    };
    if *status != observation.initial_status
        || state.status_revision(&observation.session_id, current_status)
            != observation.initial_status_revision
    {
        return None;
    }
    let status_intent_revision =
        state.status_intent_revision(&observation.session_id, current_status);
    let status_intent_matches_observation = state
        .status_intent_status(&observation.session_id, current_status)
        .is_some_and(|intent_status| intent_status == observation.status);
    // A later same-status no-op cancels older deferred writes but leaves this
    // value observation valid; a different target makes the staged transition stale.
    let initial_status = wardian_core::identity::normalize_status(&observation.initial_status);
    let observed_status = wardian_core::identity::normalize_status(&observation.status);
    // A log snapshot can lag an already-current terminal intent. A matching
    // newer intent still authorizes the ready or busy transition.
    let stale_readiness_after_terminal_intent =
        matches!(initial_status.as_str(), "off" | "error" | "action_required")
            && matches!(observed_status.as_str(), "idle" | "processing")
            && !status_intent_matches_observation;
    if (status_intent_revision != observation.initial_status_intent_revision
        && !status_intent_matches_observation)
        || stale_readiness_after_terminal_intent
    {
        return None;
    }

    let mut committed_transition = false;
    for transition in &observation.transitions {
        if *status != transition.previous_status {
            break;
        }
        if !codex_attachment_ready
            && matches!(
                wardian_core::identity::normalize_status(&transition.status).as_str(),
                "idle" | "processing"
            )
        {
            break;
        }

        *status = transition.status.clone();
        committed_transition = true;
        let _ = wardian_core::db::update_agent_status(
            &observation.session_id,
            &transition.status,
            None,
        );
        if let Ok(mut observed_at) = last_status_at.lock() {
            *observed_at = Some(transition.observed_at.clone());
        }
        if let Ok(mut watch_state) = watch_state.lock() {
            watch_state.push_event(
                "status",
                serde_json::json!({
                    "status": wardian_core::identity::normalize_status(&transition.status),
                    "observed_at": transition.observed_at,
                }),
            );
        }
    }

    let (sequence, revision) = if committed_transition {
        let revision =
            state.commit_status_revision(&observation.session_id, current_status, &status);
        let sequence = state.next_status_observation_sequence(&observation.session_id);
        (sequence, revision)
    } else {
        (
            state
                .status_observation_sequences
                .lock()
                .ok()?
                .get(&observation.session_id)
                .copied()
                .unwrap_or(0),
            observation.initial_status_revision,
        )
    };
    Some((status.clone(), sequence, revision))
}

/// How long the metrics tick waits for one agent's lifecycle gate.
///
/// The tick publishes every agent in sequence, so an unbounded wait behind one
/// New Session, restart, or restore froze status updates for the whole fleet
/// (observed: 400 s).
const LIFECYCLE_WAIT: std::time::Duration = std::time::Duration::from_millis(250);

/// Work a pass leaves for its caller, which owns the app handle.
#[derive(Default)]
pub(super) struct StatusFollowUp {
    /// Agents that just became ready for queued work.
    pub(super) wake_sessions: Vec<String>,
    /// Observations that carry a staged transition but found their agent's
    /// lifecycle gate held. The provider log's parse watermark has already
    /// advanced past what they staged, so dropping them would lose the
    /// transition; the caller publishes them once the gate frees.
    pub(super) deferred: Vec<TelemetryProviderStatus>,
}

/// Hands a pass's follow-up work to detached workers.
///
/// A wakeup runs pending native information first and queued tasks second, the
/// order the tick used to keep inline, because one worker awaits both in turn.
/// A deferred observation is published without bound, exactly as the tick used
/// to: the gate holder's own changes are still revalidated against the
/// observation's revisions, so a stale one is rejected as before.
pub(super) fn spawn_follow_up(app: &tauri::AppHandle, follow_up: StatusFollowUp) {
    if !follow_up.deferred.is_empty() {
        crate::utils::logging::log_debug(&format!(
            "[Wardian] Telemetry deferred status publication behind busy lifecycle gates: {}",
            follow_up
                .deferred
                .iter()
                .map(|observation| observation.session_id.as_str())
                .collect::<Vec<_>>()
                .join(",")
        ));
    }
    for session_id in follow_up.wake_sessions {
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            let state = tauri::Manager::state::<AppState>(&app);
            crate::control::dispatch_agent_messaging_from_status_observation(
                Some(&app),
                &state,
                &session_id,
            )
            .await;
        });
    }
    for observation in follow_up.deferred {
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            let state = tauri::Manager::state::<AppState>(&app);
            let publication =
                crate::manager::publish_telemetry_status_observation(&state, &observation).await;
            if publication.readiness.is_some()
                && apply_telemetry_provider_readiness(&state, &observation, &publication).await
            {
                crate::control::dispatch_agent_messaging_from_status_observation(
                    Some(&app),
                    &state,
                    &observation.session_id,
                )
                .await;
            }
        });
    }
}

async fn live_agent_status(state: &AppState, session_id: &str) -> Option<String> {
    let agents = state.agents.lock().await;
    agents.get(session_id).and_then(|agent| {
        agent
            .current_status
            .lock()
            .ok()
            .map(|status| status.clone())
    })
}

pub(super) async fn apply_provider_status_observations(
    state: &AppState,
    observations: &[TelemetryProviderStatus],
    metrics: &mut [AgentTelemetry],
) -> StatusFollowUp {
    let mut follow_up = StatusFollowUp::default();
    for observation in observations {
        let publication = match tokio::time::timeout(
            LIFECYCLE_WAIT,
            state.lock_agent_lifecycle(&observation.session_id),
        )
        .await
        {
            Ok(lifecycle) => {
                crate::manager::publish_telemetry_status_observation_locked(
                    state,
                    observation,
                    lifecycle,
                )
                .await
            }
            Err(_) => {
                if !observation.transitions.is_empty() {
                    follow_up.deferred.push(observation.clone());
                }
                crate::manager::TelemetryStatusPublication {
                    readiness: None,
                    current_status: live_agent_status(state, &observation.session_id).await,
                    status_revision: None,
                }
            }
        };
        if publication.readiness.is_none()
            || publication.current_status.as_deref() != Some(observation.status.as_str())
        {
            if let (Some(status), Some(metric)) = (
                publication.current_status.as_ref(),
                metrics
                    .iter_mut()
                    .find(|metric| metric.session_id == observation.session_id),
            ) {
                metric.current_status =
                    super::telemetry_display_status(status, observation.active_execution_conflict);
            }
        }
        if publication.readiness.is_none() {
            continue;
        }
        if apply_telemetry_provider_readiness(state, observation, &publication).await {
            follow_up.wake_sessions.push(observation.session_id.clone());
        }
    }
    follow_up
}

/// Records the provider-input readiness a published status implies.
///
/// Returns whether the agent just became ready for queued work. The caller owns
/// the wakeup, because delivering queued work is not bounded by telemetry.
pub(super) async fn apply_telemetry_provider_readiness(
    state: &AppState,
    observation: &TelemetryProviderStatus,
    publication: &crate::manager::TelemetryStatusPublication,
) -> bool {
    let (Some(readiness), Some(status), Some(status_revision)) = (
        publication.readiness,
        publication.current_status.as_deref(),
        publication.status_revision,
    ) else {
        return false;
    };
    let ready_evidence = (readiness == ProviderInputReadiness::Ready)
        .then_some(ProviderReadyEvidence::ProviderEvent);
    if !crate::manager::status_observation_belongs_to_current_agent(
        state,
        &observation.session_id,
        &observation.current_status,
        status,
        status_revision,
    )
    .await
    {
        return false;
    }
    let (_, became_ready) = state
        .interactions
        .record_provider_input_status_observation_with_transition(
            &observation.session_id,
            status_revision,
            observation.generation,
            readiness,
            ready_evidence,
        )
        .await;
    became_ready
        && crate::manager::status_observation_belongs_to_current_agent(
            state,
            &observation.session_id,
            &observation.current_status,
            status,
            status_revision,
        )
        .await
}
