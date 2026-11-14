use super::*;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Barrier,
};

struct Home(Option<std::ffi::OsString>);
impl Drop for Home {
    fn drop(&mut self) {
        match self.0.take() {
            Some(value) => std::env::set_var("WARDIAN_HOME", value),
            None => std::env::remove_var("WARDIAN_HOME"),
        }
    }
}
fn home(path: &std::path::Path) -> Home {
    let previous = Home(std::env::var_os("WARDIAN_HOME"));
    std::env::set_var("WARDIAN_HOME", path);
    previous
}
fn hold() -> ConversationLease {
    ConversationLease {
        agent_id: "agent".into(),
        provider: "codex".into(),
        resume_session: "old".into(),
        owner_kind: "prior_provider_hold".into(),
        owner_id: "owner".into(),
        acquisition_id: "acquisition".into(),
        owner_node_id: None,
        mode: "uncertain_previous_provider".into(),
        started_at: "2026-01-01T00:00:00Z".into(),
        heartbeat_at: "2026-01-01T00:00:00Z".into(),
        expires_at: "9999-12-31T23:59:59Z".into(),
    }
}

#[test]
fn operator_release_requires_exact_snapshot_and_preserves_unrelated_holds() {
    let _lock = crate::tests::env_lock();
    let temp = tempfile::tempdir().unwrap();
    let _home = home(temp.path());
    let expected = hold();
    let mut unrelated = expected.clone();
    unrelated.acquisition_id = "other-acquisition".into();
    save_leases(&[expected.clone(), unrelated.clone()]).unwrap();
    let mut authorized = false;
    release_previous_provider_hold_exact(&expected, |exact| {
        assert_eq!(exact, &expected);
        authorized = true;
        Ok(())
    })
    .unwrap();
    assert!(authorized);
    assert_eq!(load_leases_checked().unwrap(), vec![unrelated]);
    assert!(release_previous_provider_hold_exact(&expected, |_| panic!(
        "must not authorize missing hold"
    ))
    .is_err());
}

#[test]
fn stale_or_ambiguous_hold_never_authorizes_release() {
    let _lock = crate::tests::env_lock();
    let temp = tempfile::tempdir().unwrap();
    let _home = home(temp.path());
    let expected = hold();
    for field in 0..7 {
        let mut changed = expected.clone();
        match field {
            0 => changed.agent_id = "different-agent".into(),
            1 => changed.provider = "claude".into(),
            2 => changed.resume_session = "new-session".into(),
            3 => changed.acquisition_id = "new-acquisition".into(),
            4 => changed.owner_id = "new-owner".into(),
            5 => changed.heartbeat_at = "2026-02-01T00:00:00Z".into(),
            _ => changed.owner_node_id = Some("changed-node".into()),
        }
        save_leases(std::slice::from_ref(&changed)).unwrap();
        assert!(
            release_previous_provider_hold_exact(&expected, |_| panic!("stale authorization"))
                .is_err()
        );
        assert_eq!(load_leases_checked().unwrap(), vec![changed]);
    }
    save_leases(&[expected.clone(), expected.clone()]).unwrap();
    assert!(
        release_previous_provider_hold_exact(&expected, |_| panic!("ambiguous authorization"))
            .is_err()
    );
}

#[test]
fn authorization_failure_and_active_writer_keep_exact_hold() {
    let _lock = crate::tests::env_lock();
    let temp = tempfile::tempdir().unwrap();
    let _home = home(temp.path());
    let expected = hold();
    save_leases(std::slice::from_ref(&expected)).unwrap();
    assert!(
        release_previous_provider_hold_exact(&expected, |_| Err("audit sync failed".into()))
            .is_err()
    );
    assert_eq!(load_leases_checked().unwrap(), vec![expected.clone()]);
    let mut writer = expected.clone();
    writer.owner_kind = "agent_lifecycle".into();
    writer.owner_id = "concurrent-lifecycle".into();
    writer.mode = "lifecycle_transition".into();
    writer.acquisition_id = "writer-acquisition".into();
    writer.expires_at = (chrono::Utc::now() + chrono::Duration::minutes(5)).to_rfc3339();
    save_leases(&[expected.clone(), writer.clone()]).unwrap();
    assert!(
        release_previous_provider_hold_exact(&expected, |_| panic!("writer still active")).is_err()
    );
    assert_eq!(load_leases_checked().unwrap(), vec![expected, writer]);
}

#[test]
fn competing_operator_releases_authorize_only_once() {
    let _lock = crate::tests::env_lock();
    let temp = tempfile::tempdir().unwrap();
    let _home = home(temp.path());
    let expected = hold();
    save_leases(std::slice::from_ref(&expected)).unwrap();
    let barrier = Arc::new(Barrier::new(2));
    let authorizations = Arc::new(AtomicUsize::new(0));
    let threads: Vec<_> = (0..2)
        .map(|_| {
            let expected = expected.clone();
            let barrier = barrier.clone();
            let authorizations = authorizations.clone();
            std::thread::spawn(move || {
                barrier.wait();
                release_previous_provider_hold_exact(&expected, |_| {
                    authorizations.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                })
            })
        })
        .collect();
    let successes = threads
        .into_iter()
        .map(|thread| usize::from(thread.join().unwrap().is_ok()))
        .sum::<usize>();
    assert_eq!(successes, 1);
    assert_eq!(authorizations.load(Ordering::SeqCst), 1);
    assert!(load_leases_checked().unwrap().is_empty());
}

