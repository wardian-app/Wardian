use super::{
    derive_turn_records, effective_conversation_logging, lifecycle_record,
    narrative_from_chat_event, narrative_from_delivered_input, new_conversation_id,
    ActiveConversationHandle, ConversationArchiveContext, ConversationArchiveState,
};
use wardian_core::conversations::{
    read_jsonl_records, AgentConversationLoggingSetting, ConversationBoundaryReason,
    ConversationLoggingSetting, ConversationManifest, ConversationNarrativeRecord,
    ConversationRecordKind, ConversationSourceRecord, ConversationSpeakerType, ConversationStatus,
    ConversationTurnRecord, ConversationTurnStatus, CONVERSATION_SCHEMA,
};
use wardian_core::models::chat::{
    AgentChatEvent, AgentChatEventKind, AgentChatRole, AgentChatStatus,
};
use wardian_core::paths::{agent_conversation_dir, agent_conversations_dir};

#[test]
fn user_chat_message_converts_to_primary_narrative_record() {
    let event = chat_event(
        "event-1",
        AgentChatEventKind::Message,
        Some(AgentChatRole::User),
        Some("Please inspect the workspace."),
    );

    let record = narrative_from_chat_event(&event, 42).expect("narrative record");

    assert_eq!(record.schema, CONVERSATION_SCHEMA);
    assert_eq!(record.seq, 42);
    assert_eq!(record.at, "2026-06-15T00:00:00.000Z");
    assert_eq!(record.kind, ConversationRecordKind::Message);
    assert_eq!(record.role.as_deref(), Some("user"));
    assert_eq!(record.speaker_type, Some(ConversationSpeakerType::User));
    assert_eq!(
        record.text.as_deref(),
        Some("Please inspect the workspace.")
    );
    assert_eq!(record.event_refs, vec!["event-1".to_string()]);
}

#[test]
fn terminal_output_is_not_primary_narrative() {
    let event = chat_event(
        "event-terminal",
        AgentChatEventKind::TerminalOutput,
        None,
        Some("raw terminal output"),
    );

    assert!(narrative_from_chat_event(&event, 1).is_none());
}

#[test]
fn status_events_are_not_primary_narrative() {
    let event = chat_event(
        "event-status",
        AgentChatEventKind::Status,
        None,
        Some("Idle"),
    );

    assert!(narrative_from_chat_event(&event, 1).is_none());
}

#[test]
fn delivered_input_becomes_user_narrative_record() {
    let record = narrative_from_delivered_input(
        "2026-06-15T00:00:01.000Z",
        "Please review the patch.",
        Some("source-agent"),
        7,
    );

    assert_eq!(record.schema, CONVERSATION_SCHEMA);
    assert_eq!(record.seq, 7);
    assert_eq!(record.at, "2026-06-15T00:00:01.000Z");
    assert_eq!(record.kind, ConversationRecordKind::Message);
    assert_eq!(record.role.as_deref(), Some("user"));
    assert_eq!(record.speaker_type, Some(ConversationSpeakerType::Agent));
    assert_eq!(record.text.as_deref(), Some("Please review the patch."));
}

#[test]
fn lifecycle_record_captures_clear_reason() {
    let record = lifecycle_record(
        8,
        ConversationBoundaryReason::Clear,
        "2026-06-15T00:00:02.000Z",
    );

    assert_eq!(record.schema, CONVERSATION_SCHEMA);
    assert_eq!(record.seq, 8);
    assert_eq!(record.at, "2026-06-15T00:00:02.000Z");
    assert_eq!(record.kind, ConversationRecordKind::Lifecycle);
    assert_eq!(record.speaker_type, Some(ConversationSpeakerType::System));
    assert_eq!(record.status.as_deref(), Some("clear"));
}

#[test]
fn effective_logging_respects_agent_override_before_global_default() {
    assert_eq!(
        effective_conversation_logging(
            ConversationLoggingSetting::Disabled,
            AgentConversationLoggingSetting::Default,
        ),
        ConversationLoggingSetting::Disabled
    );
    assert_eq!(
        effective_conversation_logging(
            ConversationLoggingSetting::Disabled,
            AgentConversationLoggingSetting::Enabled,
        ),
        ConversationLoggingSetting::Enabled
    );
    assert_eq!(
        effective_conversation_logging(
            ConversationLoggingSetting::Enabled,
            AgentConversationLoggingSetting::Disabled,
        ),
        ConversationLoggingSetting::Disabled
    );
}

#[test]
fn rollover_closes_existing_handle() {
    let (_guard, _temp) = isolated_home();
    let archive = ConversationArchiveState::default();
    archive.set_active_for_test(
        "agent-1",
        ActiveConversationHandle {
            conversation_id: "conv_existing_agent_1".to_string(),
            next_seq: 1,
            provider_source_key: None,
        },
    );

    let conversation_id = archive
        .rollover_agent("agent-1", ConversationBoundaryReason::Clear)
        .expect("rollover succeeds");

    assert_eq!(conversation_id.as_deref(), Some("conv_existing_agent_1"));
    assert_eq!(archive.active_conversation_id_for_test("agent-1"), None);
}

#[test]
fn discard_agent_drops_active_handle_without_creating_archive_files() {
    let (_guard, _temp) = isolated_home();
    let archive = ConversationArchiveState::default();
    archive.set_active_for_test(
        "agent-1",
        ActiveConversationHandle {
            conversation_id: "conv_existing_agent_1".to_string(),
            next_seq: 1,
            provider_source_key: None,
        },
    );

    let conversation_id = archive.discard_agent("agent-1").expect("discard succeeds");

    assert_eq!(conversation_id.as_deref(), Some("conv_existing_agent_1"));
    assert_eq!(archive.active_conversation_id_for_test("agent-1"), None);
    assert!(!agent_conversations_dir("agent-1")
        .expect("conversations dir")
        .exists());
}

#[test]
fn append_chat_events_dedupes_repeated_loads() {
    let (_guard, _temp) = isolated_home();
    let archive = ConversationArchiveState::default();
    let event = chat_event(
        "event-1",
        AgentChatEventKind::Message,
        Some(AgentChatRole::Assistant),
        Some("Rendered from the transcript."),
    );

    let first_count = archive
        .append_chat_events("agent-1", std::slice::from_ref(&event))
        .expect("first append succeeds");
    let second_count = archive
        .append_chat_events("agent-1", &[event])
        .expect("second append succeeds");

    assert_eq!(first_count, 1);
    assert_eq!(second_count, 0);
    let conversation_id = archive
        .active_conversation_id_for_test("agent-1")
        .expect("active conversation id");
    let conversation_path =
        agent_conversation_dir("agent-1", &conversation_id).expect("conversation dir");
    let records: Vec<ConversationNarrativeRecord> =
        read_jsonl_records(&conversation_path.join("conversation.jsonl"))
            .expect("read narrative records");
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].event_refs, vec!["event-1".to_string()]);
}

#[test]
fn append_chat_events_writes_source_records_for_provider_metadata() {
    let (_guard, _temp) = isolated_home();
    let archive = ConversationArchiveState::default();
    let mut event = chat_event(
        "event-1",
        AgentChatEventKind::Message,
        Some(AgentChatRole::Assistant),
        Some("Rendered from the opencode database."),
    );
    event.provider = "opencode".to_string();
    event.turn_id = Some("message-1".to_string());
    event.source = Some("opencode_db".to_string());
    event.sequence = Some(42);
    event.metadata = serde_json::json!({
        "opencode_session_id": "ses_opencode",
        "part_id": "part_42",
        "raw_type": "text",
        "cursor": "opencode:part_42",
        "sequence": 42,
        "offset": 128
    });

    let appended = archive
        .append_chat_events("agent-1", &[event])
        .expect("append succeeds");

    assert_eq!(appended, 1);
    let conversation_id = archive
        .active_conversation_id_for_test("agent-1")
        .expect("active conversation id");
    let conversation_path =
        agent_conversation_dir("agent-1", &conversation_id).expect("conversation dir");
    let records: Vec<ConversationNarrativeRecord> =
        read_jsonl_records(&conversation_path.join("conversation.jsonl"))
            .expect("read narrative records");
    let sources: Vec<ConversationSourceRecord> =
        read_jsonl_records(&conversation_path.join("sources.jsonl")).expect("read source records");

    assert_eq!(sources.len(), 1);
    assert_eq!(sources[0].source_id, "src_1");
    assert_eq!(sources[0].provider, "opencode");
    assert_eq!(
        sources[0].provider_session_id.as_deref(),
        Some("ses_opencode")
    );
    assert_eq!(sources[0].source_kind, "opencode_db");
    assert_eq!(sources[0].cursor.as_deref(), Some("opencode:part_42"));
    assert_eq!(sources[0].offset, Some(128));
    assert_eq!(sources[0].row_id.as_deref(), Some("part_42"));
    assert_eq!(sources[0].provider_event_type.as_deref(), Some("text"));
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].source_refs, vec!["src_1".to_string()]);
}

