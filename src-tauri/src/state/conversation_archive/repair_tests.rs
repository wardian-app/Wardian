use super::{ConversationArchiveContext, ConversationArchiveState};
use wardian_core::conversations::{
    read_jsonl_records, write_jsonl_atomic, ConversationNarrativeRecord, ConversationSourceRecord,
};
use wardian_core::models::chat::{AgentChatEvent, AgentChatEventKind, AgentChatRole};
use wardian_core::paths::agent_conversation_dir;

#[test]
fn source_failure_retries_the_same_raw_event_and_preserves_large_artifact() {
    let (_guard, temp) = isolated_home();
    let archive = ConversationArchiveState::default();
    let context = archive_context();
    archive
        .append_delivered_input_with_context(context.clone(), "seed", None)
        .expect("seed archive");

    let conversation_id = archive
        .active_conversation_id_for_test("agent-1")
        .expect("active conversation");
    let conversation_dir = agent_conversation_dir("agent-1", &conversation_id).expect("directory");
    let sources_path = conversation_dir.join("sources.jsonl");
    std::fs::write(&sources_path, b"").expect("create empty source snapshot");
    let saved_sources = temp.path().join("sources.jsonl.saved");
    std::fs::rename(&sources_path, &saved_sources).expect("move source snapshot");
    std::fs::create_dir(&sources_path).expect("obstruct source destination");

    let large_text = "archive-retry-"
        .repeat(wardian_core::conversations::CONVERSATION_INLINE_TEXT_LIMIT_BYTES / 13 + 1);
    let event = source_event("source-failure-event", &large_text);
    let first_error = archive
        .append_chat_events_with_context(context.clone(), std::slice::from_ref(&event))
        .expect_err("source destination must fail after raw event publication");
    assert!(!first_error.to_string().is_empty());

    let events_path = conversation_dir.join("events.jsonl");
    let events_after_failure: Vec<AgentChatEvent> =
        read_jsonl_records(&events_path).expect("read partial event snapshot");
    let partial_event = events_after_failure
        .iter()
        .find(|archived| archived.id == event.id)
        .expect("raw event remains after source failure");
    let pre_failure_artifact_refs = partial_event.metadata["text_artifact_refs"]
        .as_array()
        .expect("partial event keeps artifact references")
        .iter()
        .map(|reference| {
            reference
                .as_str()
                .expect("artifact reference is a string")
                .to_string()
        })
        .collect::<Vec<_>>();
    assert!(!pre_failure_artifact_refs.is_empty());
    let pre_failure_artifacts = pre_failure_artifact_refs
        .iter()
        .map(|reference| {
            std::fs::read(conversation_dir.join("artifacts").join(reference))
                .expect("read artifact from partial publication")
        })
        .collect::<Vec<_>>();

    std::fs::remove_dir(&sources_path).expect("remove only test-created obstruction");
    std::fs::rename(&saved_sources, &sources_path).expect("restore source snapshot");
    archive
        .append_chat_events_with_context(context, std::slice::from_ref(&event))
        .expect("retry repairs source and narrative");

    let records: Vec<ConversationNarrativeRecord> =
        read_jsonl_records(&conversation_dir.join("conversation.jsonl"))
            .expect("read repaired narrative");
    let sources: Vec<ConversationSourceRecord> =
        read_jsonl_records(&sources_path).expect("read repaired sources");
    let events: Vec<AgentChatEvent> =
        read_jsonl_records(&events_path).expect("read repaired events");

    assert_eq!(
        records
            .iter()
            .filter(|record| record
                .event_refs
                .iter()
                .any(|event_ref| event_ref == &event.id))
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|archived| archived.id == event.id)
            .count(),
        1
    );
    assert_eq!(sources.len(), 1);
    assert_eq!(sources[0].source_id, "src_2");
    let repaired = records
        .iter()
        .find(|record| {
            record
                .event_refs
                .iter()
                .any(|event_ref| event_ref == &event.id)
        })
        .expect("repaired record");
    assert_eq!(repaired.text, None);
    assert_eq!(repaired.artifact_refs, pre_failure_artifact_refs);
    let repaired_artifacts = repaired
        .artifact_refs
        .iter()
        .map(|reference| {
            std::fs::read(conversation_dir.join("artifacts").join(reference))
                .expect("read preserved artifact")
        })
        .collect::<Vec<_>>();
    assert_eq!(repaired_artifacts, pre_failure_artifacts);
    assert_eq!(repaired_artifacts[0], large_text.as_bytes());
}

