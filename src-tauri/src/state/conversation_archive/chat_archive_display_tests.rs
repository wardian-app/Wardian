use super::*;
use crate::commands::provider_log_acquisition::{
    native_file_identity, ProviderLogCaptureState, ProviderLogContinuityAnchor,
};
use crate::providers::chat_transcript::TranscriptNormalizationState;
use crate::state::conversation_archive::storage::conversation_dir;
use crate::state::conversation_archive::{tests::isolated_home, ConversationArchiveState};
use wardian_core::conversations::{AgentConversationLoggingSetting, ConversationBoundaryReason};
use wardian_core::models::chat::{AgentChatEventKind, AgentChatRole};

fn context() -> ConversationArchiveContext {
    let mut context = ConversationArchiveContext::for_agent_id("agent-1", "codex");
    context.provider_source_key = Some("codex:one".into());
    context.provider_session_ids = vec!["one".into()];
    context
}

fn legacy_pair(temp: &Path, text: &str) -> Vec<AgentChatEvent> {
    let path = temp.join("rollout.jsonl");
    fs::write(
        &path,
        b"{\"type\":\"session_meta\",\"payload\":{\"id\":\"one\"}}\n",
    )
    .unwrap();
    let path = path.canonicalize().unwrap();
    [("mirror", "event_msg"), ("completed", "response_item")]
        .into_iter()
        .map(|(id, source)| {
            let mut event: AgentChatEvent = serde_json::from_value(json!({
                "id": format!("agent-1:provider_log:{id}"),
                "session_id": "agent-1",
                "provider": "codex",
                "kind": "message",
                "role": "assistant",
                "text": text,
                "source": source,
                "created_at": "2026-10-04T02:39:23.151Z",
                "metadata": {
                    "provider_log": true,
                    "log_path": path,
                    "provider_turn_id": "turn-one",
                    "raw_type": if source == "event_msg" { "agent_message" } else { "message" }
                }
            }))
            .unwrap();
            if source == "response_item" {
                event.turn_id = Some("native-message-one".into());
                event.metadata["provider_phase"] = json!("final_answer");
            }
            event
        })
        .collect()
}

fn snapshot(
    context: &ConversationArchiveContext,
) -> crate::commands::chat::AgentArchiveCaptureSnapshot {
    crate::commands::chat::AgentArchiveCaptureSnapshot {
        session_id: context.agent_id.clone(),
        provider: context.provider.clone(),
        resume_session: None,
        fresh_provider_session_id: None,
        cleared_provider_sessions: Vec::new(),
        current_status: "Idle".into(),
        last_status_at: None,
        log_path: None,
        agent_name: String::new(),
        agent_class: String::new(),
        workspace: String::new(),
        agent_conversation_logging: AgentConversationLoggingSetting::Default,
        watch_state: Arc::new(Mutex::new(crate::state::AgentWatchState::new(
            context.agent_id.clone(),
            32,
            4096,
        ))),
    }
}

fn cold_page(temp: &Path, events: &[AgentChatEvent]) -> AgentChatPage {
    saved_page(temp, events, true)
}

fn current_capture(mut capture: ProviderLogCaptureState) -> ProviderLogCaptureState {
    capture.status = "complete".into();
    capture.reason = None;
    let bytes = fs::read(&capture.path).unwrap();
    capture.continuity_anchor = ProviderLogContinuityAnchor {
        start: 0,
        len: bytes.len() as u64,
        sha256: digest(&bytes),
    };
    capture
}

fn save_capture(
    context: &ConversationArchiveContext,
    capture: &ProviderLogCaptureState,
) -> PathBuf {
    let path = locations(&context.agent_id)
        .unwrap()
        .0
        .with_file_name("conversation-capture.json");
    write_json_atomic(&path, &json!({ "provider_log_sources": [capture] })).unwrap();
    publish_policy(context, capture).unwrap();
    path
}

