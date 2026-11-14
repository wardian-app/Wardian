use super::*;
use crate::commands::provider_log_acquisition::{
    ProviderLogContinuityAnchor, ProviderLogDisabledSpan, ProviderLogNativeIdentity,
};
use wardian_core::control::ArchiveAdoptionNativeIdentity;

impl ConversationArchiveState {
    pub(crate) fn replace_adoption_source_for_test(&self, source: &str) -> io::Result<()> {
        use std::sync::atomic::Ordering;
        if self
            .fail_adoption_stage
            .compare_exchange(5, 0, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            let path = Path::new(source);
            let retained = path.with_extension("before-adoption-recheck");
            std::fs::rename(path, &retained)?;
            std::fs::copy(&retained, path)?;
        }
        Ok(())
    }
    pub(crate) fn adoption_fault(&self, stage: u8) -> io::Result<()> {
        use std::sync::atomic::Ordering;
        if self
            .fail_adoption_stage
            .compare_exchange(stage, 0, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            return Err(io::Error::other(format!(
                "injected archive adoption fault {stage}"
            )));
        }
        Ok(())
    }
}

fn policy_fixture() -> (Admission, ArchiveAdoptionRequest, ConversationCaptureState) {
    let session = "d18ae897-6446-494c-b095-6cd0d5ba69fe".to_string();
    let mut context = ConversationArchiveContext::for_agent_id("adoption-agent", "codex");
    context.provider_session_ids = vec![session.clone()];
    context.provider_source_key = Some("source-key".into());
    let admission = Admission {
        context,
        enabled: true,
        provider_session: Some(session.clone()),
        cleared_sessions: vec!["another-session".into()],
        configuration: "enabled-default".into(),
    };
    let request = ArchiveAdoptionRequest {
        authorization_id: uuid::Uuid::new_v4().to_string(),
        conversation_id: "saved-conversation".into(),
        provider_session: session,
        source_path: "rollout.jsonl".into(),
        start_offset: 200,
        end_offset: 400,
        native_identity: ArchiveAdoptionNativeIdentity {
            platform: "test".into(),
            primary: 1,
            secondary: 2,
        },
        approval_reference: "user-task".into(),
        approval_text: "Adopt this fixed interval into history".into(),
    };
    let capture = ConversationCaptureState {
        provider_log_sources: vec![ProviderLogCaptureState {
            provider_source_key: "source-key".into(),
            path: "rollout.jsonl".into(),
            native_identity: ProviderLogNativeIdentity {
                platform: "test".into(),
                primary: 1,
                secondary: 1,
            },
            committed_offset: 200,
            continuity_anchor: ProviderLogContinuityAnchor {
                start: 0,
                len: 200,
                sha256: "a".repeat(64),
            },
            normalizer: TranscriptNormalizationState::default(),
            status: "incomplete".into(),
            reason: Some("provider_log_source_replaced".into()),
            unknown_before_offset: Some(100),
            policy_generation: 0,
            disabled_spans: vec![],
            open_disabled_from: None,
        }],
        ..Default::default()
    };
    (admission, request, capture)
}

#[test]
fn adoption_allows_fixed_grant_without_translating_old_floor() {
    let (admission, request, capture) = policy_fixture();
    let before = serde_json::to_vec(&capture).unwrap();
    assert!(restrictions(&capture, &admission, &request).is_ok());
    assert_eq!(serde_json::to_vec(&capture).unwrap(), before);
}

#[test]
fn adoption_refuses_restriction_after_start_and_ambiguous_time_or_id() {
    let (admission, request, mut capture) = policy_fixture();
    capture.provider_log_sources[0]
        .disabled_spans
        .push(ProviderLogDisabledSpan {
            start: 210,
            end: 220,
        });
    assert!(
        restrictions(&capture, &admission, &request).is_err(),
        "later disabled range must refuse the grant"
    );
    capture.provider_log_sources[0].disabled_spans.clear();
    capture.skip_events_at_or_before = Some("2026-01-01T00:00:00Z".into());
    assert!(
        restrictions(&capture, &admission, &request).is_err(),
        "anchor has no authoritative time"
    );
    capture.skip_events_at_or_before = None;
    capture.skip_event_ids.push("opaque-old-event".into());
    assert!(restrictions(&capture, &admission, &request).is_err());
}

