//! Exact hold retention across cancellation, finite expiry, and owned exit.
use super::*;
use crate::commands::agent::{
    acquire_agent_lifecycle_transition_lease, hold_previous_provider_before_rotation,
    take_agent_runtime_for_termination,
    tests::{make_test_agent, WardianHomeGuard},
};
use wardian_core::conversation_lease::{
    load_leases_checked, try_acquire_lease, ConversationLeaseAcquireOutcome,
};

fn configured_agent(id: &str) -> ActiveAgent {
    let agent = make_test_agent();
    {
        let mut config = agent.config.lock().unwrap();
        config.session_id = id.into();
        config.session_name = id.into();
        config.provider = "mock".into();
        config.resume_session = Some(format!("provider-{id}"));
    }
    agent
}

fn assert_hold_blocks_after_expiry(
    hold: &wardian_core::conversation_lease::ConversationLeaseOwner,
) {
    let leases = load_leases_checked().unwrap();
    let agent_id = leases
        .iter()
        .find(|lease| lease.owner() == *hold)
        .unwrap()
        .agent_id
        .clone();
    let mut retry = leases
        .into_iter()
        .find(|lease| lease.mode == "lifecycle_transition" && lease.agent_id == agent_id)
        .unwrap();
    let future = chrono::Utc::now() + chrono::Duration::days(2);
    retry.owner_id = uuid::Uuid::new_v4().to_string();
    retry.acquisition_id = uuid::Uuid::new_v4().to_string();
    retry.expires_at = (future + chrono::Duration::minutes(1)).to_rfc3339();
    match try_acquire_lease(retry, &future.to_rfc3339()).unwrap() {
        ConversationLeaseAcquireOutcome::Conflict(conflict) => assert_eq!(conflict.owner(), *hold),
        ConversationLeaseAcquireOutcome::Acquired => {
            panic!("Unconfirmed writer lost its indefinite hold")
        }
    }
}

#[test]
fn unowned_pid_is_rejected_without_mutation_or_signalling() {
    let mut agent = make_test_agent();
    agent.process_id = Some(12345);
    let before = agent.config.lock().unwrap().clone();
    assert!(validate_stop_authority(&agent).is_err());
    assert_eq!(agent.process_id, Some(12345));
    assert_eq!(
        serde_json::to_value(agent.config.lock().unwrap().clone()).unwrap(),
        serde_json::to_value(before).unwrap()
    );
}

#[test]
fn missing_child_for_published_terminal_cannot_produce_exit_receipt() {
    let mut agent = make_test_agent();
    agent.runtime_generation = Some(42);
    assert!(validate_stop_authority(&agent).is_err());
}

#[tokio::test]
async fn empty_runtime_has_no_writer_to_join() {
    let mut agent = make_test_agent();
    let result = stop_with_receipt(&mut agent, None).await;
    #[cfg(windows)]
    result.unwrap();
    #[cfg(not(windows))]
    assert!(result
        .unwrap_err()
        .contains("not qualified on this platform"));
    assert!(agent.child_process.is_none());
}

#[test]
fn cancelled_shutdown_keeps_exact_conversation_hold_after_lease_expiry() {
    let _lock = crate::utils::wardian_test_env_lock();
    let _home_guard = WardianHomeGuard;
    let home = tempfile::tempdir().unwrap();
    std::env::set_var("WARDIAN_HOME", home.path());
    let mut agent = configured_agent("cancelled-workspace");
    agent.process_id = Some(12345); // Sentinel only: this test never signals a PID.
    let mut lease =
        acquire_agent_lifecycle_transition_lease(&agent.config.lock().unwrap(), "worktree")
            .unwrap();
    let hold = hold_previous_provider_before_rotation(&agent, &lease)
        .unwrap()
        .unwrap();
    lease.retain_on_drop();
    let old_runtime = take_agent_runtime_for_termination(&mut agent);
    assert!(agent.child_process.is_none() && agent.process_id.is_none());
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let (ready, observed) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            let _lease = lease;
            let _old_runtime = old_runtime;
            ready.send(()).unwrap();
            std::future::pending::<()>().await;
        });
        observed.await.unwrap();
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
    });
    assert_hold_blocks_after_expiry(&hold);
}

#[test]
fn failed_shutdown_does_not_release_the_exact_hold() {
    let _lock = crate::utils::wardian_test_env_lock();
    let _home_guard = WardianHomeGuard;
    let home = tempfile::tempdir().unwrap();
    std::env::set_var("WARDIAN_HOME", home.path());
    let mut agent = configured_agent("failed-workspace");
    agent.process_id = Some(12345); // No process authority exists; preflight must refuse.
    let mut lease =
        acquire_agent_lifecycle_transition_lease(&agent.config.lock().unwrap(), "worktree")
            .unwrap();
    let hold = hold_previous_provider_before_rotation(&agent, &lease)
        .unwrap()
        .unwrap();
    lease.retain_on_drop();
    let mut old_runtime = take_agent_runtime_for_termination(&mut agent);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    assert!(runtime
        .block_on(stop_with_receipt(&mut old_runtime, Some(&hold)))
        .is_err());
    drop(lease);
    assert_hold_blocks_after_expiry(&hold);
}

#[cfg(windows)]
#[test]
fn owned_tree_exit_releases_only_this_operations_hold() {
    let _lock = crate::utils::wardian_test_env_lock();
    let _home_guard = WardianHomeGuard;
    let home = tempfile::tempdir().unwrap();
    std::env::set_var("WARDIAN_HOME", home.path());
    let mut unrelated = configured_agent("other-owner");
    unrelated.process_id = Some(12345);
    let other_lease =
        acquire_agent_lifecycle_transition_lease(&unrelated.config.lock().unwrap(), "worktree")
            .unwrap();
    let other_hold = hold_previous_provider_before_rotation(&unrelated, &other_lease)
        .unwrap()
        .unwrap();
    drop(other_lease);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let (job, child, _pty, _descendant) =
            crate::utils::process::test_contained_pty_tree(home.path()).await;
        let mut agent = configured_agent("confirmed-workspace");
        agent.process_id = child.process_id();
        agent.child_process = Some(child);
        agent.job_object = Some(job);
        let lease =
            acquire_agent_lifecycle_transition_lease(&agent.config.lock().unwrap(), "worktree")
                .unwrap();
        let hold = hold_previous_provider_before_rotation(&agent, &lease)
            .unwrap()
            .unwrap();
        stop_with_receipt(&mut agent, Some(&hold)).await.unwrap();
        let leases = load_leases_checked().unwrap();
        assert!(!leases.iter().any(|lease| lease.owner() == hold));
        assert!(leases.iter().any(|lease| lease.owner() == other_hold));
    });
}
