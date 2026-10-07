use super::*;
use crate::providers::chat_transcript::normalize_chat_lines;

fn context() -> ConversationArchiveContext {
    let mut context = ConversationArchiveContext::for_agent_id("agent-1", "codex");
    context.provider_source_key = Some("codex:one".into());
    context.provider_session_ids = vec!["one".into()];
    context
}

fn pair(path: &str, epoch: &str) -> Vec<AgentChatEvent> {
    let mut events = normalize_chat_lines(
        "agent-1",
        "codex",
        include_str!("../../providers/fixtures/codex-user-input-mirror.jsonl").lines(),
    );
    for (index, event) in events.iter_mut().enumerate() {
        let start = 100 + index as u64 * 100;
        event.id = format!("source:agent-1:{epoch}:{start}:{}:0", "c".repeat(64));
        event.metadata["provider_log"] = json!(true);
        event.metadata["log_path"] = json!(path);
        event.metadata["provider_session_id"] = json!("one");
        event.metadata["chat_source_ref"] = json!(event.id);
        event.metadata["chat_source_epoch"] = json!(epoch);
        event.metadata["chat_source_start"] = json!(start);
        event.metadata["chat_source_end"] = json!(start + 50);
    }
    events
}

#[test]
fn narrative_cross_checkpoint_restart_and_replay_preserve_physical_members() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp
        .path()
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .to_string();
    let context = context();
    let epoch = "a".repeat(64);
    let admission = Admission {
        context: &context,
        conversation: Some("conversation"),
        epoch: &epoch,
        admission: "policy",
        path: &path,
        watermark: 1000,
        sequence_trusted: true,
    };
    let events = pair(&path, &epoch);
    let before = events.clone();
    let mut index = Index::default();
    let mut store = Store::writer(&temp.path().join("objects"));
    assert!(index
        .observe(&mut store, &admission, &events[0], "request")
        .unwrap()
        .iter()
        .all(|(_, r)| r.is_none()));
    let saved = store.put(&index).unwrap();
    drop(store);
    let mut store = Store::writer(&temp.path().join("objects"));
    let mut index: Index = store.read(&saved).unwrap();
    let updates = index
        .observe(&mut store, &admission, &events[1], "mirror")
        .unwrap();
    assert_eq!(updates.len(), 2);
    let relation = updates[0].1.clone().unwrap();
    assert_eq!(relation.members.len(), 2);
    assert!(updates.iter().all(|(_, r)| r.as_ref() == Some(&relation)));
    assert!(index
        .observe(&mut store, &admission, &events[0], "request")
        .unwrap()
        .iter()
        .all(|(_, r)| r.as_ref() == Some(&relation)));
    assert_eq!(events, before);
}

#[test]
fn repeated_native_occurrence_retracts_relation_without_losing_usable_rows() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp
        .path()
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .to_string();
    let context = context();
    let epoch = "a".repeat(64);
    let admission = Admission {
        context: &context,
        conversation: None,
        epoch: &epoch,
        admission: "policy",
        path: &path,
        watermark: 1000,
        sequence_trusted: false,
    };
    let events = pair(&path, &epoch);
    let mut index = Index::default();
    let mut store = Store::writer(&temp.path().join("objects"));
    index
        .observe(&mut store, &admission, &events[0], "request")
        .unwrap();
    index
        .observe(&mut store, &admission, &events[1], "mirror")
        .unwrap();
    let mut repeated = events[1].clone();
    repeated.id = format!("source:agent-1:{epoch}:400:{}:0", "d".repeat(64));
    repeated.metadata["chat_source_ref"] = json!(repeated.id);
    repeated.metadata["chat_source_start"] = json!(400);
    repeated.metadata["chat_source_end"] = json!(450);
    let updates = index
        .observe(&mut store, &admission, &repeated, "repeated")
        .unwrap();
    assert_eq!(updates.len(), 3);
    assert!(updates.iter().all(|(_, relation)| relation.is_none()));
}

