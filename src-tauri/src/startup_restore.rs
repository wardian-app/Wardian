//! The roster publication boundary used by application startup restoration.

use crate::state::{ActiveAgent, AppState};
use std::future::Future;
use std::sync::{Arc, Mutex};

/// Returns the exact lifecycle-transition lease that caused the failed restore
/// attempt. Only resume operations and provider-spawn owners schedule the
/// guarded retry; pause, removal, clear, and uncertain-provider ownership keep
/// their existing handling.
pub(crate) fn retryable_lifecycle_restore_lease(
    config: &wardian_core::models::AgentConfig,
    spawn_error: &str,
    leases: &[wardian_core::conversation_lease::ConversationLease],
) -> Option<wardian_core::conversation_lease::ConversationLease> {
    let resume_session = config
        .resume_session
        .as_deref()
        .filter(|session| !session.trim().is_empty())
        .or_else(|| {
            config
                .fresh_provider_session_id
                .as_deref()
                .filter(|session| !session.trim().is_empty())
        })
        .unwrap_or_default()
        .trim();
    let lease = leases.iter().find(|lease| {
        if !is_retryable_restore_owner(lease)
            || lease.mode != "lifecycle_transition"
            || (lease.agent_id != config.session_id
                && (resume_session.is_empty() || lease.resume_session != resume_session))
        {
            return false;
        }
        spawn_error
            == format!(
                "provider startup was withheld because conversation {} is leased by {} {} ({})",
                config.session_id, lease.owner_kind, lease.owner_id, lease.mode
            )
    })?;

    chrono::DateTime::parse_from_rfc3339(&lease.expires_at).ok()?;
    Some(lease.clone())
}

const RESTORE_LEASE_RECHECK_INTERVAL: std::time::Duration = std::time::Duration::from_secs(2);

/// Wait for the exact reported lifecycle lease to disappear or expire. Reload
/// persisted ownership periodically so heartbeat renewals extend the wait and
/// early release does not cost the rest of the lease TTL.
pub(crate) async fn wait_for_lifecycle_restore_lease(
    config: &wardian_core::models::AgentConfig,
    expected_lease: &wardian_core::conversation_lease::ConversationLease,
    mut load_leases: impl FnMut() -> Result<
        Vec<wardian_core::conversation_lease::ConversationLease>,
        String,
    >,
    now: impl Fn() -> chrono::DateTime<chrono::Utc>,
) -> Result<bool, String> {
    loop {
        let now = now();
        let leases = load_leases()?;
        let now_rfc3339 = now.to_rfc3339();
        let active_conflicts = leases
            .iter()
            .filter(|lease| {
                wardian_core::conversation_lease::find_active_conflict(
                    std::slice::from_ref(*lease),
                    &config.session_id,
                    provider_restore_session_identity(config),
                    &now_rfc3339,
                )
                .is_some()
            })
            .collect::<Vec<_>>();

        if active_conflicts.is_empty() {
            return Ok(true);
        }
        if active_conflicts
            .iter()
            .any(|lease| !same_retryable_lease_generation(lease, expected_lease))
        {
            // A different lifecycle operation, active provider, or uncertain
            // prior provider owns this conversation. Keep the normal safety
            // gates authoritative and do not spend the one retry on it.
            return Ok(false);
        }

        let earliest_expiry = active_conflicts
            .iter()
            .filter_map(|lease| {
                chrono::DateTime::parse_from_rfc3339(&lease.expires_at)
                    .ok()
                    .map(|expiry| expiry.with_timezone(&chrono::Utc))
            })
            .min()
            .ok_or_else(|| "active lifecycle lease has no valid expiry".to_string())?;
        let until_expiry = (earliest_expiry - now).to_std().unwrap_or_default();
        let delay = until_expiry.min(RESTORE_LEASE_RECHECK_INTERVAL);
        if delay.is_zero() {
            tokio::task::yield_now().await;
        } else {
            tokio::time::sleep(delay).await;
        }
    }
}

fn provider_restore_session_identity(config: &wardian_core::models::AgentConfig) -> &str {
    config
        .resume_session
        .as_deref()
        .filter(|session| !session.trim().is_empty())
        .or_else(|| {
            config
                .fresh_provider_session_id
                .as_deref()
                .filter(|session| !session.trim().is_empty())
        })
        .unwrap_or_default()
        .trim()
}

fn same_retryable_lease_generation(
    current: &wardian_core::conversation_lease::ConversationLease,
    expected: &wardian_core::conversation_lease::ConversationLease,
) -> bool {
    is_retryable_restore_owner(current)
        && current.mode == "lifecycle_transition"
        && current.owner_kind == expected.owner_kind
        && current.owner_id == expected.owner_id
        && current.acquisition_id == expected.acquisition_id
        && current.started_at == expected.started_at
        && current.agent_id == expected.agent_id
        && current.provider == expected.provider
        && current.resume_session == expected.resume_session
}

