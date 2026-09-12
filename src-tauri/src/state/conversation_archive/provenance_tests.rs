use super::*;
use wardian_core::models::chat::{AgentChatEventKind, AgentChatRole};

fn event(id: &str, provider: &str, root: Option<&str>) -> AgentChatEvent {
    AgentChatEvent {
        id: id.into(), session_id: "agent-1".into(), provider: provider.into(),
        kind: AgentChatEventKind::Message, role: Some(AgentChatRole::User),
        text: Some("same prompt".into()), title: None, status: None,
        turn_id: Some("0".into()), source: Some("conversation_database".into()),
        command: None, exit_code: None, path: None, language: None,
        created_at: Some("2026-09-07T09:40:00Z".into()), sequence: Some(1),
        metadata: root.map(|root| serde_json::json!({"provider_log":true,"log_path":"/isolated/session.db","input_origin":"human_input","input_purpose":"request","request_root_id":root,"provider_step_source":4})).unwrap_or_else(|| serde_json::json!({"provider_log":true,"log_path":"/isolated/session.db","archive_extra":"keep"})),
    }
}

#[test]
fn same_id_native_capture_repairs_persisted_provenance() {
    let _guard = crate::utils::wardian_test_env_lock();
    let temp = tempfile::tempdir().unwrap();
    std::env::set_var("WARDIAN_HOME", temp.path());
    let archive = ConversationArchiveState::default();
    let old = event("native-event", "antigravity", None);
    archive.append_chat_events("agent-1", &[old]).unwrap();
    let current = event("native-event", "antigravity", Some("native-event"));
    archive.append_chat_events("agent-1", &[current]).unwrap();
    let events = archive.chat_events_for_agent("agent-1").unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].metadata["request_root_id"], "native-event");
}

use crate::commands::chat::archive_identity as native_identity;

const PI_FIXTURE: &str = include_str!("fixtures/real-pi-session.jsonl");
fn pi_capture() -> (Vec<AgentChatEvent>, Vec<AgentChatEvent>) {
    let path = std::path::Path::new(
        "/isolated/2026-09-07T09-40-03-213Z_e1e33694-c782-423f-9142-bf974206a195.jsonl",
    );
    let rows: Vec<serde_json::Value> = PI_FIXTURE
        .lines()
        .map(|s| serde_json::from_str(s).unwrap())
        .collect();
    let mut legacy = crate::providers::chat_transcript::normalize_chat_lines(
        "agent-1",
        "pi",
        PI_FIXTURE.lines(),
    );
    legacy.retain(|event| event.kind == AgentChatEventKind::Message);
    for event in &mut legacy {
        // The base parser is the actual pre-#1167 parser. Remove the two
        // repaired fields if this test is later run on the integrated adapter.
        event.turn_id = None;
        event
            .metadata
            .as_object_mut()
            .unwrap()
            .remove("request_root_id");
        event.metadata["provider_log"] = serde_json::json!(true);
        event.metadata["log_path"] = serde_json::json!(path);
        event.id = native_identity::stable_provider_log_event_id(event, path);
    }
    let mut current = legacy.clone();
    for event in &mut current {
        // Boundary stimulus: project #1167's envelope mapping from the real
        // retained record; this is not a claim to retest the Pi adapter.
        let row = &rows[event.sequence.unwrap() as usize - 1];
        event.turn_id = Some(row["id"].as_str().unwrap().into());
        if event.role == Some(AgentChatRole::User) {
            event.metadata["request_root_id"] = row["id"].clone();
        }
        event.id = native_identity::stable_provider_log_event_id(event, path);
    }
    native_identity::attach_native_legacy_aliases(&mut current, path, PI_FIXTURE, true);
    (legacy, current)
}

fn isolate() -> (tokio::sync::MutexGuard<'static, ()>, tempfile::TempDir) {
    let guard = crate::utils::wardian_test_env_lock();
    let temp = tempfile::tempdir().unwrap();
    std::env::set_var("WARDIAN_HOME", temp.path());
    (guard, temp)
}

