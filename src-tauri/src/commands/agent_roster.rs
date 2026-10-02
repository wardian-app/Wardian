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
//! It is set after restoration has published every saved agent, or when the
//! home has no saved roster at all. A saved roster that cannot be read or
//! parsed leaves it unset for the session, because its agents may still exist.
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

/// Whether the Wardian home is known and has no saved roster to restore.
///
/// Only a confirmed absence lets startup vouch for the roster without
/// restoring it. An unresolved home, or a `settings/state.json` that exists but
/// could not be read or parsed, is a failure: the agents it records may still
/// exist, and treating them as deleted would discard their saved state.
pub(crate) fn saved_roster_absent(wardian_home: Option<&std::path::Path>) -> bool {
    let Some(home) = wardian_home else {
        return false;
    };
    matches!(
        std::fs::symlink_metadata(home.join("settings/state.json")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound
    )
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

    #[test]
    fn only_a_confirmed_absent_saved_roster_vouches_without_restoring() {
        let home = tempfile::tempdir().expect("temp home");
        assert!(
            saved_roster_absent(Some(home.path())),
            "a home with no saved roster has nothing left to restore"
        );

        // A saved roster that startup failed to parse still records agents
        // that may exist; its failure must not authorize pruning them.
        let settings = home.path().join("settings");
        std::fs::create_dir_all(&settings).expect("settings dir");
        std::fs::write(settings.join("state.json"), "not json").expect("malformed roster");
        assert!(!saved_roster_absent(Some(home.path())));

        // An unreadable entry in place of the file is not an absence either.
        std::fs::remove_file(settings.join("state.json")).expect("remove roster");
        std::fs::create_dir(settings.join("state.json")).expect("unreadable roster");
        assert!(!saved_roster_absent(Some(home.path())));

        assert!(
            !saved_roster_absent(None),
            "an unresolved home cannot confirm anything"
        );
    }
}