#[test]
fn adoption_refuses_clear_disable_retarget_and_wrong_start() {
    let (mut admission, mut request, capture) = policy_fixture();
    admission
        .cleared_sessions
        .push(request.provider_session.clone());
    assert!(restrictions(&capture, &admission, &request).is_err());
    admission.cleared_sessions.pop();
    admission.enabled = false;
    assert!(restrictions(&capture, &admission, &request).is_err());
    admission.enabled = true;
    admission.provider_session = Some("other".into());
    assert!(restrictions(&capture, &admission, &request).is_err());
    admission.provider_session = Some(request.provider_session.clone());
    request.start_offset = 199;
    assert!(restrictions(&capture, &admission, &request).is_err());
}

struct Fixture {
    archive: ConversationArchiveState,
    admission: Admission,
    request: ArchiveAdoptionRequest,
    capture_bytes: Vec<u8>,
    policy_bytes: Vec<u8>,
    source: PathBuf,
    archive_dir: PathBuf,
}

fn fixture(root: &Path) -> Fixture {
    fixture_with_body(root, "authorized prompt")
}

fn fixture_with_body(root: &Path, body: &str) -> Fixture {
    use std::io::Write;
    let (mut admission, mut request, mut capture) = policy_fixture();
    let source = root.join("replacement.jsonl");
    let header =
        serde_json::json!({"type":"session_meta","payload":{"id":request.provider_session}})
            .to_string()
            + "\n";
    let before = "{\"type\":\"event_msg\",\"payload\":{\"type\":\"user_message\",\"message\":\"excluded before start\"}}\n";
    let selected = serde_json::json!({"timestamp":"2026-01-01T00:00:00Z","type":"event_msg","payload":{"type":"user_message","message":body}}).to_string() + "\n";
    let prefix = format!("{header}{before}");
    let mut file = File::create(&source).unwrap();
    file.write_all(format!("{prefix}{selected}").as_bytes())
        .unwrap();
    file.sync_all().unwrap();
    request.source_path = std::fs::canonicalize(&source)
        .unwrap()
        .to_string_lossy()
        .into_owned();
    admission.context = crate::commands::chat::conversation_archive_context_from_snapshot(
        &snapshot(&admission, &source),
    );
    request.start_offset = prefix.len() as u64;
    request.end_offset = file.metadata().unwrap().len();
    let identity = crate::commands::provider_log_acquisition::native_file_identity(&file).unwrap();
    request.native_identity = ArchiveAdoptionNativeIdentity {
        platform: identity.platform,
        primary: identity.primary,
        secondary: identity.secondary,
    };
    let receipt = &mut capture.provider_log_sources[0];
    receipt.provider_source_key = admission.context.provider_source_key.clone().unwrap();
    receipt.path = request.source_path.clone();
    receipt.committed_offset = request.start_offset;
    receipt.unknown_before_offset = Some(header.len() as u64);
    receipt.continuity_anchor = ProviderLogContinuityAnchor {
        start: 0,
        len: prefix.len() as u64,
        sha256: crate::commands::chat_recent_seed::hash(prefix.as_bytes()),
    };
    let archive = ConversationArchiveState::default();
    archive
        .append_delivered_input_with_context(
            admission.context.clone(),
            "existing owned prompt",
            None,
        )
        .unwrap();
    request.conversation_id = archive
        .active_conversation_id(&admission.context.agent_id)
        .unwrap()
        .unwrap();
    let archive_dir =
        storage::conversation_dir(&admission.context.agent_id, &request.conversation_id).unwrap();
    storage::write_capture_state(&admission.context.agent_id, &capture).unwrap();
    super::super::chat_read::publish_policy(&admission.context, &capture.provider_log_sources[0])
        .unwrap();
    let capture_bytes = std::fs::read(
        archive_dir
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("conversation-capture.json"),
    )
    .unwrap();
    let policy_bytes = std::fs::read(
        archive_dir
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("chat-source-policy.json"),
    )
    .unwrap();
    // This different cleared session must not block the selected session.
    admission.cleared_sessions = vec!["a-different-session".into()];
    Fixture {
        archive,
        admission,
        request,
        capture_bytes,
        policy_bytes,
        source,
        archive_dir,
    }
}

