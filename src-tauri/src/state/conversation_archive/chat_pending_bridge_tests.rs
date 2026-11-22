use super::*;
use crate::commands::chat::{
    conversation_archive_context_from_snapshot, decorate_forward_provider_log_events,
};
use crate::commands::provider_log_acquisition::{
    acquire_provider_log_batch, ProviderLogBatch, ProviderLogCaptureState,
};
use crate::state::conversation_archive::{tests::isolated_home, ConversationArchiveState};
use std::io::Write;
use wardian_core::claude_mod::{ClaudeModLaunchPlan, ModAsset, OutcomeRecord};
use wardian_core::models::chat::AgentChatRole;
use wardian_core::models::chat_presentation::{
    ChatItemProvenance, ChatPresentedItem, ChatPresentedItemKind, ChatSubmissionStatus,
};
use wardian_core::native_transport::{
    NativeDeliveryPhase, NativeDeliveryRecord, NativeMessageEnvelope, NativeMessageOperation,
};

const SESSION: &str = "synthetic-session";
const AGENT: &str = "agent-1";

fn user(uuid: &str, parent: Option<&str>, content: &str) -> serde_json::Value {
    json!({
        "type": "user", "sessionId": SESSION, "uuid": uuid, "parentUuid": parent,
        "message": {"role": "user", "content": content}
    })
}

fn answer(uuid: &str, parent: &str, content: &str) -> serde_json::Value {
    json!({
        "type": "assistant", "sessionId": SESSION, "uuid": uuid, "parentUuid": parent,
        "message": {"id": uuid, "role": "assistant", "stop_reason": "end_turn",
                    "content": [{"type": "text", "text": content}]}
    })
}

fn append(path: &Path, row: serde_json::Value) {
    writeln!(
        std::fs::OpenOptions::new().append(true).open(path).unwrap(),
        "{row}"
    )
    .unwrap();
}

fn acquire(
    context: &ConversationArchiveContext,
    path: &Path,
    previous: Option<ProviderLogCaptureState>,
) -> ProviderLogBatch {
    let mut batch = acquire_provider_log_batch(
        AGENT,
        "claude",
        path,
        context.provider_source_key.as_deref().unwrap(),
        previous,
        true,
    )
    .unwrap();
    decorate_forward_provider_log_events(&mut batch.events, "claude", path);
    batch
}

fn commit(
    archive: &ConversationArchiveState,
    context: &ConversationArchiveContext,
    batch: &ProviderLogBatch,
) {
    archive
        .append_verified_provider_log_batch_with_context(
            context.clone(),
            &batch.events,
            batch.previous.as_ref(),
            &batch.next,
            batch.source_proof(),
        )
        .unwrap();
    tests::finish_projection(archive);
}

fn prompt(page: &AgentChatPage, text: &str) -> ChatPresentedItem {
    assert!(page.events.iter().all(|event| {
        event.metadata.get(PROMPT_STATUS).is_none() && event.metadata.get(PROMPT_DELIVERY).is_none()
    }));
    let [event] = page
        .events
        .iter()
        .filter(|event| {
            event.role == Some(AgentChatRole::User) && event.text.as_deref() == Some(text)
        })
        .collect::<Vec<_>>()[..]
    else {
        panic!("expected exactly one prompt in the published page");
    };
    let presentation = page
        .presentation
        .as_ref()
        .unwrap()
        .decode_v1()
        .unwrap()
        .unwrap();
    let [item] = presentation
        .items
        .iter()
        .filter(|item| {
            item.kind == ChatPresentedItemKind::Prompt && item.event_refs == [event.id.clone()]
        })
        .collect::<Vec<_>>()[..]
    else {
        panic!("expected exactly one owned prompt item");
    };
    item.clone()
}

#[test]
fn pending_bridge_uses_owned_claim_and_keeps_prompt_keys_through_capture() {
    assert_capture_transition(NativeDeliveryPhase::Completed, None, false);
}

#[test]
fn pending_bridge_retains_failed_and_cancelled_status_through_capture_and_reopen() {
    assert_capture_transition(
        NativeDeliveryPhase::Failed,
        Some(ChatSubmissionStatus::Failed),
        false,
    );
    assert_capture_transition(
        NativeDeliveryPhase::Cancelled,
        Some(ChatSubmissionStatus::Cancelled),
        false,
    );
}