#[test]
fn legacy_hold_without_acquisition_cannot_be_operator_released() {
    let _lock = crate::tests::env_lock();
    let temp = tempfile::tempdir().unwrap();
    let _home = home(temp.path());
    let mut legacy = hold();
    legacy.acquisition_id.clear();
    save_leases(std::slice::from_ref(&legacy)).unwrap();
    assert!(
        release_previous_provider_hold_exact(&legacy, |_| panic!("no exact acquisition")).is_err()
    );
    assert_eq!(load_leases_checked().unwrap(), vec![legacy]);
}

fn transition() -> ConversationLease {
    let mut lease = hold();
    lease.owner_kind = "agent_lifecycle".into();
    lease.mode = "lifecycle_transition".into();
    lease.owner_id = "current-transition".into();
    lease.acquisition_id = "transition-acquisition".into();
    // Fresh rotation may already have retargeted the reservation.
    lease.resume_session = "fresh-session".into();
    lease
}

#[test]
fn verified_exit_releases_exact_hold_and_preserves_current_transition() {
    let _lock = crate::tests::env_lock();
    let temp = tempfile::tempdir().unwrap();
    let _home = home(temp.path());
    let expected = hold();
    for current in [None, Some(transition())] {
        let mut leases = vec![expected.clone()];
        leases.extend(current.clone());
        save_leases(&leases).unwrap();
        let mut verified = false;
        release_previous_provider_hold_after_exit(&expected, |exact| {
            assert_eq!(exact, &expected);
            verified = true;
            Ok(())
        })
        .unwrap();
        assert!(verified);
        assert_eq!(
            load_leases_checked().unwrap(),
            current.into_iter().collect::<Vec<_>>()
        );
    }
}

#[test]
fn verified_exit_rejects_foreign_other_mode_and_ambiguous_active_owners() {
    let _lock = crate::tests::env_lock();
    let temp = tempfile::tempdir().unwrap();
    let _home = home(temp.path());
    let expected = hold();
    let current = transition();
    let mut foreign = current.clone();
    foreign.agent_id = "foreign-agent".into();
    foreign.resume_session = expected.resume_session.clone();
    let mut resume = current.clone();
    resume.mode = "background_resume".into();
    let mut fresh = current.clone();
    fresh.mode = "background_fresh".into();
    let mut other_kind = current.clone();
    other_kind.owner_kind = "automation".into();
    let mut second = current.clone();
    second.owner_id = "second-transition".into();
    second.acquisition_id = "second-acquisition".into();
    for owners in [
        vec![foreign],
        vec![resume],
        vec![fresh],
        vec![other_kind],
        vec![current.clone(), second],
        vec![current.clone(), current],
    ] {
        let mut leases = vec![expected.clone()];
        leases.extend(owners);
        save_leases(&leases).unwrap();
        assert!(release_previous_provider_hold_after_exit(&expected, |_| {
            panic!("conflicting owner must prevent verification")
        })
        .is_err());
        assert_eq!(load_leases_checked().unwrap(), leases);
    }
}

#[test]
fn verified_exit_retains_changed_missing_and_duplicate_acquisitions() {
    let _lock = crate::tests::env_lock();
    let temp = tempfile::tempdir().unwrap();
    let _home = home(temp.path());
    let expected = hold();
    // Include fields outside the owner identity to pin full-record comparison.
    for field in 0..12 {
        let mut changed = expected.clone();
        match field {
            0 => changed.agent_id = "different-agent".into(),
            1 => changed.provider = "claude".into(),
            2 => changed.resume_session = "different-session".into(),
            3 => changed.owner_kind = "agent_lifecycle".into(),
            4 => changed.owner_id = "different-owner".into(),
            5 => changed.acquisition_id = "different-acquisition".into(),
            6 => changed.acquisition_id.clear(),
            7 => changed.owner_node_id = Some("different-node".into()),
            8 => changed.mode = "lifecycle_transition".into(),
            9 => changed.started_at = "2026-02-01T00:00:00Z".into(),
            10 => changed.heartbeat_at = "2026-02-01T00:00:00Z".into(),
            _ => changed.expires_at = "9998-12-31T23:59:59Z".into(),
        }
        // owner_kind alone would make an uncertain hold invalid on load.
        if field == 3 {
            changed.mode = "lifecycle_transition".into();
        }
        save_leases(std::slice::from_ref(&changed)).unwrap();
        assert!(release_previous_provider_hold_after_exit(&expected, |_| {
            panic!("changed acquisition must prevent verification")
        })
        .is_err());
        assert_eq!(load_leases_checked().unwrap(), vec![changed]);
    }
    for leases in [vec![], vec![expected.clone(), expected.clone()]] {
        save_leases(&leases).unwrap();
        assert!(release_previous_provider_hold_after_exit(&expected, |_| {
            panic!("missing or ambiguous acquisition must prevent verification")
        })
        .is_err());
        assert_eq!(load_leases_checked().unwrap(), leases);
    }
}

#[test]
fn verified_exit_callback_error_retains_hold_and_transition() {
    let _lock = crate::tests::env_lock();
    let temp = tempfile::tempdir().unwrap();
    let _home = home(temp.path());
    let expected = hold();
    let leases = vec![expected.clone(), transition()];
    save_leases(&leases).unwrap();
    assert_eq!(
        release_previous_provider_hold_after_exit(&expected, |exact| {
            assert_eq!(exact, &expected);
            Err("writer exit receipt mismatch".into())
        })
        .unwrap_err(),
        "writer exit receipt mismatch"
    );
    assert_eq!(load_leases_checked().unwrap(), leases);
}
