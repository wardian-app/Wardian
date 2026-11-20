use super::*;
use crate::control::test_support::TestWardianHome;
use std::sync::Arc;
use wardian_core::conversation_lease::{lease_path, save_leases};

const AGENT: &str = "legacy-held-agent";
fn no_candidates(_: &AgentConfig, _: &ConversationLease) -> Vec<u32> {
    Vec::new()
}
fn candidate(_: &AgentConfig, _: &ConversationLease) -> Vec<u32> {
    vec![42]
}
fn operator() -> Result<String, String> {
    Ok("test-os-operator".into())
}
fn unavailable_operator() -> Result<String, String> {
    Err("operator identity lookup failed".into())
}

fn hold() -> ConversationLease {
    ConversationLease {
        agent_id: AGENT.into(),
        provider: "codex".into(),
        resume_session: "old-session".into(),
        owner_kind: "prior_provider_hold".into(),
        owner_id: "prior-owner".into(),
        acquisition_id: "exact-acquisition".into(),
        owner_node_id: None,
        mode: "uncertain_previous_provider".into(),
        started_at: "2026-01-01T00:00:00Z".into(),
        heartbeat_at: "2026-01-01T00:00:00Z".into(),
        expires_at: "9999-12-31T23:59:59Z".into(),
    }
}
fn history(home: &Path) -> std::path::PathBuf {
    home.join("agents")
        .join(AGENT)
        .join("lifecycle-history.jsonl")
}
async fn fixture() -> (TestWardianHome, Arc<AppState>, ConversationLease) {
    let home = TestWardianHome::new_async().await;
    let state = Arc::new(AppState::new());
    let mut agent = crate::control::tests::test_agent(AGENT, "Legacy", "Coder");
    agent.process_id = None;
    *agent.current_status.lock().unwrap() = "Off".into();
    {
        let mut config = agent.config.lock().unwrap();
        config.provider = "codex".into();
        config.resume_session = Some("old-session".into());
        config.is_off = true;
    }
    state.agents.lock().await.insert(AGENT.into(), agent);
    std::fs::create_dir_all(home.path().join("agents").join(AGENT)).unwrap();
    let hold = hold();
    save_leases(std::slice::from_ref(&hold)).unwrap();
    (home, state, hold)
}

#[tokio::test]
async fn attested_release_audits_exact_historical_hold_and_preserves_config_history_and_other_holds(
) {
    let (home, state, hold) = fixture().await;
    let mut other = hold.clone();
    other.acquisition_id = "other-acquisition".into();
    save_leases(&[hold.clone(), other.clone()]).unwrap();
    let original_history = b"{\"event\":\"existing-lifecycle-event\"}\n";
    std::fs::write(history(home.path()), original_history).unwrap();
    let archive = home
        .path()
        .join("agents")
        .join(AGENT)
        .join("conversation.jsonl");
    std::fs::write(&archive, "original history\n").unwrap();
    // The selected hold belongs to an older provider/session than current config.
    let config = state.agents.lock().await[AGENT].config.clone();
    {
        let mut config = config.lock().unwrap();
        config.provider = "claude".into();
        config.resume_session = Some("current-session".into());
    }
    let before = serde_json::to_value(&*config.lock().unwrap()).unwrap();
    let actor = MessageOrigin::WardianAgent {
        session_id: "managed-actor".into(),
    };
    let response = release_for_state(
        &state,
        AGENT,
        hold.acquisition_id.clone(),
        true,
        Some(actor.clone()),
        no_candidates,
        operator,
    )
    .await
    .unwrap();
    assert!(response.ok);
    assert_eq!(response.provider, hold.provider);
    assert_eq!(response.resume_session, hold.resume_session);
    assert_eq!(load_leases_checked().unwrap(), vec![other]);
    let log = std::fs::read(history(home.path())).unwrap();
    assert!(log.starts_with(original_history));
    let authorization: Authorization =
        serde_json::from_slice(&log[original_history.len()..]).unwrap();
    assert_eq!(authorization.hold, hold);
    assert_eq!(authorization.authorization_id, response.authorization_id);
    assert_eq!(authorization.operator_os_identity, "test-os-operator");
    assert_eq!(authorization.managed_actor, Some(actor));
    assert!(authorization.operator_verified_exit);
    assert!(chrono::DateTime::parse_from_rfc3339(&authorization.authorized_at).is_ok());
    assert_eq!(authorization.release_outcome, "authorization_only");
    assert_eq!(
        serde_json::to_value(&*config.lock().unwrap()).unwrap(),
        before
    );
    assert_eq!(
        std::fs::read_to_string(archive).unwrap(),
        "original history\n"
    );
}

