use super::*;
use crate::manager::spawn::acquire_restore_retry_spawn_lease_for_startup as admit_restore;
use chrono::{DateTime, Duration, Utc};
use std::time::Duration as SleepDuration;
use wardian_core::agent_replacement::{
    acquire_agent_roster_barrier, recover_pending_replacements, PendingAgentReplacement,
    RecoveryStatus, ReplacementJournalGuard, ReplacementPhase,
};
use wardian_core::conversation_lease::{
    load_leases_checked, release_lease_owner_persisted, save_leases, try_acquire_lease,
    ConversationLease, ConversationLeaseAcquireOutcome,
};
use wardian_core::db::agent_messaging;

fn saved_config() -> AgentConfig {
    AgentConfig {
        session_id: uuid::Uuid::new_v4().to_string(),
        provider: "mock".into(),
        provider_config: ProviderConfig::Mock(Default::default()),
        resume_session: Some(uuid::Uuid::new_v4().to_string()),
        ..Default::default()
    }
}

fn clear_lease(config: &AgentConfig, now: DateTime<Utc>) -> ConversationLease {
    ConversationLease {
        agent_id: config.session_id.clone(),
        provider: config.provider.clone(),
        resume_session: provider_restore_session_identity(config).into(),
        owner_kind: "agent_lifecycle".into(),
        owner_id: format!("clear:{}", uuid::Uuid::new_v4()),
        acquisition_id: uuid::Uuid::new_v4().to_string(),
        owner_node_id: None,
        mode: "lifecycle_transition".into(),
        started_at: (now - Duration::minutes(1)).to_rfc3339(),
        heartbeat_at: now.to_rfc3339(),
        expires_at: (now + Duration::seconds(4)).to_rfc3339(),
    }
}

fn persist(config: &AgentConfig) {
    crate::manager::try_save_state_snapshot(std::slice::from_ref(config)).unwrap();
}

fn successive_clear_leases(config: &AgentConfig, now: DateTime<Utc>) -> [ConversationLease; 2] {
    let mut first = clear_lease(config, now - Duration::minutes(3));
    first.started_at.clone_from(&first.heartbeat_at);
    let mut later = clear_lease(config, now - Duration::minutes(2));
    later.started_at = (now - Duration::minutes(2))
        .with_timezone(&chrono::FixedOffset::west_opt(5 * 3600).unwrap())
        .to_rfc3339();
    assert_ne!(first.owner_id, later.owner_id);
    [first, later]
}

#[tokio::test]
async fn later_retained_clear_cancels_wait_before_retry() {
    let _environment = crate::utils::wardian_test_env_lock_async().await;
    let _home = TestHome::new();
    let config = saved_config();
    let now = Utc::now();
    let [first, later] = successive_clear_leases(&config, now);
    save_leases(std::slice::from_ref(&first)).unwrap();
    assert!(matches!(
        try_acquire_lease(later.clone(), &later.started_at).unwrap(),
        ConversationLeaseAcquireOutcome::Acquired
    ));
    let retry = wait_for_lifecycle_restore_lease(&config, &first, load_leases_checked, || now)
        .await
        .unwrap();
    assert!(
        !retry,
        "the later clear expired between polls and still supersedes the retry"
    );
    assert_eq!(load_leases_checked().unwrap(), vec![first, later]);
}

#[tokio::test]
async fn later_retained_clear_cancels_locked_admission_after_wait() {
    let _environment = crate::utils::wardian_test_env_lock_async().await;
    let _home = TestHome::new();
    let config = saved_config();
    persist(&config);
    let now = Utc::now();
    let [first, mut later] = successive_clear_leases(&config, now);
    // The competing clear can have selected another provider conversation.
    later.resume_session = "other-replacement-conversation".into();
    save_leases(std::slice::from_ref(&first)).unwrap();
    assert!(
        wait_for_lifecycle_restore_lease(&config, &first, load_leases_checked, || now)
            .await
            .unwrap()
    );
    assert!(matches!(
        try_acquire_lease(later.clone(), &later.started_at).unwrap(),
        ConversationLeaseAcquireOutcome::Acquired
    ));
    let error = admit_restore(std::slice::from_ref(&config), &config, Some(&first))
        .expect_err("the newly observed expired clear must fence locked spawn admission");
    assert_eq!(error.conflict_lease(), Some(&later));
    assert_eq!(load_leases_checked().unwrap(), vec![first, later]);
}

