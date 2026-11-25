use super::*;
use crate::state::conversation_archive::tests::isolated_home;
use crate::state::ConversationArchiveState;
use std::io::Write;
use wardian_core::models::chat::AgentChatRole;
use wardian_core::models::chat_presentation::{ChatPresentationV1, ChatTurnStatus};

#[test]
fn a_normalized_running_prefix_fills_the_default_window_without_retiring_its_admission() {
    let (_guard, temp) = isolated_home();
    let path = temp.path().join("preview-capacity-source.jsonl");
    let mut lines = vec![
        json!({"type":"session_meta","payload":{"id":"capacity-session"}}),
        json!({"type":"turn_context","payload":{"turn_id":"capacity-turn"}}),
        json!({"type":"event_msg","payload":{"type":"task_started","turn_id":"capacity-turn"}}),
        json!({"type":"response_item","payload":{"type":"message","id":"capacity-prompt","role":"user","content":[{"type":"input_text","text":"Run synthetic tests."}],"internal_chat_message_metadata_passthrough":{"turn_id":"capacity-turn","content_item_kinds":["user.text"]}}}),
    ];
    for index in 0..100 {
        lines.push(json!({"type":"event_msg","payload":{"type":"agent_message","turn_id":"capacity-turn","message":format!("Synthetic progress {index}")}}));
    }
    let contents = lines
        .iter()
        .map(serde_json::Value::to_string)
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    assert!(contents.len() < 32 * 1024);
    let (mut source, _) = super::tests::owned_native_source_for_provider_session(
        &path,
        &contents,
        "agent-1",
        "capacity-session",
    );
    source.current_status = "Processing".into();
    let context = crate::commands::chat::conversation_archive_context_from_snapshot(&source);
    let archive = ConversationArchiveState::default();
    let mut batch = crate::commands::provider_log_acquisition::acquire_provider_log_batch(
        "agent-1",
        "codex",
        &path,
        context.provider_source_key.as_deref().unwrap(),
        None,
        true,
    )
    .unwrap();
    crate::commands::chat::decorate_forward_provider_log_events(&mut batch.events, "codex", &path);
    assert!(batch.events.len() > PAGE_ROWS);
    let prompt = batch
        .events
        .iter()
        .find(|event| event.role == Some(AgentChatRole::User))
        .expect("the native typed prompt is normalized");
    assert_eq!(prompt.text.as_deref(), Some("Run synthetic tests."));
    assert_eq!(prompt.metadata["input_origin"], "human_input");
    assert_eq!(prompt.metadata["chat_turn_adapter"], "codex_bracket_v1");
    assert_eq!(
        batch
            .events
            .iter()
            .filter(|event| event
                .text
                .as_deref()
                .is_some_and(|text| text.starts_with("Synthetic progress ")))
            .count(),
        100
    );
    assert!(batch
        .events
        .iter()
        .all(|event| event.metadata["chat_turn_membership_complete"] != true));
    archive
        .append_verified_provider_log_batch_with_context(
            context.clone(),
            &batch.events,
            None,
            &batch.next,
            batch.source_proof(),
        )
        .unwrap();
    super::tests::finish_projection(&archive, &context);
    let owner = &archive.chat_projection;
    let runtime = Arc::new(Mutex::new("Processing".into()));
    let receipt = owner.begin_recent(&source, &runtime).unwrap();
    assert!(owner
        .publish_recent(&source, &runtime, &receipt, Vec::new())
        .unwrap());

    let bridge_diagnostic = || {
        let prior = owner
            .recent
            .lock()
            .unwrap()
            .slots
            .iter()
            .find(|slot| slot.admission == receipt.0)
            .and_then(|slot| slot.binding.clone());
        match preview_bridge::read(
            &source,
            &receipt.0,
            &[],
            prior.as_ref(),
            None,
            PAGE_ROWS,
            preview_bridge::Selection {
                mode: if prior.is_some() {
                    preview_bridge::Mode::Bound
                } else {
                    preview_bridge::Mode::First
                },
                revision: None,
                hints: None,
            },
        ) {
            Ok(Some((page, binding))) => format!(
                "Some(events={}, presentation={}, binding_bytes={})",
                page.events.len(),
                page.presentation.is_some(),
                binding.charge()
            ),
            Ok(None) => "None".to_string(),
            Err(error) => format!("Err({:?}: {error})", error.kind()),
        }
    };
    let read = |revision, limit, hints| {
        match owner.recent_reader_with_hints(
            source.clone(),
            runtime.clone(),
            revision,
            limit,
            Some(hints),
        )() {
            Ok(Some(page)) if page.presentation.is_some() => {
                let charge = owner.recent.lock().unwrap().slots.iter()
                    .find(|slot| slot.admission == receipt.0)
                    .and_then(|slot| slot.binding.as_ref())
                    .map(preview_bridge::Binding::charge);
                eprintln!("capacity limit={limit} events={} records={} bytes={} binding={charge:?}",
                    page.events.len(), page.records_decoded, page.bytes_read);
                page
            },
            Ok(Some(page)) => panic!(
                "default running window lost typed presentation: events={}, progress={}, direct_bridge={}",
                page.events.len(), page.progress, bridge_diagnostic()
            ),
            Ok(None) => panic!("default running window returned None; direct_bridge={}", bridge_diagnostic()),
            Err(error) => panic!("default running window returned Err({:?}: {error}); direct_bridge={}", error.kind(), bridge_diagnostic()),
        }
    };
    let assert_running = |page: &AgentChatPage, limit| {
        assert_eq!(page.events.len(), limit);
        assert_eq!(page.recent_admission.as_deref(), Some(receipt.0.as_str()));
        assert!(
            page.next_before.is_none() && page.detail.is_none() && page.revalidated_ids.is_none()
        );
        assert!(serde_json::to_vec(page).unwrap().len() <= IPC_BYTES);
        assert!(page
            .events
            .iter()
            .all(|event| event.metadata.get("_wardian_canonical_row").is_none()));
        assert!(
            page.records_decoded <= super::super::chat_read_store::READ_OBJECTS,
            "aggregate records_decoded={}; ordinary={:?}",
            page.records_decoded,
            read_with_revalidation_and_limit(
                &crate::commands::chat::conversation_archive_context_from_snapshot(&source),
                &source,
                None,
                None,
                None,
                None,
                Some(PAGE_ROWS)
            )
            .map(|page| (
                page.records_decoded,
                page.bytes_read,
                page.events
                    .iter()
                    .filter(|event| event.metadata["chat_turn_key"].is_string()
                        && event.metadata["chat_turn_item_key"].is_string())
                    .count(),
                page.events
                    .iter()
                    .filter(|event| event.metadata["chat_display_member_ids"].is_array())
                    .count()
            ))
        );
        assert!(page.bytes_read < 2 * super::super::chat_read_store::READ_BYTES);
        let envelope = page.presentation.as_ref().unwrap();
        assert_eq!(envelope.version, 1);
        let value: ChatPresentationV1 = serde_json::from_value(envelope.payload.clone()).unwrap();
        assert!(value.validate().is_ok());
        assert_eq!(value.turns.len(), 1);
        assert_eq!(value.items.len(), limit);
        assert!(value.legacy_turns.is_empty());
        let turn = &value.turns[0];
        assert_eq!(turn.status, ChatTurnStatus::Running);
        assert!(turn.prefix_attributed);
        assert!(!turn.membership_complete && !turn.counts_complete && !turn.answer_recorded);
        assert!(turn.answer_item_keys.is_empty());
        assert_eq!(
            value.preview_bridge.as_ref().unwrap().admission_id,
            receipt.0
        );
        value
    };
    let saved_binding = || {
        let recent = owner.recent.lock().unwrap();
        let slot = recent
            .slots
            .iter()
            .find(|slot| slot.admission == receipt.0)
            .unwrap();
        (
            serde_json::to_vec(slot.binding.as_ref().unwrap()).unwrap(),
            slot.certification,
            slot.issued,
        )
    };
    let latest_backing_ids = |page: &AgentChatPage| {
        page.events
            .iter()
            .skip(page.events.len().saturating_sub(8))
            .map(|event| event.id.clone())
            .collect::<Vec<_>>()
    };
    let legacy = owner.recent_reader_with_hints(
        source.clone(),
        runtime.clone(),
        None,
        PAGE_ROWS,
        Some(Vec::new()),
    )()
    .unwrap()
    .expect("M1 returns its honest Legacy window");
    assert_eq!(legacy.events.len(), PAGE_ROWS);
    assert!(legacy.presentation.is_none());
    assert_eq!(legacy.recent_admission.as_deref(), Some(receipt.0.as_str()));
    assert!(serde_json::to_vec(&legacy).unwrap().len() <= IPC_BYTES);
    let opening = {
        let recent = owner.recent.lock().unwrap();
        let slot = recent
            .slots
            .iter()
            .find(|slot| slot.admission == receipt.0)
            .expect("M1 keeps the same live admission");
        assert!(slot.published && slot.binding.is_none());
        assert!(slot.charge <= IPC_BYTES);
        (slot.certification, slot.issued)
    };
    let hints = latest_backing_ids(&legacy);
    assert_eq!(hints.len(), 8);
    let first = read(Some(legacy.revision), PAGE_ROWS, hints);
    let before = assert_running(&first, PAGE_ROWS);
    let certification = saved_binding();
    assert!(certification.1 > opening.0);
    assert_eq!(certification.2, opening.1);
    eprintln!("default-80 binding_bytes={}", certification.0.len());
    let serialized: serde_json::Value = serde_json::from_slice(&certification.0).unwrap();
    assert!(
        serde_json::to_vec(&serialized["presentation"])
            .unwrap()
            .len()
            <= 64 * 1024
    );
    let small = read(Some(first.revision.clone()), 8, Vec::new());
    let small_value = assert_running(&small, 8);
    assert_eq!(
        small_value,
        turn_read::window(before.clone(), &small.events, None).unwrap()
    );
    assert_eq!(
        saved_binding(),
        certification,
        "an 8-row reader cannot shrink shared certification"
    );
    let refreshed = read(Some(small.revision.clone()), PAGE_ROWS, Vec::new());
    let after = assert_running(&refreshed, PAGE_ROWS);
    assert_eq!(after.turns, before.turns);
    assert_eq!(after.items, before.items);
    assert_eq!(saved_binding(), certification);
    let same_head = head("agent-1").unwrap().unwrap();
    fs::OpenOptions::new().append(true).open(&path).unwrap()
        .write_all(b"{\"type\":\"event_msg\",\"payload\":{\"type\":\"agent_message\",\"turn_id\":\"capacity-turn\",\"message\":\"Synthetic native-only extension\"}}\n")
        .unwrap();
    // No durable capture: shared certification can advance under the same Head.
    let extended = read(Some(refreshed.revision.clone()), PAGE_ROWS, Vec::new());
    assert_eq!(
        head("agent-1").unwrap().as_deref(),
        Some(same_head.as_str())
    );
    assert_eq!(
        extended.recent_admission.as_deref(),
        Some(receipt.0.as_str())
    );
    assert!(extended.next_before.is_none() && extended.detail.is_none());
    {
        let recent = owner.recent.lock().unwrap();
        let slot = recent
            .slots
            .iter()
            .find(|slot| slot.admission == receipt.0)
            .expect("the same admission survives the default-window refresh");
        assert!(slot.published && slot.headers.is_empty() && slot.binding.is_some());
        assert!(slot.charge <= IPC_BYTES);
        assert!(slot.certification > certification.1);
        assert!(!slot.certification_matches(certification.1, true));
        assert!(slot.certification_matches(certification.1, false));
        assert_eq!(slot.issued, certification.2);
    }
    assert!(
        owner
            .publish_recent(&source, &runtime, &receipt, Vec::new())
            .unwrap(),
        "certification changes must not consume the Watch publication receipt"
    );

    // Exercise only retention pressure here, not reader authority: unpublished
    // cache slots retain clones of the real admitted certificate built above.
    let binding = owner
        .recent
        .lock()
        .unwrap()
        .slots
        .iter()
        .find(|slot| slot.admission == receipt.0)
        .and_then(|slot| slot.binding.clone())
        .unwrap();
    let records_per_slot = 1 + binding.record_count();
    let copies = 512 / records_per_slot + 1;
    assert!(
        (2..=8).contains(&copies),
        "a few real certificates exceed the record limit"
    );
    let mut cache = RecentSlots::default();
    for index in 0..copies {
        cache.slots.push_back(RecentSlot {
            agent_id: format!("retention-pressure-{index}"),
            runtime: Weak::new(),
            watch: Weak::new(),
            scope: String::new(),
            request_scope: String::new(),
            watch_cursor: None,
            admission: uuid::Uuid::new_v4().to_string(),
            issued: 1,
            published: false,
            version: String::new(),
            headers: Vec::new(),
            coverage: Vec::new(),
            charge: 1024 + binding.charge(),
            binding: Some(binding.clone()),
            certification: 0,
            native_source: None,
        });
    }
    assert!(cache.slots.iter().all(|slot| slot.charge <= IPC_BYTES));
    assert!(
        cache.slots.iter().map(|slot| slot.charge).sum::<usize>() + 4096 < 1024 * 1024,
        "the byte limit must not be the reason for eviction"
    );
    assert!(copies < 512 && copies * records_per_slot > 512);
    let oldest = cache.slots.front().unwrap().admission.clone();
    trim_recent_slots(&mut cache);
    assert_eq!(cache.slots.len(), copies - 1);
    assert!(cache.slots.iter().all(|slot| slot.admission != oldest));
    assert!(
        cache
            .slots
            .iter()
            .map(|slot| 1
                + slot.headers.len()
                + slot
                    .binding
                    .as_ref()
                    .map_or(0, preview_bridge::Binding::record_count))
            .sum::<usize>()
            <= 512
    );

    let padded_header = |template: &AgentChatEvent,
                         admission: &str,
                         overhead: usize,
                         binding_bytes: usize,
                         target: usize| {
        let mut header = template.clone();
        header.metadata["chat_recent_key"] = json!(format!("{admission}:watch:slot-cap"));
        header.metadata["chat_recent_version"] = json!("slot-cap-version");
        header.metadata["capacity_padding"] = json!("");
        assert!(header
            .text
            .as_ref()
            .is_none_or(|text| text.len() <= PREVIEW_BYTES));
        let empty = serde_json::to_vec(&vec![header.clone()]).unwrap().len();
        assert!(target >= overhead + binding_bytes + empty);
        header.metadata["capacity_padding"] =
            json!("x".repeat(target - overhead - binding_bytes - empty));
        header
    };
    let overhead = {
        let recent = owner.recent.lock().unwrap();
        let slot = recent
            .slots
            .iter()
            .find(|slot| slot.admission == receipt.0)
            .unwrap();
        assert!(slot.headers.is_empty() && slot.coverage.is_empty());
        slot.charge - binding.charge() - serde_json::to_vec(&slot.headers).unwrap().len()
    };
    let oversized = padded_header(
        extended.events.last().unwrap(),
        &receipt.0,
        overhead,
        binding.charge(),
        IPC_BYTES + 1,
    );
    let standalone = overhead + serde_json::to_vec(&vec![oversized.clone()]).unwrap().len();
    assert!(
        standalone <= IPC_BYTES,
        "the standalone header cohort is valid"
    );
    assert_eq!(standalone + binding.charge(), IPC_BYTES + 1);
    assert!(standalone + binding.charge() + 4096 < 1024 * 1024);
    assert!(!owner
        .publish_recent(&source, &runtime, &receipt, vec![oversized])
        .unwrap());
    assert!(!owner
        .recent
        .lock()
        .unwrap()
        .slots
        .iter()
        .any(|slot| slot.admission == receipt.0));
    assert!(!owner
        .publish_recent(&source, &runtime, &receipt, Vec::new())
        .unwrap());

    let next = owner.begin_recent(&source, &runtime).unwrap();
    assert_ne!(next.0, receipt.0);
    assert!(owner
        .publish_recent(&source, &runtime, &next, Vec::new())
        .unwrap());
    let rebound_legacy = owner.recent_reader_with_hints(
        source.clone(),
        runtime.clone(),
        None,
        PAGE_ROWS,
        Some(Vec::new()),
    )()
    .unwrap()
    .unwrap();
    assert_eq!(rebound_legacy.events.len(), PAGE_ROWS);
    assert!(rebound_legacy.presentation.is_none());
    assert_eq!(
        rebound_legacy.recent_admission.as_deref(),
        Some(next.0.as_str())
    );
    let hints = latest_backing_ids(&rebound_legacy);
    let rebound = owner.recent_reader_with_hints(
        source.clone(),
        runtime.clone(),
        Some(rebound_legacy.revision),
        PAGE_ROWS,
        Some(hints),
    )()
    .unwrap()
    .unwrap();
    let value: ChatPresentationV1 =
        serde_json::from_value(rebound.presentation.as_ref().unwrap().payload.clone()).unwrap();
    assert_eq!(value.preview_bridge.unwrap().admission_id, next.0);
    let (overhead, binding_bytes) = {
        let recent = owner.recent.lock().unwrap();
        let slot = recent
            .slots
            .iter()
            .find(|slot| slot.admission == next.0)
            .unwrap();
        let bytes = slot
            .binding
            .as_ref()
            .expect("new A has an actual read certificate")
            .charge();
        assert!(slot.headers.is_empty() && slot.coverage.is_empty());
        (
            slot.charge - bytes - serde_json::to_vec(&slot.headers).unwrap().len(),
            bytes,
        )
    };
    let near_limit = padded_header(
        rebound.events.last().unwrap(),
        &next.0,
        overhead,
        binding_bytes,
        IPC_BYTES - 128,
    );
    let key = near_limit.metadata["chat_recent_key"]
        .as_str()
        .unwrap()
        .to_owned();
    let version = near_limit.metadata["chat_recent_version"]
        .as_str()
        .unwrap()
        .to_owned();
    let event_id = "c".repeat(512);
    let coverage = vec![RecentCoverage {
        event_id: event_id.clone(),
        key: key.clone(),
        version: version.clone(),
        native_key: None,
    }];
    let delta = serde_json::to_vec(&coverage).unwrap().len() - 2;
    assert!(delta > 128);
    assert!(IPC_BYTES - 128 + delta + 4096 < 1024 * 1024);
    assert!(owner
        .publish_recent(&source, &runtime, &next, vec![near_limit])
        .unwrap());
    {
        let recent = owner.recent.lock().unwrap();
        let slot = recent
            .slots
            .iter()
            .find(|slot| slot.admission == next.0)
            .unwrap();
        assert!(slot.binding.is_some() && slot.coverage.is_empty());
        assert_eq!(slot.charge, IPC_BYTES - 128);
    }
    owner
        .note_recent_coverage(&next, &HashMap::from([(event_id, (key, version))]))
        .unwrap();
    assert!(!owner
        .recent
        .lock()
        .unwrap()
        .slots
        .iter()
        .any(|slot| slot.admission == next.0));
    assert!(!owner
        .publish_recent(&source, &runtime, &next, Vec::new())
        .unwrap());
}