fn snapshot(
    admission: &Admission,
    source: &Path,
) -> crate::commands::chat::AgentArchiveCaptureSnapshot {
    crate::commands::chat::AgentArchiveCaptureSnapshot {
        session_id: admission.context.agent_id.clone(),
        provider: "codex".into(),
        resume_session: admission.provider_session.clone(),
        fresh_provider_session_id: None,
        cleared_provider_sessions: admission.cleared_sessions.clone(),
        current_status: "off".into(),
        last_status_at: None,
        log_path: Some(source.to_owned()),
        agent_name: admission.context.agent_name.clone(),
        agent_class: admission.context.agent_class.clone(),
        workspace: admission.context.workspace.clone(),
        agent_conversation_logging:
            wardian_core::conversations::AgentConversationLoggingSetting::Default,
        watch_state: std::sync::Arc::new(std::sync::Mutex::new(
            crate::state::AgentWatchState::new(admission.context.agent_id.clone(), 256, 64 * 1024),
        )),
    }
}

fn imported(fixture: &Fixture) -> Vec<wardian_core::models::chat::AgentChatEvent> {
    wardian_core::conversations::read_jsonl_records::<wardian_core::models::chat::AgentChatEvent>(
        &fixture.archive_dir.join("events.jsonl"),
    )
    .unwrap()
    .into_iter()
    .filter(|event| !event.metadata["archive_adoption"].is_null())
    .collect()
}

#[test]
fn adoption_requires_grant_and_imports_only_fixed_interval_without_native_receipt() {
    let (_guard, temp) = super::super::tests::isolated_home();
    let fixture = fixture(temp.path());
    assert!(!fixture
        .archive
        .advance_adoption(&fixture.admission)
        .unwrap());
    assert!(imported(&fixture).is_empty());
    let response = fixture
        .archive
        .authorize_adoption(
            &fixture.admission,
            fixture.request.clone(),
            None,
            "test-operator".into(),
        )
        .unwrap();
    assert_eq!(response.status, "pending");
    std::fs::OpenOptions::new().append(true).open(&fixture.source).unwrap().write_all(b"{\"type\":\"event_msg\",\"payload\":{\"type\":\"user_message\",\"message\":\"excluded after end\"}}\n").unwrap();
    for _ in 0..32 {
        if !fixture
            .archive
            .advance_adoption(&fixture.admission)
            .unwrap()
        {
            break;
        }
    }
    assert!(!fixture
        .archive
        .has_pending_adoption(&fixture.admission.context.agent_id)
        .unwrap());
    let events = imported(&fixture);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].text.as_deref(), Some("authorized prompt"));
    assert!(events[0].metadata["chat_source_ref"].is_null());
    assert!(events[0].metadata["chat_source_epoch"].is_null());
    let bytes = std::fs::read(
        fixture
            .archive_dir
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("conversation-capture.json"),
    )
    .unwrap();
    assert_eq!(bytes, fixture.capture_bytes);
    assert_eq!(
        std::fs::read(
            fixture
                .archive_dir
                .parent()
                .unwrap()
                .parent()
                .unwrap()
                .join("chat-source-policy.json")
        )
        .unwrap(),
        fixture.policy_bytes
    );
    assert!(
        super::super::chat_logical_index::source_proof(&fixture.admission.context)
            .unwrap()
            .is_none()
    );
}