#[tokio::test]
async fn clear_retry_ignores_history_without_a_newer_clear_for_its_agent() {
    let _environment = crate::utils::wardian_test_env_lock_async().await;
    let _home = TestHome::new();
    let config = saved_config();
    persist(&config);
    let now = Utc::now();
    let [older, expected] = successive_clear_leases(&config, now);
    let mut equal = expected.clone();
    equal.owner_id = format!("clear:{}", uuid::Uuid::new_v4());
    equal.acquisition_id = uuid::Uuid::new_v4().to_string();
    equal.started_at.clone_from(&equal.heartbeat_at);
    let [_, mut other_agent] = successive_clear_leases(&config, now + Duration::minutes(1));
    let mut resume = other_agent.clone();
    resume.owner_id = format!("resume:{}", uuid::Uuid::new_v4());
    resume.acquisition_id = uuid::Uuid::new_v4().to_string();
    other_agent.agent_id = uuid::Uuid::new_v4().to_string();
    let history = vec![older, equal, other_agent, resume];
    save_leases(&history).unwrap();
    assert!(
        wait_for_lifecycle_restore_lease(&config, &expected, load_leases_checked, || now)
            .await
            .unwrap()
    );
    drop(
        admit_restore(std::slice::from_ref(&config), &config, Some(&expected))
            .unwrap()
            .expect("retained history leaves ordinary spawn admission available"),
    );
    assert_eq!(load_leases_checked().unwrap(), history);
}

#[tokio::test]
async fn later_retained_clear_is_inert_for_resume_and_headless_retries() {
    let _environment = crate::utils::wardian_test_env_lock_async().await;
    let _home = TestHome::new();
    let config = saved_config();
    persist(&config);
    let now = Utc::now();
    let [mut resume, later] = successive_clear_leases(&config, now);
    resume.owner_id = format!("resume:{}", uuid::Uuid::new_v4());
    let headless = headless_execution_lease(
        &config,
        "background_resume",
        now - Duration::minutes(4),
        now - Duration::minutes(3),
    );
    save_leases(std::slice::from_ref(&later)).unwrap();
    assert!(
        wait_for_lifecycle_restore_lease(&config, &resume, load_leases_checked, || now)
            .await
            .unwrap()
    );
    assert!(wait_for_headless_execution_restore_lease(
        &config,
        &headless,
        load_leases_checked,
        || now
    )
    .await
    .unwrap());
    drop(
        admit_restore(std::slice::from_ref(&config), &config, None)
            .unwrap()
            .expect("ordinary admission ignores expired ownership for these retries"),
    );
    assert_eq!(load_leases_checked().unwrap(), vec![later]);
}

#[tokio::test]
async fn clear_retry_requires_all_saved_identities_and_never_attaches_as_headless() {
    let config = saved_config();
    let now = Utc::now();
    let expected = clear_lease(&config, now);
    assert_eq!(
        retryable_lifecycle_restore_lease(&config, &expected),
        Some(expected.clone())
    );
    assert!(retryable_headless_execution_restore_lease(&config, &expected).is_none());

    for mismatch in [
        ConversationLease {
            agent_id: "other-agent".into(),
            ..expected.clone()
        },
        ConversationLease {
            provider: "codex".into(),
            ..expected.clone()
        },
        ConversationLease {
            resume_session: "previous-session".into(),
            ..expected.clone()
        },
        ConversationLease {
            resume_session: String::new(),
            ..expected.clone()
        },
        ConversationLease {
            acquisition_id: String::new(),
            ..expected.clone()
        },
        ConversationLease {
            owner_node_id: Some("other-node".into()),
            ..expected.clone()
        },
        ConversationLease {
            owner_id: "clear:".into(),
            ..expected.clone()
        },
        ConversationLease {
            mode: "uncertain_previous_provider".into(),
            ..expected.clone()
        },
    ] {
        assert!(
            retryable_lifecycle_restore_lease(&config, &mismatch).is_none(),
            "{mismatch:?}"
        );
        assert!(retryable_headless_execution_restore_lease(&config, &mismatch).is_none());
    }
    let mut fresh = config.clone();
    fresh.resume_session = None;
    assert!(retryable_lifecycle_restore_lease(&fresh, &expected).is_none());
    assert!(
        !wait_for_lifecycle_restore_lease(&fresh, &expected, || Ok(vec![]), || now)
            .await
            .unwrap()
    );
    fresh.fresh_provider_session_id = config.resume_session.clone();
    assert!(retryable_lifecycle_restore_lease(&fresh, &expected).is_some());
    fresh.is_off = true;
    assert!(retryable_lifecycle_restore_lease(&fresh, &expected).is_none());
}