#[test]
fn turn_id_does_not_become_provider_session_or_rollover_key() {
    let (_guard, _temp) = isolated_home();
    let archive = ConversationArchiveState::default();
    let mut first = chat_event(
        "event-1",
        AgentChatEventKind::Message,
        Some(AgentChatRole::Assistant),
        Some("First turn."),
    );
    first.provider = "codex".to_string();
    first.turn_id = Some("turn-1".to_string());
    first.source = Some("response_item".to_string());
    let mut second = chat_event(
        "event-2",
        AgentChatEventKind::Message,
        Some(AgentChatRole::Assistant),
        Some("Second turn."),
    );
    second.provider = "codex".to_string();
    second.turn_id = Some("turn-2".to_string());
    second.source = Some("response_item".to_string());

    archive
        .append_chat_events("agent-1", &[first])
        .expect("append first turn");
    let conversation_id = archive
        .active_conversation_id_for_test("agent-1")
        .expect("active conversation");
    archive
        .append_chat_events("agent-1", &[second])
        .expect("append second turn");

    assert_eq!(
        archive
            .active_conversation_id_for_test("agent-1")
            .as_deref(),
        Some(conversation_id.as_str())
    );
    let conversation_path =
        agent_conversation_dir("agent-1", &conversation_id).expect("conversation dir");
    let sources: Vec<ConversationSourceRecord> =
        read_jsonl_records(&conversation_path.join("sources.jsonl")).expect("read source records");
    assert_eq!(sources.len(), 2);
    assert!(sources
        .iter()
        .all(|source| source.provider_session_id.is_none()));
}

#[test]
fn command_string_is_not_used_as_turn_tool_name() {
    let event = AgentChatEvent {
        command: Some("cargo test --workspace".to_string()),
        title: None,
        metadata: serde_json::json!({}),
        ..chat_event(
            "event-tool",
            AgentChatEventKind::ToolCall,
            None,
            Some("Running tests."),
        )
    };

    let record = narrative_from_chat_event(&event, 1).expect("narrative record");

    assert_eq!(record.tool, None);
}

#[test]
fn provider_turn_ids_do_not_split_without_new_user_request() {
    let records = vec![
        narrative_record_with_turn(1, "turn-1", "First A."),
        narrative_record_with_turn(2, "turn-2", "Second."),
        narrative_record_with_turn(3, "turn-1", "First B."),
    ];

    let turns = derive_turn_records("conv-test", &records, &[], &[], false);

    assert_eq!(turns.len(), 1);
    assert_eq!(turns[0].conversation_id, "conv-test");
    assert_eq!(turns[0].turn_index, 1);
    assert_eq!(turns[0].turn_key, "conv-test:turn:000001");
    assert_eq!(turns[0].seq_start, 1);
    assert_eq!(turns[0].seq_end, 3);
    assert_eq!(turns[0].status, ConversationTurnStatus::Responded);
    assert_eq!(turns[0].request.kind, "unknown");
}

#[test]
fn request_kind_marks_goal_continuation_and_agent_context_rows() {
    let records = vec![
        narrative_from_delivered_input(
            "2026-06-15T00:00:01.000Z",
            "/goal Fix the archive index.",
            None,
            1,
        ),
        narrative_from_delivered_input(
            "2026-06-15T00:00:02.000Z",
            "<codex_internal_context source=\"goal\">\nGoal: Fix the archive index.\n\nLarge scaffold omitted.\n</codex_internal_context>",
            None,
            2,
        ),
        narrative_from_delivered_input(
            "2026-06-15T00:00:03.000Z",
            "# AGENTS.md instructions for <absolute-workspace-path>\nUse project rules.",
            None,
            3,
        ),
    ];

    let turns = derive_turn_records("conv-context", &records, &[], &[], true);

    assert_eq!(turns.len(), 3);
    assert_eq!(turns[0].request.kind, "goal_start");
    assert_eq!(turns[1].request.kind, "goal_continuation");
    let goal_json = serde_json::to_value(&turns[1]).unwrap();
    assert_eq!(
        goal_json["request"]["objective_text"],
        "Fix the archive index."
    );
    assert_eq!(turns[2].request.kind, "agent_context");
    assert_eq!(turns[2].status, ConversationTurnStatus::ContextOnly);
    assert_eq!(turns[2].status_source, "mechanical_context_only");
}

#[test]
fn closed_user_request_without_assistant_is_pending_response() {
    let records = vec![narrative_from_delivered_input(
        "2026-06-15T00:00:01.000Z",
        "Please inspect the archive.",
        None,
        1,
    )];

    let turns = derive_turn_records("conv-pending", &records, &[], &[], false);

    assert_eq!(turns.len(), 1);
    assert_eq!(turns[0].status, ConversationTurnStatus::PendingResponse);
    assert_eq!(turns[0].status_source, "mechanical_no_assistant_message");
}

#[test]
fn append_chat_events_writes_factual_turns_index() {
    let (_guard, _temp) = isolated_home();
    let archive = ConversationArchiveState::default();
    let mut user = chat_event(
        "event-user",
        AgentChatEventKind::Message,
        Some(AgentChatRole::User),
        Some("Run the tests."),
    );
    user.turn_id = Some("turn-1".to_string());
    user.source = Some("response_item".to_string());
    user.metadata = serde_json::json!({
        "log_path": "<absolute-workspace-path>/codex.jsonl",
        "raw_type": "message"
    });
    let mut tool = chat_event(
        "event-tool",
        AgentChatEventKind::ToolCall,
        None,
        Some("Need test output."),
    );
    tool.turn_id = Some("turn-1".to_string());
    tool.title = Some("shell_command".to_string());
    tool.command = Some("cargo test".to_string());
    tool.status = Some(AgentChatStatus::Running);
    tool.source = Some("response_item".to_string());
    let mut result = chat_event(
        "event-result",
        AgentChatEventKind::ToolResult,
        Some(AgentChatRole::Tool),
        Some("test failed"),
    );
    result.turn_id = Some("turn-1".to_string());
    result.title = Some("shell_command".to_string());
    result.status = Some(AgentChatStatus::Failed);
    result.exit_code = Some(101);
    result.source = Some("response_item".to_string());
    let mut assistant = chat_event(
        "event-assistant",
        AgentChatEventKind::Message,
        Some(AgentChatRole::Assistant),
        Some("The test failed."),
    );
    assistant.turn_id = Some("turn-1".to_string());
    assistant.source = Some("response_item".to_string());

    archive
        .append_chat_events_with_context(
            archive_context("session-one"),
            &[user, tool, result, assistant],
        )
        .expect("append events");

    let conversation_id = archive
        .active_conversation_id_for_test("agent-1")
        .expect("active conversation id");
    archive
        .rollover_agent("agent-1", ConversationBoundaryReason::Shutdown)
        .expect("close conversation");
    let conversation_path =
        agent_conversation_dir("agent-1", &conversation_id).expect("conversation dir");
    let records: Vec<ConversationNarrativeRecord> =
        read_jsonl_records(&conversation_path.join("conversation.jsonl"))
            .expect("read narrative records");
    let turns: Vec<ConversationTurnRecord> =
        read_jsonl_records(&conversation_path.join("turns.jsonl")).expect("read turns");
    let manifest: ConversationManifest = serde_json::from_str(
        &std::fs::read_to_string(conversation_path.join("manifest.json")).unwrap(),
    )
    .expect("read manifest");

    assert_eq!(records[0].turn_id.as_deref(), Some("turn-1"));
    assert_eq!(turns.len(), 1);
    assert_eq!(turns[0].conversation_id, conversation_id);
    assert_eq!(turns[0].turn_index, 1);
    assert_eq!(turns[0].turn_key, format!("{conversation_id}:turn:000001"));
    assert_eq!(turns[0].seq_start, 1);
    assert_eq!(turns[0].seq_end, 4);
    assert_eq!(turns[0].request.seq, 1);
    assert_eq!(turns[0].request.kind, "user_request");
    assert_eq!(turns[0].request.text.as_deref(), Some("Run the tests."));
    let assistant_result = turns[0]
        .assistant_result
        .as_ref()
        .expect("assistant result");
    assert_eq!(assistant_result.seq, 4);
    assert_eq!(assistant_result.text.as_deref(), Some("The test failed."));
    assert_eq!(turns[0].status, ConversationTurnStatus::Responded);
    assert_eq!(turns[0].status_source, "mechanical_assistant_message");
    assert_eq!(turns[0].counts.records, 4);
    assert_eq!(turns[0].counts.assistant_messages, 1);
    assert_eq!(turns[0].counts.tool_calls, 1);
    assert_eq!(turns[0].counts.tool_results, 1);
    assert_eq!(turns[0].counts.failed_tool_results, 1);
    assert_eq!(turns[0].counts.nonzero_tool_results, 1);
    assert_eq!(turns[0].tools_used.get("shell_command"), Some(&1));
    assert!(turns[0].files.read.is_empty());
    assert!(turns[0].files.written.is_empty());
    assert!(turns[0].external_side_effects.is_empty());
    assert_eq!(turns[0].failure_signals[0].kind, "tool_failed");
    assert_eq!(turns[0].failure_signals[0].seq, 3);
    assert!(turns[0]
        .record_refs
        .event_refs
        .contains(&"event-user".to_string()));
    assert_eq!(manifest.record_count, 4);
    assert_eq!(manifest.turn_count, 1);
    assert!(manifest.has_turns);
    assert!(!manifest.lifecycle_only);

    let json = serde_json::to_value(&turns[0]).unwrap();
    assert!(json.get("capture_quality").is_none());
    assert!(json.get("notes_for_evolver").is_none());
    assert!(json.get("user_correction").is_none());
    let manifest_json = serde_json::to_value(&manifest).unwrap();
    assert!(manifest_json.get("capture_quality").is_none());
}

