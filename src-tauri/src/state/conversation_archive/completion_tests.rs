use super::*;
use wardian_core::models::chat::AgentChatStatus;

fn fixture() -> (AgentChatEvent, AgentChatEvent, ConversationNarrativeRecord) {
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/real-agy-tool-completion.json")).unwrap();
    (
        serde_json::from_value(fixture["running"].clone()).unwrap(),
        serde_json::from_value(fixture["completed"].clone()).unwrap(),
        serde_json::from_value(fixture["narrative"].clone()).unwrap(),
    )
}

#[test]
fn retained_agy_completion_repairs_live_and_persisted_original_row() {
    let _guard = crate::utils::wardian_test_env_lock();
    let temp = tempfile::tempdir().unwrap();
    std::env::set_var("WARDIAN_HOME", temp.path());
    let (running, completed, narrative) = fixture();
    let archive = ConversationArchiveState::default();
    archive
        .append_chat_events(&running.session_id, std::slice::from_ref(&running))
        .unwrap();
    let id = archive
        .active_conversation_id(&running.session_id)
        .unwrap()
        .unwrap();
    let dir = conversation_dir(&running.session_id, &id).unwrap();
    // Retain the actual narrative sequence/time/source reference from the run.
    write_jsonl_atomic(
        &dir.join("conversation.jsonl"),
        std::slice::from_ref(&narrative),
    )
    .unwrap();
    let before = std::fs::read(dir.join("events.jsonl")).unwrap();
    let live =
        provenance::merge_current_capture(vec![completed.clone()], vec![running.clone()]).unwrap();
    assert_eq!(live.len(), 1);
    assert_eq!(live[0].id, running.id);
    assert_eq!(live[0].text, completed.text);
    assert_eq!(live[0].metadata["provider_step_status"], 3);
    assert_eq!(
        live[0].status,
        Some(AgentChatStatus::Unknown),
        "DONE does not assert success"
    );
    assert_eq!(
        std::fs::read(dir.join("events.jsonl")).unwrap(),
        before,
        "live-only repair does not write"
    );
    assert_eq!(
        archive
            .append_chat_events(&running.session_id, std::slice::from_ref(&completed))
            .unwrap(),
        1 // The existing API counts an in-place repair as a change.
    );
    let restarted = ConversationArchiveState::default();
    let persisted = restarted
        .chat_events_for_agent(&running.session_id)
        .unwrap();
    assert_eq!(persisted.len(), 1);
    assert_eq!(persisted[0].id, running.id);
    assert_eq!(persisted[0].text, completed.text);
    assert_eq!(persisted[0].metadata["provider_step_status"], 3);
    let (_, records) = restarted.show(&id).unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].seq, 4);
    assert_eq!(records[0].at, narrative.at);
    assert_eq!(records[0].source_refs, narrative.source_refs);
    assert_eq!(records[0].event_refs, narrative.event_refs);
    assert_eq!(records[0].text, completed.text);
    // A stale running capture must not downgrade terminal text or status.
    restarted
        .append_chat_events(&running.session_id, std::slice::from_ref(&running))
        .unwrap();
    assert_eq!(
        restarted
            .chat_events_for_agent(&completed.session_id)
            .unwrap(),
        persisted
    );
    assert_eq!(restarted.show(&id).unwrap().1, records);
}

#[test]
fn completion_requires_same_native_location_and_never_rewrites_terminal_result() {
    let (running, completed, _) = fixture();
    for key in [
        "step_index",
        "tool_ordinal",
        "provider_step_type",
        "provider_step_source",
        "log_source",
        "provider_step_status",
    ] {
        let mut missing = completed.clone();
        missing.metadata.as_object_mut().unwrap().remove(key);
        let mut archived = vec![running.clone()];
        provenance::refresh_events(&mut archived, &[missing]).unwrap();
        assert_eq!(archived[0].text, running.text, "missing {key}");
    }
    for key in [
        "step_index",
        "tool_ordinal",
        "provider_step_type",
        "provider_step_source",
        "log_source",
        "provider_step_status",
    ] {
        let mut old = running.clone();
        old.metadata.as_object_mut().unwrap().remove(key);
        let mut archived = vec![old];
        provenance::refresh_events(&mut archived, std::slice::from_ref(&completed)).unwrap();
        assert_eq!(archived[0].text, running.text, "missing archived {key}");
    }
    for key in ["step_index", "tool_ordinal", "provider_step_type"] {
        let mut wrong = completed.clone();
        wrong.metadata[key] = serde_json::json!(99);
        let mut archived = vec![running.clone()];
        assert!(provenance::refresh_events(&mut archived, &[wrong]).is_err());
        assert_eq!(archived[0].text, running.text, "foreign {key}");
    }
    let mut foreign = completed.clone();
    foreign.metadata["log_path"] = serde_json::json!("/another/session.db");
    let mut archived = vec![running.clone()];
    assert!(!provenance::refresh_events(&mut archived, &[foreign]).unwrap());
    let mut archived = vec![completed.clone()];
    let mut conflicting = completed.clone();
    conflicting.text = Some("different terminal output without native revision evidence".into());
    provenance::refresh_events(&mut archived, &[conflicting, running]).unwrap();
    assert_eq!(archived[0].text, completed.text);
    assert_eq!(archived[0].metadata["provider_step_status"], 3);
}

#[test]
fn completed_tool_large_text_materializes_once_and_repairs_stale_narrative() {
    let _guard = crate::utils::wardian_test_env_lock();
    let temp = tempfile::tempdir().unwrap();
    std::env::set_var("WARDIAN_HOME", temp.path());
    let (running, mut completed, _) = fixture();
    completed.text = Some("large completed output\n".repeat(1000));
    let archive = ConversationArchiveState::default();
    archive
        .append_chat_events(&running.session_id, std::slice::from_ref(&running))
        .unwrap();
    let id = archive
        .active_conversation_id(&running.session_id)
        .unwrap()
        .unwrap();
    let dir = conversation_dir(&running.session_id, &id).unwrap();
    let stale = std::fs::read(dir.join("conversation.jsonl")).unwrap();
    archive
        .append_chat_events(&running.session_id, std::slice::from_ref(&completed))
        .unwrap();
    let (_, records) = archive.show(&id).unwrap();
    assert!(records[0].text.is_none());
    assert_eq!(records[0].artifact_refs.len(), 1);
    assert_eq!(
        std::fs::read_to_string(dir.join("artifacts").join(&records[0].artifact_refs[0])).unwrap(),
        completed.text.clone().unwrap()
    );
    // Simulate successful event publication followed by failed narrative publish.
    std::fs::write(dir.join("conversation.jsonl"), stale).unwrap();
    archive
        .append_chat_events(&running.session_id, std::slice::from_ref(&completed))
        .unwrap();
    assert_eq!(archive.show(&id).unwrap().1, records);
    let before = std::fs::read(dir.join("events.jsonl")).unwrap();
    archive
        .append_chat_events(&running.session_id, &[completed, running.clone()])
        .unwrap();
    assert_eq!(std::fs::read(dir.join("events.jsonl")).unwrap(), before);
    assert_eq!(std::fs::read_dir(dir.join("artifacts")).unwrap().count(), 1);
}