#[tokio::test(start_paused = true)]
async fn clear_retry_follows_renewal_through_expiry_without_removing_the_lease() {
    let _environment = crate::utils::wardian_test_env_lock_async().await;
    let _home = TestHome::new();
    let config = saved_config();
    let now = Utc::now();
    let start = tokio::time::Instant::now();
    let lease = clear_lease(&config, now);
    save_leases(std::slice::from_ref(&lease)).unwrap();
    let task_config = config.clone();
    let task_lease = lease.clone();
    let waiter = tokio::spawn(async move {
        wait_for_lifecycle_restore_lease(&task_config, &task_lease, load_leases_checked, || {
            now + Duration::from_std(start.elapsed()).unwrap()
        })
        .await
    });
    tokio::task::yield_now().await;
    tokio::time::advance(SleepDuration::from_secs(2)).await;
    let mut renewed = lease.clone();
    renewed.heartbeat_at = (now + Duration::seconds(2)).to_rfc3339();
    renewed.expires_at = (now + Duration::seconds(8)).to_rfc3339();
    save_leases(std::slice::from_ref(&renewed)).unwrap();
    tokio::time::advance(SleepDuration::from_secs(3)).await;
    tokio::task::yield_now().await;
    assert!(
        !waiter.is_finished(),
        "the original expiry no longer permits retry"
    );
    assert!(waiter.await.unwrap().unwrap());
    assert!(start.elapsed() >= SleepDuration::from_secs(8));
    assert_eq!(load_leases_checked().unwrap(), vec![renewed]);
}