#[test]
fn real_pi_changed_ids_repair_one_row_and_existing_double_rows() {
    let (_guard, _temp) = isolate();
    for double in [false, true] {
        let case_home = tempfile::tempdir().unwrap();
        std::env::set_var("WARDIAN_HOME", case_home.path());
        let archive = ConversationArchiveState::default();
        let (legacy, current) = pi_capture();
        let mut context = ConversationArchiveContext::for_agent_id("agent-1", "pi");
        context.provider_source_key = Some("pi:source:retained".into());
        archive
            .append_chat_events_with_context(context.clone(), &legacy)
            .unwrap();
        let id = archive
            .active_conversation_id(&context.agent_id)
            .unwrap()
            .unwrap();
        let dir = conversation_dir(&context.agent_id, &id).unwrap();
        if double {
            // Seed the exact post-#1167/pre-boundary failure: both ordinary
            // old/new event IDs and narrative rows have already been appended.
            let mut records: Vec<ConversationNarrativeRecord> =
                read_jsonl_records(&dir.join("conversation.jsonl")).unwrap();
            for (index, event) in current.iter().enumerate() {
                let mut unaliased = event.clone();
                unaliased
                    .metadata
                    .as_object_mut()
                    .unwrap()
                    .remove("legacy_event_ids");
                append_jsonl_record(&dir.join("events.jsonl"), &unaliased).unwrap();
                records.push(narrative_from_chat_event(&unaliased, index as u64 + 3).unwrap());
            }
            write_jsonl_atomic(&dir.join("conversation.jsonl"), &records).unwrap();
        }
        archive
            .append_chat_events_with_context(context.clone(), &current)
            .unwrap();
        let restarted = ConversationArchiveState::default();
        let events = restarted.chat_events_for_agent(&context.agent_id).unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].id, legacy[0].id, "original ID survives downgrade");
        assert_eq!(events[0].metadata["request_root_id"], "ec1b3195");
        let (_, records) = restarted.show(&id).unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].seq, 1);
        assert_eq!(records[0].request_root_id.as_deref(), Some("ec1b3195"));
        assert!(records[0].event_refs.contains(&legacy[0].id));
        assert!(records[0].event_refs.contains(&current[0].id));
        let before = std::fs::read(dir.join("conversation.jsonl")).unwrap();
        assert_eq!(
            restarted
                .append_chat_events_with_context(context.clone(), &current)
                .unwrap(),
            0
        );
        assert_eq!(
            restarted
                .append_chat_events_with_context(context, &legacy)
                .unwrap(),
            0,
            "older adapter cannot undo enrichment"
        );
        assert_eq!(
            before,
            std::fs::read(dir.join("conversation.jsonl")).unwrap()
        );
        let turns = restarted
            .turn_records_for_conversations(&restarted.list(Some("agent-1"), false).unwrap())
            .unwrap();
        assert_eq!(turns.len(), 1);
    }
}

#[test]
fn agy_explicit_sources_refresh_narrative_and_standalone_replay() {
    let (_guard, _temp) = isolate();
    let archive = ConversationArchiveState::default();
    let old = vec![
        event("source4", "antigravity", None),
        event("source2", "antigravity", None),
    ];
    archive.append_chat_events("agent-1", &old).unwrap();
    let mut current = vec![
        event("source4", "antigravity", Some("source4")),
        event("source2", "antigravity", None),
    ];
    current[1].metadata["provider_step_source"] = serde_json::json!(2);
    current[1].metadata["input_origin"] = serde_json::json!("provider_internal");
    current[1].metadata["input_purpose"] = serde_json::json!("internal");
    archive.append_chat_events("agent-1", &current).unwrap();
    let id = archive.active_conversation_id("agent-1").unwrap().unwrap();
    let replay = ConversationArchiveState::default();
    let events = replay.chat_events_for_agent("agent-1").unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].metadata["request_root_id"], "source4");
    assert_eq!(events[1].role, Some(AgentChatRole::System));
    assert_eq!(events[1].metadata["input_origin"], "provider_internal");
    assert!(events[1].metadata.get("request_root_id").is_none());
    let (_, records) = replay.show(&id).unwrap();
    assert_eq!(records[1].role.as_deref(), Some("system"));
    assert_eq!(records[1].input_purpose.as_deref(), Some("internal"));
    assert_eq!(records[0].request_root_id.as_deref(), Some("source4"));
    assert_eq!(replay.append_chat_events("agent-1", &old).unwrap(), 0);
}