#[test]
fn same_batch_exact_event_repeat_is_idempotent() {
    let (_guard, _temp) = isolated_home();
    let archive = ConversationArchiveState::default();
    let context = archive_context();
    let event = source_event("same-batch-exact", "same event");

    archive
        .append_chat_events_with_context(context, &[event.clone(), event])
        .expect("exact same-batch repeat is idempotent");

    let conversation_id = archive
        .active_conversation_id_for_test("agent-1")
        .expect("active conversation");
    let conversation_dir = agent_conversation_dir("agent-1", &conversation_id).expect("directory");
    let events: Vec<AgentChatEvent> =
        read_jsonl_records(&conversation_dir.join("events.jsonl")).expect("read events");
    let records: Vec<ConversationNarrativeRecord> =
        read_jsonl_records(&conversation_dir.join("conversation.jsonl")).expect("read records");
    assert_eq!(
        events
            .iter()
            .filter(|event| event.id == "same-batch-exact")
            .count(),
        1
    );
    assert_eq!(records.len(), 1);
}

#[test]
fn same_batch_conflicting_exact_identity_fails_before_publication() {
    let (_guard, _temp) = isolated_home();
    let archive = ConversationArchiveState::default();
    let context = archive_context();
    let first = source_event("same-batch-conflict", "first observation");
    let second = source_event("same-batch-conflict", "conflicting observation");

    let error = archive
        .append_chat_events_with_context(context, &[first, second])
        .expect_err("conflicting exact identity must fail closed");
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    assert_eq!(archive.active_conversation_id_for_test("agent-1"), None);
}

#[test]
fn same_batch_conflicting_legacy_alias_fails_before_publication() {
    let (_guard, _temp) = isolated_home();
    let archive = ConversationArchiveState::default();
    let context = archive_context();
    let mut first = source_event("legacy-owner-one", "first owner");
    first.metadata["legacy_event_ids"] = serde_json::json!(["shared-same-batch-alias"]);
    let mut second = source_event("legacy-owner-two", "second owner");
    second.metadata["legacy_event_ids"] = serde_json::json!(["shared-same-batch-alias"]);

    let error = archive
        .append_chat_events_with_context(context, &[first, second])
        .expect_err("conflicting legacy alias must fail closed");
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    assert_eq!(archive.active_conversation_id_for_test("agent-1"), None);
}

#[test]
fn same_batch_provider_enrichment_keeps_one_observation() {
    let (_guard, _temp) = isolated_home();
    let archive = ConversationArchiveState::default();
    let context = archive_context();
    let mut first = source_event("same-batch-enrichment", "provider observation");
    first.metadata = serde_json::json!({
        "provider_log": true,
        "provider_session_id": "session-one",
        "log_path": "provider.jsonl",
        "raw_type": "message"
    });
    let mut enriched = first.clone();
    enriched.metadata["provider_turn_id"] = serde_json::json!("turn-42");
    enriched.source = Some("provider_log".to_string());

    archive
        .append_chat_events_with_context(context, &[first, enriched])
        .expect("same observation is enriched rather than rejected");

    let conversation_id = archive
        .active_conversation_id_for_test("agent-1")
        .expect("active conversation");
    let conversation_dir = agent_conversation_dir("agent-1", &conversation_id).expect("directory");
    let events: Vec<AgentChatEvent> =
        read_jsonl_records(&conversation_dir.join("events.jsonl")).expect("read events");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].metadata["provider_turn_id"], "turn-42");
    assert_eq!(events[0].source.as_deref(), Some("provider_log"));
}