#[test]
fn active_conversation_refresh_writes_turns_index_and_manifest_summary() {
    let (_guard, _temp) = isolated_home();
    let archive = ConversationArchiveState::default();
    let mut user = chat_event(
        "event-user-open",
        AgentChatEventKind::Message,
        Some(AgentChatRole::User),
        Some("Keep the archive useful while this task is still open."),
    );
    user.turn_id = Some("provider-user-turn".to_string());
    let mut assistant = chat_event(
        "event-assistant-open",
        AgentChatEventKind::Message,
        Some(AgentChatRole::Assistant),
        Some("I am still working on it."),
    );
    assistant.turn_id = Some("provider-assistant-turn".to_string());

    archive
        .append_chat_events_with_context(archive_context("session-one"), &[user, assistant])
        .expect("append open events");

    let conversation_id = archive
        .active_conversation_id_for_test("agent-1")
        .expect("active conversation id");
    let conversation_path =
        agent_conversation_dir("agent-1", &conversation_id).expect("conversation dir");
    let turns_path = conversation_path.join("turns.jsonl");
    let turns: Vec<serde_json::Value> = read_jsonl_records(&turns_path).expect("read turns");
    let manifest: ConversationManifest = serde_json::from_str(
        &std::fs::read_to_string(conversation_path.join("manifest.json")).unwrap(),
    )
    .expect("read manifest");

    assert!(turns_path.exists());
    assert_eq!(turns.len(), 1);
    assert_eq!(turns[0]["conversation_id"], conversation_id);
    assert_eq!(turns[0]["turn_index"], 1);
    assert_eq!(
        turns[0]["turn_key"],
        format!("{conversation_id}:turn:000001")
    );
    assert_eq!(turns[0]["status"], "responded");
    assert_eq!(turns[0]["status_source"], "mechanical_assistant_message");
    assert_eq!(turns[0]["request"]["kind"], "user_request");
    assert_eq!(turns[0]["request"]["seq"], 1);
    assert_eq!(turns[0]["schema"], 2);
    assert_eq!(
        turns[0]["provider_native_refs"][0]["provider_session_id"],
        "session-one"
    );
    assert_eq!(
        turns[0]["provider_native_refs"][0]["source_kind"],
        "provider_session"
    );
    assert_eq!(
        turns[0]["assistant_result"]["text"],
        "I am still working on it."
    );
    assert_eq!(turns[0]["counts"]["records"], 2);
    assert_eq!(turns[0]["counts"]["assistant_messages"], 1);
    assert_eq!(manifest.turn_count, 1);
    assert!(manifest.has_turns);
}

#[test]
fn provider_tool_call_turn_ids_stay_inside_surrounding_user_request_turn() {
    let (_guard, _temp) = isolated_home();
    let archive = ConversationArchiveState::default();
    let mut user = chat_event(
        "event-user-tool-heavy",
        AgentChatEventKind::Message,
        Some(AgentChatRole::User),
        Some("Inspect the project and fix the bug."),
    );
    user.turn_id = Some("provider-user-turn".to_string());
    let mut first_tool = chat_event(
        "event-tool-a",
        AgentChatEventKind::ToolCall,
        None,
        Some("Read a file."),
    );
    first_tool.turn_id = Some("tool-call-a".to_string());
    first_tool.title = Some("shell_command".to_string());
    let mut first_result = chat_event(
        "event-result-a",
        AgentChatEventKind::ToolResult,
        Some(AgentChatRole::Tool),
        Some("file contents"),
    );
    first_result.turn_id = Some("tool-call-a".to_string());
    first_result.title = Some("shell_command".to_string());
    let mut second_tool = chat_event(
        "event-tool-b",
        AgentChatEventKind::ToolCall,
        None,
        Some("Patch a file."),
    );
    second_tool.turn_id = Some("tool-call-b".to_string());
    second_tool.title = Some("apply_patch".to_string());
    let mut second_result = chat_event(
        "event-result-b",
        AgentChatEventKind::ToolResult,
        Some(AgentChatRole::Tool),
        Some("patch applied"),
    );
    second_result.turn_id = Some("tool-call-b".to_string());
    second_result.title = Some("apply_patch".to_string());
    let mut assistant = chat_event(
        "event-assistant-tool-heavy",
        AgentChatEventKind::Message,
        Some(AgentChatRole::Assistant),
        Some("Implemented the fix."),
    );
    assistant.turn_id = Some("provider-assistant-turn".to_string());

    archive
        .append_chat_events_with_context(
            archive_context("session-one"),
            &[
                user,
                first_tool,
                first_result,
                second_tool,
                second_result,
                assistant,
            ],
        )
        .expect("append tool-heavy request");

    let conversation_id = archive
        .active_conversation_id_for_test("agent-1")
        .expect("active conversation id");
    let conversation_path =
        agent_conversation_dir("agent-1", &conversation_id).expect("conversation dir");
    let turns: Vec<serde_json::Value> =
        read_jsonl_records(&conversation_path.join("turns.jsonl")).expect("read turns");

    assert_eq!(turns.len(), 1);
    assert_eq!(turns[0]["seq_start"], 1);
    assert_eq!(turns[0]["seq_end"], 6);
    assert_eq!(
        turns[0]["request"]["text"],
        "Inspect the project and fix the bug."
    );
    assert_eq!(turns[0]["assistant_result"]["text"], "Implemented the fix.");
    assert_eq!(turns[0]["counts"]["records"], 6);
    assert_eq!(turns[0]["counts"]["tool_calls"], 2);
    assert_eq!(turns[0]["counts"]["tool_results"], 2);
    assert_eq!(turns[0]["tools_used"]["shell_command"], 1);
    assert_eq!(turns[0]["tools_used"]["apply_patch"], 1);
}