#[test]
fn disabled_capture_repairs_only_view_and_retains_archive_history() {
    let (_guard, _temp) = isolate();
    for provider in ["pi", "antigravity"] {
        let case_home = tempfile::tempdir().unwrap();
        std::env::set_var("WARDIAN_HOME", case_home.path());
        let archive = ConversationArchiveState::default();
        let (mut old, current) = if provider == "pi" {
            pi_capture()
        } else {
            (
                vec![event("native", "antigravity", None)],
                vec![event("native", "antigravity", Some("native"))],
            )
        };
        let mut history = old[0].clone();
        history.id = "archive-only".into();
        history.text = Some("Older retained context".into());
        old.push(history);
        let mut context = ConversationArchiveContext::for_agent_id("agent-1", provider);
        context.provider_source_key = Some(format!("{provider}:source:retained"));
        archive
            .append_chat_events_with_context(context.clone(), &old)
            .unwrap();
        let id = archive.active_conversation_id("agent-1").unwrap().unwrap();
        let dir = conversation_dir("agent-1", &id).unwrap();
        let before: Vec<_> = [
            "events.jsonl",
            "conversation.jsonl",
            "turns.jsonl",
            "manifest.json",
        ]
        .iter()
        .map(|name| std::fs::read(dir.join(name)).unwrap())
        .collect();
        // Existing disabled capture cutoff behavior is separate from repair.
        archive
            .discard_agent_with_context(context.clone(), &current)
            .unwrap();
        let replay = ConversationArchiveState::default();
        for _ in 0..2 {
            let view = provenance::merge_current_capture(
                current.clone(),
                replay.chat_events_for_capture(&context).unwrap(),
            )
            .unwrap();
            assert_eq!(view.len(), old.len());
            assert!(view[0].metadata["request_root_id"].is_string());
            assert_eq!(view.last().unwrap().id, "archive-only");
        }
        let after: Vec<_> = [
            "events.jsonl",
            "conversation.jsonl",
            "turns.jsonl",
            "manifest.json",
        ]
        .iter()
        .map(|name| std::fs::read(dir.join(name)).unwrap())
        .collect();
        assert_eq!(before, after);
        assert!(replay.chat_events_for_agent("agent-1").unwrap()[0]
            .metadata
            .get("request_root_id")
            .is_none());
        // Re-enabling capture can enrich prior logged rows despite the cutoff,
        // but must not ingest a new event observed only while disabled.
        let mut disabled_only = current[0].clone();
        disabled_only.id = "disabled-only".into();
        disabled_only
            .metadata
            .as_object_mut()
            .unwrap()
            .remove("legacy_event_ids");
        let mut capture = current.clone();
        capture.push(disabled_only);
        archive
            .discard_agent_with_context(context.clone(), &capture)
            .unwrap();
        replay
            .append_chat_events_with_context(context, &capture)
            .unwrap();
        let repaired = replay.chat_events_for_agent("agent-1").unwrap();
        assert_eq!(repaired.len(), old.len());
        assert!(repaired[0].metadata["request_root_id"].is_string());
        assert!(!repaired.iter().any(|event| event.id == "disabled-only"));
    }
}

#[test]
fn equal_text_foreign_sources_and_missing_evidence_never_merge() {
    let a = event("first", "pi", None);
    let b = event("second", "pi", Some("second"));
    let merged = provenance::merge_current_capture(vec![b], vec![a.clone()]).unwrap();
    assert_eq!(merged.len(), 2);
    for field in ["log_path", "provider_session_id"] {
        let mut old = a.clone();
        let mut current = event("first", "pi", Some("root"));
        old.metadata[field] = serde_json::json!("one");
        current.metadata[field] = serde_json::json!("two");
        let merged = provenance::merge_current_capture(vec![current], vec![old]).unwrap();
        assert_eq!(merged.len(), 2);
        assert!(merged[0].metadata.get("request_root_id").is_none());
    }
    let mut foreign = event("first", "pi", Some("root"));
    foreign.session_id = "foreign-agent".into();
    assert_eq!(
        provenance::merge_current_capture(vec![foreign], vec![a.clone()])
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        provenance::merge_current_capture(vec![], vec![a.clone()]).unwrap(),
        vec![a]
    );
}

