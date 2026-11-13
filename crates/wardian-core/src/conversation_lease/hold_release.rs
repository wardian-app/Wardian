//! Exact hold release after caller-supplied authorization or verified exit evidence.
use super::*;

/// Compare the complete inspected hold under both lease locks, durably record
/// authorization via `authorize`, then remove only that exact acquisition.
///
/// The callback runs under the lease locks and must not call lease APIs. It must
/// append and sync operator authorization before returning success. A callback error
/// leaves all leases intact; a save error may leave an authorization record,
/// which must never be represented as a completed release.
pub fn release_previous_provider_hold_exact(
    expected: &ConversationLease,
    authorize: impl FnOnce(&ConversationLease) -> Result<(), String>,
) -> Result<(), String> {
    release_exact_hold(expected, false, authorize)
}

/// Release an exact hold after `verify` validates backend-owned writer-exit
/// evidence for the complete hold record. Core trusts this adapter callback;
/// callers must validate their private exit receipt, not infer exit from a PID.
///
/// The callback runs under both lease locks and must not call lease APIs. Callback
/// errors leave all leases intact; a save error does not confirm release.
/// A single active same-agent `agent_lifecycle` /
/// `lifecycle_transition` reservation may remain for normal fresh rotation;
/// other active conflicting owners and ambiguous transitions exclude release.
pub fn release_previous_provider_hold_after_exit(
    expected: &ConversationLease,
    verify: impl FnOnce(&ConversationLease) -> Result<(), String>,
) -> Result<(), String> {
    release_exact_hold(expected, true, verify)
}

fn release_exact_hold(
    expected: &ConversationLease,
    allow_current_transition: bool,
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
    let now = chrono::Utc::now();
    let mut transitions = 0;
    for lease in leases.iter().filter(|lease| {
        lease.mode != "uncertain_previous_provider"
            && lease_conflicts(lease, &expected.agent_id, &expected.resume_session, now)
    }) {
        if !allow_current_transition
            || lease.agent_id != expected.agent_id
            || lease.owner_kind != "agent_lifecycle"
            || lease.mode != "lifecycle_transition"
        {
            return Err("an active conversation owner still excludes hold release".into());
        }
        transitions += 1;
        if transitions > 1 {
            return Err("multiple active lifecycle transitions exclude hold release".into());
        }
    }
    authorize(&leases[index])?;
    leases.remove(index);
    save_leases(&leases).map_err(|error| {
        format!("hold release write failed after authorization; release is not confirmed: {error}")
    })
}

#[cfg(test)]
mod tests;