fn padded_pair(temp: &Path) -> Vec<AgentChatEvent> {
    let mut events = legacy_pair(temp, &"A complete saved answer. ".repeat(60));
    for index in 0..60 {
        let mut padding = events[0].clone();
        padding.id = format!("padding-{index}");
        padding.kind = AgentChatEventKind::Status;
        padding.role = None;
        padding.text = Some(format!("Saved activity {index}"));
        events.push(padding);
    }
    events
}

fn drain_capture(archive: &ConversationArchiveState, context: &ConversationArchiveContext) {
    for _ in 0..256 {
        let bootstrapping = archive.bootstrap_saved_chat(context).unwrap();
        let indexing = archive
            .chat_projection
            .advance_capture(&context.agent_id)
            .unwrap();
        if !bootstrapping && !indexing {
            return;
        }
    }
    panic!("owned archive handoff did not settle");
}

fn begin_recount(archive: &ConversationArchiveState, context: &ConversationArchiveContext) -> Head {
    while archive.chat_projection.advance(&context.agent_id).unwrap() {}
    assert!(archive
        .chat_projection
        .has_pending_capture_work(&context.agent_id));
    for _ in 0..64 {
        assert!(archive.bootstrap_saved_chat(context).unwrap());
        let Some(reference) = head(&context.agent_id).unwrap() else {
            continue;
        };
        let mut store = Store::new(&locations(&context.agent_id).unwrap().1);
        let published: Head = store.read(&reference).unwrap();
        if published.progress == "indexing" && published.bootstrap.is_some() {
            return published;
        }
    }
    panic!("the handoff never published a cold checkpoint");
}

fn complete_body(
    context: &ConversationArchiveContext,
    page: &AgentChatPage,
    event: &AgentChatEvent,
) -> String {
    let mut next = event.metadata["chat_detail_ref"]
        .as_str()
        .map(str::to_owned);
    let mut body = String::new();
    for _ in 0..16 {
        let Some(reference) = next.take() else {
            return body;
        };
        let detail = read(
            context,
            &snapshot(context),
            None,
            Some(&page.revision),
            Some(&reference),
        )
        .unwrap()
        .detail
        .unwrap();
        assert_eq!(detail.event_id, event.id);
        body.push_str(&detail.text);
        next = detail.next;
    }
    panic!("saved body did not finish");
}