#[test]
fn apply_patch_turn_extracts_written_and_mentioned_paths() {
    let (_guard, _temp) = isolated_home();
    let archive = ConversationArchiveState::default();
    let mut user = chat_event(
        "event-user-paths",
        AgentChatEventKind::Message,
        Some(AgentChatRole::User),
        Some("Fix the turns index."),
    );
    user.turn_id = Some("provider-user-turn".to_string());
    let mut patch = chat_event(
        "event-tool-patch",
        AgentChatEventKind::ToolCall,
        None,
        Some(
            "*** Begin Patch\n\
             *** Update File: src-tauri/src/state/conversation_archive/turns.rs\n\
             @@\n\
             -old\n\
             +new\n\
             *** Add File: docs/specs/2026-06-25-turns-jsonl-request-index.md\n\
             +# Turns index\n\
             *** End Patch",
        ),
    );
    patch.turn_id = Some("tool-call-patch".to_string());
    patch.title = Some("apply_patch".to_string());
    let mut assistant = chat_event(
        "event-assistant-paths",
        AgentChatEventKind::Message,
        Some(AgentChatRole::Assistant),
        Some(
            "Updated src-tauri/src/state/conversation_archive/turns.rs and docs/specs/2026-06-25-turns-jsonl-request-index.md.",
        ),
    );
    assistant.turn_id = Some("provider-assistant-turn".to_string());

    archive
        .append_chat_events_with_context(archive_context("session-one"), &[user, patch, assistant])
        .expect("append patch turn");

    let conversation_id = archive
        .active_conversation_id_for_test("agent-1")
        .expect("active conversation id");
    let conversation_path =
        agent_conversation_dir("agent-1", &conversation_id).expect("conversation dir");
    let turns: Vec<serde_json::Value> =
        read_jsonl_records(&conversation_path.join("turns.jsonl")).expect("read turns");

    assert_eq!(turns.len(), 1);
    assert_eq!(turns[0]["schema"], 2);
    assert_eq!(
        turns[0]["files"]["written"],
        serde_json::json!([
            "src-tauri/src/state/conversation_archive/turns.rs",
            "docs/specs/2026-06-25-turns-jsonl-request-index.md"
        ])
    );
    assert_eq!(
        turns[0]["files"]["mentioned"],
        serde_json::json!([
            "src-tauri/src/state/conversation_archive/turns.rs",
            "docs/specs/2026-06-25-turns-jsonl-request-index.md"
        ])
    );
    assert_eq!(turns[0]["external_side_effects"][0]["kind"], "file_edit");
    assert_eq!(
        turns[0]["external_side_effects"][0]["paths"],
        serde_json::json!([
            "src-tauri/src/state/conversation_archive/turns.rs",
            "docs/specs/2026-06-25-turns-jsonl-request-index.md"
        ])
    );
    assert!(turns[0]["external_side_effects"][0]["summary"]
        .as_str()
        .unwrap()
        .contains("src-tauri/src/state/conversation_archive/turns.rs"));
}

#[test]
fn active_open_task_with_one_hundred_tool_calls_writes_one_request_turn() {
    let (_guard, _temp) = isolated_home();
    let archive = ConversationArchiveState::default();
    let mut events = Vec::new();
    let mut user = chat_event(
        "event-user-hundred-tools",
        AgentChatEventKind::Message,
        Some(AgentChatRole::User),
        Some("Run the large investigation."),
    );
    user.turn_id = Some("provider-user-turn".to_string());
    events.push(user);
    for index in 0..100 {
        let tool_name = if index % 2 == 0 {
            "shell_command"
        } else {
            "apply_patch"
        };
        let mut tool = chat_event(
            &format!("event-tool-{index}"),
            AgentChatEventKind::ToolCall,
            None,
            Some("tool call"),
        );
        tool.turn_id = Some(format!("tool-call-{index}"));
        tool.title = Some(tool_name.to_string());
        events.push(tool);
        let mut result = chat_event(
            &format!("event-result-{index}"),
            AgentChatEventKind::ToolResult,
            Some(AgentChatRole::Tool),
            Some("tool result"),
        );
        result.turn_id = Some(format!("tool-call-{index}"));
        result.title = Some(tool_name.to_string());
        events.push(result);
    }
    let mut assistant = chat_event(
        "event-assistant-hundred-tools",
        AgentChatEventKind::Message,
        Some(AgentChatRole::Assistant),
        Some("Large investigation complete."),
    );
    assistant.turn_id = Some("provider-assistant-turn".to_string());
    events.push(assistant);

    archive
        .append_chat_events_with_context(archive_context("session-one"), &events)
        .expect("append tool-heavy open task");

    let conversation_id = archive
        .active_conversation_id_for_test("agent-1")
        .expect("active conversation id");
    let conversation_path =
        agent_conversation_dir("agent-1", &conversation_id).expect("conversation dir");
    let turns: Vec<serde_json::Value> =
        read_jsonl_records(&conversation_path.join("turns.jsonl")).expect("read turns");
    let manifest: ConversationManifest = serde_json::from_str(
        &std::fs::read_to_string(conversation_path.join("manifest.json")).unwrap(),
    )
    .expect("read manifest");

    assert_eq!(turns.len(), 1);
    assert_eq!(turns[0]["seq_start"], 1);
    assert_eq!(turns[0]["seq_end"], 202);
    assert_eq!(turns[0]["counts"]["records"], 202);
    assert_eq!(turns[0]["counts"]["tool_calls"], 100);
    assert_eq!(turns[0]["counts"]["tool_results"], 100);
    assert_eq!(turns[0]["tools_used"]["shell_command"], 50);
    assert_eq!(turns[0]["tools_used"]["apply_patch"], 50);
    assert_eq!(manifest.turn_count, 1);
    assert!(manifest.has_turns);
}

#[test]
fn append_chat_events_rolls_over_when_provider_source_changes() {
    let (_guard, _temp) = isolated_home();
    let archive = ConversationArchiveState::default();
    let first = chat_event(
        "event-1",
        AgentChatEventKind::Message,
        Some(AgentChatRole::Assistant),
        Some("First provider source."),
    );
    let second = chat_event(
        "event-2",
        AgentChatEventKind::Message,
        Some(AgentChatRole::Assistant),
        Some("Second provider source."),
    );

    archive
        .append_chat_events_with_context(archive_context("session-one"), &[first])
        .expect("append first source");
    let first_conversation_id = archive
        .active_conversation_id_for_test("agent-1")
        .expect("first conversation");
    archive
        .append_chat_events_with_context(archive_context("session-two"), &[second])
        .expect("append second source");
    let second_conversation_id = archive
        .active_conversation_id_for_test("agent-1")
        .expect("second conversation");

    assert_ne!(first_conversation_id, second_conversation_id);
    let first_manifest_path = agent_conversation_dir("agent-1", &first_conversation_id)
        .expect("first conversation dir")
        .join("manifest.json");
    let first_manifest: ConversationManifest =
        serde_json::from_str(&std::fs::read_to_string(first_manifest_path).unwrap())
            .expect("read first manifest");
    assert_eq!(first_manifest.status, ConversationStatus::Interrupted);
    assert_eq!(
        first_manifest.boundary_reason,
        ConversationBoundaryReason::ProviderSourceChanged
    );
    assert_eq!(
        first_manifest.provider_session_ids,
        vec!["session-one".to_string()]
    );
    assert_eq!(
        first_manifest.provider_source_key.as_deref(),
        Some("codex:session:session-one")
    );
}

#[test]
fn append_chat_events_hydrates_open_conversation_for_same_provider_source() {
    let (_guard, _temp) = isolated_home();
    let first_archive = ConversationArchiveState::default();
    let first = chat_event(
        "event-1",
        AgentChatEventKind::Message,
        Some(AgentChatRole::Assistant),
        Some("Before restart."),
    );
    first_archive
        .append_chat_events_with_context(archive_context("session-one"), &[first])
        .expect("append before restart");
    let conversation_id = first_archive
        .active_conversation_id_for_test("agent-1")
        .expect("conversation id before restart");
    let second_archive = ConversationArchiveState::default();
    let second = chat_event(
        "event-2",
        AgentChatEventKind::Message,
        Some(AgentChatRole::Assistant),
        Some("After restart."),
    );

    second_archive
        .append_chat_events_with_context(archive_context("session-one"), &[second])
        .expect("append after restart");

    assert_eq!(
        second_archive.active_conversation_id_for_test("agent-1"),
        Some(conversation_id.clone())
    );
    let conversation_path =
        agent_conversation_dir("agent-1", &conversation_id).expect("conversation dir");
    let records: Vec<ConversationNarrativeRecord> =
        read_jsonl_records(&conversation_path.join("conversation.jsonl"))
            .expect("read narrative records");
    assert_eq!(records.len(), 2);
    assert_eq!(records[0].seq, 1);
    assert_eq!(records[1].seq, 2);
}

