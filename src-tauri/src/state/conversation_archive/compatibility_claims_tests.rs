use super::*;
use crate::commands::{
    chat::decorate_forward_provider_log_events,
    provider_log_acquisition::{
        acquire_provider_log_batch, observe_provider_log_policy_with_identity,
    },
};
use wardian_core::models::chat::AgentChatRole;

#[test]
fn codex_source_receipt_rejects_ambiguous_foreign_generated_or_modified_legacy_owners() {
    for scenario in [
        "missing-proof",
        "duplicate-owner",
        "duplicate-current",
        "native-identity",
        "epoch",
        "generated",
        "session",
        "metadata",
        "body",
        "source",
        "ordinal",
        "different-byte",
    ] {
        let (_guard, temp) = super::super::tests::isolated_home();
        let path = temp.path().join("codex.jsonl");
        std::fs::write(&path, "").unwrap();
        let context = ConversationArchiveContext {
            agent_id: "agent-1".into(),
            agent_name: "Synthetic Codex".into(),
            agent_class: "Coder".into(),
            workspace: "<absolute-workspace-path>".into(),
            provider: "codex".into(),
            provider_session_ids: vec!["native-session".into()],
            provider_source_key: Some("codex:session:native-session".into()),
        };
        let initial = acquire_provider_log_batch(
            "agent-1",
            "codex",
            &path,
            context.provider_source_key.as_ref().unwrap(),
            None,
            true,
        )
        .unwrap();
        let state = ConversationArchiveState::default();
        state
            .append_provider_log_batch_with_context(context.clone(), &[], None, &initial.next)
            .unwrap();
        let raw = r#"{"type":"response_item","turn_id":"turn-1","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"A source-qualified answer"}]}}"#;
        std::fs::write(&path, format!("{raw}\n")).unwrap();
        let mut batch = acquire_provider_log_batch(
            "agent-1",
            "codex",
            &path,
            context.provider_source_key.as_ref().unwrap(),
            Some(initial.next.clone()),
            true,
        )
        .unwrap();
        decorate_forward_provider_log_events(&mut batch.events, "codex", &path);
        let mut old = batch.events[0].clone();
        old.id = crate::commands::chat::archive_identity::legacy_provider_log_event_id(&old, &path);
        for key in [
            "chat_source_ref",
            "chat_source_start",
            "chat_source_end",
            "chat_source_epoch",
            "provider_log_row_offset",
            "legacy_event_ids",
        ] {
            old.metadata.as_object_mut().unwrap().remove(key);
        }
        state
            .append_chat_events_with_context(context.clone(), std::slice::from_ref(&old))
            .unwrap();
        let conversation = state.active_conversation_id_for_test("agent-1").unwrap();
        let directory = conversation_dir("agent-1", &conversation).unwrap();
        let manifest = read_manifest(&directory.join("manifest.json")).unwrap();
        let records: Vec<ConversationNarrativeRecord> =
            read_jsonl_records(&directory.join("conversation.jsonl")).unwrap();
        let before_archive = std::fs::read(directory.join("events.jsonl")).unwrap();
        let mut archived = vec![old.clone()];
        let mut test_context = context.clone();
        if scenario == "different-byte" {
            std::fs::write(&path, format!("{{\"type\":\"ignored\"}}\n{raw}\n")).unwrap();
            batch = acquire_provider_log_batch(
                "agent-1",
                "codex",
                &path,
                context.provider_source_key.as_ref().unwrap(),
                Some(initial.next.clone()),
                true,
            )
            .unwrap();
            decorate_forward_provider_log_events(&mut batch.events, "codex", &path);
            assert_ne!(batch.events[0].metadata["chat_source_start"], 0);
        }
        batch.events[0].metadata["legacy_event_ids"] = serde_json::json!([old.id]);
        match scenario {
            "duplicate-owner" => archived.push(old.clone()),
            "duplicate-current" => batch.events.push(batch.events[0].clone()),
            "native-identity" => batch.next.native_identity.primary ^= 1,
            "epoch" => {
                batch.events[0].metadata["chat_source_epoch"] = serde_json::json!("foreign-epoch")
            }
            "generated" => archived[0].metadata["generated"] = serde_json::json!(true),
            "session" => {
                archived[0].metadata["provider_session_id"] = serde_json::json!("foreign-session")
            }
            "metadata" => {
                batch.events[0]
                    .metadata
                    .as_object_mut()
                    .unwrap()
                    .remove("chat_source_end");
            }
            "body" => batch.events[0].text = Some("Unobserved content".into()),
            "source" => test_context.provider_source_key = Some("codex:session:foreign".into()),
            "ordinal" => {
                let reference = batch.events[0].metadata["chat_source_ref"]
                    .as_str()
                    .unwrap()
                    .to_string();
                batch.events[0].metadata["chat_source_ref"] =
                    serde_json::json!(format!("{}1", &reference[..reference.len() - 1]));
            }
            _ => {}
        }
        let mut checkpoint = read_capture_state("agent-1").unwrap();
        assert!(
            prepare(
                Preparation {
                    context: &test_context,
                    conversation_id: &conversation,
                    manifest: manifest.as_ref(),
                    archived: &archived,
                    records: &records,
                    events: &batch.events,
                    previous: batch.previous.as_ref(),
                    next: &batch.next,
                    source_proof: if scenario == "missing-proof" {
                        None
                    } else {
                        batch.source_proof()
                    },
                },
                &mut checkpoint
            )
            .is_err(),
            "unqualified migration admitted: {scenario}"
        );
        assert_eq!(
            state
                .provider_log_capture_state(
                    "agent-1",
                    context.provider_source_key.as_ref().unwrap()
                )
                .unwrap(),
            Some(initial.next)
        );
        assert_eq!(
            std::fs::read(directory.join("events.jsonl")).unwrap(),
            before_archive
        );
    }
}

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
        codex_sequence: None,
        codex_owner_digest: None,
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
            source_proof: None,
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
            next: &batch.next,
            source_proof: None,
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