#[tokio::test(start_paused = true)]
async fn released_clear_allows_one_restore_and_one_durable_task_claim() {
    let _environment = crate::utils::wardian_test_env_lock_async().await;
    let home = TestHome::new();
    let config = saved_config();
    persist(&config);
    let settings = home.directory.path().join("settings/state.json");
    let saved_before = std::fs::read(&settings).unwrap();
    let archive = wardian_core::paths::agent_conversations_dir(&config.session_id)
        .unwrap()
        .join("clear-retry-beforeimage.jsonl");
    let archived_body = b"{\"role\":\"assistant\",\"content\":\"retained response\"}\n";
    std::fs::create_dir_all(archive.parent().unwrap()).unwrap();
    std::fs::write(&archive, archived_body).unwrap();
    let now = Utc::now();
    let mut lease = clear_lease(&config, now);
    lease.expires_at = (now + Duration::minutes(5)).to_rfc3339();
    save_leases(std::slice::from_ref(&lease)).unwrap();

    let admit = || {
        agent_messaging::with_db(|conn| {
            agent_messaging::admit(
                conn,
                agent_messaging::Admission {
                    sender: "fixture-sender",
                    recipient: &config.session_id,
                    message: "synthetic queued followup",
                    idempotency_key: Some("queued-before-recovery"),
                    task: true,
                    generation: 1,
                },
            )
        })
        .unwrap()
    };
    let queued = admit();
    let state = Arc::new(AppState::new());
    let expected_status = publish_error_placeholder(&state, &config).await;
    let restore_slots = Arc::new(tokio::sync::Semaphore::new(2));
    let (poll_started, mut polls) = tokio::sync::mpsc::unbounded_channel();
    let mut retries = Vec::new();
    for _ in 0..2 {
        let (config, lease, state, status, task_id) = (
            config.clone(),
            lease.clone(),
            state.clone(),
            expected_status.clone(),
            queued.record.id.clone(),
        );
        let slots = restore_slots.clone();
        let mut poll_started = Some(poll_started.clone());
        retries.push(tokio::spawn(async move {
            retry_once_after_lifecycle_lease_clear(
                &state,
                &config,
                &status,
                || async {
                    wait_for_lifecycle_restore_lease(
                        &config,
                        &lease,
                        || {
                            let leases = load_leases_checked()?;
                            if let Some(started) = poll_started.take() {
                                started.send(()).unwrap();
                            }
                            Ok(leases)
                        },
                        || now,
                    )
                    .await
                    .unwrap()
                },
                || async { slots.acquire_owned().await.ok() },
                |publication, permit| {
                    let (state, config, lease) = (&state, &config, &lease);
                    async move {
                        let _permit = permit;
                        let spawn_lease =
                            admit_restore(std::slice::from_ref(config), config, Some(lease))
                                .unwrap()
                                .expect("ordinary spawn admission");
                        assert_eq!(spawn_lease.owner().owner_kind, "provider_spawn");
                        let mut runtime = crate::restored_agent_without_process(
                            config.clone(),
                            "Idle",
                            String::new(),
                            None,
                            None,
                        );
                        runtime.runtime_generation = Some(2);
                        publication.publish(state, runtime).await;
                        agent_messaging::with_db(|conn| {
                            let claim =
                                agent_messaging::claim_next_task(conn, &config.session_id, 2)?
                                    .expect("one pending task");
                            assert_eq!(claim.record.id, task_id);
                            agent_messaging::finish_claim(conn, &claim, "provider_accepted")
                        })
                        .unwrap();
                    }
                },
            )
            .await
        }));
    }
    drop(poll_started);
    assert_eq!(polls.recv().await, Some(()));
    assert_eq!(polls.recv().await, Some(()));
    assert!(retries.iter().all(|retry| !retry.is_finished()));
    assert_eq!(restore_slots.available_permits(), 2);
    assert_eq!(admit().delivery_state, "pending");
    assert_eq!(load_leases_checked().unwrap(), vec![lease.clone()]);
    assert!(state
        .try_lock_agent_lifecycle(&config.session_id)
        .await
        .is_some());
    let conflict = admit_restore(std::slice::from_ref(&config), &config, Some(&lease))
        .expect_err("the lease still excludes direct spawn admission");
    assert_eq!(conflict.conflict_lease(), Some(&lease));

    release_lease_owner_persisted(&lease.owner()).unwrap();
    let mut completed = 0;
    for retry in retries {
        completed += usize::from(retry.await.unwrap().is_some());
    }
    assert_eq!(completed, 1);
    let repeated = admit();
    assert!(repeated.duplicate);
    assert_eq!(repeated.record.id, queued.record.id);
    assert_eq!(repeated.delivery_state, "provider_accepted");
    assert!(agent_messaging::with_db(|conn| {
        agent_messaging::claim_next_task(conn, &config.session_id, 2)
    })
    .unwrap()
    .is_none());
    assert_eq!(std::fs::read(settings).unwrap(), saved_before);
    assert_eq!(std::fs::read(archive).unwrap(), archived_body);
    assert!(load_leases_checked().unwrap().is_empty());
}