#[test]
fn append_chat_events_adopts_late_provider_source_key_for_existing_input() {
    let (_guard, _temp) = isolated_home();
    let archive = ConversationArchiveState::default();
    let input_context = ConversationArchiveContext {
        provider_source_key: None,
        provider_session_ids: Vec::new(),
        ..archive_context("session-one")
    };
    archive
        .append_delivered_input_with_context(input_context, "User prompt before source.", None)
        .expect("append delivered input");
    let conversation_id = archive
        .active_conversation_id_for_test("agent-1")
        .expect("input conversation id");
    let response = chat_event(
        "event-1",
        AgentChatEventKind::Message,
        Some(AgentChatRole::Assistant),
        Some("Assistant response after source discovery."),
    );

    archive
        .append_chat_events_with_context(archive_context("session-one"), &[response])
        .expect("append response");

    assert_eq!(
        archive.active_conversation_id_for_test("agent-1"),
        Some(conversation_id.clone())
    );
    let conversation_path =
        agent_conversation_dir("agent-1", &conversation_id).expect("conversation dir");
    let records: Vec<ConversationNarrativeRecord> =
        read_jsonl_records(&conversation_path.join("conversation.jsonl"))
            .expect("read narrative records");
    let manifest: ConversationManifest = serde_json::from_str(
        &std::fs::read_to_string(conversation_path.join("manifest.json")).unwrap(),
    )
    .expect("read manifest");
    assert_eq!(records.len(), 2);
    assert_eq!(records[0].seq, 1);
    assert_eq!(records[1].seq, 2);
    assert_eq!(
        manifest.provider_source_key.as_deref(),
        Some("codex:session:session-one")
    );
}

#[test]
fn append_preserves_adopted_provider_source_key_across_unkeyed_append_and_restart() {
    let (_guard, _temp) = isolated_home();
    let archive = ConversationArchiveState::default();
    let unkeyed_context = ConversationArchiveContext {
        provider_source_key: None,
        provider_session_ids: Vec::new(),
        ..archive_context("session-one")
    };
    archive
        .append_delivered_input_with_context(
            unkeyed_context.clone(),
            "User prompt before source.",
            None,
        )
        .expect("append unkeyed delivered input");
    let conversation_id = archive
        .active_conversation_id_for_test("agent-1")
        .expect("input conversation id");
    let response = chat_event(
        "event-1",
        AgentChatEventKind::Message,
        Some(AgentChatRole::Assistant),
        Some("Assistant response after source discovery."),
    );
    archive
        .append_chat_events_with_context(archive_context("session-one"), &[response])
        .expect("append keyed response");
    archive
        .append_delivered_input_with_context(
            unkeyed_context,
            "Follow-up prompt without source metadata.",
            None,
        )
        .expect("append later unkeyed delivered input");

    let conversation_path =
        agent_conversation_dir("agent-1", &conversation_id).expect("conversation dir");
    let manifest: ConversationManifest = serde_json::from_str(
        &std::fs::read_to_string(conversation_path.join("manifest.json")).unwrap(),
    )
    .expect("read manifest");
    assert_eq!(
        manifest.provider_source_key.as_deref(),
        Some("codex:session:session-one")
    );
    assert_eq!(manifest.provider_session_ids, vec!["session-one"]);

    let second_archive = ConversationArchiveState::default();
    let next_response = chat_event(
        "event-2",
        AgentChatEventKind::Message,
        Some(AgentChatRole::Assistant),
        Some("Different provider session response."),
    );
    second_archive
        .append_chat_events_with_context(archive_context("session-two"), &[next_response])
        .expect("append different session after restart");
    let next_conversation_id = second_archive
        .active_conversation_id_for_test("agent-1")
        .expect("next active conversation id");

    assert_ne!(next_conversation_id, conversation_id);
    let old_manifest: ConversationManifest = serde_json::from_str(
        &std::fs::read_to_string(conversation_path.join("manifest.json")).unwrap(),
    )
    .expect("read old manifest");
    assert_eq!(old_manifest.status, ConversationStatus::Interrupted);
    assert_eq!(
        old_manifest.boundary_reason,
        ConversationBoundaryReason::ProviderSourceChanged
    );
}

#[test]
fn append_chat_events_writes_event_records_for_completed_provider_events() {
    let (_guard, _temp) = isolated_home();
    let archive = ConversationArchiveState::default();
    let event = chat_event(
        "event-1",
        AgentChatEventKind::Message,
        Some(AgentChatRole::Assistant),
        Some("Persist this provider event."),
    );

    let appended = archive
        .append_chat_events("agent-1", std::slice::from_ref(&event))
        .expect("append succeeds");

    assert_eq!(appended, 1);
    let conversation_id = archive
        .active_conversation_id_for_test("agent-1")
        .expect("active conversation id");
    let conversation_path =
        agent_conversation_dir("agent-1", &conversation_id).expect("conversation dir");
    let events: Vec<AgentChatEvent> =
        read_jsonl_records(&conversation_path.join("events.jsonl")).expect("read event records");

    assert_eq!(events, vec![event]);
}

#[test]
fn append_chat_events_materializes_large_text_payloads() {
    let (_guard, _temp) = isolated_home();
    let archive = ConversationArchiveState::default();
    let large_text =
        "x".repeat(wardian_core::conversations::CONVERSATION_INLINE_TEXT_LIMIT_BYTES + 1);
    let event = chat_event(
        "event-large",
        AgentChatEventKind::ToolResult,
        Some(AgentChatRole::Tool),
        Some(&large_text),
    );

    let appended = archive
        .append_chat_events("agent-1", &[event])
        .expect("append succeeds");

    assert_eq!(appended, 1);
    let conversation_id = archive
        .active_conversation_id_for_test("agent-1")
        .expect("active conversation id");
    let conversation_path =
        agent_conversation_dir("agent-1", &conversation_id).expect("conversation dir");
    let records: Vec<ConversationNarrativeRecord> =
        read_jsonl_records(&conversation_path.join("conversation.jsonl"))
            .expect("read narrative records");
    let events: Vec<AgentChatEvent> =
        read_jsonl_records(&conversation_path.join("events.jsonl")).expect("read event records");

    assert_eq!(records.len(), 1);
    assert_eq!(records[0].text, None);
    assert!(records[0]
        .excerpt
        .as_deref()
        .is_some_and(|excerpt| large_text.starts_with(excerpt)));
    assert_eq!(
        records[0].artifact_refs,
        vec!["event-large-0001.txt".to_string()]
    );
    assert_eq!(
        std::fs::read_to_string(
            conversation_path
                .join("artifacts")
                .join("event-large-0001.txt")
        )
        .expect("read artifact"),
        large_text
    );
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].text, None);
    assert!(events[0].metadata["text_excerpt"]
        .as_str()
        .is_some_and(|excerpt| large_text.starts_with(excerpt)));
    assert_eq!(
        events[0].metadata["text_artifact_refs"][0],
        "event-large-0001.txt"
    );
}

#[test]
fn append_delivered_input_materializes_large_text_payloads() {
    let (_guard, _temp) = isolated_home();
    let archive = ConversationArchiveState::default();
    let large_text =
        "y".repeat(wardian_core::conversations::CONVERSATION_INLINE_TEXT_LIMIT_BYTES + 1);

    let appended = archive
        .append_delivered_input("agent-1", &large_text, None)
        .expect("append succeeds");

    assert_eq!(appended, 1);
    let conversation_id = archive
        .active_conversation_id_for_test("agent-1")
        .expect("active conversation id");
    let conversation_path =
        agent_conversation_dir("agent-1", &conversation_id).expect("conversation dir");
    let records: Vec<ConversationNarrativeRecord> =
        read_jsonl_records(&conversation_path.join("conversation.jsonl"))
            .expect("read narrative records");

    assert_eq!(records.len(), 1);
    assert_eq!(records[0].text, None);
    assert!(records[0]
        .excerpt
        .as_deref()
        .is_some_and(|excerpt| large_text.starts_with(excerpt)));
    assert_eq!(
        records[0].artifact_refs,
        vec!["delivered-input-1-0001.txt".to_string()]
    );
    assert_eq!(
        std::fs::read_to_string(
            conversation_path
                .join("artifacts")
                .join("delivered-input-1-0001.txt")
        )
        .expect("read artifact"),
        large_text
    );
}

#[test]
fn append_delivered_input_appends_to_active_conversation_with_monotonic_seq() {
    let (_guard, _temp) = isolated_home();
    let archive = ConversationArchiveState::default();

    let first = archive
        .append_delivered_input("agent-1", "First live prompt.", Some("source-agent"))
        .expect("first append succeeds");
    let second = archive
        .append_delivered_input("agent-1", "Second live prompt.", None)
        .expect("second append succeeds");

    assert_eq!(first, 1);
    assert_eq!(second, 1);
    let conversation_id = archive
        .active_conversation_id_for_test("agent-1")
        .expect("active conversation id");
    let conversation_path =
        agent_conversation_dir("agent-1", &conversation_id).expect("conversation dir");
    let records: Vec<ConversationNarrativeRecord> =
        read_jsonl_records(&conversation_path.join("conversation.jsonl"))
            .expect("read narrative records");
    assert_eq!(records.len(), 2);
    assert_eq!(records[0].seq, 1);
    assert_eq!(records[1].seq, 2);
    assert_eq!(
        records[0].speaker_type,
        Some(ConversationSpeakerType::Agent)
    );
    assert_eq!(
        records[1].speaker_type,
        Some(ConversationSpeakerType::Unknown)
    );
}