#[test]
fn exact_original_sequence_bridge_requires_native_and_legacy_one_to_one() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp
        .path()
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .to_string();
    let context = context();
    let epoch = "a".repeat(64);
    let admission = Admission {
        context: &context,
        conversation: None,
        epoch: &epoch,
        admission: "policy",
        path: &path,
        watermark: 1000,
        sequence_trusted: true,
    };
    let mut native = pair(&path, &epoch).remove(0);
    native.metadata["chat_compatibility_legacy_id"] = json!("original-algorithm-id");
    native.metadata["chat_compatibility_source_sequence"] = json!(7);
    let mut legacy = native.clone();
    legacy.id = "original-algorithm-id".into();
    legacy.metadata.as_object_mut().unwrap().retain(|key, _| {
        !key.starts_with("chat_source_") && !key.starts_with("chat_compatibility_")
    });
    legacy.metadata["chat_legacy_source_sequence"] = json!(7);
    let mut index = Index::default();
    let mut store = Store::writer(&temp.path().join("objects"));
    index
        .observe(&mut store, &admission, &legacy, "old")
        .unwrap();
    index
        .observe(&mut store, &admission, &native, "native")
        .unwrap();
    index.qualify_legacy(false);
    let positive = index.activate(&mut store, 8).unwrap();
    assert!(positive.iter().any(|(_, r)| r
        .as_ref()
        .is_some_and(|r| r.id == legacy.id && r.members.contains(&native.id))));
    let repeated = index
        .observe(
            &mut store,
            &admission,
            &legacy,
            "different-original-envelope",
        )
        .unwrap();
    assert!(repeated.iter().all(|(_, r)| r.is_none()));
    assert_eq!(native.metadata["chat_source_ref"], json!(native.id));
}

#[test]
fn unsequenced_bridge_waits_for_explicit_complete_prefix_not_page_or_tail() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp
        .path()
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .to_string();
    let context = context();
    let epoch = "a".repeat(64);
    let admission = Admission {
        context: &context,
        conversation: None,
        epoch: &epoch,
        admission: "policy",
        path: &path,
        watermark: 1000,
        sequence_trusted: false,
    };
    let mut native = pair(&path, &epoch).remove(0);
    native.metadata["chat_compatibility_legacy_id"] = json!("old-id");
    let mut legacy = native.clone();
    legacy.id = "old-id".into();
    legacy.metadata.as_object_mut().unwrap().retain(|key, _| {
        !key.starts_with("chat_source_") && !key.starts_with("chat_compatibility_")
    });
    let mut index = Index::default();
    let mut store = Store::writer(&temp.path().join("objects"));
    index
        .observe(&mut store, &admission, &native, "native")
        .unwrap();
    index
        .observe(&mut store, &admission, &legacy, "legacy")
        .unwrap();
    index.qualify_legacy(false);
    assert!(index
        .activate(&mut store, 8)
        .unwrap()
        .iter()
        .all(|(_, r)| r.is_none()));
    index.qualify_legacy(true);
    assert!(index
        .activate(&mut store, 8)
        .unwrap()
        .iter()
        .any(|(_, r)| r.is_some()));
}

#[test]
fn foreign_generated_and_retired_epoch_cannot_enter_relation_counts() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp
        .path()
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .to_string();
    let context = context();
    let epoch = "a".repeat(64);
    let admission = Admission {
        context: &context,
        conversation: None,
        epoch: &epoch,
        admission: "policy",
        path: &path,
        watermark: 1000,
        sequence_trusted: true,
    };
    let events = pair(&path, &epoch);
    for key in [
        "generated",
        "provider_session_id",
        "chat_source_epoch",
        "log_path",
    ] {
        let mut wrong = events[1].clone();
        wrong.metadata[key] = if key == "generated" {
            json!(true)
        } else {
            json!("foreign")
        };
        let mut index = Index::default();
        let mut store = Store::writer(&temp.path().join(key));
        index
            .observe(&mut store, &admission, &events[0], "request")
            .unwrap();
        assert!(index
            .observe(&mut store, &admission, &wrong, "wrong")
            .unwrap()
            .is_empty());
        assert!(index
            .observe(&mut store, &admission, &events[0], "request")
            .unwrap()
            .iter()
            .all(|(_, r)| r.is_none()));
    }
}

