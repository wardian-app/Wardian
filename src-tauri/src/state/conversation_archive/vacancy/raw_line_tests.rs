use super::*;
use crate::state::conversation_archive::{self as archive, repair, ConversationArchiveContext};
use std::path::PathBuf;
use wardian_core::conversations::{self, ConversationNarrativeRecord, ConversationSourceRecord};
use wardian_core::models::chat::AgentChatEvent;

struct RawLineFixture {
    _temp: tempfile::TempDir,
    state: ConversationArchiveState,
    context: ConversationArchiveContext,
    directory: PathBuf,
    conversation_id: String,
    records: Vec<ConversationNarrativeRecord>,
    _guard: tokio::sync::MutexGuard<'static, ()>,
}

fn fixture() -> RawLineFixture {
    fixture_with_text(false)
}

fn fixture_with_text(large_neighbour: bool) -> RawLineFixture {
    let (guard, temp) = archive::tests::isolated_home();
    let context = ConversationArchiveContext::for_agent_id("vacancy-raw-agent", "claude");
    let state = ConversationArchiveState::default();
    state
        .append_delivered_input_with_context(context.clone(), "one request", None)
        .unwrap();
    for seq in 2..=5 {
        let event: AgentChatEvent = serde_json::from_value(serde_json::json!({
            "id":format!("raw-assistant-{seq}"), "session_id":context.agent_id,
            "provider":"claude", "kind":"message", "role":"assistant",
            "text":if large_neighbour && seq == 2 {
                "body".repeat(wardian_core::conversations::CONVERSATION_INLINE_TEXT_LIMIT_BYTES)
            } else { format!("physical member {seq}") }, "source":"stream_json",
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
    let removed = records.iter().find(|record| record.seq == 3).unwrap();
    let removed_events = removed.event_refs.clone();
    let removed_sources = removed.source_refs.clone();
    records.retain(|record| record.seq != 3);
    let mut events: Vec<AgentChatEvent> =
        conversations::read_jsonl_records(&directory.join("events.jsonl")).unwrap();
    events.retain(|event| !removed_events.contains(&event.id));
    let mut sources: Vec<ConversationSourceRecord> =
        conversations::read_jsonl_records(&directory.join("sources.jsonl")).unwrap();
    sources.retain(|source| !removed_sources.contains(&source.source_id));
    let turns = archive::derive_turn_records(&conversation_id, &records, &events, &sources, true);
    conversations::write_jsonl_atomic(&directory.join("events.jsonl"), &events).unwrap();
    conversations::write_jsonl_atomic(&directory.join("sources.jsonl"), &sources).unwrap();
    conversations::write_jsonl_atomic(&directory.join("turns.jsonl"), &turns).unwrap();

    // Authorize the bytes actually present on disk, including historical serde
    // defaults that were omitted and whitespace that serialization normalizes.
    let mut bytes = Vec::new();
    for record in &records {
        if matches!(record.seq, 2 | 4) {
            let mut value = serde_json::to_value(record).unwrap();
            let object = value.as_object_mut().unwrap();
            for key in [
                "turn_id",
                "input_origin",
                "input_purpose",
                "request_root_id",
                "causal_ref",
            ] {
                assert!(object.remove(key).is_none_or(|value| value.is_null()));
            }
            let mut raw = b" \t".to_vec();
            raw.extend(serde_json::to_vec(&value).unwrap());
            raw.extend_from_slice(b" \r");
            assert_eq!(
                serde_json::from_slice::<ConversationNarrativeRecord>(&raw).unwrap(),
                *record
            );
            bytes.extend(raw);
        } else {
            bytes.extend(serde_json::to_vec(record).unwrap());
        }
        bytes.push(b'\n');
    }
    fs::write(directory.join("conversation.jsonl"), bytes).unwrap();
    let path = directory.join("manifest.json");
    let mut manifest: conversations::ConversationManifest =
        serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    manifest.record_count = records.len() as u64;
    manifest.turn_count = turns.len() as u64;
    conversations::write_json_atomic(&path, &manifest).unwrap();
    RawLineFixture {
        _temp: temp,
        state,
        context,
        directory,
        conversation_id,
        records,
        _guard: guard,
    }
}

fn approve(f: &RawLineFixture) -> Vec<u8> {
    let request = ArchiveVacancyRequest {
        conversation_id: f.conversation_id.clone(),
        sequence: 3,
        authorization_id: uuid::Uuid::new_v4().to_string(),
        approval_reference: "recorded-operator-task".into(),
        approval_text: "Approve the proven never-written narrative slot 3".into(),
    };
    let report = inspect_vacancy(&f.directory, &f.conversation_id, 3).unwrap();
    assert!(report.eligible, "{:?}", report.rules);
    f.state
        .authorize_vacancy(&f.context.agent_id, request)
        .unwrap();
    fs::read(f.directory.join("vacancies.jsonl")).unwrap()
}

fn native_tail(f: &RawLineFixture) -> AgentChatEvent {
    let event: AgentChatEvent = serde_json::from_value(serde_json::json!({
        "id":"raw-future-native", "session_id":f.context.agent_id, "provider":"claude",
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
    event
}

#[test]
fn native_event_first_recovery_preserves_authorized_neighbour_raw_lines() {
    let f = fixture();
    let path = f.directory.join("conversation.jsonl");
    let original = fs::read(&path).unwrap();
    let original_events = fs::read(f.directory.join("events.jsonl")).unwrap();
    let ledger = approve(&f);
    assert_eq!(fs::read(&path).unwrap(), original);
    native_tail(&f);
    validate_vacancies(&f.directory, &f.conversation_id).unwrap();

    f.state
        .append_delivered_input_with_context(f.context.clone(), "later input after native", None)
        .unwrap();

    let published = fs::read(&path).unwrap();
    assert!(published.starts_with(&original));
    let (_, records) = f.state.show(&f.conversation_id).unwrap();
    assert_eq!(
        records.iter().map(|record| record.seq).collect::<Vec<_>>(),
        [1, 2, 4, 5, 6, 7]
    );
    assert_eq!(records[4].text.as_deref(), Some("native event-first tail"));
    assert_eq!(records[5].text.as_deref(), Some("later input after native"));
    assert!(fs::read(f.directory.join("events.jsonl"))
        .unwrap()
        .starts_with(&original_events));
    assert_eq!(
        fs::read(f.directory.join("vacancies.jsonl")).unwrap(),
        ledger
    );
    assert_eq!(
        validate_vacancies(&f.directory, &f.conversation_id)
            .unwrap()
            .into_iter()
            .collect::<Vec<_>>(),
        [3]
    );
    assert!(f.state.archive_sequence_gap(&f.context).unwrap().is_none());
}

#[test]
fn recovery_rejects_changed_existing_neighbour_before_any_publication() {
    let mut f = fixture();
    let ledger = approve(&f);
    let event = native_tail(&f);
    let mut record = archive::narrative_from_chat_event(&event, 6).unwrap();
    let source = archive::source_record_from_chat_event(&event, 6).unwrap();
    record.source_refs = vec![source.source_id.clone()];
    let events = conversations::read_jsonl_records(&f.directory.join("events.jsonl")).unwrap();
    let names = [
        "conversation.jsonl",
        "events.jsonl",
        "sources.jsonl",
        "turns.jsonl",
        "manifest.json",
    ];
    let before = names.map(|name| fs::read(f.directory.join(name)).unwrap());
    f.records.iter_mut().find(|row| row.seq == 2).unwrap().text = Some("changed neighbour".into());
    let desired = f.records.clone();

    let error = repair::publish_recovered_observations(
        &publication::Publication::load(&f.directory, &f.conversation_id).unwrap(),
        &f.directory.join("conversation.jsonl"),
        &f.directory.join("sources.jsonl"),
        &mut f.records,
        &events,
        vec![repair::RecoveredObservation {
            record,
            sources: vec![source],
        }],
    )
    .unwrap_err();

    let conflict = error
        .get_ref()
        .and_then(|error| error.downcast_ref::<wardian_core::archive_vacancy::VacancyConflict>())
        .unwrap();
    assert_eq!(conflict.sequence, 3);
    assert_eq!(conflict.rule, "neighbour_hashes_match");
    assert_eq!(f.records, desired);
    for (name, bytes) in names.into_iter().zip(before) {
        assert_eq!(fs::read(f.directory.join(name)).unwrap(), bytes, "{name}");
    }
    assert_eq!(
        fs::read(f.directory.join("vacancies.jsonl")).unwrap(),
        ledger
    );
    validate_vacancies(&f.directory, &f.conversation_id).unwrap();
}

#[test]
fn recovery_allows_non_neighbour_semantic_updates() {
    let mut f = fixture();
    let ledger = approve(&f);
    let event = native_tail(&f);
    let path = f.directory.join("conversation.jsonl");
    let original = fs::read(&path).unwrap();
    let unchanged_prefix = original
        .split_inclusive(|byte| *byte == b'\n')
        .take(3)
        .flatten()
        .copied()
        .collect::<Vec<_>>();
    let mut record = archive::narrative_from_chat_event(&event, 6).unwrap();
    let source = archive::source_record_from_chat_event(&event, 6).unwrap();
    record.source_refs = vec![source.source_id.clone()];
    let events = conversations::read_jsonl_records(&f.directory.join("events.jsonl")).unwrap();
    f.records
        .iter_mut()
        .find(|row| row.seq == 5)
        .unwrap()
        .summary = Some("updated non-neighbour".into());

    assert_eq!(
        repair::publish_recovered_observations(
            &publication::Publication::load(&f.directory, &f.conversation_id).unwrap(),
            &path,
            &f.directory.join("sources.jsonl"),
            &mut f.records,
            &events,
            vec![repair::RecoveredObservation {
                record,
                sources: vec![source],
            }],
        )
        .unwrap(),
        1
    );

    assert!(fs::read(&path).unwrap().starts_with(&unchanged_prefix));
    let published: Vec<ConversationNarrativeRecord> =
        conversations::read_jsonl_records(&path).unwrap();
    assert_eq!(published, f.records);
    assert_eq!(
        published
            .iter()
            .find(|row| row.seq == 5)
            .unwrap()
            .summary
            .as_deref(),
        Some("updated non-neighbour")
    );
    assert_eq!(published.last().unwrap().seq, 6);
    assert_eq!(
        fs::read(f.directory.join("vacancies.jsonl")).unwrap(),
        ledger
    );
    validate_vacancies(&f.directory, &f.conversation_id).unwrap();
}

fn event_for_sequence(f: &RawLineFixture, sequence: u64) -> AgentChatEvent {
    let record = f.records.iter().find(|row| row.seq == sequence).unwrap();
    conversations::read_jsonl_records::<AgentChatEvent>(&f.directory.join("events.jsonl"))
        .unwrap()
        .into_iter()
        .find(|event| record.event_refs.contains(&event.id))
        .unwrap()
}

#[test]
fn capture_refresh_without_recovery_preserves_protected_raw_payloads() {
    let f = fixture();
    let events_path = f.directory.join("events.jsonl");
    let raw = fs::read(&events_path).unwrap();
    let padded = raw
        .split_inclusive(|byte| *byte == b'\n')
        .flat_map(|line| {
            let mut padded = b" \t".to_vec();
            padded.extend_from_slice(line);
            padded
        })
        .collect::<Vec<_>>();
    fs::write(&events_path, padded).unwrap();
    approve(&f);
    let mut event = event_for_sequence(&f, 5);
    event.turn_id = Some("later-provider-turn".into());
    f.state
        .append_chat_events_with_context(f.context.clone(), &[event])
        .unwrap();
    let (_, records) = f.state.show(&f.conversation_id).unwrap();
    assert_eq!(records.len(), 4);
    assert_eq!(
        records
            .iter()
            .find(|row| row.seq == 5)
            .unwrap()
            .turn_id
            .as_deref(),
        Some("later-provider-turn")
    );
    validate_vacancies(&f.directory, &f.conversation_id).unwrap();
}

#[test]
fn capture_rejects_protected_refresh_before_any_canonical_write() {
    for body_refresh in [false, true] {
        let f = fixture();
        approve(&f);
        let names = [
            "manifest.json",
            "conversation.jsonl",
            "events.jsonl",
            "sources.jsonl",
            "turns.jsonl",
            "vacancies.jsonl",
        ];
        let before = names.map(|name| fs::read(f.directory.join(name)).unwrap());
        let mut event = event_for_sequence(&f, 2);
        event.turn_id = Some("changed-protected-turn".into());
        if body_refresh {
            event.turn_id = None;
            event.text = Some(
                "x".repeat(wardian_core::conversations::CONVERSATION_INLINE_TEXT_LIMIT_BYTES + 1),
            );
            event.metadata["provider_phase"] = serde_json::json!("final_answer");
        }
        assert!(f
            .state
            .append_chat_events_with_context(f.context.clone(), &[event])
            .is_err());
        for (name, bytes) in names.into_iter().zip(before) {
            assert!(
                fs::read(f.directory.join(name)).unwrap() == bytes,
                "changed {name}"
            );
        }
        assert!(!f.directory.join("artifacts").exists());
        validate_vacancies(&f.directory, &f.conversation_id).unwrap();
    }
}

#[test]
fn capture_replays_unchanged_protected_events_without_mutating_the_ledger() {
    let f = fixture();
    let ledger = approve(&f);
    let events = [event_for_sequence(&f, 2), event_for_sequence(&f, 4)];
    f.state
        .append_chat_events_with_context(f.context.clone(), &events)
        .unwrap();
    assert_eq!(
        fs::read(f.directory.join("vacancies.jsonl")).unwrap(),
        ledger
    );
    validate_vacancies(&f.directory, &f.conversation_id).unwrap();
}

#[test]
fn capture_replays_the_same_protected_body_before_appending_a_tail() {
    let f = fixture_with_text(true);
    let ledger = approve(&f);
    let record = f.records.iter().find(|row| row.seq == 2).unwrap();
    assert_eq!(record.artifact_refs.len(), 1);
    let mut replay = event_for_sequence(&f, 2);
    assert!(replay.text.is_none());
    replay.text = Some(
        fs::read_to_string(f.directory.join("artifacts").join(&record.artifact_refs[0])).unwrap(),
    );
    let mut later = replay.clone();
    later.id = "native-after-body-replay".into();
    later.text = Some("later native body".into());
    later.metadata = serde_json::json!({});
    f.state
        .append_chat_events_with_context(f.context.clone(), &[replay, later])
        .unwrap();
    let (_, records) = f.state.show(&f.conversation_id).unwrap();
    assert_eq!(
        records
            .iter()
            .find(|row| row.text.as_deref() == Some("later native body"))
            .unwrap()
            .seq,
        6
    );
    assert_eq!(
        fs::read_dir(f.directory.join("artifacts")).unwrap().count(),
        1
    );
    assert_eq!(
        fs::read(f.directory.join("vacancies.jsonl")).unwrap(),
        ledger
    );
    validate_vacancies(&f.directory, &f.conversation_id).unwrap();
}