#[test]
fn delivered_merge_retry_does_not_duplicate_source_rows() {
    let (_guard, _temp) = isolated_home();
    let archive = ConversationArchiveState::default();
    let context = archive_context();
    archive
        .append_delivered_input_with_context(context.clone(), "Review this patch", None)
        .expect("delivered input");

    let conversation_id = archive
        .active_conversation_id_for_test("agent-1")
        .expect("active conversation");
    let conversation_dir = agent_conversation_dir("agent-1", &conversation_id).expect("directory");
    let events_path = conversation_dir.join("events.jsonl");
    let original_event_permissions = std::fs::metadata(&events_path)
        .expect("event snapshot metadata")
        .permissions();
    let mut read_only = original_event_permissions.clone();
    read_only.set_readonly(true);
    std::fs::set_permissions(&events_path, read_only).expect("obstruct event writes");

    let event = AgentChatEvent {
        role: Some(AgentChatRole::User),
        metadata: serde_json::json!({
            "provider_log": true,
            "provider_session_id": "session-one",
            "log_path": "provider.jsonl",
            "raw_type": "message"
        }),
        ..source_event("provider-delivery", "Review this patch")
    };
    let first_error = archive
        .append_chat_events_with_context(context.clone(), std::slice::from_ref(&event))
        .expect_err("event destination must fail after source publication");
    assert!(!first_error.to_string().is_empty());
    let source_path = conversation_dir.join("sources.jsonl");
    let sources_after_failure: Vec<ConversationSourceRecord> =
        read_jsonl_records(&source_path).expect("read source published before event failure");
    assert_eq!(sources_after_failure.len(), 1);

    std::fs::set_permissions(&events_path, original_event_permissions)
        .expect("restore event permissions");
    archive
        .append_chat_events_with_context(context, std::slice::from_ref(&event))
        .expect("retry completes delivered merge");
    archive
        .append_chat_events_with_context(archive_context(), std::slice::from_ref(&event))
        .expect("replaying completed delivered merge is idempotent");

    let records: Vec<ConversationNarrativeRecord> =
        read_jsonl_records(&conversation_dir.join("conversation.jsonl"))
            .expect("read merged narrative");
    let sources: Vec<ConversationSourceRecord> =
        read_jsonl_records(&source_path).expect("read merged sources");
    assert_eq!(sources.len(), 1);
    assert_eq!(sources[0].source_id, "src_1");
    assert_eq!(records.len(), 1);
    assert!(records[0]
        .event_refs
        .iter()
        .any(|event_ref| event_ref == &event.id));
}

#[test]
fn shared_sender_source_identity_keeps_distinct_rows_on_retry() {
    let (_guard, _temp) = isolated_home();
    let archive = ConversationArchiveState::default();
    let context = archive_context();
    let first = AgentChatEvent {
        role: Some(AgentChatRole::User),
        metadata: serde_json::json!({
            "sender_agent_id": "peer-agent"
        }),
        ..source_event("peer-delivery-1", "same text")
    };
    let second = AgentChatEvent {
        role: Some(AgentChatRole::User),
        metadata: serde_json::json!({
            "sender_agent_id": "peer-agent"
        }),
        ..source_event("peer-delivery-2", "same text")
    };

    archive
        .append_chat_events_with_context(context.clone(), &[first.clone(), second])
        .expect("append distinct sender observations");
    archive
        .append_chat_events_with_context(context, &[first])
        .expect("retry first sender observation");

    let conversation_id = archive
        .active_conversation_id_for_test("agent-1")
        .expect("active conversation");
    let conversation_dir = agent_conversation_dir("agent-1", &conversation_id).expect("directory");
    let sources: Vec<ConversationSourceRecord> =
        read_jsonl_records(&conversation_dir.join("sources.jsonl")).expect("read sources");
    assert_eq!(sources.len(), 2);
    assert_eq!(
        sources
            .iter()
            .map(|source| source.source_id.as_str())
            .collect::<Vec<_>>(),
        vec!["agent:peer-agent", "agent:peer-agent"]
    );
    assert_eq!(
        sources
            .iter()
            .map(|source| source.cursor.as_deref())
            .collect::<Vec<_>>(),
        vec![Some("1"), Some("2")]
    );
}