#[test]
fn deferred_narrative_activation_is_bounded_persisted_and_rejects_partial_watermark() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp
        .path()
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .to_string();
    let context = context();
    let epoch = "a".repeat(64);
    let admission = Admission {
        context: &context,
        conversation: None,
        epoch: &epoch,
        admission: "policy",
        path: &path,
        watermark: 1000,
        sequence_trusted: false,
    };
    let mut index = Index::default();
    index.defer_narratives();
    let mut store = Store::writer(&temp.path().join("objects"));
    let events = pair(&path, &epoch);
    for (i, event) in events.iter().enumerate() {
        assert!(index
            .observe(&mut store, &admission, event, &i.to_string())
            .unwrap()
            .iter()
            .all(|(_, relation)| relation.is_none()));
    }
    index.qualify_narratives(500);
    assert!(index
        .activate_narratives(&mut store, 1)
        .unwrap()
        .iter()
        .all(|(_, relation)| relation.is_none()));
    index.qualify_narratives(1000);
    let saved = store.put(&index).unwrap();
    drop(store);
    let mut store = Store::writer(&temp.path().join("objects"));
    let mut index: Index = store.read(&saved).unwrap();
    assert!(index.narrative_pending());
    let updates = index.activate_narratives(&mut store, 1).unwrap();
    assert_eq!(updates.len(), 2);
    assert!(updates
        .iter()
        .all(|(_, relation)| relation.as_ref().is_some_and(|r| r.members.len() == 2)));
    // The exact-limit checkpoint persists its cursor; the next bounded pass
    // proves exhaustion without replaying the native source or row history.
    let saved = store.put(&index).unwrap();
    let mut index: Index = store.read(&saved).unwrap();
    assert!(index.activate_narratives(&mut store, 1).unwrap().is_empty());
    assert!(!index.narrative_pending());
}

#[test]
fn pending_sequence_counts_activate_later_without_member_evidence_conflict() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp
        .path()
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .to_string();
    let context = context();
    let epoch = "a".repeat(64);
    let admission = Admission {
        context: &context,
        conversation: None,
        epoch: &epoch,
        admission: "policy",
        path: &path,
        watermark: 1000,
        sequence_trusted: false,
    };
    let mut native = pair(&path, &epoch).remove(0);
    native.metadata["chat_compatibility_legacy_id"] = json!("old-id");
    native.metadata["chat_compatibility_source_sequence"] = json!(7);
    let mut legacy = native.clone();
    legacy.id = "old-id".into();
    legacy.metadata.as_object_mut().unwrap().retain(|key, _| {
        !key.starts_with("chat_source_") && !key.starts_with("chat_compatibility_")
    });
    legacy.metadata["chat_legacy_source_sequence"] = json!(7);
    let mut index = Index::default();
    let mut store = Store::writer(&temp.path().join("objects"));
    index
        .observe(&mut store, &admission, &native, "native")
        .unwrap();
    index
        .observe(&mut store, &admission, &legacy, "legacy")
        .unwrap();
    index.qualify_legacy(false);
    assert!(index
        .activate(&mut store, 8)
        .unwrap()
        .iter()
        .all(|(_, relation)| relation.is_none()));
    let saved = store.put(&index).unwrap();
    let mut index: Index = store.read(&saved).unwrap();
    index.qualify_sequences(true);
    assert!(index
        .activate(&mut store, 8)
        .unwrap()
        .iter()
        .any(|(_, relation)| relation.as_ref().is_some_and(|r| r.id == legacy.id)));
}