#[test]
fn provider_user_message_does_not_duplicate_delivered_input_prompt() {
    let (_guard, _temp) = isolated_home();
    let archive = ConversationArchiveState::default();
    archive
        .append_delivered_input_with_context(
            archive_context("session-one"),
            "Run the focused tests.",
            None,
        )
        .expect("append delivered input");
    let mut provider_event = chat_event(
        "provider-user-1",
        AgentChatEventKind::Message,
        Some(AgentChatRole::User),
        Some("Run the focused tests."),
    );
    provider_event.provider = "codex".to_string();
    provider_event.turn_id = Some("turn-1".to_string());
    provider_event.source = Some("response_item".to_string());
    provider_event.metadata = serde_json::json!({
        "provider_session_id": "session-one",
        "raw_type": "message"
    });

    archive
        .append_chat_events_with_context(archive_context("session-one"), &[provider_event])
        .expect("append provider event");

    let conversation_id = archive
        .active_conversation_id_for_test("agent-1")
        .expect("active conversation id");
    let conversation_path =
        agent_conversation_dir("agent-1", &conversation_id).expect("conversation dir");
    let records: Vec<ConversationNarrativeRecord> =
        read_jsonl_records(&conversation_path.join("conversation.jsonl"))
            .expect("read narrative records");
    let sources: Vec<ConversationSourceRecord> =
        read_jsonl_records(&conversation_path.join("sources.jsonl")).expect("read source records");

    assert_eq!(records.len(), 1);
    assert_eq!(records[0].text.as_deref(), Some("Run the focused tests."));
    assert_eq!(records[0].event_refs.len(), 2);
    assert!(records[0]
        .event_refs
        .iter()
        .any(|event_ref| event_ref.starts_with("generated:")));
    assert!(records[0]
        .event_refs
        .iter()
        .any(|event_ref| event_ref == "provider-user-1"));
    assert_eq!(records[0].source_refs, vec!["src_1".to_string()]);
    assert_eq!(sources.len(), 1);
    assert_eq!(sources[0].provider, "codex");
}

#[test]
fn repeated_same_text_delivered_inputs_are_not_ambiguously_merged() {
    let (_guard, _temp) = isolated_home();
    let archive = ConversationArchiveState::default();
    archive
        .append_delivered_input_with_context(archive_context("session-one"), "Again.", None)
        .expect("append first delivered input");
    archive
        .append_delivered_input_with_context(archive_context("session-one"), "Again.", None)
        .expect("append second delivered input");
    let mut provider_event = chat_event(
        "provider-user-1",
        AgentChatEventKind::Message,
        Some(AgentChatRole::User),
        Some("Again."),
    );
    provider_event.provider = "codex".to_string();
    provider_event.turn_id = Some("turn-1".to_string());
    provider_event.source = Some("response_item".to_string());
    provider_event.metadata = serde_json::json!({
        "provider_session_id": "session-one",
        "raw_type": "message"
    });

    archive
        .append_chat_events_with_context(archive_context("session-one"), &[provider_event])
        .expect("append provider event");

    let conversation_id = archive
        .active_conversation_id_for_test("agent-1")
        .expect("active conversation id");
    let conversation_path =
        agent_conversation_dir("agent-1", &conversation_id).expect("conversation dir");
    let records: Vec<ConversationNarrativeRecord> =
        read_jsonl_records(&conversation_path.join("conversation.jsonl"))
            .expect("read narrative records");

    assert_eq!(records.len(), 3);
    assert!(records[0]
        .event_refs
        .iter()
        .all(|event_ref| event_ref.starts_with("generated:")));
    assert!(records[1]
        .event_refs
        .iter()
        .all(|event_ref| event_ref.starts_with("generated:")));
    assert_eq!(records[2].event_refs, vec!["provider-user-1".to_string()]);
    assert_eq!(records[2].turn_id.as_deref(), Some("turn-1"));
}

#[test]
fn append_empty_delivered_input_does_not_create_conversation_files() {
    let (_guard, _temp) = isolated_home();
    let archive = ConversationArchiveState::default();

    let appended = archive
        .append_delivered_input("agent-1", "   ", Some("source-agent"))
        .expect("append succeeds");

    assert_eq!(appended, 0);
    assert_eq!(archive.active_conversation_id_for_test("agent-1"), None);
    assert!(!agent_conversations_dir("agent-1")
        .expect("conversations dir")
        .exists());
}

#[test]
fn append_lifecycle_boundary_uses_existing_active_conversation() {
    let (_guard, _temp) = isolated_home();
    let archive = ConversationArchiveState::default();
    archive
        .append_delivered_input("agent-1", "Before clear.", None)
        .expect("append input");

    let appended = archive
        .append_lifecycle_boundary("agent-1", ConversationBoundaryReason::Clear)
        .expect("append lifecycle");

    assert_eq!(appended, 1);
    let conversation_id = archive
        .active_conversation_id_for_test("agent-1")
        .expect("active conversation id");
    let conversation_path =
        agent_conversation_dir("agent-1", &conversation_id).expect("conversation dir");
    let records: Vec<ConversationNarrativeRecord> =
        read_jsonl_records(&conversation_path.join("conversation.jsonl"))
            .expect("read narrative records");
    assert_eq!(records.len(), 2);
    assert_eq!(records[1].seq, 2);
    assert_eq!(records[1].kind, ConversationRecordKind::Lifecycle);
    assert_eq!(records[1].status.as_deref(), Some("clear"));
}

#[test]
fn append_lifecycle_boundary_after_restart_hydrates_open_conversation_and_preserves_manifest_metadata(
) {
    let (_guard, _temp) = isolated_home();
    let first_archive = ConversationArchiveState::default();
    first_archive
        .append_chat_events_with_context(
            archive_context("session-one"),
            &[chat_event(
                "event-1",
                AgentChatEventKind::Message,
                Some(AgentChatRole::Assistant),
                Some("Before clear."),
            )],
        )
        .expect("append provider event");
    let conversation_id = first_archive
        .active_conversation_id_for_test("agent-1")
        .expect("active conversation id");

    let second_archive = ConversationArchiveState::default();
    second_archive
        .append_lifecycle_boundary("agent-1", ConversationBoundaryReason::Clear)
        .expect("append lifecycle after restart");

    assert_eq!(
        second_archive.active_conversation_id_for_test("agent-1"),
        Some(conversation_id.clone())
    );
    let conversation_path =
        agent_conversation_dir("agent-1", &conversation_id).expect("conversation dir");
    let records: Vec<ConversationNarrativeRecord> =
        read_jsonl_records(&conversation_path.join("conversation.jsonl"))
            .expect("read narrative records");
    let manifest: ConversationManifest = serde_json::from_str(
        &std::fs::read_to_string(conversation_path.join("manifest.json")).unwrap(),
    )
    .expect("read manifest");

    assert_eq!(records.len(), 2);
    assert_eq!(records[1].kind, ConversationRecordKind::Lifecycle);
    assert_eq!(manifest.provider, "codex");
    assert_eq!(manifest.agent_name, "CoderOne");
    assert_eq!(manifest.agent_class, "Coder");
    assert_eq!(manifest.workspace, "<absolute-workspace-path>");
    assert_eq!(
        manifest.provider_source_key.as_deref(),
        Some("codex:session:session-one")
    );
}