fn codex_fixture(
    temp: &std::path::Path,
) -> (
    ConversationArchiveState,
    ConversationArchiveContext,
    AgentChatEvent,
    crate::commands::provider_log_acquisition::ProviderLogBatch,
) {
    let raw = r#"{"type":"response_item","turn_id":"turn-1","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"A source-qualified answer"}]}}"#;
    codex_fixture_with_row(temp, raw)
}

fn codex_fixture_with_row(
    temp: &std::path::Path,
    raw: &str,
) -> (
    ConversationArchiveState,
    ConversationArchiveContext,
    AgentChatEvent,
    crate::commands::provider_log_acquisition::ProviderLogBatch,
) {
    let path = temp.join("codex.jsonl");
    std::fs::write(&path, "").unwrap();
    let context = ConversationArchiveContext {
        agent_id: "agent-1".into(),
        agent_name: "Synthetic Codex".into(),
        agent_class: "Coder".into(),
        workspace: "<absolute-workspace-path>".into(),
        provider: "codex".into(),
        provider_session_ids: vec!["native-session".into()],
        provider_source_key: Some("codex:session:native-session".into()),
    };
    let initial = acquire_provider_log_batch(
        "agent-1",
        "codex",
        &path,
        context.provider_source_key.as_ref().unwrap(),
        None,
        true,
    )
    .unwrap();
    let state = ConversationArchiveState::default();
    state
        .append_provider_log_batch_with_context(context.clone(), &[], None, &initial.next)
        .unwrap();
    std::fs::write(&path, format!("{raw}\n")).unwrap();
    let mut batch = acquire_provider_log_batch(
        "agent-1",
        "codex",
        &path,
        context.provider_source_key.as_ref().unwrap(),
        Some(initial.next),
        true,
    )
    .unwrap();
    decorate_forward_provider_log_events(&mut batch.events, "codex", &path);
    let mut old = batch.events[0].clone();
    old.id = crate::commands::chat::archive_identity::legacy_provider_log_event_id(&old, &path);
    for key in [
        "chat_source_ref",
        "chat_source_start",
        "chat_source_end",
        "chat_source_epoch",
        "provider_log_row_offset",
        "legacy_event_ids",
    ] {
        old.metadata.as_object_mut().unwrap().remove(key);
    }
    state
        .append_chat_events_with_context(context.clone(), std::slice::from_ref(&old))
        .unwrap();
    batch.events[0].metadata["legacy_event_ids"] = serde_json::json!([old.id]);
    (state, context, old, batch)
}

