//! Exact operator-authorized release; no process observations confer authority.
use super::*;

/// Compare the complete inspected hold under both lease locks, durably record
/// authorization via `authorize`, then remove only that exact acquisition.
///
/// The callback runs under the lease locks and must not call lease APIs. It must
/// append and sync authorization before returning success. A callback error
/// leaves all leases intact; a save error may leave an authorization record,
/// which must never be represented as a completed release.
pub fn release_previous_provider_hold_exact(
    expected: &ConversationLease,
    authorize: impl FnOnce(&ConversationLease) -> Result<(), String>,
) -> Result<(), String> {
    if expected.owner_kind != "prior_provider_hold"
        || expected.mode != "uncertain_previous_provider"
        || [
            &expected.agent_id,
            &expected.provider,
            &expected.resume_session,
            &expected.owner_id,
            &expected.acquisition_id,
        ]
        .iter()
        .any(|value| value.trim().is_empty())
    {
        return Err("exact previous-provider hold identity is required".into());
    }
    let _process_guard = LEASE_FILE_LOCK
        .lock()
        .map_err(|_| "conversation lease lock poisoned".to_string())?;
    let _file_guard = acquire_lease_file_lock()?;
    let mut leases = load_leases_checked()?;
    let matching: Vec<_> = leases
        .iter()
        .enumerate()
        .filter(|(_, lease)| lease_matches_owner(lease, &expected.owner()))
        .collect();
    if matching.len() != 1 || matching[0].1 != expected {
        return Err(
            "previous-provider hold changed or is missing; inspect the exact acquisition again"
                .into(),
        );
    }
    let index = matching[0].0;
    if leases.iter().any(|lease| {
        lease.mode != "uncertain_previous_provider"
            && lease_conflicts(
                lease,
                &expected.agent_id,
                &expected.resume_session,
                chrono::Utc::now(),
            )
    }) {
        return Err("an active conversation owner still excludes hold release".into());
    }
    authorize(&leases[index])?;
    leases.remove(index);
    save_leases(&leases).map_err(|error| {
        format!("hold release write failed after authorization; release is not confirmed: {error}")
    })
}

#[cfg(test)]
mod tests;