#[tokio::test]
async fn missing_attestation_candidates_and_foreign_acquisition_refuse_without_audit() {
    let (home, state, hold) = fixture().await;
    assert!(release_for_state(
        &state,
        AGENT,
        hold.acquisition_id.clone(),
        true,
        None,
        no_candidates,
        unavailable_operator
    )
    .await
    .is_err());
    assert!(release_for_state(
        &state,
        AGENT,
        hold.acquisition_id.clone(),
        false,
        None,
        no_candidates,
        operator
    )
    .await
    .is_err());
    assert!(release_for_state(
        &state,
        AGENT,
        hold.acquisition_id.clone(),
        true,
        None,
        candidate,
        operator
    )
    .await
    .is_err());
    let mut foreign = hold.clone();
    foreign.agent_id = "foreign-agent".into();
    foreign.acquisition_id = "foreign-acquisition".into();
    save_leases(&[hold.clone(), foreign.clone()]).unwrap();
    assert!(release_for_state(
        &state,
        AGENT,
        foreign.acquisition_id.clone(),
        true,
        None,
        no_candidates,
        operator
    )
    .await
    .is_err());
    assert_eq!(load_leases_checked().unwrap(), vec![hold, foreign]);
    assert!(!history(home.path()).exists());
}

#[tokio::test]
async fn owned_runtime_and_retained_stop_refuse_operator_override() {
    let (home, state, hold) = fixture().await;
    state
        .agents
        .lock()
        .await
        .get_mut(AGENT)
        .unwrap()
        .runtime_generation = Some(7);
    assert!(release_for_state(
        &state,
        AGENT,
        hold.acquisition_id.clone(),
        true,
        None,
        no_candidates,
        operator
    )
    .await
    .is_err());
    state
        .agents
        .lock()
        .await
        .get_mut(AGENT)
        .unwrap()
        .runtime_generation = None;
    let mut captured = crate::control::tests::test_agent(AGENT, "Legacy", "Coder");
    captured.process_id = None;
    captured.config.lock().unwrap().provider = "codex".into();
    let guard = crate::manager::codex_stop::prepare_stop(home.path(), AGENT)
        .unwrap()
        .with_prior_hold(Some(hold.owner()))
        .capture(captured);
    assert!(release_for_state(
        &state,
        AGENT,
        hold.acquisition_id.clone(),
        true,
        None,
        no_candidates,
        operator
    )
    .await
    .is_err());
    assert_eq!(load_leases_checked().unwrap(), vec![hold]);
    assert!(!history(home.path()).exists());
    guard.begin_stop().wait().await.unwrap();
}

#[tokio::test]
async fn malformed_oversized_or_unwritable_history_tail_keeps_hold() {
    let (home, state, hold) = fixture().await;
    let path = history(home.path());
    std::fs::create_dir(&path).unwrap();
    assert!(release_for_state(
        &state,
        AGENT,
        hold.acquisition_id.clone(),
        true,
        None,
        no_candidates,
        operator
    )
    .await
    .is_err());
    std::fs::remove_dir(&path).unwrap();
    let mut oversized =
        serde_json::to_vec(&serde_json::json!({"event":"x".repeat(MAX_RECORD_BYTES)})).unwrap();
    oversized.push(b'\n');
    for bytes in [b"partial".to_vec(), b"invalid-json\n".to_vec(), oversized] {
        std::fs::write(&path, &bytes).unwrap();
        assert!(release_for_state(
            &state,
            AGENT,
            hold.acquisition_id.clone(),
            true,
            None,
            no_candidates,
            operator
        )
        .await
        .is_err());
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        assert_eq!(load_leases_checked().unwrap(), vec![hold.clone()]);
    }
}

#[tokio::test]
async fn valid_history_over_one_mib_remains_appendable_and_preserves_every_prior_byte() {
    let (home, state, hold) = fixture().await;
    let path = history(home.path());
    let prior = b"{\"event\":\"prior-lifecycle-event\"}\n".repeat(40_000);
    assert!(prior.len() > 1024 * 1024);
    std::fs::write(&path, &prior).unwrap();
    let response = release_for_state(
        &state,
        AGENT,
        hold.acquisition_id.clone(),
        true,
        None,
        no_candidates,
        operator,
    )
    .await
    .unwrap();
    assert!(response.ok);
    let after = std::fs::read(path).unwrap();
    assert_eq!(&after[..prior.len()], &prior);
    let authorization: Authorization = serde_json::from_slice(&after[prior.len()..]).unwrap();
    assert_eq!(authorization.hold, hold);
    assert_eq!(authorization.authorization_id, response.authorization_id);
    assert!(load_leases_checked().unwrap().is_empty());
}