fn append_codex_row(path: &std::path::Path, turn: &str) {
    use std::io::Write;
    let row = serde_json::json!({
        "type": "response_item", "turn_id": turn,
        "payload": {"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": turn}]}
    });
    writeln!(
        std::fs::OpenOptions::new().append(true).open(path).unwrap(),
        "{row}"
    )
    .unwrap();
}

fn codex_restart_batch(
    state: &ConversationArchiveState,
    context: &ConversationArchiveContext,
    path: &std::path::Path,
) -> crate::commands::provider_log_acquisition::ProviderLogBatch {
    let key = context.provider_source_key.as_ref().unwrap();
    let mut batch = acquire_provider_log_batch(
        &context.agent_id,
        "codex",
        path,
        key,
        state
            .provider_log_capture_state(&context.agent_id, key)
            .unwrap(),
        true,
    )
    .unwrap();
    decorate_forward_provider_log_events(&mut batch.events, "codex", path);
    batch
}

#[test]
fn codex_committed_claim_recovers_at_eof_or_new_rows_after_repeated_postcursor_crashes() {
    for new_rows_before_recovery in [false, true] {
        let (_guard, temp) = super::super::tests::isolated_home();
        let (state, context, old, batch) = codex_fixture(temp.path());
        let path = temp.path().join("codex.jsonl");
        let conversation = state.active_conversation_id_for_test("agent-1").unwrap();
        let directory = conversation_dir("agent-1", &conversation).unwrap();
        state.fail_compatibility_stage.store(4, Ordering::SeqCst);
        let error = state
            .append_verified_provider_log_batch_with_context(
                context.clone(),
                &batch.events,
                batch.previous.as_ref(),
                &batch.next,
                batch.source_proof(),
            )
            .unwrap_err();
        assert!(error.to_string().contains("after cursor"));
        assert!(pending(&read_capture_state("agent-1").unwrap(), "agent-1").unwrap());
        assert_eq!(
            state
                .provider_log_capture_state(
                    "agent-1",
                    context.provider_source_key.as_ref().unwrap()
                )
                .unwrap(),
            Some(batch.next.clone())
        );
        let original_events: Vec<AgentChatEvent> =
            read_jsonl_records(&directory.join("events.jsonl")).unwrap();
        assert_eq!(original_events.len(), 1);
        let owner = &original_events[0];
        assert_eq!(owner.id, old.id);
        assert_eq!(owner.sequence, old.sequence);
        assert_eq!(owner.text, old.text);
        assert_eq!(
            owner.metadata["chat_source_ref"],
            batch.events[0].metadata["chat_source_ref"]
        );
        drop(state);
        if new_rows_before_recovery {
            append_codex_row(&path, "turn-2");
        }
        let restarted = ConversationArchiveState::default();
        let retry = codex_restart_batch(&restarted, &context, &path);
        assert_eq!(retry.source_proof().is_some(), new_rows_before_recovery);
        assert_eq!(retry.events.len(), usize::from(new_rows_before_recovery));
        restarted
            .fail_compatibility_stage
            .store(4, Ordering::SeqCst);
        let error = restarted
            .append_verified_provider_log_batch_with_context(
                context.clone(),
                &retry.events,
                retry.previous.as_ref(),
                &retry.next,
                retry.source_proof(),
            )
            .unwrap_err();
        assert!(
            error.to_string().contains("after cursor"),
            "recovery failed before cursor: {error}"
        );
        assert!(pending(&read_capture_state("agent-1").unwrap(), "agent-1").unwrap());
        drop(restarted);
        let restarted = ConversationArchiveState::default();
        let eof = codex_restart_batch(&restarted, &context, &path);
        assert!(eof.events.is_empty());
        assert!(eof.source_proof().is_none());
        restarted
            .append_verified_provider_log_batch_with_context(
                context.clone(),
                &eof.events,
                eof.previous.as_ref(),
                &eof.next,
                eof.source_proof(),
            )
            .unwrap();
        assert!(!pending(&read_capture_state("agent-1").unwrap(), "agent-1").unwrap());
        let before_replay = std::fs::read(directory.join("events.jsonl")).unwrap();
        let repeated_eof = codex_restart_batch(&restarted, &context, &path);
        restarted
            .append_verified_provider_log_batch_with_context(
                context.clone(),
                &repeated_eof.events,
                repeated_eof.previous.as_ref(),
                &repeated_eof.next,
                repeated_eof.source_proof(),
            )
            .unwrap();
        assert_eq!(
            std::fs::read(directory.join("events.jsonl")).unwrap(),
            before_replay
        );
        append_codex_row(&path, "turn-3");
        let next = codex_restart_batch(&restarted, &context, &path);
        restarted
            .append_verified_provider_log_batch_with_context(
                context,
                &next.events,
                next.previous.as_ref(),
                &next.next,
                next.source_proof(),
            )
            .unwrap();
        let events: Vec<AgentChatEvent> =
            read_jsonl_records(&directory.join("events.jsonl")).unwrap();
        assert_eq!(events.len(), 2 + usize::from(new_rows_before_recovery));
        assert_eq!(events.iter().filter(|event| event.id == old.id).count(), 1);
        assert_eq!(
            events
                .iter()
                .filter(
                    |event| event.metadata["chat_source_ref"] == owner.metadata["chat_source_ref"]
                )
                .count(),
            1
        );
        assert_eq!(&events[0], owner);
        let records: Vec<ConversationNarrativeRecord> =
            read_jsonl_records(&directory.join("conversation.jsonl")).unwrap();
        assert_eq!(records.len(), events.len());
        assert_eq!(
            records
                .iter()
                .filter(|record| record.event_refs.contains(&old.id))
                .count(),
            1
        );
        assert_canonical_alias(owner.metadata["chat_source_ref"].as_str().unwrap(), &old.id);
    }
}