#[test]
fn ambiguous_legacy_event_identity_rejects_without_new_publication() {
    let (_guard, _temp) = isolated_home();
    let archive = ConversationArchiveState::default();
    let context = archive_context();
    let first = source_event("legacy-owner-one", "first owner");
    let second = source_event("legacy-owner-two", "second owner");
    archive
        .append_chat_events_with_context(context.clone(), std::slice::from_ref(&first))
        .expect("append first legacy owner");
    archive
        .append_chat_events_with_context(context.clone(), std::slice::from_ref(&second))
        .expect("append second legacy owner");

    let conversation_id = archive
        .active_conversation_id_for_test("agent-1")
        .expect("active conversation");
    let conversation_dir = agent_conversation_dir("agent-1", &conversation_id).expect("directory");
    let events_path = conversation_dir.join("events.jsonl");
    let mut durable_events: Vec<AgentChatEvent> =
        read_jsonl_records(&events_path).expect("read durable owners");
    for event in &mut durable_events {
        event
            .metadata
            .as_object_mut()
            .expect("durable event metadata object")
            .insert(
                "legacy_event_ids".to_string(),
                serde_json::json!(["shared-legacy-id"]),
            );
    }
    write_jsonl_atomic(&events_path, &durable_events).expect("seed ambiguous legacy owners");

    let mut retry = source_event("new-legacy-attempt", "new attempt");
    retry
        .metadata
        .as_object_mut()
        .expect("retry metadata object")
        .insert(
            "legacy_event_ids".to_string(),
            serde_json::json!(["shared-legacy-id"]),
        );
    let error = archive
        .append_chat_events_with_context(context, std::slice::from_ref(&retry))
        .expect_err("shared legacy identity must fail closed");
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);

    let records: Vec<ConversationNarrativeRecord> =
        read_jsonl_records(&conversation_dir.join("conversation.jsonl"))
            .expect("read unchanged records");
    let events: Vec<AgentChatEvent> =
        read_jsonl_records(&conversation_dir.join("events.jsonl")).expect("read unchanged events");
    assert_eq!(records.len(), 2);
    assert_eq!(events.len(), 2);
}

#[test]
fn duplicate_narrative_owner_rejects_exact_event_retry() {
    let (_guard, _temp) = isolated_home();
    let archive = ConversationArchiveState::default();
    let context = archive_context();
    let event = source_event("duplicate-owner-event", "owned once");
    archive
        .append_chat_events_with_context(context.clone(), std::slice::from_ref(&event))
        .expect("append event");

    let conversation_id = archive
        .active_conversation_id_for_test("agent-1")
        .expect("active conversation");
    let conversation_dir = agent_conversation_dir("agent-1", &conversation_id).expect("directory");
    let conversation_path = conversation_dir.join("conversation.jsonl");
    let mut records: Vec<ConversationNarrativeRecord> =
        read_jsonl_records(&conversation_path).expect("read records");
    let mut duplicate = records[0].clone();
    duplicate.seq = 2;
    records.push(duplicate);
    write_jsonl_atomic(&conversation_path, &records).expect("write duplicate owner fixture");

    let error = archive
        .append_chat_events_with_context(context, std::slice::from_ref(&event))
        .expect_err("duplicate narrative owner must fail closed");
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
}

