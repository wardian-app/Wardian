//! The operator record repairs an existing stopped adoption through normal grant admission.
use super::*;
use wardian_core::conversations::{self, ConversationNarrativeRecord, ConversationSourceRecord};
use wardian_core::models::chat::AgentChatEvent;

#[test]
fn stopped_adoption_resumes_after_approved_vacancy_and_normal_regrant() {
    let (_guard, temp) = super::super::super::tests::isolated_home();
    let f = fixture(temp.path());
    for text in ["never-written fixture slot", "physical next neighbour"] {
        f.archive
            .append_delivered_input_with_context(f.admission.context.clone(), text, None)
            .unwrap();
    }
    let narrative = f.archive_dir.join("conversation.jsonl");
    let mut records: Vec<ConversationNarrativeRecord> =
        conversations::read_jsonl_records(&narrative).unwrap();
    let removed = records
        .iter()
        .find(|record| record.seq == 2)
        .unwrap()
        .clone();
    records.retain(|record| record.seq != 2);
    let events_path = f.archive_dir.join("events.jsonl");
    let mut events: Vec<AgentChatEvent> = conversations::read_jsonl_records(&events_path).unwrap();
    events.retain(|event| !removed.event_refs.contains(&event.id));
    let sources_path = f.archive_dir.join("sources.jsonl");
    let mut sources: Vec<ConversationSourceRecord> =
        conversations::read_jsonl_records(&sources_path).unwrap();
    sources.retain(|source| !removed.source_refs.contains(&source.source_id));
    conversations::write_jsonl_atomic(&narrative, &records).unwrap();
    conversations::write_jsonl_atomic(&events_path, &events).unwrap();
    conversations::write_jsonl_atomic(&sources_path, &sources).unwrap();
    let turns = super::super::super::derive_turn_records(
        &f.request.conversation_id,
        &records,
        &events,
        &sources,
        true,
    );
    conversations::write_jsonl_atomic(&f.archive_dir.join("turns.jsonl"), &turns).unwrap();
    let manifest_path = f.archive_dir.join("manifest.json");
    let mut manifest: conversations::ConversationManifest =
        serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
    manifest.record_count = records.len() as u64;
    manifest.turn_count = turns.len() as u64;
    conversations::write_json_atomic(&manifest_path, &manifest).unwrap();
    f.archive
        .authorize_adoption(
            &f.admission,
            f.request.clone(),
            None,
            "test-operator".into(),
        )
        .unwrap();
    assert!(!f.archive.advance_adoption(&f.admission).unwrap());
    assert!(!f
        .archive
        .has_pending_adoption(&f.admission.context.agent_id)
        .unwrap());
    assert_eq!(
        f.archive
            .archive_sequence_gap(&f.admission.context)
            .unwrap()
            .unwrap()
            .sequence,
        2
    );
    let names = [
        "manifest.json",
        "conversation.jsonl",
        "events.jsonl",
        "sources.jsonl",
        "turns.jsonl",
    ];
    let before = names.map(|name| std::fs::read(f.archive_dir.join(name)).unwrap());
    let approved = wardian_core::control::ArchiveVacancyRequest {
        conversation_id: f.request.conversation_id.clone(),
        sequence: 2,
        authorization_id: uuid::Uuid::new_v4().to_string(),
        approval_reference: "isolated-fixture-task".into(),
        approval_text: "Approve the proven never-written slot 2 in this isolated fixture".into(),
    };
    let report = wardian_core::archive_vacancy::inspect_vacancy(
        &f.archive_dir,
        &approved.conversation_id,
        2,
    )
    .unwrap();
    assert!(report.eligible, "{:?}", report.rules);
    assert!(!f.archive_dir.join("vacancies.jsonl").exists());
    let result = f
        .archive
        .authorize_vacancy(&f.admission.context.agent_id, approved)
        .unwrap();
    assert!(result.created && result.adoption_regrant_required);
    assert!(f
        .archive
        .archive_sequence_gap(&f.admission.context)
        .unwrap()
        .is_none());
    for (name, bytes) in names.into_iter().zip(before) {
        assert_eq!(std::fs::read(f.archive_dir.join(name)).unwrap(), bytes);
    }
    let capture = f
        .archive_dir
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("conversation-capture.json");
    assert_eq!(std::fs::read(capture).unwrap(), f.capture_bytes);
    let mut regrant = f.request.clone();
    regrant.authorization_id = uuid::Uuid::new_v4().to_string();
    f.archive
        .authorize_adoption(&f.admission, regrant, None, "test-operator".into())
        .unwrap();
    for _ in 0..32 {
        f.archive.advance_adoption(&f.admission).unwrap();
        if !f
            .archive
            .has_pending_adoption(&f.admission.context.agent_id)
            .unwrap()
        {
            break;
        }
    }
    assert!(!f
        .archive
        .has_pending_adoption(&f.admission.context.agent_id)
        .unwrap());
    let (_, visible) = f.archive.show(&f.request.conversation_id).unwrap();
    assert!(visible
        .iter()
        .any(|record| record.text.as_deref() == Some("authorized prompt")));
    assert!(!visible.iter().any(|record| record.seq == 2));
    assert_eq!(
        wardian_core::archive_vacancy::validate_vacancies(
            &f.archive_dir,
            &f.request.conversation_id
        )
        .unwrap()
        .into_iter()
        .collect::<Vec<_>>(),
        [2]
    );
}
