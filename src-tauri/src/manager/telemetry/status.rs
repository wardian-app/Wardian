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