#[test]
fn generated_recovery_fills_completed_prefix_before_new_admission() {
    let (_guard, _temp) = isolated_home();
    let archive = ConversationArchiveState::default();
    let context = archive_context();
    archive
        .append_delivered_input_with_context(context.clone(), "seed", None)
        .expect("seed archive");
    archive
        .append_delivered_input_with_context(context.clone(), "missing prefix", None)
        .expect("append missing-prefix row");
    archive
        .append_delivered_input_with_context(context.clone(), "later row", None)
        .expect("append later row");

    let conversation_id = archive
        .active_conversation_id_for_test("agent-1")
        .expect("active conversation");
    let conversation_dir = agent_conversation_dir("agent-1", &conversation_id).expect("directory");
    let conversation_path = conversation_dir.join("conversation.jsonl");
    let records: Vec<ConversationNarrativeRecord> =
        read_jsonl_records(&conversation_path).expect("read records");
    write_jsonl_atomic(
        &conversation_path,
        &records
            .into_iter()
            .filter(|record| record.seq != 2)
            .collect::<Vec<_>>(),
    )
    .expect("remove only the durable narrative row");
    drop(archive);

    let reopened = ConversationArchiveState::default();
    reopened
        .append_delivered_input_with_context(context, "new after recovery", None)
        .expect("recover sequence 2 before admitting sequence 4");

    let records: Vec<ConversationNarrativeRecord> =
        read_jsonl_records(&conversation_path).expect("read repaired records");
    assert_eq!(
        records.iter().map(|record| record.seq).collect::<Vec<_>>(),
        vec![1, 2, 3, 4]
    );
    assert_eq!(records[1].text.as_deref(), Some("missing prefix"));
    assert_eq!(records[2].text.as_deref(), Some("later row"));
    assert_eq!(records[3].text.as_deref(), Some("new after recovery"));
}

#[test]
fn unresolved_sequence_gap_fails_closed_before_new_admission() {
    let (_guard, _temp) = isolated_home();
    let archive = ConversationArchiveState::default();
    let context = archive_context();
    archive
        .append_delivered_input_with_context(context.clone(), "seed", None)
        .expect("seed archive");
    archive
        .append_delivered_input_with_context(context.clone(), "missing row", None)
        .expect("append missing row");
    archive
        .append_delivered_input_with_context(context.clone(), "later row", None)
        .expect("append later row");

    let conversation_id = archive
        .active_conversation_id_for_test("agent-1")
        .expect("active conversation");
    let conversation_dir = agent_conversation_dir("agent-1", &conversation_id).expect("directory");
    let conversation_path = conversation_dir.join("conversation.jsonl");
    let events_path = conversation_dir.join("events.jsonl");
    let records: Vec<ConversationNarrativeRecord> =
        read_jsonl_records(&conversation_path).expect("read records");
    write_jsonl_atomic(
        &conversation_path,
        &records
            .into_iter()
            .filter(|record| record.seq != 2)
            .collect::<Vec<_>>(),
    )
    .expect("remove only missing narrative row");
    let events: Vec<AgentChatEvent> = read_jsonl_records(&events_path).expect("read events");
    write_jsonl_atomic(
        &events_path,
        &events
            .into_iter()
            .filter(|event| event.id != format!("generated:{conversation_id}:2"))
            .collect::<Vec<_>>(),
    )
    .expect("remove only unrecoverable raw observation");
    drop(archive);

    let reopened = ConversationArchiveState::default();
    let error = reopened
        .append_delivered_input_with_context(context, "must not be admitted", None)
        .expect_err("unresolved sequence gap must fail closed");
    assert_eq!(error.kind(), std::io::ErrorKind::WouldBlock);
    let after: Vec<ConversationNarrativeRecord> =
        read_jsonl_records(&conversation_path).expect("read unchanged records");
    assert_eq!(
        after.iter().map(|record| record.seq).collect::<Vec<_>>(),
        vec![1, 3]
    );
}