#[test]
fn adoption_retires_pending_work_and_never_revives_after_logging_returns() {
    let (_guard, temp) = super::super::tests::isolated_home();
    let mut fixture = fixture(temp.path());
    fixture
        .archive
        .authorize_adoption(
            &fixture.admission,
            fixture.request.clone(),
            None,
            "test-operator".into(),
        )
        .unwrap();
    fixture.admission.enabled = false;
    assert!(!fixture
        .archive
        .advance_adoption(&fixture.admission)
        .unwrap());
    fixture.admission.enabled = true;
    let restarted = ConversationArchiveState::default();
    assert!(!restarted.advance_adoption(&fixture.admission).unwrap());
    assert!(imported(&fixture).is_empty());
    let response = restarted
        .authorize_adoption(
            &fixture.admission,
            fixture.request,
            None,
            "test-operator".into(),
        )
        .unwrap();
    assert_eq!(response.status, "retired");
}

#[test]
fn adoption_recovers_audit_artifact_event_and_checkpoint_faults_once() {
    use std::sync::atomic::Ordering;
    let (_guard, temp) = super::super::tests::isolated_home();
    for stage in 1..=4 {
        let root = temp.path().join(format!("fault-{stage}"));
        std::fs::create_dir_all(&root).unwrap();
        std::env::set_var("WARDIAN_HOME", &root);
        let body = "owned full body ".repeat(6000).trim().to_owned();
        let fixture = fixture_with_body(&root, &body);
        fixture
            .archive
            .fail_adoption_stage
            .store(stage, Ordering::SeqCst);
        let authorization = fixture.archive.authorize_adoption(
            &fixture.admission,
            fixture.request.clone(),
            None,
            "test-operator".into(),
        );
        if stage == 1 {
            assert!(authorization.is_err());
        } else {
            authorization.unwrap();
            assert!(
                fixture
                    .archive
                    .advance_adoption(&fixture.admission)
                    .is_err(),
                "fault {stage}"
            );
        }
        let restarted = ConversationArchiveState::default();
        for _ in 0..32 {
            if !restarted.advance_adoption(&fixture.admission).unwrap() {
                break;
            }
        }
        assert!(!restarted
            .has_pending_adoption(&fixture.admission.context.agent_id)
            .unwrap());
        let events = imported(&fixture);
        assert_eq!(
            events.len(),
            1,
            "fault {stage} must not duplicate physical observations"
        );
        let records: Vec<wardian_core::conversations::ConversationNarrativeRecord> =
            wardian_core::conversations::read_jsonl_records(
                &fixture.archive_dir.join("conversation.jsonl"),
            )
            .unwrap();
        assert_eq!(
            records
                .iter()
                .filter(|record| record.event_refs.contains(&events[0].id))
                .count(),
            1,
            "fault {stage} must recover narrative"
        );
        let record = records
            .iter()
            .find(|record| record.event_refs.contains(&events[0].id))
            .unwrap();
        assert!(record.text.is_none());
        assert_eq!(record.artifact_refs.len(), 1);
        assert_eq!(
            std::fs::read_to_string(
                fixture
                    .archive_dir
                    .join("artifacts")
                    .join(&record.artifact_refs[0])
            )
            .unwrap(),
            body
        );
        let repeated = restarted
            .authorize_adoption(
                &fixture.admission,
                fixture.request.clone(),
                None,
                "test-operator".into(),
            )
            .unwrap();
        assert_eq!(repeated.status, "complete");
        let audit =
            std::fs::read_to_string(history_path(&fixture.admission.context.agent_id).unwrap())
                .unwrap();
        assert_eq!(audit.lines().count(), 1);
        assert_eq!(
            std::fs::read(
                fixture
                    .archive_dir
                    .parent()
                    .unwrap()
                    .parent()
                    .unwrap()
                    .join("conversation-capture.json")
            )
            .unwrap(),
            fixture.capture_bytes
        );
    }
}

