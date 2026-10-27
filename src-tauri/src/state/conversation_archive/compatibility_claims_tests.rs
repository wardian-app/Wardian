use super::*;
use crate::commands::{
    chat::decorate_forward_provider_log_events,
    provider_log_acquisition::{
        acquire_provider_log_batch, observe_provider_log_policy_with_identity,
    },
};
use wardian_core::models::chat::AgentChatRole;

#[test]
fn reservation_survives_restart_and_rejects_coordinate_ordinal_and_reverse_reuse() {
    let temp = tempfile::tempdir().unwrap();
    let mut store = Store::writer(temp.path());
    let mut root = load(&mut store, None).unwrap();
    let claim = Claim {
        scope: "scope".into(),
        canonical_id: "old".into(),
        coordinate: "physical:10:digest:0".into(),
        native_uuid: "native".into(),
        request_root_id: "root".into(),
        end: 20,
    };
    reserve(&mut store, &mut root, "owner", &claim).unwrap();
    let checkpoint = Checkpoint {
        version: 1,
        root: store.put(&root).unwrap(),
    };
    drop(store);
    let mut store = Store::writer(temp.path());
    let mut root = load(&mut store, Some(&checkpoint)).unwrap();
    reserve(&mut store, &mut root, "owner", &claim).unwrap();
    for coordinate in ["physical:21:digest:0", "physical:10:digest:1"] {
        let mut competing = claim.clone();
        competing.coordinate = coordinate.into();
        assert!(reserve(&mut store, &mut root, "owner", &competing).is_err());
    }
    let mut competing = claim.clone();
    competing.canonical_id = "other".into();
    assert!(reserve(&mut store, &mut root, "other-owner", &competing).is_err());
}

#[test]
fn legacy_absence_is_distinct_from_wrong_missing_corrupt_or_versioned_modern_roots() {
    let temp = tempfile::tempdir().unwrap();
    let mut store = Store::writer(temp.path());
    assert!(load(&mut store, None).is_ok());
    for checkpoint in [
        Checkpoint {
            version: 2,
            root: digest(b"missing"),
        },
        Checkpoint {
            version: 1,
            root: "invalid".into(),
        },
        Checkpoint {
            version: 1,
            root: digest(b"missing"),
        },
    ] {
        assert!(load(&mut store, Some(&checkpoint)).is_err());
    }
    let reference = store
        .put(&Root {
            version: 1,
            ..Root::default()
        })
        .unwrap();
    assert!(load(
        &mut store,
        Some(&Checkpoint {
            version: 1,
            root: reference.clone()
        })
    )
    .is_err());
    std::fs::write(temp.path().join(&reference), b"corrupt").unwrap();
    assert!(load(
        &mut store,
        Some(&Checkpoint {
            version: 1,
            root: reference
        })
    )
    .is_err());
}

