use super::*;
use crate::state::conversation_archive::{self as archive, ConversationArchiveContext};
use std::path::PathBuf;
use wardian_core::conversations::{
    self, ConversationNarrativeRecord, ConversationSourceRecord, ConversationTurnRecord,
};
use wardian_core::models::chat::AgentChatEvent;

struct Fixture {
    _temp: tempfile::TempDir,
    state: ConversationArchiveState,
    context: ConversationArchiveContext,
    directory: PathBuf,
    records: Vec<ConversationNarrativeRecord>,
    _guard: tokio::sync::MutexGuard<'static, ()>,
}

fn fixture(head: u64, holes: &[u64]) -> Fixture {
    let (guard, temp) = archive::tests::isolated_home();
    let context = ConversationArchiveContext::for_agent_id("vacancy-agent", "claude");
    let state = ConversationArchiveState::default();
    state
        .append_delivered_input_with_context(context.clone(), "one request", None)
        .unwrap();
    for seq in 2..=head {
        let event: AgentChatEvent = serde_json::from_value(serde_json::json!({
            "id":format!("assistant-{seq}"), "session_id":context.agent_id,
            "provider":"claude", "kind":"message", "role":"assistant",
            "text":format!("physical member {seq}"), "turn_id":"provider-turn",
            "source":"stream_json", "sequence":3,
            "created_at":"2026-11-22T00:00:00Z", "metadata":{}
        }))
        .unwrap();
        state
            .append_chat_events_with_context(context.clone(), &[event])
            .unwrap();
    }
    let conversation_id = state
        .active_conversation_id(&context.agent_id)
        .unwrap()
        .unwrap();
    let directory = conversation_dir(&context.agent_id, &conversation_id).unwrap();
    let mut records: Vec<ConversationNarrativeRecord> =
        conversations::read_jsonl_records(&directory.join("conversation.jsonl")).unwrap();
    let removed_events = records
        .iter()
        .filter(|record| holes.contains(&record.seq))
        .flat_map(|record| record.event_refs.clone())
        .collect::<Vec<_>>();
    let removed_sources = records
        .iter()
        .filter(|record| holes.contains(&record.seq))
        .flat_map(|record| record.source_refs.clone())
        .collect::<Vec<_>>();
    records.retain(|record| !holes.contains(&record.seq));
    let mut events: Vec<AgentChatEvent> =
        conversations::read_jsonl_records(&directory.join("events.jsonl")).unwrap();
    events.retain(|event| !removed_events.contains(&event.id));
    let mut sources: Vec<ConversationSourceRecord> =
        conversations::read_jsonl_records(&directory.join("sources.jsonl")).unwrap();
    sources.retain(|source| !removed_sources.contains(&source.source_id));
    let turns = archive::derive_turn_records(&conversation_id, &records, &events, &sources, true);
    for (name, bytes) in [
        ("conversation.jsonl", jsonl(&records)),
        ("events.jsonl", jsonl(&events)),
        ("sources.jsonl", jsonl(&sources)),
        ("turns.jsonl", jsonl(&turns)),
    ] {
        fs::write(directory.join(name), bytes).unwrap();
    }
    let path = directory.join("manifest.json");
    let mut manifest: conversations::ConversationManifest =
        serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    manifest.record_count = records.len() as u64;
    manifest.turn_count = turns.len() as u64;
    conversations::write_json_atomic(&path, &manifest).unwrap();
    Fixture {
        _temp: temp,
        state,
        context,
        directory,
        records,
        _guard: guard,
    }
}

fn jsonl<T: serde::Serialize>(records: &[T]) -> Vec<u8> {
    records
        .iter()
        .flat_map(|record| {
            let mut line = serde_json::to_vec(record).unwrap();
            line.push(b'\n');
            line
        })
        .collect()
}

fn request(f: &Fixture, sequence: u64) -> ArchiveVacancyRequest {
    ArchiveVacancyRequest {
        conversation_id: f.directory.file_name().unwrap().to_str().unwrap().into(),
        sequence,
        authorization_id: uuid::Uuid::new_v4().to_string(),
        approval_reference: "recorded-operator-task".into(),
        approval_text: format!("Approve the proven never-written narrative slot {sequence}"),
    }
}

fn canonical(f: &Fixture) -> Vec<Vec<u8>> {
    [
        "manifest.json",
        "conversation.jsonl",
        "events.jsonl",
        "sources.jsonl",
        "turns.jsonl",
    ]
    .map(|name| fs::read(f.directory.join(name)).unwrap())
    .into()
}