#[tokio::test]
async fn clear_retry_refuses_retarget_reacquisition_and_prior_hold_at_both_gates() {
    let _environment = crate::utils::wardian_test_env_lock_async().await;
    let _home = TestHome::new();
    let config = saved_config();
    persist(&config);
    let now = Utc::now();
    let expected = clear_lease(&config, now);
    assert!(
        wait_for_lifecycle_restore_lease(&config, &expected, || Ok(vec![]), || now)
            .await
            .unwrap()
    );

    let mut retargeted = expected.clone();
    retargeted.resume_session = "retargeted-provider-conversation".into();
    // The retarget can expire in the gap between polls or before admission.
    retargeted.expires_at = (now - Duration::seconds(1)).to_rfc3339();
    retargeted.heartbeat_at = (now - Duration::seconds(2)).to_rfc3339();
    let mut restarted = expected.clone();
    restarted.acquisition_id = uuid::Uuid::new_v4().to_string();
    restarted.heartbeat_at = retargeted.heartbeat_at.clone();
    restarted.expires_at = retargeted.expires_at.clone();
    let mut new_owner = clear_lease(&config, now);
    new_owner.expires_at = (now + Duration::minutes(5)).to_rfc3339();
    let mut held = retargeted.clone();
    held.owner_kind = "prior_provider_hold".into();
    held.owner_id = "previous-provider:fixture".into();
    held.acquisition_id = uuid::Uuid::new_v4().to_string();
    held.resume_session = expected.resume_session.clone();
    held.mode = "uncertain_previous_provider".into();
    for conflict in [retargeted, restarted, new_owner, held.clone()] {
        save_leases(std::slice::from_ref(&conflict)).unwrap();
        assert!(
            !wait_for_lifecycle_restore_lease(&config, &expected, load_leases_checked, || now)
                .await
                .unwrap()
        );
        let error = admit_restore(std::slice::from_ref(&config), &config, Some(&expected))
            .expect_err("changed or uncertain ownership fences admission");
        assert_eq!(error.conflict_lease(), Some(&conflict));
        assert_eq!(load_leases_checked().unwrap(), vec![conflict]);
    }

    held.resume_session = "old-provider-conversation".into();
    save_leases(std::slice::from_ref(&held)).unwrap();
    assert!(
        wait_for_lifecycle_restore_lease(&config, &expected, load_leases_checked, || now)
            .await
            .unwrap()
    );
    drop(
        admit_restore(std::slice::from_ref(&config), &config, Some(&expected))
            .unwrap()
            .expect("a hold on a different conversation is preserved"),
    );
    assert_eq!(load_leases_checked().unwrap(), vec![held]);
}

#[tokio::test]
async fn clear_retry_checks_the_durable_intent_selected_by_replacement_recovery() {
    let _environment = crate::utils::wardian_test_env_lock_async().await;
    let home = TestHome::new();
    let original = saved_config();
    let mut replacement = original.clone();
    replacement.resume_session = Some(uuid::Uuid::new_v4().to_string());
    let mut paused = original.clone();
    paused.is_off = true;
    let mut changed = original.clone();
    changed.session_name = "changed while waiting".into();
    let original_lease = clear_lease(&original, Utc::now());
    let replacement_lease = clear_lease(&replacement, Utc::now());
    let expected = [original.clone(), replacement.clone()];
    for different in [paused, changed] {
        persist(&different);
        assert!(admit_restore(&expected, &original, Some(&original_lease))
            .unwrap()
            .is_none());
    }
    assert!(load_leases_checked().unwrap().is_empty());

    let journal_dir = home.directory.path().join("settings/agent-replacements");
    for (phase, saved, matching, mismatched) in [
        (
            ReplacementPhase::Prepared,
            &original,
            &original_lease,
            &replacement_lease,
        ),
        (
            ReplacementPhase::LiveInstalled,
            &replacement,
            &replacement_lease,
            &original_lease,
        ),
    ] {
        persist(saved);
        let barrier = acquire_agent_roster_barrier(true).unwrap().unwrap();
        let mut journal = ReplacementJournalGuard::begin(
            PendingAgentReplacement::new(
                "clear",
                &original.session_id,
                original.clone(),
                replacement.clone(),
                None,
                None,
                None,
                false,
            ),
            barrier,
        )
        .unwrap();
        journal.advance(phase).unwrap();
        drop(journal);
        let path = std::fs::read_dir(&journal_dir)
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let before = std::fs::read(&path).unwrap();
        assert!(admit_restore(&expected, saved, Some(matching))
            .unwrap()
            .is_none());
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert!(load_leases_checked().unwrap().is_empty());
        assert!(matches!(recover_pending_replacements(true).unwrap(),
            RecoveryStatus::Recovered(recovered) if recovered.len() == 1));
        assert!(admit_restore(&expected, saved, Some(mismatched))
            .unwrap()
            .is_none());
        let other_config = if phase == ReplacementPhase::Prepared {
            &replacement
        } else {
            &original
        };
        assert!(admit_restore(&expected, other_config, Some(matching))
            .unwrap()
            .is_none());
        drop(
            admit_restore(&expected, saved, Some(matching))
                .unwrap()
                .expect("normal admission uses the config selected by journal recovery"),
        );
        assert!(!path.exists());
    }
}