fn is_retryable_restore_owner(lease: &wardian_core::conversation_lease::ConversationLease) -> bool {
    lease.owner_kind == "provider_spawn"
        || (lease.owner_kind == "agent_lifecycle" && lease.owner_id.starts_with("resume:"))
}

/// Waits without holding an agent lifecycle gate or startup restore slot,
/// then obtains a slot and claims the exact current Error placeholder.
pub(crate) async fn retry_once_after_lifecycle_lease_clear<
    Wait,
    WaitFuture,
    Slot,
    Output,
    AcquireSlot,
    AcquireSlotFuture,
    Attempt,
    AttemptFuture,
>(
    state: &AppState,
    expected_config: &wardian_core::models::AgentConfig,
    expected_status: &Arc<Mutex<String>>,
    wait_until_clear: Wait,
    acquire_slot: AcquireSlot,
    attempt: Attempt,
) -> Option<Output>
where
    Wait: FnOnce() -> WaitFuture,
    WaitFuture: Future<Output = bool>,
    AcquireSlot: FnOnce() -> AcquireSlotFuture,
    AcquireSlotFuture: Future<Output = Option<Slot>>,
    Attempt: FnOnce(RestorePublication, Slot) -> AttemptFuture,
    AttemptFuture: Future<Output = Output>,
{
    if !wait_until_clear().await {
        return None;
    }
    let slot = acquire_slot().await?;
    let publication =
        RestorePublication::begin_retry_if_current_error(state, expected_config, expected_status)
            .await?;
    Some(attempt(publication, slot).await)
}

pub(crate) fn has_active_headless_execution_lease(
    config: &wardian_core::models::AgentConfig,
    leases: &[wardian_core::conversation_lease::ConversationLease],
    now_rfc3339: &str,
) -> bool {
    wardian_core::conversation_lease::find_active_execution_conflict(
        leases,
        &config.session_id,
        config.resume_session.as_deref().unwrap_or_default(),
        now_rfc3339,
    )
    .is_some()
}

/// Own one startup restoration from config selection through final publication.
pub(crate) struct RestorePublication {
    session_id: String,
    _lifecycle: tokio::sync::OwnedMutexGuard<()>,
}

impl RestorePublication {
    /// Claim an unregistered agent before selecting its saved configuration.
    /// A registered owner wins over the startup snapshot, including when a
    /// mutation completed while this claim waited for the lifecycle gate.
    /// Keep the claim alive through placeholder and final/error publication.
    pub(crate) async fn begin(state: &AppState, session_id: &str) -> Option<Self> {
        let lifecycle = state.lock_agent_lifecycle(session_id).await;
        if state.agents.lock().await.contains_key(session_id) {
            return None;
        }
        Some(Self {
            session_id: session_id.to_owned(),
            _lifecycle: lifecycle,
        })
    }

    /// Reclaim only the exact failed startup placeholder after its lease wait.
    /// The caller must acquire the lifecycle gate before this check and keep
    /// the returned claim through the retry and its final publication.
    pub(crate) async fn begin_retry_if_current_error(
        state: &AppState,
        expected_config: &wardian_core::models::AgentConfig,
        expected_status: &Arc<Mutex<String>>,
    ) -> Option<Self> {
        let session_id = &expected_config.session_id;
        let lifecycle = state.lock_agent_lifecycle(session_id).await;
        let is_current_error = {
            let agents = state.agents.lock().await;
            let agent = agents.get(session_id)?;
            let current_config = agent.config.lock().unwrap();
            let current_status = agent.current_status.lock().unwrap();
            let configs_match = match (
                serde_json::to_value(&*current_config),
                serde_json::to_value(expected_config),
            ) {
                (Ok(current), Ok(expected)) => current == expected,
                _ => false,
            };
            std::sync::Arc::ptr_eq(&agent.current_status, expected_status)
                && *current_status == "Error"
                && agent.runtime_generation.is_none()
                && agent.process_id.is_none()
                && configs_match
        };
        if !is_current_error {
            return None;
        }
        Some(Self {
            session_id: session_id.clone(),
            _lifecycle: lifecycle,
        })
    }