#[test]
fn approved_vacancy_is_idempotent_preserves_archive_and_counts_only_physical_members() {
    let f = fixture(5, &[3]);
    let request = request(&f, 3);
    let before = canonical(&f);
    let dry = inspect_vacancy(&f.directory, &request.conversation_id, 3).unwrap();
    assert!(dry.eligible, "{:?}", dry.rules);
    assert_eq!(dry.committed_head_sequence, 5);
    assert_eq!(canonical(&f), before);
    assert!(!f.directory.join("vacancies.jsonl").exists());
    let response = f
        .state
        .authorize_vacancy(&f.context.agent_id, request.clone())
        .unwrap();
    assert!(response.created);
    assert!(response.adoption_regrant_required);
    assert_eq!(canonical(&f), before);
    let ledger = fs::read(f.directory.join("vacancies.jsonl")).unwrap();
    assert!(
        !f.state
            .authorize_vacancy(&f.context.agent_id, request.clone())
            .unwrap()
            .created
    );
    assert_eq!(
        fs::read(f.directory.join("vacancies.jsonl")).unwrap(),
        ledger
    );
    let (manifest, visible) = f.state.show(&request.conversation_id).unwrap();
    assert_eq!(manifest.record_count, 4);
    assert_eq!(visible.len(), 4);
    assert_eq!(
        visible.iter().map(|record| record.seq).collect::<Vec<_>>(),
        [1, 2, 4, 5]
    );
    let turns: Vec<ConversationTurnRecord> =
        conversations::read_jsonl_records(&f.directory.join("turns.jsonl")).unwrap();
    assert_eq!(turns.len(), 1);
    assert_eq!((turns[0].seq_start, turns[0].seq_end), (1, 5));
    assert_eq!(turns[0].counts.records, 4);
    assert_eq!(turns[0].counts.assistant_messages, 3);
    assert_eq!(
        f.state
            .chat_events_for_agent(&f.context.agent_id)
            .unwrap()
            .len(),
        4
    );
}

#[test]
fn historical_pending_generated_envelope_is_rejected_before_ledger_creation() {
    let mut f = fixture(5, &[3]);
    // Keep the original generated envelope and its resolved sources; only the
    // physical narrative is missing, as after an interrupted publication.
    f.records.retain(|record| record.seq != 1);
    conversations::write_jsonl_atomic(&f.directory.join("conversation.jsonl"), &f.records).unwrap();
    let events: Vec<AgentChatEvent> =
        conversations::read_jsonl_records(&f.directory.join("events.jsonl")).unwrap();
    assert!(events.iter().any(|event| {
        event.metadata["generated"] == true && event.metadata["archive_record"]["seq"] == 1
    }));
    assert_eq!(
        f.records
            .iter()
            .map(|record| record.seq)
            .collect::<Vec<_>>(),
        [2, 4, 5]
    );
    let request = request(&f, 3);
    let before = canonical(&f);
    let ledger_path = f.directory.join("vacancies.jsonl");
    assert!(!ledger_path.exists());

    let error = f
        .state
        .authorize_vacancy(&f.context.agent_id, request.clone())
        .unwrap_err();

    assert!(
        !ledger_path.exists(),
        "rejected approval persisted a ledger: {error}"
    );
    assert!(
        error.to_string().contains("pending_record_boundary"),
        "{error}"
    );
    assert_eq!(canonical(&f), before);
    assert!(validate_vacancies(&f.directory, &request.conversation_id)
        .unwrap()
        .is_empty());
}

#[test]
fn second_vacancy_append_preserves_prior_line_bytes_verbatim() {
    let f = fixture(8, &[3, 6]);
    let first = request(&f, 3);
    f.state
        .authorize_vacancy(&f.context.agent_id, first)
        .unwrap();
    let path = f.directory.join("vacancies.jsonl");
    let bytes = fs::read(&path).unwrap();
    let mut spaced = vec![b'{', b' '];
    spaced.extend_from_slice(&bytes[1..]);
    fs::write(&path, &spaced).unwrap();
    f.state
        .authorize_vacancy(&f.context.agent_id, request(&f, 6))
        .unwrap();
    assert!(fs::read(&path).unwrap().starts_with(&spaced));
    assert_eq!(
        validate_vacancies(
            &f.directory,
            f.directory.file_name().unwrap().to_str().unwrap()
        )
        .unwrap()
        .into_iter()
        .collect::<Vec<_>>(),
        [3, 6]
    );
}