fn fixture(
    temp: &std::path::Path,
) -> (
    ConversationArchiveState,
    ConversationArchiveContext,
    AgentChatEvent,
    crate::commands::provider_log_acquisition::ProviderLogBatch,
) {
    let path = temp.join("claude.jsonl");
    let first = "{\"type\":\"user\",\"uuid\":\"first\",\"message\":{\"role\":\"user\",\"content\":\"First\"}}\n";
    std::fs::write(&path, first).unwrap();
    let context = ConversationArchiveContext {
        agent_id: "agent-1".into(),
        agent_name: "Agent".into(),
        agent_class: "Coder".into(),
        workspace: temp.to_string_lossy().into(),
        provider: "claude".into(),
        provider_session_ids: vec!["native-session".into()],
        provider_source_key: Some("claude:session:native-session".into()),
    };
    let mut initial = acquire_provider_log_batch(
        "agent-1",
        "claude",
        &path,
        context.provider_source_key.as_ref().unwrap(),
        None,
        true,
    )
    .unwrap();
    decorate_forward_provider_log_events(&mut initial.events, "claude", &path);
    let state = ConversationArchiveState::default();
    state
        .append_provider_log_batch_with_context(
            context.clone(),
            &initial.events,
            initial.previous.as_ref(),
            &initial.next,
        )
        .unwrap();
    let second = "{\"type\":\"user\",\"uuid\":\"second\",\"message\":{\"role\":\"user\",\"content\":\"Second\"}}\n";
    std::fs::write(&path, format!("{first}{second}")).unwrap();
    let mut batch = acquire_provider_log_batch(
        "agent-1",
        "claude",
        &path,
        context.provider_source_key.as_ref().unwrap(),
        Some(initial.next),
        true,
    )
    .unwrap();
    let mut old = batch.events[0].clone();
    for key in [
        "chat_source_ref",
        "chat_source_start",
        "chat_source_end",
        "chat_source_epoch",
    ] {
        old.metadata.as_object_mut().unwrap().remove(key);
    }
    decorate_forward_provider_log_events(std::slice::from_mut(&mut old), "claude", &path);
    state
        .append_chat_events_with_context(context.clone(), std::slice::from_ref(&old))
        .unwrap();
    decorate_forward_provider_log_events(&mut batch.events, "claude", &path);
    (state, context, old, batch)
}

fn policy_then_acquire(
    state: &ConversationArchiveState,
    context: &ConversationArchiveContext,
    path: &std::path::Path,
) -> crate::commands::provider_log_acquisition::ProviderLogBatch {
    let key = context.provider_source_key.as_ref().unwrap();
    let previous = state
        .provider_log_capture_state(&context.agent_id, key)
        .unwrap();
    let policy = observe_provider_log_policy_with_identity(path, key, previous, true, true, None)
        .unwrap()
        .unwrap();
    assert_eq!(policy.next.status, "pending");
    assert!(policy.next.reason.is_none());
    state
        .append_provider_log_batch_with_context(
            context.clone(),
            &[],
            policy.previous.as_ref(),
            &policy.next,
        )
        .unwrap();
    let mut batch = acquire_provider_log_batch(
        &context.agent_id,
        "claude",
        path,
        key,
        Some(policy.next),
        true,
    )
    .unwrap();
    decorate_forward_provider_log_events(&mut batch.events, "claude", path);
    batch
}

fn assert_canonical_alias(coordinate: &str, canonical: &str) {
    let (head, objects) = chat_read::locations("agent-1").unwrap();
    let reference: String = serde_json::from_slice(&std::fs::read(head).unwrap()).unwrap();
    let mut store = Store::new(&objects);
    let head: chat_read::Head = store.read(&reference).unwrap();
    assert_eq!(
        chat_read::alias(&mut store, &head, coordinate)
            .unwrap()
            .unwrap()
            .id,
        canonical
    );
}

#[test]
fn reused_seek_nodes_never_cache_missing_corrupt_or_malformed_objects() {
    let temp = tempfile::tempdir().unwrap();
    let mut store = Store::writer(temp.path());
    let malformed = store.put_bytes(b"not-json").unwrap();
    let corrupt = store.put_bytes(b"original").unwrap();
    std::fs::write(temp.path().join(&corrupt), b"changed").unwrap();
    let invalid_child = store.put(&serde_json::json!({
        "key": "middle", "value": digest(b"value"), "left": "invalid", "right": null, "height": 1,
    })).unwrap();
    for reference in [digest(b"missing"), malformed, corrupt, invalid_child] {
        let mut cache = super::super::chat_read_store::SeekCache::default();
        for _ in 0..2 {
            assert!(store
                .get_reusing_nodes(&Some(reference.clone()), "before", &mut cache)
                .is_err());
        }
    }
}