#[test]
fn codex_committed_recovery_rejects_changed_owners_policy_frames_and_ledgers_without_seal() {
    for scenario in [
        "body",
        "kind",
        "tool-name",
        "tool-input",
        "tool-input-text",
        "role",
        "title",
        "command",
        "source",
        "turn",
        "request-root",
        "native-session",
        "provider",
        "agent",
        "sequence",
        "path",
        "coordinate",
        "epoch",
        "start",
        "end",
        "offset",
        "unframed",
        "duplicate-owner",
        "duplicate-coordinate",
        "missing-record",
        "duplicate-record",
        "cursor-behind",
        "identity",
        "policy",
        "unknown",
        "disabled",
        "open-disabled",
        "incomplete",
        "replaced-reason",
        "forward-ledger",
        "reverse-ledger",
        "missing-digest",
    ] {
        let (_guard, temp) = super::super::tests::isolated_home();
        let (state, context, old, batch) = codex_fixture(temp.path());
        state.fail_compatibility_stage.store(4, Ordering::SeqCst);
        assert!(state
            .append_verified_provider_log_batch_with_context(
                context.clone(),
                &batch.events,
                batch.previous.as_ref(),
                &batch.next,
                batch.source_proof(),
            )
            .unwrap_err()
            .to_string()
            .contains("after cursor"));
        let conversation = state.active_conversation_id_for_test("agent-1").unwrap();
        let directory = conversation_dir("agent-1", &conversation).unwrap();
        let manifest = read_manifest(&directory.join("manifest.json")).unwrap();
        let mut archived: Vec<AgentChatEvent> =
            read_jsonl_records(&directory.join("events.jsonl")).unwrap();
        let mut records: Vec<ConversationNarrativeRecord> =
            read_jsonl_records(&directory.join("conversation.jsonl")).unwrap();
        let before_archive = std::fs::read(directory.join("events.jsonl")).unwrap();
        let mut checkpoint = read_capture_state("agent-1").unwrap();
        let mut previous = batch.next.clone();
        let mut next = batch.next.clone();
        match scenario {
            "body" => archived[0].text = Some("Changed body".into()),
            "kind" => archived[0].kind = AgentChatEventKind::ToolCall,
            "tool-name" => archived[0].metadata["tool_name"] = serde_json::json!("foreign-tool"),
            "tool-input" => {
                archived[0].metadata["tool_input"] =
                    serde_json::json!({"command":"pwd", "timeout":20})
            }
            "tool-input-text" => {
                archived[0].metadata["tool_input_text"] = serde_json::json!("foreign-arguments")
            }
            "role" => archived[0].role = Some(AgentChatRole::User),
            "title" => archived[0].title = Some("Changed title".into()),
            "command" => archived[0].command = Some("changed-command".into()),
            "source" => archived[0].source = Some("foreign-source".into()),
            "turn" => archived[0].turn_id = Some("foreign-turn".into()),
            "request-root" => {
                archived[0].metadata["request_root_id"] = serde_json::json!("foreign-root")
            }
            "native-session" => {
                archived[0].metadata["provider_session_id"] = serde_json::json!("foreign-session")
            }
            "provider" => archived[0].provider = "claude".into(),
            "agent" => archived[0].session_id = "foreign-agent".into(),
            "sequence" => archived[0].sequence = old.sequence.map(|sequence| sequence + 1),
            "path" => archived[0].metadata["log_path"] = serde_json::json!("foreign-path"),
            "coordinate" => {
                archived[0].metadata["chat_source_ref"] = serde_json::json!("foreign-coordinate")
            }
            "epoch" => {
                archived[0].metadata["chat_source_epoch"] = serde_json::json!("foreign-epoch")
            }
            "start" => archived[0].metadata["chat_source_start"] = serde_json::json!(1),
            "end" => {
                archived[0].metadata["chat_source_end"] =
                    serde_json::json!(next.committed_offset + 1)
            }
            "offset" => archived[0].metadata["provider_log_row_offset"] = serde_json::json!(1),
            "unframed" => {
                for key in [
                    "chat_source_ref",
                    "chat_source_start",
                    "chat_source_end",
                    "chat_source_epoch",
                    "provider_log_row_offset",
                ] {
                    archived[0].metadata.as_object_mut().unwrap().remove(key);
                }
            }
            "duplicate-owner" => archived.push(archived[0].clone()),
            "duplicate-coordinate" => {
                let mut other = archived[0].clone();
                other.id = "foreign-owner".into();
                archived.push(other);
            }
            "missing-record" => records.clear(),
            "duplicate-record" => records.push(records[0].clone()),
            "cursor-behind" => previous.committed_offset -= 1,
            "identity" => next.native_identity.primary ^= 1,
            "policy" => next.policy_generation += 1,
            "unknown" => {
                previous.unknown_before_offset = Some(1);
                next.unknown_before_offset = Some(1);
            }
            "disabled" => {
                let span = crate::commands::provider_log_acquisition::ProviderLogDisabledSpan {
                    start: 0,
                    end: next.committed_offset,
                };
                previous.disabled_spans.push(span.clone());
                next.disabled_spans.push(span);
            }
            "open-disabled" => {
                previous.open_disabled_from = Some(0);
                next.open_disabled_from = Some(0);
            }
            "incomplete" => next.status = "incomplete".into(),
            "replaced-reason" => next.reason = Some("provider_log_source_replaced".into()),
            "forward-ledger" | "reverse-ledger" | "missing-digest" => {
                let mut store = Store::writer(&objects("agent-1").unwrap());
                let mut root = load(&mut store, checkpoint.compatibility_claims.as_ref()).unwrap();
                let (key, reference) = store.page(&root.pending, None, 1).unwrap().remove(0);
                let mut claim: Claim = store.read(&reference).unwrap();
                let coordinate_key =
                    digest(format!("coordinate:{}:{}", claim.scope, claim.coordinate).as_bytes());
                claim.codex_owner_digest = None;
                let changed = store.put(&claim).unwrap();
                match scenario {
                    "forward-ledger" => {
                        root.owners = Some(store.insert(&root.owners, &key, &changed).unwrap())
                    }
                    "reverse-ledger" => {
                        root.owners = Some(
                            store
                                .insert(&root.owners, &coordinate_key, &changed)
                                .unwrap(),
                        )
                    }
                    _ => {
                        root.owners = Some(store.insert(&root.owners, &key, &changed).unwrap());
                        root.owners = Some(
                            store
                                .insert(&root.owners, &coordinate_key, &changed)
                                .unwrap(),
                        );
                        root.pending = Some(store.insert(&root.pending, &key, &changed).unwrap());
                    }
                }
                checkpoint.compatibility_claims = Some(Checkpoint {
                    version: 1,
                    root: store.put(&root).unwrap(),
                });
            }
            _ => unreachable!(),
        }
        assert!(
            prepare(
                Preparation {
                    context: &context,
                    conversation_id: &conversation,
                    manifest: manifest.as_ref(),
                    archived: &archived,
                    records: &records,
                    events: &[],
                    previous: Some(&previous),
                    next: &next,
                    source_proof: None,
                },
                &mut checkpoint
            )
            .is_err(),
            "unqualified committed recovery admitted: {scenario}"
        );
        assert_eq!(
            std::fs::read(directory.join("events.jsonl")).unwrap(),
            before_archive
        );
        assert_eq!(
            state
                .provider_log_capture_state(
                    "agent-1",
                    context.provider_source_key.as_ref().unwrap()
                )
                .unwrap(),
            Some(batch.next)
        );
        assert!(pending(&read_capture_state("agent-1").unwrap(), "agent-1").unwrap());
    }
}