fn saved_page(temp: &Path, events: &[AgentChatEvent], retire_first: bool) -> AgentChatPage {
    let context = context();
    let archive = ConversationArchiveState::default();
    let path = temp.join("rollout.jsonl").canonicalize().unwrap();
    let file = File::open(&path).unwrap();
    let capture = ProviderLogCaptureState {
        provider_source_key: "codex:one".into(),
        path: path.to_string_lossy().into_owned(),
        native_identity: native_file_identity(&file).unwrap(),
        committed_offset: file.metadata().unwrap().len(),
        continuity_anchor: ProviderLogContinuityAnchor::default(),
        normalizer: TranscriptNormalizationState::default(),
        status: "incomplete".into(),
        reason: Some("provider_log_source_replaced".into()),
        unknown_before_offset: None,
        policy_generation: 0,
        disabled_spans: Vec::new(),
        open_disabled_from: None,
    };
    if !retire_first {
        // Model an ordinary persisted head from before native authority was lost.
        save_capture(&context, &current_capture(capture.clone()));
    }
    archive
        .append_chat_events_with_context(context.clone(), events)
        .unwrap();
    if !retire_first {
        while archive.chat_projection.advance("agent-1").unwrap() {}
    }
    let conversation = archive.active_conversation_id("agent-1").unwrap().unwrap();
    let directory = conversation_dir("agent-1", &conversation).unwrap();
    let capture_path = save_capture(&context, &capture);
    assert!(source_policy(&context).is_err());
    assert!(
        crate::state::conversation_archive::chat_logical_index::source_proof(&context)
            .unwrap()
            .is_none()
    );

    // Derived display admission must leave the canonical archive and capture policy intact.
    let protected = [
        directory.join("events.jsonl"),
        directory.join("conversation.jsonl"),
        directory.join("manifest.json"),
        directory.join("sources.jsonl"),
        directory.join("turns.jsonl"),
        capture_path,
        policy_path("agent-1").unwrap(),
    ]
    .into_iter()
    .map(|path| {
        let bytes = fs::read(&path).unwrap();
        (path, bytes)
    })
    .collect::<Vec<_>>();
    if retire_first {
        retire("agent-1").unwrap();
    } else {
        let mut store = Store::new(&locations("agent-1").unwrap().1);
        let previous: Head = store.read(&head("agent-1").unwrap().unwrap()).unwrap();
        assert_eq!(previous.progress, "ready");
        assert!(
            previous.bootstrap.is_none(),
            "reproduce a normal committed head after restart"
        );
    }
    drop(archive);
    let mut settled = false;
    for _ in 0..128 {
        if !ConversationArchiveState::default()
            .bootstrap_saved_chat(&context)
            .unwrap()
        {
            settled = true;
            break;
        }
    }
    assert!(
        settled,
        "cold archive display admission must settle after restart"
    );
    let page = read(&context, &snapshot(&context), None, None, None).unwrap();
    // A blocked recent source may still serve saved archive rows.
    assert!(page.source_epoch.is_none());
    let archived_ids = fs::read_to_string(directory.join("events.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<AgentChatEvent>(line).unwrap().id)
        .collect::<BTreeSet<_>>();
    let mut expected_aliases = BTreeSet::new();
    for event in &page.events {
        let Some(members) = event.metadata["chat_display_member_ids"].as_array() else {
            continue;
        };
        assert!(event.id.starts_with("archive-display:"));
        assert_eq!(members.len(), 2);
        for member in members {
            let id = member.as_str().unwrap();
            assert!(!id.starts_with("source:"));
            assert!(
                archived_ids.contains(id),
                "display members must be physical archive rows"
            );
            expected_aliases.insert((id.to_owned(), event.id.clone()));
        }
    }
    // Presentation aliases reconcile loaded archive rows with their display
    // group. They cannot introduce native source-coordinate correspondence.
    let actual_aliases = page
        .aliases
        .iter()
        .map(|alias| {
            assert!(!alias.observation_id.starts_with("source:"));
            assert!(alias.canonical_id.starts_with("archive-display:"));
            (alias.observation_id.clone(), alias.canonical_id.clone())
        })
        .collect::<BTreeSet<_>>();
    assert_eq!(
        actual_aliases, expected_aliases,
        "aliases must match exact archived display membership"
    );
    assert!(page
        .events
        .iter()
        .all(|event| event.metadata["chat_source_ref"].is_null()
            && event.metadata["chat_source_epoch"].is_null()));
    for (path, before) in protected {
        assert_eq!(
            fs::read(&path).unwrap(),
            before,
            "bootstrap changed {}",
            path.display()
        );
    }
    page
}

fn assistant_rows(page: &AgentChatPage) -> Vec<&AgentChatEvent> {
    page.events
        .iter()
        .filter(|event| {
            event.kind == AgentChatEventKind::Message
                && event.role == Some(AgentChatRole::Assistant)
        })
        .collect()
}

#[test]
fn archive_display_coalesces_unique_legacy_pair_without_native_source_proof() {
    let (_guard, temp) = isolated_home();
    let text = "A complete sentence about the paired client. ".repeat(48);
    assert!(text.len() > PREVIEW_BYTES);
    let events = legacy_pair(temp.path(), &text);
    let page = cold_page(temp.path(), &events);
    let rows = assistant_rows(&page);
    assert_eq!(
        rows.len(),
        1,
        "one archived mirror/completion pair must display once"
    );
    let members = rows[0].metadata["chat_display_member_ids"]
        .as_array()
        .expect("the display relation retains both physical observations");
    assert_eq!(members.len(), 2);
    for event in &events {
        assert!(members
            .iter()
            .any(|member| member.as_str() == Some(event.id.as_str())));
    }
}

#[test]
fn archive_display_source_free_replay_advances_across_restart_and_stays_quiet() {
    const USERS: usize = 512;
    let (_guard, temp) = isolated_home();
    let context = context();
    let mut events = legacy_pair(temp.path(), "The saved assistant answer is complete.");
    for index in 0..USERS {
        let mut event = events[0].clone();
        event.id = format!("saved-user-{index}");
        event.role = Some(AgentChatRole::User);
        event.text = Some(format!("Saved user message {index}"));
        event.metadata["raw_type"] = json!("user_message");
        event.metadata["provider_turn_id"] = json!(format!("saved-turn-{index}"));
        events.push(event);
    }
    let mut archive = ConversationArchiveState::default();
    archive
        .append_chat_events_with_context(context.clone(), &events)
        .unwrap();
    assert!(
        crate::state::conversation_archive::chat_logical_index::source_proof(&context)
            .unwrap()
            .is_none()
    );
    let mut warm_settled = false;
    for _ in 0..256 {
        if !archive.chat_projection.advance("agent-1").unwrap() {
            warm_settled = true;
            break;
        }
    }
    assert!(
        warm_settled,
        "ordinary queues must finish in bounded checkpoints"
    );
    assert!(archive.chat_projection.has_pending_capture_work("agent-1"));

    let mut previous: Option<(String, (u64, u64))> = None;
    let mut advanced = false;
    let mut restarted = false;
    let mut settled = false;
    for checkpoint in 0..1024 {
        let pending = archive.bootstrap_saved_chat(&context).unwrap();
        if let Some(reference) = head("agent-1").unwrap() {
            let mut store = Store::new(&locations("agent-1").unwrap().1);
            let published: Head = store.read(&reference).unwrap();
            if let Some(progress) = published.bootstrap.as_ref() {
                let cursors = (progress.narrative_before, progress.event_before);
                if let Some((generation, before)) = previous.as_ref() {
                    assert_eq!(
                        &published.generation, generation,
                        "unchanged inputs must not reselect"
                    );
                    assert!(
                        cursors.0 <= before.0 && cursors.1 <= before.1,
                        "physical replay cursors must never move backward"
                    );
                    advanced |= cursors != *before;
                }
                previous = Some((published.generation, cursors));
            }
        }
        if checkpoint == 6 {
            assert!(
                pending,
                "the large cohort must still have persisted work at restart"
            );
            archive = ConversationArchiveState::default();
            restarted = true;
        }
        if !pending && !archive.chat_projection.advance_capture("agent-1").unwrap() {
            settled = true;
            break;
        }
    }
    assert!(
        settled && restarted && advanced,
        "source-free recount must advance, survive restart and terminate"
    );
    assert!(cold_bootstrap::archive_display_ready(&context).unwrap());
    let mut users = BTreeSet::new();
    let mut assistants = 0;
    let mut cursor = None;
    let mut exhausted = false;
    for _ in 0..128 {
        let page = read(&context, &snapshot(&context), cursor.as_deref(), None, None).unwrap();
        for event in page.events {
            match event.role {
                Some(AgentChatRole::User) => assert!(users.insert(event.id)),
                Some(AgentChatRole::Assistant) => {
                    assistants += 1;
                    assert!(event.id.starts_with("archive-display:"));
                }
                _ => panic!("unexpected saved message kind"),
            }
        }
        cursor = page.next_before;
        if cursor.is_none() {
            exhausted = true;
            break;
        }
    }
    assert!(exhausted, "the full saved cohort must exhaust Older");
    assert_eq!(
        users,
        (0..USERS)
            .map(|index| format!("saved-user-{index}"))
            .collect()
    );
    assert_eq!(assistants, 1);
    let revision = head("agent-1").unwrap();
    for _ in 0..3 {
        assert!(!archive.bootstrap_saved_chat(&context).unwrap());
        assert!(!archive.chat_projection.advance_capture("agent-1").unwrap());
        assert_eq!(
            head("agent-1").unwrap(),
            revision,
            "quiet input cannot restart the recount"
        );
    }
}

#[test]
fn archive_display_upgrades_ready_committed_head_without_explicit_retire() {
    let (_guard, temp) = isolated_home();
    let events = legacy_pair(temp.path(), &"A saved answer. ".repeat(90));
    let page = saved_page(temp.path(), &events, false);
    let rows = assistant_rows(&page);
    assert_eq!(
        rows.len(),
        1,
        "ordinary background bootstrap must upgrade a ready persisted head"
    );
    assert!(rows[0].id.starts_with("archive-display:"));
}

#[test]
fn archive_display_stays_grouped_after_a_normal_delivered_input() {
    let (_guard, temp) = isolated_home();
    let events = legacy_pair(temp.path(), &"A saved answer. ".repeat(90));
    let initial = cold_page(temp.path(), &events);
    assert_eq!(assistant_rows(&initial).len(), 1);
    let context = context();
    let capture_path = locations("agent-1")
        .unwrap()
        .0
        .with_file_name("conversation-capture.json");
    let capture_before = fs::read(&capture_path).unwrap();
    let archive = ConversationArchiveState::default();
    archive
        .append_delivered_input_with_context(context.clone(), "A later user prompt", None)
        .unwrap();
    for _ in 0..128 {
        let bootstrapping = archive.bootstrap_saved_chat(&context).unwrap();
        let indexing = archive.chat_projection.advance_capture("agent-1").unwrap();
        if !bootstrapping && !indexing {
            break;
        }
    }
    let page = read(&context, &snapshot(&context), None, None, None).unwrap();
    assert_eq!(
        assistant_rows(&page).len(),
        1,
        "a normal delivered input must preserve archived assistant grouping"
    );
    assert!(page
        .events
        .iter()
        .any(|event| event.text.as_deref() == Some("A later user prompt")));
    assert_eq!(fs::read(capture_path).unwrap(), capture_before);
}

#[test]
fn archive_display_retries_and_restarts_after_input_during_handoff() {
    let (_guard, temp) = isolated_home();
    let events = padded_pair(temp.path());
    let initial = cold_page(temp.path(), &events);
    let context = context();
    let archive = ConversationArchiveState::default();
    archive
        .append_delivered_input_with_context(context.clone(), "First prompt", None)
        .unwrap();
    let checkpoint = begin_recount(&archive, &context);
    let fence = InputFence {
        conversation_id: checkpoint.conversation_id.clone(),
        source_epoch: checkpoint.source_epoch.clone(),
        policy_generation: None,
    };
    assert!(archive
        .chat_projection
        .commit_input(&context, &checkpoint.conversation_id, &fence, || {
            panic!("a handed-off warm owner must not execute a receipt write");
        })
        .unwrap()
        .is_none());
    let before_append = head("agent-1").unwrap();
    archive
        .append_delivered_input_with_context(context.clone(), "Second prompt", None)
        .unwrap();
    assert_eq!(
        head("agent-1").unwrap(),
        before_append,
        "an append must not republish retained warm roots"
    );
    let directory = conversation_dir("agent-1", initial.conversation_id.as_ref().unwrap()).unwrap();
    let head_path = locations("agent-1").unwrap().0;
    assert!(head_path.starts_with(temp.path()));
    assert!(directory.starts_with(temp.path()));
    let backup = directory.join("head-handoff-backup.json");
    fs::rename(&head_path, &backup).unwrap();
    fs::create_dir(&head_path).unwrap();
    assert!(archive.bootstrap_saved_chat(&context).is_err());
    assert!(archive.chat_projection.has_pending_capture_work("agent-1"));
    fs::remove_dir(&head_path).unwrap();
    fs::rename(backup, &head_path).unwrap();
    drop(archive);
    let restarted = ConversationArchiveState::default();
    drain_capture(&restarted, &context);
    let page = read(&context, &snapshot(&context), None, None, None).unwrap();
    let assistants = assistant_rows(&page);
    assert_eq!(assistants.len(), 1);
    assert_eq!(
        complete_body(&context, &page, assistants[0]),
        events[0].text.as_deref().unwrap()
    );
    for prompt in ["First prompt", "Second prompt"] {
        assert_eq!(
            page.events
                .iter()
                .filter(|event| event.text.as_deref() == Some(prompt))
                .count(),
            1
        );
    }
    let settled = head("agent-1").unwrap();
    assert!(!restarted.bootstrap_saved_chat(&context).unwrap());
    assert!(!restarted
        .chat_projection
        .advance_capture("agent-1")
        .unwrap());
    assert_eq!(head("agent-1").unwrap(), settled);
}

#[test]
fn archive_display_recounts_a_physical_collision_absent_from_candidate_identity() {
    let (_guard, temp) = isolated_home();
    let events = padded_pair(temp.path());
    let initial = cold_page(temp.path(), &events);
    let context = context();
    let archive = ConversationArchiveState::default();
    archive
        .append_delivered_input_with_context(context.clone(), "First prompt", None)
        .unwrap();
    begin_recount(&archive, &context);
    let directory = conversation_dir("agent-1", initial.conversation_id.as_ref().unwrap()).unwrap();
    wardian_core::conversations::append_jsonl_record(&directory.join("events.jsonl"), &events[1])
        .unwrap();
    let before_append = head("agent-1").unwrap();
    archive
        .append_delivered_input_with_context(context.clone(), "Collision prompt", None)
        .expect_err("the ordinary writer refuses ambiguous durable identities before materializing a Candidate");
    assert_eq!(head("agent-1").unwrap(), before_append);
    drain_capture(&archive, &context);
    let page = read(&context, &snapshot(&context), None, None, None).unwrap();
    let assistants = assistant_rows(&page);
    assert_eq!(
        assistants.len(),
        2,
        "both complementary forms stay separate when one physical side repeats"
    );
    assert!(assistants
        .iter()
        .all(|event| event.metadata["chat_display_member_ids"].is_null()));
    assert!(!page
        .events
        .iter()
        .any(|event| event.text.as_deref() == Some("Collision prompt")));
}

#[test]
fn archive_display_handoff_retires_without_resurrecting_warm_state() {
    for returning_native in [false, true] {
        let (_guard, temp) = isolated_home();
        let events = padded_pair(temp.path());
        cold_page(temp.path(), &events);
        let context = context();
        let archive = ConversationArchiveState::default();
        archive
            .append_delivered_input_with_context(context.clone(), "A pending prompt", None)
            .unwrap();
        begin_recount(&archive, &context);
        if returning_native {
            let path = locations("agent-1")
                .unwrap()
                .0
                .with_file_name("conversation-capture.json");
            let state: serde_json::Value =
                serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
            let capture: ProviderLogCaptureState =
                serde_json::from_value(state["provider_log_sources"][0].clone()).unwrap();
            save_capture(&context, &current_capture(capture));
            let before = head("agent-1").unwrap();
            assert!(!archive.bootstrap_saved_chat(&context).unwrap());
            assert_eq!(head("agent-1").unwrap(), before);
        } else {
            archive
                .rollover_agent("agent-1", ConversationBoundaryReason::Clear)
                .unwrap();
            assert!(head("agent-1").unwrap().is_none());
        }
        assert!(!archive.chat_projection.has_pending_capture_work("agent-1"));
        assert!(!archive.chat_projection.advance_capture("agent-1").unwrap());
    }
}

#[test]
fn archive_display_preserves_a_committed_head_with_current_native_proof() {
    let (_guard, temp) = isolated_home();
    let events = legacy_pair(temp.path(), "A saved answer.");
    let page = cold_page(temp.path(), &events);
    let context = context();
    let capture_path = locations("agent-1")
        .unwrap()
        .0
        .with_file_name("conversation-capture.json");
    let state: serde_json::Value =
        serde_json::from_slice(&fs::read(&capture_path).unwrap()).unwrap();
    let capture: ProviderLogCaptureState =
        serde_json::from_value(state["provider_log_sources"][0].clone()).unwrap();
    save_capture(&context, &current_capture(capture));
    assert!(
        crate::state::conversation_archive::chat_logical_index::source_proof(&context)
            .unwrap()
            .is_some()
    );
    let owner = ProjectionOwner::default();
    owner
        .committed(
            Candidate {
                context: context.clone(),
                conversation_id: page.conversation_id.unwrap(),
                events,
                committed_output_bytes: 0,
                verified_ids: Default::default(),
                generated_ids: Default::default(),
                generated_input_bindings: Default::default(),
                source_epoch: None,
            },
            "native-ready".into(),
        )
        .unwrap();
    while owner.advance("agent-1").unwrap() {}
    drop(owner);
    let before = head("agent-1").unwrap();
    let mut store = Store::new(&locations("agent-1").unwrap().1);
    let published: Head = store.read(before.as_ref().unwrap()).unwrap();
    assert!(published.bootstrap.is_none());
    assert_eq!(published.progress, "ready");
    assert!(!ConversationArchiveState::default()
        .bootstrap_saved_chat(&context)
        .unwrap());
    assert_eq!(
        head("agent-1").unwrap(),
        before,
        "a current native proof retains its canonical head"
    );
}

#[test]
fn archive_display_keeps_ambiguous_completions_separate() {
    let (_guard, temp) = isolated_home();
    let mut events = legacy_pair(temp.path(), "The same answer.");
    let mut second = events[1].clone();
    second.id = "agent-1:provider_log:completed-two".into();
    second.turn_id = Some("native-message-two".into());
    events.push(second);
    let page = cold_page(temp.path(), &events);
    assert_eq!(assistant_rows(&page).len(), 3);
    assert!(page
        .events
        .iter()
        .all(|event| event.metadata["chat_display_member_ids"].is_null()));
}

#[test]
fn archive_display_keeps_different_provider_turns_separate() {
    let (_guard, temp) = isolated_home();
    let mut events = legacy_pair(temp.path(), "The same answer.");
    events[1].metadata["provider_turn_id"] = json!("turn-two");
    let page = cold_page(temp.path(), &events);
    assert_eq!(assistant_rows(&page).len(), 2);
}

#[test]
fn archive_display_keeps_different_log_sources_separate() {
    let (_guard, temp) = isolated_home();
    let mut events = legacy_pair(temp.path(), "The same answer.");
    let other = temp.path().join("other-rollout.jsonl");
    fs::write(&other, b"another provider source\n").unwrap();
    events[1].metadata["log_path"] = json!(other.canonicalize().unwrap());
    let page = cold_page(temp.path(), &events);
    assert_eq!(assistant_rows(&page).len(), 2);
}

#[test]
fn archive_display_uses_full_source_scope_before_header_clipping() {
    let (_guard, temp) = isolated_home();
    let mut events = legacy_pair(temp.path(), "The same answer.");
    let path = format!(
        "{}/{}rollout.jsonl",
        events[0].metadata["log_path"].as_str().unwrap(),
        "archived-segment/".repeat(32)
    );
    assert!(path.len() > 256);
    for event in &mut events {
        event.metadata["log_path"] = json!(path);
    }
    let page = cold_page(temp.path(), &events);
    assert_eq!(assistant_rows(&page).len(), 1);
}

#[test]
fn archive_display_keeps_sources_with_identical_clipped_paths_separate() {
    let (_guard, temp) = isolated_home();
    let mut events = legacy_pair(temp.path(), "The same answer.");
    let prefix = format!(
        "{}/{}",
        events[0].metadata["log_path"].as_str().unwrap(),
        "archived-segment/".repeat(32)
    );
    events[0].metadata["log_path"] = json!(format!("{prefix}left.jsonl"));
    events[1].metadata["log_path"] = json!(format!("{prefix}rght.jsonl"));
    let page = cold_page(temp.path(), &events);
    let rows = assistant_rows(&page);
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].metadata["log_path"], rows[1].metadata["log_path"]);
}