#[tokio::test]
async fn completed_claim_publication_progresses_past_8193_unclaimed_legacy_rows() {
    let _guard = crate::utils::wardian_test_env_lock_async().await;
    let previous_home = std::env::var_os("WARDIAN_HOME");
    let temp = tempfile::tempdir().unwrap();
    std::env::set_var("WARDIAN_HOME", temp.path());
    let (state, context, old, batch) = fixture(temp.path());
    state
        .append_provider_log_batch_with_context(
            context.clone(),
            &batch.events,
            batch.previous.as_ref(),
            &batch.next,
        )
        .unwrap();
    let conversation = state.active_conversation_id_for_test("agent-1").unwrap();
    let directory = conversation_dir("agent-1", &conversation).unwrap();
    let before = std::fs::read(directory.join("events.jsonl")).unwrap();
    let mut events: Vec<AgentChatEvent> =
        read_jsonl_records(&directory.join("events.jsonl")).unwrap();
    for index in 0..8193 {
        let mut unclaimed = old.clone();
        unclaimed.id = format!("unclaimed-{index}");
        events.push(unclaimed);
    }
    state
        .publish_chat_candidate(chat_read::Candidate {
            context,
            conversation_id: conversation,
            events,
            committed_output_bytes: before.len() as u64,
            verified_ids: HashSet::new(),
            generated_ids: HashSet::new(),
            generated_input_bindings: HashMap::new(),
            source_epoch: None,
        })
        .unwrap();
    assert_canonical_alias(
        batch.events[0].metadata["chat_source_ref"]
            .as_str()
            .unwrap(),
        &old.id,
    );
    let (head_path, objects) = chat_read::locations("agent-1").unwrap();
    let reference: String = serde_json::from_slice(&std::fs::read(head_path).unwrap()).unwrap();
    let mut store = Store::new(&objects);
    let head: serde_json::Value = store.read(&reference).unwrap();
    assert_eq!(head["row_count"], 2);
    assert_eq!(
        std::fs::read(directory.join("events.jsonl")).unwrap(),
        before
    );
    match previous_home {
        Some(value) => std::env::set_var("WARDIAN_HOME", value),
        None => std::env::remove_var("WARDIAN_HOME"),
    }
}

#[tokio::test]
async fn ordinary_policy_bridge_survives_native_generated_and_restart_rebuilds() {
    let _guard = crate::utils::wardian_test_env_lock_async().await;
    let previous_home = std::env::var_os("WARDIAN_HOME");
    let temp = tempfile::tempdir().unwrap();
    std::env::set_var("WARDIAN_HOME", temp.path());
    let (state, context, old, _) = fixture(temp.path());
    let conversation = state.active_conversation_id_for_test("agent-1").unwrap();
    let directory = conversation_dir("agent-1", &conversation).unwrap();
    let events_before = std::fs::read(directory.join("events.jsonl")).unwrap();
    let records_before = std::fs::read(directory.join("conversation.jsonl")).unwrap();
    let path = temp.path().join("claude.jsonl");
    let batch = policy_then_acquire(&state, &context, &path);
    assert_eq!(batch.previous.as_ref().unwrap().status, "pending");
    state
        .append_provider_log_batch_with_context(
            context.clone(),
            &batch.events,
            batch.previous.as_ref(),
            &batch.next,
        )
        .unwrap();
    assert_eq!(
        std::fs::read(directory.join("events.jsonl")).unwrap(),
        events_before
    );
    assert_eq!(
        std::fs::read(directory.join("conversation.jsonl")).unwrap(),
        records_before
    );
    let coordinate = batch.events[0].metadata["chat_source_ref"]
        .as_str()
        .unwrap();
    assert_canonical_alias(coordinate, &old.id);
    assert!(!pending(&read_capture_state("agent-1").unwrap(), "agent-1").unwrap());

    use std::io::Write;
    std::fs::OpenOptions::new().append(true).open(&path).unwrap().write_all(
        b"{\"type\":\"user\",\"uuid\":\"third\",\"message\":{\"role\":\"user\",\"content\":\"Third\"}}\n",
    ).unwrap();
    let next = policy_then_acquire(&state, &context, &path);
    state
        .append_provider_log_batch_with_context(
            context.clone(),
            &next.events,
            next.previous.as_ref(),
            &next.next,
        )
        .unwrap();
    assert_canonical_alias(coordinate, &old.id);
    state
        .append_delivered_input_with_context(context.clone(), "Generated input", None)
        .unwrap();
    assert_canonical_alias(coordinate, &old.id);
    drop(state);
    let restarted = ConversationArchiveState::default();
    restarted
        .append_delivered_input_with_context(context.clone(), "After restart", None)
        .unwrap();
    assert_canonical_alias(coordinate, &old.id);
    assert!(std::fs::read(directory.join("events.jsonl"))
        .unwrap()
        .starts_with(&events_before));
    assert!(std::fs::read(directory.join("conversation.jsonl"))
        .unwrap()
        .starts_with(&records_before));
    match previous_home {
        Some(value) => std::env::set_var("WARDIAN_HOME", value),
        None => std::env::remove_var("WARDIAN_HOME"),
    }
}