#[test]
fn generated_append_recovers_old_then_admits_distinct_new_call() {
    let (_guard, _temp) = isolated_home();
    let archive = ConversationArchiveState::default();
    let context = archive_context();
    archive
        .append_delivered_input_with_context(context.clone(), "seed", None)
        .expect("seed archive");

    let conversation_id = archive
        .active_conversation_id_for_test("agent-1")
        .expect("active conversation");
    let conversation_dir = agent_conversation_dir("agent-1", &conversation_id).expect("directory");
    let sources_path = conversation_dir.join("sources.jsonl");
    std::fs::write(&sources_path, b"").expect("create empty source snapshot");
    let original_source_permissions = std::fs::metadata(&sources_path)
        .expect("source snapshot metadata")
        .permissions();
    let mut read_only = original_source_permissions.clone();
    read_only.set_readonly(true);
    std::fs::set_permissions(&sources_path, read_only).expect("obstruct source writes");

    let first_error = archive
        .append_delivered_input_with_context(context.clone(), "from peer", Some("peer-agent"))
        .expect_err("source destination must fail after generated event publication");
    assert!(!first_error.to_string().is_empty());

    std::fs::set_permissions(&sources_path, original_source_permissions)
        .expect("restore source permissions");
    archive
        .append_delivered_input_with_context(context, "from peer", Some("peer-agent"))
        .expect("retry recovers the old row and admits a distinct new call");

    let events: Vec<AgentChatEvent> =
        read_jsonl_records(&conversation_dir.join("events.jsonl")).expect("read events");
    let records: Vec<ConversationNarrativeRecord> =
        read_jsonl_records(&conversation_dir.join("conversation.jsonl")).expect("read records");
    let sources: Vec<ConversationSourceRecord> =
        read_jsonl_records(&sources_path).expect("read sources");
    assert_eq!(
        events
            .iter()
            .filter(|event| event.metadata["generated"] == true)
            .count(),
        3
    );
    assert_eq!(records.len(), 3);
    assert_eq!(sources.len(), 2);
    assert_eq!(
        sources
            .iter()
            .map(|source| source.cursor.as_deref())
            .collect::<Vec<_>>(),
        vec![Some("2"), Some("3")]
    );
}

#[test]
fn reopened_archive_recovers_raw_events_before_new_input_and_retains_artifact_refs() {
    let (_guard, temp) = isolated_home();
    let archive = ConversationArchiveState::default();
    let context = archive_context();
    archive
        .append_delivered_input_with_context(context.clone(), "seed", None)
        .expect("seed archive");

    let conversation_id = archive
        .active_conversation_id_for_test("agent-1")
        .expect("active conversation");
    let conversation_dir = agent_conversation_dir("agent-1", &conversation_id).expect("directory");
    let sources_path = conversation_dir.join("sources.jsonl");
    let saved_sources = temp.path().join("reopen-sources.jsonl.saved");
    std::fs::write(&sources_path, b"").expect("create empty source snapshot");
    std::fs::rename(&sources_path, &saved_sources).expect("move source snapshot");
    std::fs::create_dir(&sources_path).expect("obstruct source destination");

    let large_text = "reopen-archive-"
        .repeat(wardian_core::conversations::CONVERSATION_INLINE_TEXT_LIMIT_BYTES / 13 + 1);
    let orphan = source_event("reopen-orphan", &large_text);
    archive
        .append_chat_events_with_context(context.clone(), std::slice::from_ref(&orphan))
        .expect_err("source failure leaves a durable raw observation");

    std::fs::remove_dir(&sources_path).expect("remove only test obstruction");
    std::fs::rename(&saved_sources, &sources_path).expect("restore source snapshot");
    drop(archive);

    let reopened = ConversationArchiveState::default();
    let new_event = source_event("reopen-new", "new input after restart");
    reopened
        .append_chat_events_with_context(context, std::slice::from_ref(&new_event))
        .expect("recover raw observations before admitting new input");

    let records: Vec<ConversationNarrativeRecord> =
        read_jsonl_records(&conversation_dir.join("conversation.jsonl"))
            .expect("read recovered records");
    assert_eq!(
        records.iter().map(|record| record.seq).collect::<Vec<_>>(),
        vec![1, 2, 3]
    );
    let recovered = records
        .iter()
        .find(|record| {
            record
                .event_refs
                .iter()
                .any(|event_ref| event_ref == &orphan.id)
        })
        .expect("recovered orphan record");
    assert_eq!(recovered.text, None);
    assert!(!recovered.artifact_refs.is_empty());
    assert_eq!(
        std::fs::read_to_string(
            conversation_dir
                .join("artifacts")
                .join(&recovered.artifact_refs[0])
        )
        .expect("read retained orphan artifact"),
        large_text
    );
    assert!(records.iter().any(|record| record
        .event_refs
        .iter()
        .any(|event_ref| event_ref == &new_event.id)));
}

