//! Whether the agent roster this process serves is complete.
//!
//! Startup restoration publishes saved agents one at a time, emitting
//! `agents-updated` after each, and only after the scheduler and listeners have
//! started. Until it finishes, `list_agents` answers with part of the roster,
//! and nothing in that answer says so. Most consumers do not care: the next
//! refresh fills in the rest. A consumer that treats absence as deletion — the
//! Garden pruning saved positions for agents that no longer exist — would
//! destroy state for every agent not yet restored.
//!
//! The flag is monotonic. A caller that reads `true` *before* listing agents is
//! guaranteed a complete roster from that list, because every saved agent was
//! already published when the flag was set.

use std::sync::atomic::Ordering;

use tauri::{AppHandle, Emitter, Manager, State};

use crate::state::AppState;

/// True once startup restoration has published every saved agent.
///
/// Read this before `list_agents`, never after: only that order lets a `true`
/// vouch for the list that follows.
#[tauri::command]
pub fn agent_roster_restored(state: State<'_, AppState>) -> bool {
    roster_restored(&state)
}

/// Record that the roster is complete and prompt listeners to re-read it.
///
/// The final per-agent `agents-updated` may already have been handled before
/// this flag was set, in which case that refresh was still marked partial. One
/// more event gives every window a refresh that sees the flag.
pub(crate) fn mark_agent_roster_restored(app: &AppHandle) {
    if mark_restored(&app.state::<AppState>()) {
        let _ = app.emit("agents-updated", ());
    }
}

fn roster_restored(state: &AppState) -> bool {
    state.agent_roster_restored.load(Ordering::Acquire)
}

/// Sets the flag, returning whether this call was the one that set it.
fn mark_restored(state: &AppState) -> bool {
    !state.agent_roster_restored.swap(true, Ordering::AcqRel)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roster_starts_incomplete_and_is_marked_complete_once() {
        let state = AppState::default();
        assert!(!roster_restored(&state));

        assert!(mark_restored(&state), "first mark sets the flag");
        assert!(roster_restored(&state));

        // Startup marks after pass 1 and again on the fallback path; the second
        // mark must not emit a redundant refresh.
        assert!(!mark_restored(&state), "a repeated mark is a no-op");
        assert!(roster_restored(&state));
    }
}