#[test]
fn codex_committed_tool_claims_recover_at_eof_with_original_kind_and_body() {
    for raw in [
        r#"{"type":"response_item","turn_id":"turn-1","payload":{"type":"function_call","name":"shell_command","arguments":"{\"command\":\"pwd\"}"}}"#,
        r#"{"type":"response_item","turn_id":"turn-1","payload":{"type":"function_call_output","output":"Completed tool output"}}"#,
    ] {
        let (_guard, temp) = super::super::tests::isolated_home();
        let (state, context, old, batch) = codex_fixture_with_row(temp.path(), raw);
        assert!(matches!(
            old.kind,
            AgentChatEventKind::ToolCall | AgentChatEventKind::ToolResult
        ));
        let conversation = state.active_conversation_id_for_test("agent-1").unwrap();
        let directory = conversation_dir("agent-1", &conversation).unwrap();
        state.fail_compatibility_stage.store(4, Ordering::SeqCst);
        assert!(state
            .append_verified_provider_log_batch_with_context(
                context.clone(),
                &batch.events,
                batch.previous.as_ref(),
                &batch.next,
                batch.source_proof(),
            )
            .unwrap_err()
            .to_string()
            .contains("after cursor"));
        assert!(pending(&read_capture_state("agent-1").unwrap(), "agent-1").unwrap());
        assert_eq!(
            state
                .provider_log_capture_state(
                    "agent-1",
                    context.provider_source_key.as_ref().unwrap()
                )
                .unwrap(),
            Some(batch.next.clone())
        );
        let before = std::fs::read(directory.join("events.jsonl")).unwrap();
        let narrative_before = std::fs::read(directory.join("conversation.jsonl")).unwrap();
        let owners: Vec<AgentChatEvent> =
            read_jsonl_records(&directory.join("events.jsonl")).unwrap();
        assert_eq!(owners.len(), 1);
        assert_eq!(owners[0].id, old.id);
        assert_eq!(owners[0].sequence, old.sequence);
        assert_eq!(owners[0].kind, old.kind);
        assert_eq!(owners[0].text, old.text);
        assert_eq!(owners[0].command, old.command);
        if old.kind == AgentChatEventKind::ToolCall {
            let mut changed = owners.clone();
            changed[0].metadata["tool_input"]["timeout"] = serde_json::json!(20);
            assert_eq!(changed[0].command, owners[0].command);
            let manifest = read_manifest(&directory.join("manifest.json")).unwrap();
            let records: Vec<ConversationNarrativeRecord> =
                read_jsonl_records(&directory.join("conversation.jsonl")).unwrap();
            let mut checkpoint = read_capture_state("agent-1").unwrap();
            assert!(prepare(
                Preparation {
                    context: &context,
                    conversation_id: &conversation,
                    manifest: manifest.as_ref(),
                    archived: &changed,
                    records: &records,
                    events: &[],
                    previous: Some(&batch.next),
                    next: &batch.next,
                    source_proof: None,
                },
                &mut checkpoint
            )
            .is_err());
        }
        drop(state);
        let restarted = ConversationArchiveState::default();
        let eof = codex_restart_batch(&restarted, &context, &temp.path().join("codex.jsonl"));
        assert!(eof.events.is_empty());
        assert!(eof.source_proof().is_none());
        restarted
            .append_verified_provider_log_batch_with_context(
                context,
                &eof.events,
                eof.previous.as_ref(),
                &eof.next,
                eof.source_proof(),
            )
            .unwrap();
        assert!(!pending(&read_capture_state("agent-1").unwrap(), "agent-1").unwrap());
        assert_eq!(
            std::fs::read(directory.join("events.jsonl")).unwrap(),
            before
        );
        let records: Vec<ConversationNarrativeRecord> =
            read_jsonl_records(&directory.join("conversation.jsonl")).unwrap();
        assert_eq!(records.len(), 1);
        // Capture enrichment retains the canonical ID and its observed source
        // alias in one narrative; recovery must preserve that entire snapshot.
        assert_eq!(
            std::fs::read(directory.join("conversation.jsonl")).unwrap(),
            narrative_before
        );
        assert_eq!(records[0].event_refs.len(), 2);
        assert_eq!(
            records[0]
                .event_refs
                .iter()
                .filter(|id| *id == &old.id)
                .count(),
            1
        );
        assert_eq!(
            records[0]
                .event_refs
                .iter()
                .filter(|id| Some(id.as_str()) == owners[0].metadata["chat_source_ref"].as_str())
                .count(),
            1
        );
        assert_canonical_alias(
            owners[0].metadata["chat_source_ref"].as_str().unwrap(),
            &old.id,
        );
    }
}