#[tokio::test]
async fn prepared_claim_failures_keep_old_cursor_and_history_then_restart_recovers() {
    let _guard = crate::utils::wardian_test_env_lock_async().await;
    let previous_home = std::env::var_os("WARDIAN_HOME");
    for stage in [1, 2, 3, 4] {
        let temp = tempfile::tempdir().unwrap();
        std::env::set_var("WARDIAN_HOME", temp.path());
        let (state, context, old, batch) = fixture(temp.path());
        let conversation = state.active_conversation_id_for_test("agent-1").unwrap();
        let directory = conversation_dir("agent-1", &conversation).unwrap();
        let events_before = std::fs::read(directory.join("events.jsonl")).unwrap();
        let records_before = std::fs::read(directory.join("conversation.jsonl")).unwrap();
        if stage == 3 {
            state
                .fail_next_chat_cursor_commit
                .store(true, Ordering::SeqCst);
        } else {
            state
                .fail_compatibility_stage
                .store(stage, Ordering::SeqCst);
        }
        assert!(state
            .append_provider_log_batch_with_context(
                context.clone(),
                &batch.events,
                batch.previous.as_ref(),
                &batch.next
            )
            .is_err());
        let captured = read_capture_state("agent-1").unwrap();
        assert_eq!(
            captured.provider_log_sources[0].committed_offset,
            if stage == 4 {
                batch.next.committed_offset
            } else {
                batch.previous.as_ref().unwrap().committed_offset
            }
        );
        assert_eq!(captured.compatibility_claims.is_some(), stage != 1);
        drop(state);
        let restarted = ConversationArchiveState::default();
        if stage == 4 {
            restarted
                .append_provider_log_batch_with_context(
                    context.clone(),
                    &[],
                    Some(&batch.next),
                    &batch.next,
                )
                .unwrap();
        } else {
            let retry =
                policy_then_acquire(&restarted, &context, &temp.path().join("claude.jsonl"));
            restarted
                .append_provider_log_batch_with_context(
                    context.clone(),
                    &retry.events,
                    retry.previous.as_ref(),
                    &retry.next,
                )
                .unwrap();
        }
        assert_eq!(
            std::fs::read(directory.join("events.jsonl")).unwrap(),
            events_before
        );
        assert_eq!(
            std::fs::read(directory.join("conversation.jsonl")).unwrap(),
            records_before
        );
        assert!(!pending(&read_capture_state("agent-1").unwrap(), "agent-1").unwrap());
        let (head, objects) = chat_read::locations("agent-1").unwrap();
        let reference: String = serde_json::from_slice(&std::fs::read(head).unwrap()).unwrap();
        let mut store = Store::new(&objects);
        let head: chat_read::Head = store.read(&reference).unwrap();
        let row = chat_read::alias(
            &mut store,
            &head,
            batch.events[0].metadata["chat_source_ref"]
                .as_str()
                .unwrap(),
        )
        .unwrap()
        .unwrap();
        assert_eq!(row.id, old.id);
    }
    match previous_home {
        Some(value) => std::env::set_var("WARDIAN_HOME", value),
        None => std::env::remove_var("WARDIAN_HOME"),
    }
}