#[test]
fn conflicting_or_oversized_approval_never_changes_the_ledger() {
    let f = fixture(5, &[3]);
    let mut approved = request(&f, 3);
    f.state
        .authorize_vacancy(&f.context.agent_id, approved.clone())
        .unwrap();
    let before = fs::read(f.directory.join("vacancies.jsonl")).unwrap();
    approved.approval_text.push_str(" another scope");
    assert!(f
        .state
        .authorize_vacancy(&f.context.agent_id, approved)
        .is_err());
    assert_eq!(
        fs::read(f.directory.join("vacancies.jsonl")).unwrap(),
        before
    );
    let mut too_large = request(&f, 2);
    too_large.approval_text = "x".repeat(16_385);
    assert!(f
        .state
        .authorize_vacancy(&f.context.agent_id, too_large)
        .is_err());
    assert_eq!(
        fs::read(f.directory.join("vacancies.jsonl")).unwrap(),
        before
    );
}

#[test]
fn real_record_after_vacancy_fails_all_canonical_readers_and_persists_terminal_status() {
    let f = fixture(5, &[3]);
    let approved = request(&f, 3);
    f.state
        .authorize_vacancy(&f.context.agent_id, approved.clone())
        .unwrap();
    let mut conflict = f.records[1].clone();
    conflict.seq = 3;
    conversations::append_jsonl_record(&f.directory.join("conversation.jsonl"), &conflict).unwrap();
    let before = canonical(&f);
    assert!(f.state.show(&approved.conversation_id).is_err());
    let entries = f.state.list(Some(&f.context.agent_id), false).unwrap();
    assert!(f.state.turn_records_for_conversations(&entries).is_err());
    assert!(f
        .state
        .turn_records_for_conversations_resilient(&entries)
        .is_err());
    assert!(f.state.chat_events_for_agent(&f.context.agent_id).is_err());
    let error = f
        .state
        .append_delivered_input_with_context(f.context.clone(), "new request", None)
        .unwrap_err();
    assert!(error.to_string().contains("vacancy"));
    let stop = conversations::read_archive_sequence_gap(&f.context.agent_id)
        .unwrap()
        .unwrap();
    assert!(stop.vacancy_rule.is_some());
    assert!(stop.reason().contains("vacancy conflict"));
    assert_eq!(canonical(&f), before);
    assert!(ConversationArchiveState::default()
        .archive_sequence_gap(&f.context)
        .unwrap()
        .unwrap()
        .reason()
        .contains("vacancy conflict"));
}

#[test]
fn vacancy_keeps_later_partial_generated_write_recoverable_without_reusing_its_sequence() {
    let f = fixture(5, &[3]);
    let approved = request(&f, 3);
    f.state
        .authorize_vacancy(&f.context.agent_id, approved.clone())
        .unwrap();
    f.state
        .append_delivered_input_with_context(f.context.clone(), "durable future input", None)
        .unwrap();
    let path = f.directory.join("conversation.jsonl");
    let mut records: Vec<ConversationNarrativeRecord> =
        conversations::read_jsonl_records(&path).unwrap();
    assert_eq!(records.last().unwrap().seq, 6);
    records.retain(|record| record.seq != 6);
    conversations::write_jsonl_atomic(&path, &records).unwrap();
    // The durable generated event remains the original recovery authority.
    validate_vacancies(&f.directory, &approved.conversation_id).unwrap();
    f.state
        .append_delivered_input_with_context(f.context.clone(), "distinct later input", None)
        .unwrap();
    let (_, records) = f.state.show(&approved.conversation_id).unwrap();
    assert_eq!(
        records
            .iter()
            .find(|record| record.text.as_deref() == Some("durable future input"))
            .unwrap()
            .seq,
        6
    );
    assert_eq!(
        records
            .iter()
            .find(|record| record.text.as_deref() == Some("distinct later input"))
            .unwrap()
            .seq,
        7
    );
    assert!(!records.iter().any(|record| record.seq == 3));
}