#[test]
fn lifecycle_boundary_with_context_does_not_create_unknown_stub() {
    let (_guard, _temp) = isolated_home();
    let archive = ConversationArchiveState::default();
    let context = ConversationArchiveContext {
        agent_id: "agent-1".to_string(),
        agent_name: "CoderOne".to_string(),
        agent_class: "Coder".to_string(),
        workspace: "<absolute-workspace-path>".to_string(),
        provider: "codex".to_string(),
        provider_session_ids: vec!["session-one".to_string()],
        provider_source_key: Some("codex:session:session-one".to_string()),
    };

    archive
        .append_lifecycle_boundary_with_context(context, ConversationBoundaryReason::Clear)
        .expect("append contextual lifecycle");
    let conversation_id = archive
        .active_conversation_id_for_test("agent-1")
        .expect("active conversation id");
    archive
        .rollover_agent("agent-1", ConversationBoundaryReason::Clear)
        .expect("close conversation");

    let conversation_path =
        agent_conversation_dir("agent-1", &conversation_id).expect("conversation dir");
    let manifest: ConversationManifest = serde_json::from_str(
        &std::fs::read_to_string(conversation_path.join("manifest.json")).unwrap(),
    )
    .expect("read manifest");
    let records: Vec<ConversationNarrativeRecord> =
        read_jsonl_records(&conversation_path.join("conversation.jsonl"))
            .expect("read narrative records");
    let turns: Vec<ConversationTurnRecord> =
        read_jsonl_records(&conversation_path.join("turns.jsonl")).expect("read turns");

    assert_eq!(manifest.agent_name, "CoderOne");
    assert_eq!(manifest.agent_class, "Coder");
    assert_eq!(manifest.workspace, "<absolute-workspace-path>");
    assert_eq!(manifest.provider, "codex");
    assert_eq!(
        manifest.provider_session_ids,
        vec!["session-one".to_string()]
    );
    assert_eq!(
        manifest.provider_source_key.as_deref(),
        Some("codex:session:session-one")
    );
    assert_eq!(manifest.record_count, 1);
    assert_eq!(manifest.turn_count, 1);
    assert!(manifest.has_turns);
    assert!(manifest.lifecycle_only);
    let manifest_json = serde_json::to_value(&manifest).unwrap();
    assert!(manifest_json.get("capture_quality").is_none());
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].kind, ConversationRecordKind::Lifecycle);
    assert_eq!(turns.len(), 1);
    assert_eq!(turns[0].status, ConversationTurnStatus::Lifecycle);
    assert_eq!(turns[0].request.kind, "lifecycle");
}

#[test]
fn lifecycle_source_does_not_create_provider_native_ref() {
    let (_guard, _temp) = isolated_home();
    let archive = ConversationArchiveState::default();

    archive
        .append_lifecycle_boundary_with_context(
            archive_context("session-one"),
            ConversationBoundaryReason::Clear,
        )
        .expect("append lifecycle");

    let conversation_id = archive
        .active_conversation_id_for_test("agent-1")
        .expect("active conversation id");
    let conversation_path =
        agent_conversation_dir("agent-1", &conversation_id).expect("conversation dir");
    let manifest: ConversationManifest = serde_json::from_str(
        &std::fs::read_to_string(conversation_path.join("manifest.json")).unwrap(),
    )
    .expect("read manifest");
    let sources: Vec<ConversationSourceRecord> =
        read_jsonl_records(&conversation_path.join("sources.jsonl")).expect("read source records");

    assert!(sources
        .iter()
        .any(|source| source.source_kind == "wardian_lifecycle"));
    assert!(manifest.provider_native_refs.is_empty());
}

#[test]
fn discarded_logging_state_skips_disabled_transcript_events_when_reenabled() {
    let (_guard, _temp) = isolated_home();
    let archive = ConversationArchiveState::default();
    archive
        .append_chat_events_with_context(
            archive_context("session-one"),
            &[chat_event_at(
                "event-a",
                "2026-01-01T00:00:00.000Z",
                "Before disabled.",
            )],
        )
        .expect("append enabled event");
    archive
        .discard_agent_with_context(archive_context("session-one"), &[])
        .expect("discard active capture state");

    let disabled_event = chat_event_at("event-b", "2026-01-01T00:00:01.000Z", "While disabled.");
    let enabled_event = chat_event_at("event-c", "2999-01-01T00:00:00.000Z", "After re-enabled.");
    archive
        .append_chat_events_with_context(
            archive_context("session-one"),
            &[disabled_event, enabled_event],
        )
        .expect("append after re-enabled");

    let conversation_id = archive
        .active_conversation_id_for_test("agent-1")
        .expect("active conversation id");
    let conversation_path =
        agent_conversation_dir("agent-1", &conversation_id).expect("conversation dir");
    let records: Vec<ConversationNarrativeRecord> =
        read_jsonl_records(&conversation_path.join("conversation.jsonl"))
            .expect("read narrative records");
    let texts = records
        .iter()
        .filter_map(|record| record.text.as_deref())
        .collect::<Vec<_>>();

    assert!(texts.contains(&"Before disabled."));
    assert!(!texts.contains(&"While disabled."));
    assert!(texts.contains(&"After re-enabled."));
}

#[test]
fn disabled_capture_state_skips_observed_events_without_timestamps_when_reenabled() {
    let (_guard, _temp) = isolated_home();
    let archive = ConversationArchiveState::default();
    archive
        .append_chat_events_with_context(
            archive_context("session-one"),
            &[chat_event_at(
                "event-a",
                "2026-01-01T00:00:00.000Z",
                "Before disabled.",
            )],
        )
        .expect("append enabled event");

    let disabled_event = chat_event_without_created_at("event-b", "While disabled.");
    archive
        .discard_agent_with_context(
            archive_context("session-one"),
            std::slice::from_ref(&disabled_event),
        )
        .expect("discard active capture state with observed event");
    let enabled_event = chat_event_without_created_at("event-c", "After re-enabled.");
    archive
        .append_chat_events_with_context(
            archive_context("session-one"),
            &[disabled_event, enabled_event],
        )
        .expect("append after re-enabled");

    let conversation_id = archive
        .active_conversation_id_for_test("agent-1")
        .expect("active conversation id");
    let conversation_path =
        agent_conversation_dir("agent-1", &conversation_id).expect("conversation dir");
    let records: Vec<ConversationNarrativeRecord> =
        read_jsonl_records(&conversation_path.join("conversation.jsonl"))
            .expect("read narrative records");
    let texts = records
        .iter()
        .filter_map(|record| record.text.as_deref())
        .collect::<Vec<_>>();

    assert!(texts.contains(&"Before disabled."));
    assert!(!texts.contains(&"While disabled."));
    assert!(texts.contains(&"After re-enabled."));
}

#[test]
fn disabled_capture_state_does_not_skip_reused_event_ids_from_new_provider_source() {
    let (_guard, _temp) = isolated_home();
    let archive = ConversationArchiveState::default();
    let disabled_event = chat_event_without_created_at("agent-1:1", "While disabled.");
    archive
        .discard_agent_with_context(
            archive_context("session-one"),
            std::slice::from_ref(&disabled_event),
        )
        .expect("discard disabled event for session one");

    let reused_id_event = chat_event_without_created_at("agent-1:1", "Fresh session event.");
    archive
        .append_chat_events_with_context(archive_context("session-two"), &[reused_id_event])
        .expect("append reused id from new source");

    let conversation_id = archive
        .active_conversation_id_for_test("agent-1")
        .expect("active conversation id");
    let conversation_path =
        agent_conversation_dir("agent-1", &conversation_id).expect("conversation dir");
    let records: Vec<ConversationNarrativeRecord> =
        read_jsonl_records(&conversation_path.join("conversation.jsonl"))
            .expect("read narrative records");

    assert!(records
        .iter()
        .any(|record| record.text.as_deref() == Some("Fresh session event.")));
    assert!(!records
        .iter()
        .any(|record| record.text.as_deref() == Some("While disabled.")));
}

#[test]
fn disabled_capture_state_does_not_skip_timestamped_events_from_new_provider_source() {
    let (_guard, _temp) = isolated_home();
    let archive = ConversationArchiveState::default();
    let disabled_event = chat_event_at("agent-1:1", "2026-01-01T00:00:00.000Z", "While disabled.");
    archive
        .discard_agent_with_context(
            archive_context("session-one"),
            std::slice::from_ref(&disabled_event),
        )
        .expect("discard disabled event for session one");

    let reused_id_event = chat_event_at(
        "agent-1:1",
        "2026-01-01T00:00:00.000Z",
        "Fresh session timestamped event.",
    );
    archive
        .append_chat_events_with_context(archive_context("session-two"), &[reused_id_event])
        .expect("append reused timestamped id from new source");

    let conversation_id = archive
        .active_conversation_id_for_test("agent-1")
        .expect("active conversation id");
    let conversation_path =
        agent_conversation_dir("agent-1", &conversation_id).expect("conversation dir");
    let records: Vec<ConversationNarrativeRecord> =
        read_jsonl_records(&conversation_path.join("conversation.jsonl"))
            .expect("read narrative records");

    assert!(records
        .iter()
        .any(|record| record.text.as_deref() == Some("Fresh session timestamped event.")));
    assert!(!records
        .iter()
        .any(|record| record.text.as_deref() == Some("While disabled.")));
}