#[test]
fn pending_bridge_polls_terminal_receipts_without_changes_to_the_source_or_head() {
    assert_capture_transition(
        NativeDeliveryPhase::Failed,
        Some(ChatSubmissionStatus::Failed),
        true,
    );
    assert_capture_transition(
        NativeDeliveryPhase::Cancelled,
        Some(ChatSubmissionStatus::Cancelled),
        true,
    );
}

fn assert_capture_transition(
    phase: NativeDeliveryPhase,
    terminal: Option<ChatSubmissionStatus>,
    after_first_canonical_read: bool,
) {
    let (_guard, temp) = isolated_home();
    wardian_core::db::init_db().unwrap();
    let plan = ClaudeModLaunchPlan::new(temp.path(), AGENT, 7, SESSION).unwrap();
    plan.materialize(&[ModAsset {
        relative_path: "fixture.txt",
        contents: b"synthetic",
    }])
    .unwrap();
    let inbox = plan.mod_dir().parent().unwrap().join("inbox");
    let envelope = NativeMessageEnvelope {
        message_id: "fixture-message".into(),
        interaction_id: "fixture-interaction".into(),
        target_agent_id: AGENT.into(),
        sender_agent_id: None,
        parent_interaction_id: None,
        caller_idempotency_key: None,
        generation: 7,
        operation: NativeMessageOperation::StartTurn,
        deadline_at: None,
        body: "Check the synthetic archive.".into(),
    };
    let mut record = NativeDeliveryRecord {
        envelope,
        canonical_hash: "fixture".into(),
        provider: "claude".into(),
        transport: "claude_mod".into(),
        claude_mod_as_user: true,
        phase: NativeDeliveryPhase::TurnStarted,
        provider_request_id: None,
        provider_turn_id: None,
        detail: None,
        created_at: "2026-01-01T00:00:00Z".into(),
        updated_at: "2026-01-01T00:00:00Z".into(),
    };
    wardian_core::db::upsert_native_delivery(&record).unwrap();

    let path = temp.path().join(format!("{SESSION}.jsonl"));
    std::fs::write(
        &path,
        format!(
            "{}\n{}\n",
            user("warm", None, "Warmup."),
            answer("warm-answer", "warm", "Ready.")
        ),
    )
    .unwrap();
    let base = ConversationArchiveContext::for_agent_id(AGENT, "claude");
    let mut source = tests::snapshot(&base);
    source.resume_session = Some(SESSION.into());
    source.log_path = Some(path.clone());
    source.current_status = "Processing".into();
    let context = conversation_archive_context_from_snapshot(&source);
    let archive = ConversationArchiveState::default();
    let warm = acquire(&context, &path, None);
    commit(&archive, &context, &warm);
    assert!(head(AGENT).unwrap().is_some());

    let text = &record.envelope.body;
    let body = format!("{}\n{text}", record.envelope.transcript_header());
    append(&path, user("live", Some("warm-answer"), &body));
    let read = |prior: Option<&preview_bridge::Binding>| {
        preview_bridge::read(
            &source,
            "synthetic-admission",
            &[],
            prior,
            None,
            PAGE_ROWS,
            preview_bridge::Selection {
                mode: if prior.is_some() {
                    preview_bridge::Mode::Bound
                } else {
                    preview_bridge::Mode::First
                },
                hints: None,
                revision: None,
            },
        )
        .unwrap()
        .expect("owned Claude source qualifies through the published Head")
    };

    // A literal token can be printed before any valid mod claim.
    let (unclaimed, _) = read(None);
    assert_eq!(prompt(&unclaimed, text).submission_status, None);
    write_json_atomic(
        &inbox.join("claimed/fixture-interaction.json"),
        &OutcomeRecord {
            generation: 7,
            interaction_id: "fixture-interaction".into(),
            session_id: SESSION.into(),
        },
    )
    .unwrap();
    let mut foreign = source.clone();
    foreign.resume_session = Some("different-session".into());
    let foreign = crate::commands::chat_recent_seed::read_qualifying(&foreign, PAGE_ROWS).unwrap();
    assert!(
        foreign.events.is_empty(),
        "an owned token does not grant another source"
    );

    let (pending, binding) = read(None);
    assert_eq!(pending.next_before, None);
    assert_eq!(pending.revalidated_ids, None);
    let before = prompt(&pending, text);
    assert_eq!(before.provenance, ChatItemProvenance::Provisional);
    assert_eq!(
        before.submission_status,
        Some(ChatSubmissionStatus::Delivered)
    );

    record.phase = NativeDeliveryPhase::ProviderAccepted;
    wardian_core::db::upsert_native_delivery(&record).unwrap();
    let (without_queue_receipt, _) = read(Some(&binding));
    assert_eq!(
        prompt(&without_queue_receipt, text).submission_status,
        Some(ChatSubmissionStatus::Delivered),
    );
    write_json_atomic(
        &inbox.join("queued/fixture-interaction.json"),
        &OutcomeRecord {
            generation: 7,
            interaction_id: "fixture-interaction".into(),
            session_id: SESSION.into(),
        },
    )
    .unwrap();
    let (queued, _) = read(Some(&binding));
    let queued = prompt(&queued, text);
    assert_eq!(queued.item_key, before.item_key);
    assert_eq!(queued.submission_status, Some(ChatSubmissionStatus::Queued));
    record.phase = NativeDeliveryPhase::TurnStarted;
    wardian_core::db::upsert_native_delivery(&record).unwrap();

    // Acquire the row with its owned delivery while the phase is still
    // nonterminal. The later durable outcome must survive publication of it.
    append(
        &path,
        answer("live-answer", "live", "Synthetic check complete."),
    );
    let captured = acquire(&context, &path, Some(warm.next));
    assert!(captured
        .events
        .iter()
        .any(|event| event.metadata[PROMPT_DELIVERY] == "7:fixture-interaction"));
    if !after_first_canonical_read {
        record.phase = phase;
        wardian_core::db::upsert_native_delivery(&record).unwrap();
        if terminal.is_some() {
            let (ending, _) = read(Some(&binding));
            assert_eq!(prompt(&ending, text).submission_status, terminal);
        }
    }
    commit(&archive, &context, &captured);
    let replaced = if after_first_canonical_read {
        let first = super::read(&context, &source, None, None, None).unwrap();
        let first_item = prompt(&first, text);
        assert_eq!(first_item.provenance, ChatItemProvenance::Canonical);
        assert_eq!(first_item.submission_status, None);
        let pinned = head(AGENT).unwrap();
        let unchanged = super::read(&context, &source, None, Some(&first.revision), None).unwrap();
        assert!(unchanged.unchanged);

        record.phase = phase;
        wardian_core::db::upsert_native_delivery(&record).unwrap();
        let updated = super::read(&context, &source, None, Some(&first.revision), None).unwrap();
        assert_eq!(head(AGENT).unwrap(), pinned);
        assert!(!updated.unchanged);
        assert!(!updated.reset);
        assert_ne!(updated.revision, first.revision);
        assert_eq!(prompt(&updated, text).event_refs, first_item.event_refs);
        let unchanged =
            super::read(&context, &source, None, Some(&updated.revision), None).unwrap();
        assert!(unchanged.unchanged);
        updated
    } else {
        read(Some(&binding)).0
    };
    let after = prompt(&replaced, text);
    assert_eq!(after.provenance, ChatItemProvenance::Canonical);
    assert_eq!(after.submission_status, terminal);
    assert_eq!(after.item_key, before.item_key);
    assert_eq!(after.turn_key, before.turn_key);
    assert_eq!(after.order_key, before.order_key);
    assert!(
        after.event_refs == before.event_refs
            || replaced.aliases.iter().any(|alias| {
                alias.observation_id == before.event_refs[0]
                    && alias.canonical_id == after.event_refs[0]
            }),
        "the same physical event must survive capture or resolve through its exact alias"
    );
    assert!(replaced.events.iter().any(|event| {
        event.id == after.event_refs[0] && event.metadata["chat_provisional"] != true
    }));

    drop(archive);
    let reopened = super::read(&context, &source, None, None, None).unwrap();
    let restored = prompt(&reopened, text);
    assert_eq!(restored.item_key, after.item_key);
    assert_eq!(restored.turn_key, after.turn_key);
    assert_eq!(restored.event_refs, after.event_refs);
    assert_eq!(restored.submission_status, terminal);
}