#[test]
fn vacancy_keeps_native_event_first_publication_recoverable() {
    let f = fixture(5, &[3]);
    let approved = request(&f, 3);
    f.state
        .authorize_vacancy(&f.context.agent_id, approved.clone())
        .unwrap();
    let event: AgentChatEvent = serde_json::from_value(serde_json::json!({
        "id":"future-native", "session_id":f.context.agent_id, "provider":"claude",
        "kind":"message", "role":"assistant", "text":"native event-first tail",
        "source":"stream_json", "created_at":"2026-11-22T01:00:00Z", "metadata":{}
    }))
    .unwrap();
    let path = f.directory.join("events.jsonl");
    conversations::append_jsonl_record(&path, &event).unwrap();
    fs::OpenOptions::new()
        .write(true)
        .open(path)
        .unwrap()
        .sync_all()
        .unwrap();
    f.state
        .append_delivered_input_with_context(f.context.clone(), "later input after native", None)
        .unwrap();
    let (_, records) = f.state.show(&approved.conversation_id).unwrap();
    assert_eq!(
        records
            .iter()
            .find(|row| row.text.as_deref() == Some("native event-first tail"))
            .unwrap()
            .seq,
        6
    );
    assert_eq!(
        records
            .iter()
            .find(|row| row.text.as_deref() == Some("later input after native"))
            .unwrap()
            .seq,
        7
    );
    assert!(f.state.archive_sequence_gap(&f.context).unwrap().is_none());
}

#[test]
fn vacancy_keeps_generated_event_first_recoverable_before_source_publication() {
    let f = fixture(5, &[3]);
    let approved = request(&f, 3);
    f.state
        .authorize_vacancy(&f.context.agent_id, approved.clone())
        .unwrap();
    let names = [
        "manifest.json",
        "conversation.jsonl",
        "events.jsonl",
        "sources.jsonl",
        "turns.jsonl",
    ];
    let before = canonical(&f);
    f.state
        .append_delivered_input_with_context(
            f.context.clone(),
            "generated before source",
            Some("synthetic-sender"),
        )
        .unwrap();
    // Preserve only the newly durable event, as at the first publication boundary.
    for (name, bytes) in names.into_iter().zip(before) {
        if name != "events.jsonl" {
            fs::write(f.directory.join(name), bytes).unwrap();
        }
    }
    f.state
        .append_delivered_input_with_context(f.context.clone(), "later input after generated", None)
        .unwrap();
    let (_, records) = f.state.show(&approved.conversation_id).unwrap();
    assert_eq!(
        records
            .iter()
            .find(|row| row.text.as_deref() == Some("generated before source"))
            .unwrap()
            .seq,
        6
    );
    assert_eq!(
        records
            .iter()
            .find(|row| row.text.as_deref() == Some("later input after generated"))
            .unwrap()
            .seq,
        7
    );
    let sources: Vec<ConversationSourceRecord> =
        conversations::read_jsonl_records(&f.directory.join("sources.jsonl")).unwrap();
    assert!(sources
        .iter()
        .any(|source| source.source_id == "agent:synthetic-sender"
            && source.cursor.as_deref() == Some("6")));
}

#[test]
fn owned_legacy_generated_event_keeps_its_format_when_capture_resumes() {
    let f = fixture(5, &[3]);
    let approved = request(&f, 3);
    let path = f.directory.join("events.jsonl");
    let mut events: Vec<AgentChatEvent> = conversations::read_jsonl_records(&path).unwrap();
    let legacy = events
        .iter_mut()
        .find(|event| event.metadata["generated"] == true)
        .unwrap();
    let legacy_id = legacy.id.clone();
    assert!(legacy
        .metadata
        .as_object_mut()
        .unwrap()
        .remove("archive_record")
        .is_some());
    conversations::write_jsonl_atomic(&path, &events).unwrap();
    f.state
        .authorize_vacancy(&f.context.agent_id, approved.clone())
        .unwrap();
    f.state
        .append_delivered_input_with_context(f.context.clone(), "after legacy row", None)
        .unwrap();
    let (_, records) = f.state.show(&approved.conversation_id).unwrap();
    assert_eq!(
        records
            .iter()
            .find(|row| row.text.as_deref() == Some("after legacy row"))
            .unwrap()
            .seq,
        6
    );
    assert!(!records.iter().any(|row| row.seq == 3));
    let saved: Vec<AgentChatEvent> = conversations::read_jsonl_records(&path).unwrap();
    assert!(saved
        .iter()
        .find(|event| event.id == legacy_id)
        .unwrap()
        .metadata
        .get("archive_record")
        .is_none());
    let ledger = fs::read(f.directory.join("vacancies.jsonl")).unwrap();
    assert!(
        !f.state
            .authorize_vacancy(&f.context.agent_id, approved)
            .unwrap()
            .created
    );
    assert_eq!(
        fs::read(f.directory.join("vacancies.jsonl")).unwrap(),
        ledger
    );
}