#[test]
fn archive_display_counts_older_ambiguity_across_bootstrap_checkpoints() {
    let (_guard, temp) = isolated_home();
    let pair = legacy_pair(temp.path(), "The same answer.");
    let mut second = pair[1].clone();
    second.id = "agent-1:provider_log:older-completion".into();
    second.turn_id = Some("older-native-message".into());
    let mut events = vec![second];
    for index in 0..ADMIT_ROWS + 8 {
        let mut filler = pair[1].clone();
        filler.id = format!("agent-1:provider_log:filler-{index}");
        filler.turn_id = Some(format!("filler-native-message-{index}"));
        filler.text = Some(format!("Unrelated archived answer {index}."));
        events.push(filler);
    }
    events.extend(pair);
    let page = cold_page(temp.path(), &events);
    let target = assistant_rows(&page)
        .into_iter()
        .filter(|event| event.text.as_deref() == Some("The same answer."))
        .collect::<Vec<_>>();
    assert_eq!(target.len(), 3);
    assert!(target
        .iter()
        .all(|event| event.metadata["chat_display_member_ids"].is_null()));
}

#[test]
fn archive_display_compares_full_bodies_beyond_identical_previews() {
    let (_guard, temp) = isolated_home();
    let prefix = "The shared preview is insufficient evidence. ".repeat(48);
    let mut events = legacy_pair(temp.path(), &format!("{prefix}one"));
    events[1].text = Some(format!("{prefix}two"));
    assert_eq!(
        events[0].text.as_ref().unwrap().len(),
        events[1].text.as_ref().unwrap().len()
    );
    let page = cold_page(temp.path(), &events);
    assert_eq!(assistant_rows(&page).len(), 2);
}