#[test]
fn adoption_audit_failure_never_creates_job_or_archive_rows() {
    let (_guard, temp) = super::super::tests::isolated_home();
    let fixture = fixture(temp.path());
    std::fs::write(
        history_path(&fixture.admission.context.agent_id).unwrap(),
        b"{\"torn\":",
    )
    .unwrap();
    assert!(fixture
        .archive
        .authorize_adoption(
            &fixture.admission,
            fixture.request.clone(),
            None,
            "test-operator".into()
        )
        .is_err());
    assert!(!job_path(
        &fixture.admission.context.agent_id,
        &fixture.request.authorization_id
    )
    .unwrap()
    .exists());
    assert!(imported(&fixture).is_empty());
}

#[test]
fn adoption_refuses_missing_approval_wrong_scope_and_closed_destination() {
    let (_guard, temp) = super::super::tests::isolated_home();
    let fixture = fixture(temp.path());
    for change in 0..6 {
        let mut request = fixture.request.clone();
        match change {
            0 => request.approval_reference.clear(),
            1 => request.approval_text.clear(),
            2 => request.provider_session = "other-session".into(),
            3 => request.native_identity.secondary ^= 1,
            4 => request.conversation_id = "foreign-destination".into(),
            _ => {
                request.source_path = temp
                    .path()
                    .join("other-source")
                    .to_string_lossy()
                    .into_owned()
            }
        }
        assert!(fixture
            .archive
            .authorize_adoption(&fixture.admission, request, None, "test-operator".into())
            .is_err());
        assert!(imported(&fixture).is_empty());
    }
    fixture
        .archive
        .authorize_adoption(
            &fixture.admission,
            fixture.request.clone(),
            None,
            "test-operator".into(),
        )
        .unwrap();
    let path = fixture.archive_dir.join("manifest.json");
    let mut manifest = storage::read_manifest(&path).unwrap().unwrap();
    manifest.status = ConversationStatus::Closed;
    write_json_atomic(&path, &manifest).unwrap();
    assert!(!fixture
        .archive
        .advance_adoption(&fixture.admission)
        .unwrap());
    assert!(imported(&fixture).is_empty());
}

#[test]
fn adoption_retires_a_source_policy_fence_without_changing_capture_receipt() {
    let (_guard, temp) = super::super::tests::isolated_home();
    let fixture = fixture(temp.path());
    fixture
        .archive
        .authorize_adoption(
            &fixture.admission,
            fixture.request.clone(),
            None,
            "test-operator".into(),
        )
        .unwrap();
    let policy = fixture
        .archive_dir
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("chat-source-policy.json");
    write_json_atomic(
        &policy,
        &Option::<super::super::chat_read::SourcePolicy>::None,
    )
    .unwrap();
    assert!(!fixture
        .archive
        .advance_adoption(&fixture.admission)
        .unwrap());
    std::fs::write(policy, &fixture.policy_bytes).unwrap();
    assert!(!fixture
        .archive
        .advance_adoption(&fixture.admission)
        .unwrap());
    assert!(imported(&fixture).is_empty());
    assert_eq!(
        std::fs::read(
            fixture
                .archive_dir
                .parent()
                .unwrap()
                .parent()
                .unwrap()
                .join("conversation-capture.json")
        )
        .unwrap(),
        fixture.capture_bytes
    );
}

#[test]
fn adoption_refuses_foreign_audit_without_touching_its_real_job() {
    let (_guard, temp) = super::super::tests::isolated_home();
    let fixture = fixture(temp.path());
    fixture
        .archive
        .authorize_adoption(
            &fixture.admission,
            fixture.request.clone(),
            None,
            "test-operator".into(),
        )
        .unwrap();
    let job = job_path(
        &fixture.admission.context.agent_id,
        &fixture.request.authorization_id,
    )
    .unwrap();
    let before = std::fs::read(&job).unwrap();
    let foreign = "foreign-agent";
    std::fs::create_dir_all(directory(foreign).unwrap()).unwrap();
    std::fs::copy(
        history_path(&fixture.admission.context.agent_id).unwrap(),
        history_path(foreign).unwrap(),
    )
    .unwrap();
    let mut admission = fixture.admission.clone();
    admission.context.agent_id = foreign.into();
    assert!(fixture.archive.advance_adoption(&admission).is_err());
    assert_eq!(std::fs::read(job).unwrap(), before);
    assert!(imported(&fixture).is_empty());
}