#[test]
fn conversation_ids_include_collision_resistant_nonce() {
    let id = new_conversation_id("agent-1");
    let has_uuid_nonce = id
        .split('_')
        .any(|part| part.len() == 32 && part.chars().all(|ch| ch.is_ascii_hexdigit()));

    assert!(
        has_uuid_nonce,
        "conversation id should include a UUID nonce: {id}"
    );
}

#[test]
fn generated_records_write_event_and_source_records() {
    let (_guard, _temp) = isolated_home();
    let archive = ConversationArchiveState::default();
    archive
        .append_delivered_input("agent-1", "Prompt from peer.", Some("source-agent"))
        .expect("append delivered input");
    archive
        .append_lifecycle_boundary("agent-1", ConversationBoundaryReason::Clear)
        .expect("append lifecycle boundary");

    let conversation_id = archive
        .active_conversation_id_for_test("agent-1")
        .expect("active conversation id");
    let conversation_path =
        agent_conversation_dir("agent-1", &conversation_id).expect("conversation dir");
    let records: Vec<ConversationNarrativeRecord> =
        read_jsonl_records(&conversation_path.join("conversation.jsonl"))
            .expect("read narrative records");
    let events: Vec<AgentChatEvent> =
        read_jsonl_records(&conversation_path.join("events.jsonl")).expect("read event records");
    let sources: Vec<ConversationSourceRecord> =
        read_jsonl_records(&conversation_path.join("sources.jsonl")).expect("read source records");

    assert_eq!(events.len(), 2);
    assert!(records.iter().all(|record| !record.event_refs.is_empty()));
    assert!(records.iter().all(|record| !record.source_refs.is_empty()));
    assert!(sources
        .iter()
        .any(|source| source.source_kind == "wardian_agent"
            && source.provider_session_id.as_deref() == Some("source-agent")));
    assert!(sources
        .iter()
        .any(|source| source.source_kind == "wardian_lifecycle"));
}

#[test]
fn list_conversation_archive_reads_requested_agent_only() {
    let (_guard, _temp) = isolated_home();
    let archive = ConversationArchiveState::default();
    archive
        .append_delivered_input("agent-1", "Agent one prompt.", None)
        .expect("append agent one");
    archive
        .append_delivered_input("agent-2", "Agent two prompt.", None)
        .expect("append agent two");

    let entries = archive
        .list(Some("agent-1"), false)
        .expect("list agent conversations");

    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].agent_id, "agent-1");
    assert!(entries[0].path.starts_with("agents/agent-1/conversations/"));
}

#[test]
fn list_conversation_archive_scope_all_scans_agent_owned_indexes() {
    let (_guard, _temp) = isolated_home();
    let archive = ConversationArchiveState::default();
    archive
        .append_delivered_input("agent-1", "Agent one prompt.", None)
        .expect("append agent one");
    archive
        .append_delivered_input("agent-2", "Agent two prompt.", None)
        .expect("append agent two");

    let entries = archive
        .list(None, true)
        .expect("list all agent conversations");
    let agent_ids = entries
        .iter()
        .map(|entry| entry.agent_id.as_str())
        .collect::<std::collections::HashSet<_>>();

    assert_eq!(entries.len(), 2);
    assert!(agent_ids.contains("agent-1"));
    assert!(agent_ids.contains("agent-2"));
}

#[test]
fn list_conversation_archive_without_scope_all_uses_current_agent_env() {
    let (_guard, _temp) = isolated_home();
    let archive = ConversationArchiveState::default();
    archive
        .append_delivered_input("agent-1", "Agent one prompt.", None)
        .expect("append agent one");
    archive
        .append_delivered_input("agent-2", "Agent two prompt.", None)
        .expect("append agent two");
    std::env::set_var("WARDIAN_SESSION_ID", "agent-2");

    let entries = archive
        .list(None, false)
        .expect("list current agent conversations");

    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].agent_id, "agent-2");
    std::env::remove_var("WARDIAN_SESSION_ID");
}

#[test]
fn list_conversation_archive_without_agent_or_scope_all_requires_current_agent() {
    let (_guard, _temp) = isolated_home();
    std::env::remove_var("WARDIAN_SESSION_ID");
    let archive = ConversationArchiveState::default();

    let error = archive.list(None, false).unwrap_err();

    assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
    assert!(error.to_string().contains("WARDIAN_SESSION_ID"));
}

#[test]
fn show_conversation_archive_reads_manifest_and_records_from_agent_owned_path() {
    let (_guard, _temp) = isolated_home();
    let archive = ConversationArchiveState::default();
    archive
        .append_delivered_input("agent-1", "Show this prompt.", None)
        .expect("append prompt");
    let conversation_id = archive
        .active_conversation_id_for_test("agent-1")
        .expect("active conversation");

    let (manifest, records) = archive
        .show(&conversation_id)
        .expect("show conversation archive");

    assert_eq!(manifest.agent_id, "agent-1");
    assert_eq!(manifest.conversation_id, conversation_id);
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].text.as_deref(), Some("Show this prompt."));
}

#[test]
fn terminal_output_events_are_skipped_and_do_not_create_conversation_files() {
    let (_guard, _temp) = isolated_home();
    let archive = ConversationArchiveState::default();
    let event = chat_event(
        "event-terminal",
        AgentChatEventKind::TerminalOutput,
        None,
        Some("raw terminal repaint"),
    );

    let appended = archive
        .append_chat_events("agent-1", &[event])
        .expect("append succeeds");

    assert_eq!(appended, 0);
    assert_eq!(archive.active_conversation_id_for_test("agent-1"), None);
    assert!(!agent_conversations_dir("agent-1")
        .expect("conversations dir")
        .exists());
}

fn chat_event(
    id: &str,
    kind: AgentChatEventKind,
    role: Option<AgentChatRole>,
    text: Option<&str>,
) -> AgentChatEvent {
    AgentChatEvent {
        id: id.to_string(),
        session_id: "agent-1".to_string(),
        provider: "codex".to_string(),
        kind,
        role,
        text: text.map(ToString::to_string),
        title: None,
        status: None,
        turn_id: None,
        source: None,
        command: None,
        exit_code: None,
        path: None,
        language: None,
        created_at: Some("2026-06-15T00:00:00.000Z".to_string()),
        sequence: None,
        metadata: serde_json::json!({}),
    }
}

fn chat_event_at(id: &str, created_at: &str, text: &str) -> AgentChatEvent {
    AgentChatEvent {
        created_at: Some(created_at.to_string()),
        ..chat_event(
            id,
            AgentChatEventKind::Message,
            Some(AgentChatRole::Assistant),
            Some(text),
        )
    }
}

fn chat_event_without_created_at(id: &str, text: &str) -> AgentChatEvent {
    AgentChatEvent {
        created_at: None,
        ..chat_event(
            id,
            AgentChatEventKind::Message,
            Some(AgentChatRole::Assistant),
            Some(text),
        )
    }
}

fn narrative_record_with_turn(seq: u64, turn_id: &str, text: &str) -> ConversationNarrativeRecord {
    ConversationNarrativeRecord {
        schema: CONVERSATION_SCHEMA,
        seq,
        turn_id: Some(turn_id.to_string()),
        at: "2026-06-15T00:00:00.000Z".to_string(),
        kind: ConversationRecordKind::Message,
        role: Some("assistant".to_string()),
        speaker_type: Some(ConversationSpeakerType::Assistant),
        text: Some(text.to_string()),
        tool: None,
        status: None,
        summary: None,
        excerpt: None,
        event_refs: Vec::new(),
        source_refs: Vec::new(),
        artifact_refs: Vec::new(),
    }
}

fn archive_context(provider_session_id: &str) -> ConversationArchiveContext {
    ConversationArchiveContext {
        agent_id: "agent-1".to_string(),
        agent_name: "CoderOne".to_string(),
        agent_class: "Coder".to_string(),
        workspace: "<absolute-workspace-path>".to_string(),
        provider: "codex".to_string(),
        provider_session_ids: vec![provider_session_id.to_string()],
        provider_source_key: Some(format!("codex:session:{provider_session_id}")),
    }
}

fn isolated_home() -> (std::sync::MutexGuard<'static, ()>, tempfile::TempDir) {
    let guard = crate::utils::wardian_test_env_lock();
    let temp = tempfile::tempdir().expect("temp dir");
    std::env::set_var("WARDIAN_HOME", temp.path());
    (guard, temp)
}