#[test]
fn generated_orphan_is_recovered_before_distinct_same_text_invocation() {
    let (_guard, _temp) = isolated_home();
    let archive = ConversationArchiveState::default();
    let context = archive_context();
    archive
        .append_delivered_input_with_context(context.clone(), "seed", None)
        .expect("seed archive");

    let conversation_id = archive
        .active_conversation_id_for_test("agent-1")
        .expect("active conversation");
    let conversation_dir = agent_conversation_dir("agent-1", &conversation_id).expect("directory");
    let sources_path = conversation_dir.join("sources.jsonl");
    std::fs::write(&sources_path, b"").expect("create empty source snapshot");
    let original_source_permissions = std::fs::metadata(&sources_path)
        .expect("source snapshot metadata")
        .permissions();
    let mut read_only = original_source_permissions.clone();
    read_only.set_readonly(true);
    std::fs::set_permissions(&sources_path, read_only).expect("obstruct source writes");

    archive
        .append_delivered_input_with_context(
            context.clone(),
            "durable generated orphan",
            Some("peer-agent"),
        )
        .expect_err("generated source failure leaves a durable raw observation");
    std::fs::set_permissions(&sources_path, original_source_permissions)
        .expect("restore source permissions");
    drop(archive);

    let reopened = ConversationArchiveState::default();
    reopened
        .append_delivered_input_with_context(context, "durable generated orphan", None)
        .expect("recover old generated row, then append new invocation");

    let records: Vec<ConversationNarrativeRecord> =
        read_jsonl_records(&conversation_dir.join("conversation.jsonl"))
            .expect("read generated records");
    assert_eq!(
        records.iter().map(|record| record.seq).collect::<Vec<_>>(),
        vec![1, 2, 3]
    );
    assert_eq!(records[1].text.as_deref(), Some("durable generated orphan"));
    assert_eq!(records[2].text.as_deref(), Some("durable generated orphan"));
    let events: Vec<AgentChatEvent> =
        read_jsonl_records(&conversation_dir.join("events.jsonl")).expect("read events");
    assert!(events
        .iter()
        .any(|event| event.id == format!("generated:{conversation_id}:2")));
    assert!(events
        .iter()
        .any(|event| event.id == format!("generated:{conversation_id}:3")));
}