#[tokio::test]
async fn foreign_generated_missing_proof_privacy_and_nonunique_owners_never_bridge() {
    let _guard = crate::utils::wardian_test_env_lock_async().await;
    let temp = tempfile::tempdir().unwrap();
    let previous_home = std::env::var_os("WARDIAN_HOME");
    std::env::set_var("WARDIAN_HOME", temp.path());
    let (state, context, old, batch) = fixture(temp.path());
    let mut bad = old.clone();
    bad.metadata["generated"] = serde_json::json!(true);
    assert!(!eligible(
        &bad,
        &batch.events[0],
        &batch.next,
        &context.provider_session_ids
    ));
    for key in ["session_id", "provider", "uuid", "path", "native-session"] {
        let mut bad = old.clone();
        match key {
            "session_id" => bad.session_id = "foreign".into(),
            "provider" => bad.provider = "codex".into(),
            "uuid" => bad.metadata["request_root_id"] = serde_json::json!("foreign"),
            "path" => bad.metadata["log_path"] = serde_json::json!("foreign"),
            _ => bad.metadata["provider_session_id"] = serde_json::json!("foreign"),
        }
        assert!(!eligible(
            &bad,
            &batch.events[0],
            &batch.next,
            &context.provider_session_ids
        ));
    }
    for replacement in [0, 1, 2] {
        let mut source = batch.next.clone();
        match replacement {
            0 => source.native_identity.primary += 1,
            1 => source.unknown_before_offset = Some(source.committed_offset),
            _ => source.disabled_spans.push(
                crate::commands::provider_log_acquisition::ProviderLogDisabledSpan {
                    start: 0,
                    end: source.committed_offset,
                },
            ),
        }
        assert!(!admitted(&batch.events[0], &source));
    }
    let conversation = state.active_conversation_id_for_test("agent-1").unwrap();
    let directory = conversation_dir("agent-1", &conversation).unwrap();
    let manifest = read_manifest(&directory.join("manifest.json")).unwrap();
    let records: Vec<ConversationNarrativeRecord> =
        read_jsonl_records(&directory.join("conversation.jsonl")).unwrap();
    let mut checkpoint = read_capture_state("agent-1").unwrap();
    let (remaining, overlay) = prepare(
        Preparation {
            context: &context,
            conversation_id: &conversation,
            manifest: manifest.as_ref(),
            archived: std::slice::from_ref(&old),
            records: &records,
            events: &batch.events,
            previous: None,
            next: &batch.next,
        },
        &mut checkpoint,
    )
    .unwrap();
    assert_eq!(remaining.len(), 1);
    assert!(overlay.is_empty());
    assert!(prepare(
        Preparation {
            context: &context,
            conversation_id: &conversation,
            manifest: manifest.as_ref(),
            archived: &[old.clone(), old.clone()],
            records: &records,
            events: &batch.events,
            previous: batch.previous.as_ref(),
            next: &batch.next
        },
        &mut checkpoint,
    )
    .is_err());
    // A different role also cannot attach to a genuine native UUID.
    let mut different_role = batch.events[0].clone();
    different_role.role = Some(AgentChatRole::Assistant);
    assert!(!eligible(
        &old,
        &different_role,
        &batch.next,
        &context.provider_session_ids
    ));
    match previous_home {
        Some(value) => std::env::set_var("WARDIAN_HOME", value),
        None => std::env::remove_var("WARDIAN_HOME"),
    }
}