#[test]
fn adoption_overlapping_grants_keep_physical_identity_across_chunk_boundaries() {
    let (_guard, temp) = super::super::tests::isolated_home();
    let mut fixture = fixture(temp.path());
    fixture
        .archive
        .authorize_adoption(
            &fixture.admission,
            fixture.request.clone(),
            None,
            "test-operator".into(),
        )
        .unwrap();
    for _ in 0..16 {
        if !fixture
            .archive
            .advance_adoption(&fixture.admission)
            .unwrap()
        {
            break;
        }
    }
    assert_eq!(imported(&fixture).len(), 1);
    let original_id = imported(&fixture)[0].id.clone();
    let mut source = std::fs::OpenOptions::new()
        .append(true)
        .open(&fixture.source)
        .unwrap();
    writeln!(source, "{}", serde_json::json!({"timestamp":"2026-01-01T00:00:01Z","type":"event_msg","payload":{"type":"user_message","message":"second separately approved prompt"}})).unwrap();
    fixture.request.end_offset = source.metadata().unwrap().len();
    drop(source);
    fixture.request.authorization_id = uuid::Uuid::new_v4().to_string();
    fixture.request.approval_reference = "second-user-grant-for-expanded-interval".into();
    fixture
        .archive
        .authorize_adoption(
            &fixture.admission,
            fixture.request.clone(),
            None,
            "test-operator".into(),
        )
        .unwrap();
    for _ in 0..16 {
        if !fixture
            .archive
            .advance_adoption(&fixture.admission)
            .unwrap()
        {
            break;
        }
    }
    let events = imported(&fixture);
    assert_eq!(events.len(), 2);
    assert_eq!(
        events
            .iter()
            .filter(|event| event.id == original_id)
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| event.text.as_deref() == Some("second separately approved prompt"))
            .count(),
        1
    );
    assert!(!fixture
        .archive
        .has_pending_adoption(&fixture.admission.context.agent_id)
        .unwrap());
}

#[test]
fn adoption_post_read_replacement_retires_even_after_original_identity_returns() {
    use std::sync::atomic::Ordering;
    let (_guard, temp) = super::super::tests::isolated_home();
    let fixture = fixture(temp.path());
    fixture
        .archive
        .authorize_adoption(
            &fixture.admission,
            fixture.request.clone(),
            None,
            "test-operator".into(),
        )
        .unwrap();
    let original = crate::commands::provider_log_acquisition::native_file_identity(
        &File::open(&fixture.source).unwrap(),
    )
    .unwrap();
    fixture
        .archive
        .fail_adoption_stage
        .store(5, Ordering::SeqCst);
    let attempted = fixture.archive.advance_adoption(&fixture.admission);
    assert!(attempted.is_err() || !attempted.unwrap());
    assert!(imported(&fixture).is_empty());
    std::fs::remove_file(&fixture.source).unwrap();
    std::fs::rename(
        fixture.source.with_extension("before-adoption-recheck"),
        &fixture.source,
    )
    .unwrap();
    assert_eq!(
        crate::commands::provider_log_acquisition::native_file_identity(
            &File::open(&fixture.source).unwrap()
        )
        .unwrap(),
        original
    );
    let restarted = ConversationArchiveState::default();
    assert!(
        !restarted.advance_adoption(&fixture.admission).unwrap(),
        "a detected invalidation must not resume after restoration"
    );
    assert!(imported(&fixture).is_empty());
    let work: AdoptionWork = bounded_json(
        &job_path(
            &fixture.admission.context.agent_id,
            &fixture.request.authorization_id,
        )
        .unwrap(),
        JOB_BYTES,
    )
    .unwrap();
    assert_eq!(work.status, "retired");
}