    /// Publish either the placeholder or its completed runtime without holding
    /// the roster locks during provider initialization.
    pub(crate) async fn publish(&self, state: &AppState, agent: ActiveAgent) -> Arc<Mutex<String>> {
        let session_id = agent.config.lock().unwrap().session_id.clone();
        assert_eq!(
            session_id, self.session_id,
            "restoration claim belongs to another agent"
        );
        let status = agent.current_status.clone();
        if agent.runtime_generation.is_none()
            && agent.process_id.is_none()
            && *status.lock().unwrap() == "Error"
        {
            let output = agent
                .watch_state
                .lock()
                .unwrap()
                .raw_snapshot_since(None, Some(262_144))
                .map(|snapshot| snapshot.text)
                .unwrap_or_default();
            let output = if output.is_empty() {
                "Wardian could not restore this agent. No provider error details were recorded.\r\n"
                    .to_owned()
            } else {
                output
            };
            if let Err(error) = state
                .terminal_sessions
                .start_failure_terminal(&session_id, output.as_bytes())
                .await
            {
                crate::manager::log_debug(&format!(
                    "[Wardian] Failed to publish restoration error terminal for {session_id}: {error}"
                ));
            }
        }
        let mut agents = state.agents.lock().await;
        let mut order = state.agent_order.lock().await;
        if !order.contains(&session_id) {
            order.push(session_id.clone());
        }
        agents.insert(session_id, agent);
        status
    }

    /// A spawned provider is committed only after its exact runtime enters the
    /// roster. Dropping this future during publication marks the watcher failed.
    pub(crate) async fn publish_spawned(
        &self,
        state: &AppState,
        agent: ActiveAgent,
        mut disposition: crate::manager::SpawnPublicationDisposition,
    ) -> Arc<Mutex<String>> {
        // A successful synchronous Codex spawn has completed owner attachment,
        // but deferred status work can discard itself against the old placeholder.
        // Install the runtime first, then reconcile while this restore still
        // owns its lifecycle claim. No roster lock is held while locking status.
        let attached_codex = agent.runtime_generation.is_some()
            && agent
                .config
                .lock()
                .is_ok_and(|config| config.provider == "codex" && !config.is_off)
            && crate::manager::codex_onboarding::codex_attachment_is_ready(&agent);
        let status = self.publish(state, agent).await;
        if attached_codex {
            if let Ok(mut current) = status.lock() {
                if *current == "Starting" {
                    let admitted = state.status_intent_status(&self.session_id, &status);
                    let reconciled = admitted
                        .filter(|value| {
                            matches!(
                                wardian_core::identity::normalize_status(value).as_str(),
                                "idle" | "processing" | "action_required" | "off" | "error"
                            )
                        })
                        .unwrap_or_else(|| "Idle".to_string());
                    *current = reconciled.clone();
                    state.commit_status_revision(&self.session_id, &status, &reconciled);
                }
            }
        }
        disposition.commit();
        status
    }
}

/// Persist startup's normalized placeholders without overwriting durable
/// remove, pause, or config changes made after startup selected its snapshot.
pub(crate) async fn persist_roster(
    state: &AppState,
    selected_configs: &[wardian_core::models::AgentConfig],
) -> Result<(), String> {
    loop {
        {
            let agents = state.agents.lock().await;
            let order = state.agent_order.lock().await;
            // Config mutation can own the durable barrier while waiting for
            // these maps. Never wait for that barrier while holding the maps.
            if let Some(_barrier) =
                wardian_core::agent_replacement::acquire_agent_roster_barrier(false)
                    .map_err(|error| error.to_string())?
            {
                let snapshot = crate::manager::state_configs_snapshot(&agents, &order);
                let home = crate::manager::get_wardian_home()
                    .ok_or_else(|| "Could not locate Wardian home".to_string())?;
                let state_path = home.join("settings").join("state.json");
                let state_json =
                    std::fs::read_to_string(&state_path).map_err(|error| error.to_string())?;
                let mut durable_configs =
                    serde_json::from_str::<Vec<wardian_core::models::AgentConfig>>(&state_json)
                        .map_err(|error| error.to_string())?;
                for runtime_config in &snapshot {
                    let Some(selected_config) = selected_configs
                        .iter()
                        .find(|config| config.session_id == runtime_config.session_id)
                    else {
                        continue;
                    };
                    let Some(durable_config) = durable_configs
                        .iter_mut()
                        .find(|config| config.session_id == runtime_config.session_id)
                    else {
                        continue;
                    };
                    let unchanged_since_selection = match (
                        serde_json::to_value(&*durable_config),
                        serde_json::to_value(selected_config),
                    ) {
                        (Ok(durable), Ok(selected)) => durable == selected,
                        _ => false,
                    };
                    if unchanged_since_selection {
                        *durable_config = runtime_config.clone();
                    }
                }
                return crate::manager::try_save_state_snapshot_unlocked(&durable_configs);
            }
        }
        // Wait off the async executor, with no roster locks or stale snapshot.
        // Re-read the current roster after the competing writer completes.
        tokio::task::spawn_blocking(|| {
            wardian_core::agent_replacement::acquire_agent_roster_barrier(true).map(drop)
        })
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())?;
    }
}

#[cfg(test)]
mod tests;