#[test]
fn history_validation_reads_only_a_bounded_tail_and_accepts_maximum_record() {
    struct Counted {
        cursor: std::io::Cursor<Vec<u8>>,
        bytes_read: usize,
    }
    impl Read for Counted {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            let count = self.cursor.read(buffer)?;
            self.bytes_read += count;
            Ok(count)
        }
    }
    impl Seek for Counted {
        fn seek(&mut self, position: SeekFrom) -> std::io::Result<u64> {
            self.cursor.seek(position)
        }
    }
    let mut bytes = vec![b'x'; 2 * MAX_RECORD_BYTES];
    bytes.push(b'\n');
    let mut record = serde_json::to_vec(&serde_json::json!({"event":"prior"})).unwrap();
    record.resize(MAX_RECORD_BYTES - 1, b' ');
    record.push(b'\n');
    assert_eq!(record.len(), MAX_RECORD_BYTES);
    bytes.extend(record);
    let mut counted = Counted {
        cursor: std::io::Cursor::new(bytes),
        bytes_read: 0,
    };
    validate_history_tail(&mut counted).unwrap();
    assert!(counted.bytes_read <= MAX_RECORD_BYTES + 1);
}

#[tokio::test]
async fn lease_write_failure_records_authorization_only_and_keeps_hold() {
    let (home, state, hold) = fixture().await;
    let blocker = lease_path().unwrap().with_extension("json.tmp");
    std::fs::create_dir(&blocker).unwrap();
    let error = release_for_state(
        &state,
        AGENT,
        hold.acquisition_id.clone(),
        true,
        None,
        no_candidates,
        operator,
    )
    .await
    .unwrap_err();
    assert!(error.message.contains("release is not confirmed"));
    let authorization: Authorization =
        serde_json::from_slice(&std::fs::read(history(home.path())).unwrap()).unwrap();
    assert_eq!(authorization.release_outcome, "authorization_only");
    assert_eq!(authorization.hold, hold);
    assert_eq!(load_leases_checked().unwrap(), vec![hold]);
}

#[tokio::test]
async fn lifecycle_gate_serializes_release_and_refuses_runtime_installed_while_waiting() {
    let (home, state, hold) = fixture().await;
    let gate = state.lock_agent_lifecycle(AGENT).await;
    let waiting = state.clone();
    let acquisition = hold.acquisition_id.clone();
    let task = tokio::spawn(async move {
        release_for_state(
            &waiting,
            AGENT,
            acquisition,
            true,
            None,
            no_candidates,
            operator,
        )
        .await
    });
    tokio::task::yield_now().await;
    assert!(!task.is_finished());
    state
        .agents
        .lock()
        .await
        .get_mut(AGENT)
        .unwrap()
        .runtime_generation = Some(8);
    drop(gate);
    assert!(task.await.unwrap().is_err());
    state
        .agents
        .lock()
        .await
        .get_mut(AGENT)
        .unwrap()
        .runtime_generation = None;
    assert_eq!(load_leases_checked().unwrap(), vec![hold]);
    assert!(!history(home.path()).exists());
}

#[test]
fn candidate_veto_covers_selected_hold_and_different_current_provider_session() {
    let config = AgentConfig {
        session_id: AGENT.into(),
        provider: "claude".into(),
        resume_session: Some("current-session".into()),
        fresh_provider_session_id: Some("pending-session".into()),
        ..Default::default()
    };
    let identities = candidate_identities(&config, &hold());
    for identity in [
        (AGENT.into(), "codex".into()),
        ("old-session".into(), "codex".into()),
        (AGENT.into(), "claude".into()),
        ("current-session".into(), "claude".into()),
        ("pending-session".into(), "claude".into()),
    ] {
        assert!(identities.contains(&identity));
    }
}

#[test]
fn operator_identity_comes_from_os_principal() {
    let identity = operator_os_identity().unwrap();
    #[cfg(windows)]
    assert!(identity.starts_with("windows-account:"));
    #[cfg(not(windows))]
    assert!(identity
        .strip_prefix("unix-uid:")
        .unwrap()
        .parse::<u32>()
        .is_ok());
}