#[test]
fn archive_display_relation_is_recounted_for_a_new_archive_generation() {
    let (_guard, temp) = isolated_home();
    let events = legacy_pair(temp.path(), "The same answer.");
    let before = cold_page(temp.path(), &events);
    assert_eq!(assistant_rows(&before).len(), 1);
    let mut second = events[1].clone();
    second.id = "agent-1:provider_log:completed-two".into();
    second.turn_id = Some("native-message-two".into());
    let after = cold_page(temp.path(), &[second]);
    assert_ne!(before.generation, after.generation);
    assert_eq!(
        assistant_rows(&after).len(),
        3,
        "new archive extent invalidates prior uniqueness"
    );
}

#[test]
fn archive_display_relation_does_not_cross_conversations() {
    let (_guard, temp) = isolated_home();
    let events = legacy_pair(temp.path(), "The same answer.");
    let archive = ConversationArchiveState::default();
    archive
        .append_chat_events_with_context(context(), &events[..1])
        .unwrap();
    let old = archive.active_conversation_id("agent-1").unwrap().unwrap();
    archive
        .rollover_agent("agent-1", ConversationBoundaryReason::Clear)
        .unwrap();
    let page = cold_page(temp.path(), &events[1..]);
    assert_ne!(page.conversation_id.as_deref(), Some(old.as_str()));
    let rows = assistant_rows(&page);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, events[1].id);
    assert!(rows[0].metadata["chat_display_member_ids"].is_null());
}