#[test]
fn pi_aliases_need_complete_unique_native_evidence() {
    let (legacy, current) = pi_capture();
    let path = std::path::Path::new(current[0].metadata["log_path"].as_str().unwrap());
    for (text, complete) in [
        (PI_FIXTURE.to_string(), false),
        (
            PI_FIXTURE.lines().skip(3).collect::<Vec<_>>().join("\n"),
            true,
        ),
    ] {
        let mut events = current.clone();
        for e in &mut events {
            e.metadata
                .as_object_mut()
                .unwrap()
                .remove("legacy_event_ids");
        }
        native_identity::attach_native_legacy_aliases(&mut events, path, &text, complete);
        assert!(events
            .iter()
            .all(|e| e.metadata.get("legacy_event_ids").is_none()));
    }
    let mut events = legacy.clone();
    native_identity::attach_native_legacy_aliases(&mut events, path, PI_FIXTURE, true);
    assert!(
        events
            .iter()
            .all(|e| e.metadata.get("legacy_event_ids").is_none()),
        "no root manufactured for old adapter"
    );
    let mut rows: Vec<serde_json::Value> = PI_FIXTURE
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let mut repeat = rows[3].clone();
    repeat["id"] = serde_json::json!("repeat-native-id");
    rows.push(repeat);
    let text = rows
        .iter()
        .map(|row| row.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    let mut events = current.clone();
    for e in &mut events {
        e.metadata
            .as_object_mut()
            .unwrap()
            .remove("legacy_event_ids");
    }
    let mut repeat = events[0].clone();
    repeat.sequence = Some(6);
    repeat.turn_id = Some("repeat-native-id".into());
    repeat.id = native_identity::stable_provider_log_event_id(&repeat, path);
    events.push(repeat);
    native_identity::attach_native_legacy_aliases(&mut events, path, &text, true);
    assert!(events[0].metadata.get("legacy_event_ids").is_none());
    assert!(events[2].metadata.get("legacy_event_ids").is_none());
    assert_ne!(events[0].id, events[2].id);
}

#[test]
fn conflicting_native_evidence_fails_without_partial_publication() {
    let (_guard, _temp) = isolate();
    let archive = ConversationArchiveState::default();
    let old = event("native", "antigravity", Some("native"));
    archive
        .append_chat_events("agent-1", std::slice::from_ref(&old))
        .unwrap();
    let id = archive.active_conversation_id("agent-1").unwrap().unwrap();
    let dir = conversation_dir("agent-1", &id).unwrap();
    let before = std::fs::read(dir.join("events.jsonl")).unwrap();
    let mut conflict = old;
    conflict.metadata["request_root_id"] = serde_json::json!("different-native-root");
    assert_eq!(
        archive
            .append_chat_events("agent-1", &[conflict])
            .unwrap_err()
            .kind(),
        io::ErrorKind::InvalidData
    );
    assert_eq!(before, std::fs::read(dir.join("events.jsonl")).unwrap());
}

#[test]
fn concurrent_current_and_older_captures_converge() {
    let (_guard, _temp) = isolate();
    let archive = Arc::new(ConversationArchiveState::default());
    let (legacy, current) = pi_capture();
    archive.append_chat_events("agent-1", &legacy).unwrap();
    std::thread::scope(|scope| {
        for events in [&legacy, &current, &legacy, &current] {
            let archive = archive.clone();
            scope.spawn(move || {
                for _ in 0..5 {
                    archive.append_chat_events("agent-1", events).unwrap();
                }
            });
        }
    });
    let events = archive.chat_events_for_agent("agent-1").unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].metadata["request_root_id"], "ec1b3195");
}

#[cfg(windows)]
#[test]
fn failed_atomic_replacement_preserves_snapshot_and_retry_repairs_partial_publication() {
    use std::os::windows::fs::OpenOptionsExt;
    let (_guard, _temp) = isolate();
    for blocked in [
        "events.jsonl",
        "conversation.jsonl",
        "turns.jsonl",
        "manifest.json",
    ] {
        let case_home = tempfile::tempdir().unwrap();
        std::env::set_var("WARDIAN_HOME", case_home.path());
        let archive = ConversationArchiveState::default();
        let agent = "agent-1";
        let mut context = ConversationArchiveContext::for_agent_id(agent, "pi");
        context.provider_source_key = Some("pi:source:retained".into());
        let (legacy, current) = pi_capture();
        archive
            .append_chat_events_with_context(context.clone(), &legacy)
            .unwrap();
        let id = archive.active_conversation_id(agent).unwrap().unwrap();
        let dir = conversation_dir(agent, &id).unwrap();
        let before = std::fs::read(dir.join(blocked)).unwrap();
        let reader = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(1 | 2)
            .open(dir.join(blocked))
            .unwrap();
        archive
            .append_chat_events_with_context(context.clone(), &current)
            .expect_err("delete-sharing denied");
        assert_eq!(before, std::fs::read(dir.join(blocked)).unwrap());
        drop(reader);
        archive
            .append_chat_events_with_context(context.clone(), &current)
            .unwrap();
        let (_, records) = archive.show(&id).unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].request_root_id.as_deref(), Some("ec1b3195"));
        assert_eq!(
            archive
                .append_chat_events_with_context(context, &current)
                .unwrap(),
            0
        );
    }
}

