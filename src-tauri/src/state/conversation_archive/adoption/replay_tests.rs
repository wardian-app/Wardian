//! Optional private-copy reproduction. Inputs are read-only copies; the owned
//! archive, authorization and checkpoints exist only in the isolated test home.
use super::*;

pub(super) fn adoption_replays_staged_source_copy_with_original_privacy_boundaries() {
    let source = std::fs::canonicalize(
        std::env::var_os("WARDIAN_ADOPTION_SOURCE_COPY").expect("staged source copy"),
    )
    .unwrap();
    let capture_path = PathBuf::from(
        std::env::var_os("WARDIAN_ADOPTION_CAPTURE_COPY").expect("staged capture copy"),
    );
    let numeric_end: u64 = std::env::var("WARDIAN_ADOPTION_PINNED_END")
        .expect("pinned preparation end")
        .parse()
        .unwrap();
    let original_source_length = std::fs::metadata(&source).unwrap().len();
    let original_capture_bytes = std::fs::read(capture_path).unwrap();
    let mut capture: ConversationCaptureState =
        serde_json::from_slice(&original_capture_bytes).unwrap();
    assert_eq!(capture.provider_log_sources.len(), 1);
    let header: serde_json::Value = serde_json::from_slice(&{
        use std::io::BufRead;
        let mut bytes = Vec::new();
        io::BufReader::new(File::open(&source).unwrap())
            .take(16 * 1024 * 1024)
            .read_until(b'\n', &mut bytes)
            .unwrap();
        bytes
    })
    .unwrap();
    let session = header["payload"]["id"].as_str().unwrap().to_owned();
    let identity = crate::commands::provider_log_acquisition::native_file_identity(
        &File::open(&source).unwrap(),
    )
    .unwrap();
    let start = capture.provider_log_sources[0].committed_offset;
    let old_floor = capture.provider_log_sources[0].unknown_before_offset;
    assert!(old_floor.is_some_and(|floor| floor > 0 && floor < start));
    assert_eq!(
        capture.provider_log_sources[0].reason.as_deref(),
        Some("provider_log_source_replaced")
    );
    let (_guard, _temporary_home) = super::super::tests::isolated_home();
    let retained_parent = std::env::var_os("WARDIAN_ADOPTION_REPLAY_OUTPUT").map(PathBuf::from);
    let temp = if let Some(parent) = &retained_parent {
        std::fs::create_dir_all(parent).unwrap();
        tempfile::Builder::new()
            .prefix("isolated-adoption-")
            .tempdir_in(parent)
            .unwrap()
    } else {
        tempfile::tempdir().unwrap()
    };
    std::env::set_var("WARDIAN_HOME", temp.path());
    let agent_id = std::env::var("WARDIAN_ADOPTION_REPLAY_AGENT")
        .unwrap_or_else(|_| "isolated-adoption-replay".into());
    let mut context = ConversationArchiveContext::for_agent_id(&agent_id, "codex");
    context.provider_session_ids = vec![session.clone()];
    // Locator rebinding belongs only to this isolated fixture. The supplied
    // original capture copy and its complete exclusion policy remain read-only.
    context.provider_source_key = Some(capture.provider_log_sources[0].provider_source_key.clone());
    capture.provider_log_sources[0].path = source.to_string_lossy().into_owned();
    let archive = ConversationArchiveState::default();
    archive
        .append_delivered_input_with_context(context.clone(), "existing archive fixture", None)
        .unwrap();
    storage::write_capture_state(&context.agent_id, &capture).unwrap();
    super::super::chat_read::publish_policy(&context, &capture.provider_log_sources[0]).unwrap();
    let conversation_id = archive
        .active_conversation_id(&context.agent_id)
        .unwrap()
        .unwrap();
    let dir = storage::conversation_dir(&context.agent_id, &conversation_id).unwrap();
    let agent_dir = dir.parent().unwrap().parent().unwrap();
    let capture_before = std::fs::read(agent_dir.join("conversation-capture.json")).unwrap();
    let policy_before = std::fs::read(agent_dir.join("chat-source-policy.json")).unwrap();
    let request = ArchiveAdoptionRequest {
        authorization_id: uuid::Uuid::new_v4().to_string(),
        conversation_id,
        provider_session: session.clone(),
        source_path: source.to_string_lossy().into_owned(),
        start_offset: start,
        end_offset: numeric_end,
        native_identity: wardian_core::control::ArchiveAdoptionNativeIdentity {
            platform: identity.platform,
            primary: identity.primary,
            secondary: identity.secondary,
        },
        approval_reference: std::env::var("WARDIAN_ADOPTION_APPROVAL_REFERENCE")
            .unwrap_or_else(|_| "isolated-copy-of-recorded-user-grant".into()),
        approval_text: std::env::var("WARDIAN_ADOPTION_APPROVAL_TEXT").unwrap_or_else(|_| {
            "Adopt only the approved replacement interval into durable history".into()
        }),
    };
    let admission = Admission {
        context,
        enabled: true,
        provider_session: Some(session),
        cleared_sessions: vec!["different-cleared-session".into()],
        configuration: "isolated-enabled-copy".into(),
    };
    archive
        .authorize_adoption(
            &admission,
            request.clone(),
            None,
            "isolated-test-operator".into(),
        )
        .unwrap();
    let started = std::time::Instant::now();
    let mut steps = 0;
    while archive.advance_adoption(&admission).unwrap() {
        steps += 1;
        assert!(steps < 4096, "fixed interval must reach quiescence");
        if steps % 50 == 0 {
            eprintln!(
                "private adoption replay: {steps} bounded steps, {} s",
                started.elapsed().as_secs()
            );
        }
    }
    let work: AdoptionWork = bounded_json(
        &job_path(&admission.context.agent_id, &request.authorization_id).unwrap(),
        JOB_BYTES,
    )
    .unwrap();
    assert_eq!(work.status, "complete");
    assert!(work.completed_at.is_some());
    let events: Vec<wardian_core::models::chat::AgentChatEvent> =
        wardian_core::conversations::read_jsonl_records(&dir.join("events.jsonl")).unwrap();
    let imported: Vec<_> = events
        .iter()
        .filter(|event| !event.metadata["archive_adoption"].is_null())
        .collect();
    assert!(imported.len() > 100);
    let ids: std::collections::HashSet<_> = imported.iter().map(|event| &event.id).collect();
    assert_eq!(ids.len(), imported.len());
    for event in &imported {
        let physical = &event.metadata["archive_adoption"];
        assert!(physical["record_start"].as_u64().unwrap() >= start);
        assert!(physical["record_end"].as_u64().unwrap() <= work.grant.selection.end);
        assert!(event.metadata["chat_source_ref"].is_null());
        assert!(event.metadata["chat_source_epoch"].is_null());
    }
    assert_eq!(
        std::fs::read(agent_dir.join("conversation-capture.json")).unwrap(),
        capture_before
    );
    assert_eq!(
        std::fs::read(agent_dir.join("chat-source-policy.json")).unwrap(),
        policy_before
    );
    assert_eq!(
        std::fs::metadata(&source).unwrap().len(),
        original_source_length
    );
    assert_eq!(work.grant.selection.end, numeric_end);
    assert!(!archive
        .has_pending_adoption(&admission.context.agent_id)
        .unwrap());
    eprintln!("private adoption replay complete: {} observations, {steps} bounded steps, {} s; original cursor/floor/source policy unchanged; isolated archive only", imported.len(), started.elapsed().as_secs());
    // Retain only the outcome in logs. TempDir cleanup owns this test home.
    if retained_parent.is_some() {
        let _ = temp.keep();
    }
}