#[test]
fn codex_sealed_tool_migration_rejects_changed_current_or_archived_arguments() {
    let (_guard, temp) = super::super::tests::isolated_home();
    let raw = r#"{"type":"response_item","turn_id":"turn-1","payload":{"type":"function_call","name":"shell_command","arguments":"{\"command\":\"pwd\",\"timeout\":10}"}}"#;
    let (state, context, old, batch) = codex_fixture_with_row(temp.path(), raw);
    let proof = batch.source_proof().unwrap();
    assert!(proof.proves(&batch.events[0]));
    let conversation = state.active_conversation_id_for_test("agent-1").unwrap();
    let directory = conversation_dir("agent-1", &conversation).unwrap();
    let manifest = read_manifest(&directory.join("manifest.json")).unwrap();
    let archived: Vec<AgentChatEvent> =
        read_jsonl_records(&directory.join("events.jsonl")).unwrap();
    let records: Vec<ConversationNarrativeRecord> =
        read_jsonl_records(&directory.join("conversation.jsonl")).unwrap();
    let before = std::fs::read(directory.join("events.jsonl")).unwrap();
    for field in [
        "tool-input",
        "title",
        "command",
        "tool-name",
        "tool-input-text",
    ] {
        let mut event = batch.events[0].clone();
        match field {
            "title" => event.title = Some("Foreign tool".into()),
            "command" => event.command = Some("foreign-command".into()),
            "tool-name" => event.metadata["tool_name"] = serde_json::json!("foreign-tool"),
            "tool-input" => event.metadata["tool_input"]["timeout"] = serde_json::json!(20),
            _ => event.metadata["tool_input_text"] = serde_json::json!("foreign-arguments"),
        }
        assert!(
            !proof.proves(&event),
            "unobserved tool body admitted by receipt: {field}"
        );
        let mut checkpoint = read_capture_state("agent-1").unwrap();
        assert!(
            prepare(
                Preparation {
                    context: &context,
                    conversation_id: &conversation,
                    manifest: manifest.as_ref(),
                    archived: &archived,
                    records: &records,
                    events: std::slice::from_ref(&event),
                    previous: batch.previous.as_ref(),
                    next: &batch.next,
                    source_proof: Some(proof),
                },
                &mut checkpoint
            )
            .is_err(),
            "changed current tool admitted: {field}"
        );
    }
    let mut changed = archived.clone();
    changed[0].metadata["tool_input"]["timeout"] = serde_json::json!(20);
    assert_eq!(changed[0].id, old.id);
    assert_eq!(changed[0].command, old.command);
    assert_eq!(changed[0].sequence, old.sequence);
    let mut checkpoint = read_capture_state("agent-1").unwrap();
    assert!(prepare(
        Preparation {
            context: &context,
            conversation_id: &conversation,
            manifest: manifest.as_ref(),
            archived: &changed,
            records: &records,
            events: &batch.events,
            previous: batch.previous.as_ref(),
            next: &batch.next,
            source_proof: Some(proof),
        },
        &mut checkpoint
    )
    .is_err());
    assert_eq!(
        std::fs::read(directory.join("events.jsonl")).unwrap(),
        before
    );
    assert_eq!(
        state
            .provider_log_capture_state("agent-1", context.provider_source_key.as_ref().unwrap())
            .unwrap(),
        batch.previous
    );
    assert!(!pending(&read_capture_state("agent-1").unwrap(), "agent-1").unwrap());
}