#[test]
fn retained_real_agy_delivery_alias_exposes_native_source_without_duplicate_prompt() {
    let (_guard, _temp) = isolate();
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/real-agy-delivered.json")).unwrap();
    let events: Vec<AgentChatEvent> = serde_json::from_value(fixture["events"].clone()).unwrap();
    let record: ConversationNarrativeRecord =
        serde_json::from_value(fixture["record"].clone()).unwrap();
    let mut projected = events.clone();
    assert!(
        provenance::bind_delivered_inputs(&mut projected, std::slice::from_ref(&record)).unwrap()
    );
    assert_eq!(projected.len(), 1);
    assert_eq!(projected[0].id, events[0].id);
    assert_eq!(projected[0].metadata["provider_log"], true);
    assert_eq!(
        projected[0].source.as_deref(),
        Some("conversation_database")
    );
    assert_eq!(
        projected[0].metadata["log_path"],
        events[1].metadata["log_path"]
    );
    assert_eq!(projected[0].metadata["input_origin"], "human_input");
    assert_eq!(projected[0].metadata["request_root_id"], "wardian:input:1");
    assert_eq!(
        provenance::merge_current_capture(vec![events[1].clone()], projected)
            .unwrap()
            .len(),
        1
    );

    // Fresh capture follows the existing broker-delivery path. The source
    // metadata must survive the first reconcile, replay, and repeated capture.
    let archive = ConversationArchiveState::default();
    let context = ConversationArchiveContext::for_agent_id("agent-1", "antigravity");
    archive
        .append_delivered_input_with_context(
            context.clone(),
            events[0].text.as_deref().unwrap(),
            None,
        )
        .unwrap();
    archive
        .append_chat_events_with_context(context.clone(), &[events[1].clone()])
        .unwrap();
    let id = archive.active_conversation_id("agent-1").unwrap().unwrap();
    let dir = conversation_dir("agent-1", &id).unwrap();
    let persisted: Vec<AgentChatEvent> = read_jsonl_records(&dir.join("events.jsonl")).unwrap();
    assert_eq!(persisted.len(), 1);
    assert_eq!(persisted[0].metadata["provider_log"], true);
    assert_eq!(
        archive
            .append_chat_events_with_context(context, &[events[1].clone()])
            .unwrap(),
        0
    );
    assert_eq!(archive.chat_events_for_agent("agent-1").unwrap().len(), 1);
    // A delivered row alone is not native evidence.
    let mut no_native = vec![events[0].clone()];
    assert!(!provenance::bind_delivered_inputs(&mut no_native, &[record]).unwrap());
    assert!(no_native[0].metadata.get("provider_log").is_none());
}

#[test]
fn native_context_cannot_claim_delivered_input_by_equal_text() {
    let (_guard, _temp) = isolate();
    let archive = ConversationArchiveState::default();
    archive
        .append_delivered_input("agent-1", "same prompt", Some("peer"))
        .unwrap();
    let mut native = event("internal", "antigravity", None);
    native.metadata["input_origin"] = serde_json::json!("provider_internal");
    native.metadata["input_purpose"] = serde_json::json!("internal");
    native.metadata["provider_step_source"] = serde_json::json!(2);
    archive.append_chat_events("agent-1", &[native]).unwrap();
    let id = archive.active_conversation_id("agent-1").unwrap().unwrap();
    let (_, records) = archive.show(&id).unwrap();
    assert_eq!(records.len(), 2);
    assert_eq!(
        records[0].input_origin,
        Some(wardian_core::conversations::ConversationInputOrigin::AgentInput)
    );
    assert!(records[0]
        .event_refs
        .iter()
        .all(|id| id.starts_with("generated:")));
    assert_eq!(
        records[1].input_origin,
        Some(wardian_core::conversations::ConversationInputOrigin::ProviderInternal)
    );
}

#[test]
fn foreign_agent_capture_is_rejected_before_creating_archive() {
    let (_guard, _temp) = isolate();
    let archive = ConversationArchiveState::default();
    let mut foreign = event("foreign", "pi", Some("root"));
    foreign.session_id = "different-agent".into();
    assert_eq!(
        archive
            .append_chat_events("agent-1", &[foreign])
            .unwrap_err()
            .kind(),
        io::ErrorKind::InvalidInput
    );
    assert!(archive.list(Some("agent-1"), false).unwrap().is_empty());
}

#[test]
fn archive_only_roles_and_tail_replay_order_preserve_claude_contract() {
    let mut old = event("old", "claude", None);
    old.metadata["input_origin"] = serde_json::json!("context_injection");
    old.sequence = Some(200);
    let mut current = event("new", "claude", None);
    current.sequence = Some(1);
    let view = provenance::merge_current_capture(vec![current], vec![old]).unwrap();
    assert_eq!(view[0].role, Some(AgentChatRole::System));
    assert_eq!(view[0].sequence, Some(1));
    assert_eq!(view[1].sequence, Some(2));
    assert_eq!(view[0].id, "old");
    assert_eq!(view[1].id, "new");
}