#[test]
fn adoption_retires_changed_policy_or_closed_destination_before_mutation() {
    let (_guard, temp) = super::super::tests::isolated_home();
    let fixture = fixture(temp.path());
    fixture
        .archive
        .authorize_adoption(
            &fixture.admission,
            fixture.request.clone(),
            None,
            "test-operator".into(),
        )
        .unwrap();
    let mut capture = storage::read_capture_state(&fixture.admission.context.agent_id).unwrap();
    capture.provider_log_sources[0].policy_generation += 1;
    storage::write_capture_state(&fixture.admission.context.agent_id, &capture).unwrap();
    assert!(!fixture
        .archive
        .advance_adoption(&fixture.admission)
        .unwrap());
    assert!(imported(&fixture).is_empty());
    let work: AdoptionWork = bounded_json(
        &job_path(
            &fixture.admission.context.agent_id,
            &fixture.request.authorization_id,
        )
        .unwrap(),
        JOB_BYTES,
    )
    .unwrap();
    assert_eq!(work.status, "retired");
}

#[test]
fn adoption_archive_paging_and_owned_detail_survive_native_source_removal() {
    if std::env::var_os("WARDIAN_ADOPTION_SOURCE_COPY").is_some() {
        super::replay_tests::adoption_replays_staged_source_copy_with_original_privacy_boundaries();
    }
    let (_guard, temp) = super::super::tests::isolated_home();
    let body = "complete owned body ".repeat(6000).trim().to_owned();
    let mut fixture = fixture_with_body(temp.path(), &body);
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&fixture.source)
        .unwrap();
    for index in 0..100 {
        writeln!(file, "{}", serde_json::json!({"timestamp":"2026-01-01T00:00:00Z","type":"event_msg","payload":{"type":"user_message","message":format!("separate prompt {index}")}})).unwrap();
    }
    fixture.request.end_offset = file.metadata().unwrap().len();
    drop(file);
    fixture
        .archive
        .authorize_adoption(
            &fixture.admission,
            fixture.request.clone(),
            None,
            "test-operator".into(),
        )
        .unwrap();
    for _ in 0..64 {
        if !fixture
            .archive
            .advance_adoption(&fixture.admission)
            .unwrap()
        {
            break;
        }
    }
    assert!(!fixture
        .archive
        .has_pending_adoption(&fixture.admission.context.agent_id)
        .unwrap());
    for _ in 0..256 {
        let cold = fixture
            .archive
            .bootstrap_saved_chat(&fixture.admission.context)
            .unwrap();
        let warm = fixture
            .archive
            .chat_projection
            .advance_capture(&fixture.admission.context.agent_id)
            .unwrap();
        if !cold && !warm {
            break;
        }
    }
    std::fs::remove_file(&fixture.source).unwrap();
    let snapshot = snapshot(&fixture.admission, &fixture.source);
    let read = |cursor: Option<&str>, detail: Option<&str>| {
        super::super::chat_read::read(&fixture.admission.context, &snapshot, cursor, None, detail)
            .unwrap()
    };
    let mut page = read(None, None);
    assert_eq!(page.progress, "capture_source_replaced");
    assert!(page.source_epoch.is_none());
    assert!(page.recent_admission.is_none());
    assert!(page.next_before.is_some());
    let mut events = page.events;
    while let Some(cursor) = page.next_before {
        page = read(Some(&cursor), None);
        events.extend(page.events.clone());
    }
    assert_eq!(events.len(), 102);
    let row = events
        .iter()
        .find(|event| {
            event.id.starts_with("archive-adoption:")
                && event.metadata["chat_detail_ref"].is_string()
        })
        .unwrap();
    let mut next = Some(
        row.metadata["chat_detail_ref"]
            .as_str()
            .unwrap()
            .to_string(),
    );
    let mut actual = String::new();
    while let Some(reference) = next {
        let detail = read(None, Some(&reference)).detail.unwrap();
        assert_eq!(detail.event_id, row.id);
        actual.push_str(&detail.text);
        next = detail.next;
    }
    assert_eq!(actual, body);
    assert!(!super::super::chat_read::input_fence(
        &fixture.admission.context,
        Some(&fixture.source)
    )
    .is_ok_and(|fence| fence.is_some()));
}