#[test]
fn old_generated_orphan_without_payload_fails_closed_without_mutation() {
    let (_guard, _temp) = isolated_home();
    let archive = ConversationArchiveState::default();
    let context = archive_context();
    archive
        .append_delivered_input_with_context(context.clone(), "seed", None)
        .expect("seed archive");
    archive
        .append_delivered_input_with_context(context.clone(), "old generated row", None)
        .expect("create generated row");

    let conversation_id = archive
        .active_conversation_id_for_test("agent-1")
        .expect("active conversation");
    let conversation_dir = agent_conversation_dir("agent-1", &conversation_id).expect("directory");
    let events_path = conversation_dir.join("events.jsonl");
    let conversation_path = conversation_dir.join("conversation.jsonl");
    let mut events: Vec<AgentChatEvent> = read_jsonl_records(&events_path).expect("read events");
    let old_event_id = format!("generated:{conversation_id}:2");
    let old_event = events
        .iter_mut()
        .find(|event| event.id == old_event_id)
        .expect("generated event");
    old_event
        .metadata
        .as_object_mut()
        .expect("generated metadata object")
        .remove("archive_record");
    write_jsonl_atomic(&events_path, &events).expect("write ambiguous legacy event");
    let mut records: Vec<ConversationNarrativeRecord> =
        read_jsonl_records(&conversation_path).expect("read records");
    records.pop().expect("orphan record");
    write_jsonl_atomic(&conversation_path, &records).expect("remove only test record");
    drop(archive);

    let reopened = ConversationArchiveState::default();
    let error = reopened
        .append_delivered_input_with_context(context, "must not bind", None)
        .expect_err("ambiguous old generated orphan must fail closed");
    assert_eq!(error.kind(), std::io::ErrorKind::WouldBlock);
    let after: Vec<ConversationNarrativeRecord> =
        read_jsonl_records(&conversation_path).expect("read unchanged records");
    assert_eq!(after.len(), 1);
    let events_after: Vec<AgentChatEvent> =
        read_jsonl_records(&events_path).expect("read unchanged events");
    assert!(events_after
        .iter()
        .any(|event| event.id == old_event_id && event.metadata["archive_record"].is_null()));
}

#[test]
fn projection_repair_does_not_consume_new_generated_input() {
    let (_guard, _temp) = isolated_home();
    let archive = ConversationArchiveState::default();
    let context = archive_context();
    archive
        .append_delivered_input_with_context(context.clone(), "seed", None)
        .expect("seed archive");
    let conversation_id = archive
        .active_conversation_id_for_test("agent-1")
        .expect("active conversation");
    let conversation_dir = agent_conversation_dir("agent-1", &conversation_id).expect("directory");
    std::fs::remove_file(conversation_dir.join("turns.jsonl")).expect("remove turns projection");

    let appended = archive
        .append_delivered_input_with_context(context, "new after projection repair", None)
        .expect("repair projections and append new input");
    assert_eq!(appended, 1);
    let records: Vec<ConversationNarrativeRecord> =
        read_jsonl_records(&conversation_dir.join("conversation.jsonl")).expect("read records");
    assert_eq!(records.len(), 2);
    assert_eq!(
        records[1].text.as_deref(),
        Some("new after projection repair")
    );
}

fn archive_context() -> ConversationArchiveContext {
    ConversationArchiveContext {
        agent_id: "agent-1".to_string(),
        agent_name: "CoderOne".to_string(),
        agent_class: "Coder".to_string(),
        workspace: "<absolute-workspace-path>".to_string(),
        provider: "codex".to_string(),
        provider_session_ids: vec!["session-one".to_string()],
        provider_source_key: Some("codex:session:session-one".to_string()),
    }
}

fn source_event(id: &str, text: &str) -> AgentChatEvent {
    AgentChatEvent {
        id: id.to_string(),
        session_id: "agent-1".to_string(),
        provider: "codex".to_string(),
        kind: AgentChatEventKind::Message,
        role: Some(AgentChatRole::Assistant),
        text: Some(text.to_string()),
        title: None,
        status: None,
        turn_id: None,
        source: Some("response_item".to_string()),
        command: None,
        exit_code: None,
        path: None,
        language: None,
        created_at: Some("2026-06-15T00:00:00.000Z".to_string()),
        sequence: None,
        metadata: serde_json::json!({
            "provider_session_id": "session-one",
            "raw_type": "message"
        }),
    }
}

fn isolated_home() -> (tokio::sync::MutexGuard<'static, ()>, tempfile::TempDir) {
    let guard = crate::utils::wardian_test_env_lock();
    let temp = tempfile::tempdir().expect("temp dir");
    std::env::set_var("WARDIAN_HOME", temp.path());
    (guard, temp)
}
