use super::*;
use wardian_core::models::chat::{AgentChatEvent, AgentChatEventKind, AgentChatRole};

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

fn canonical_event_refs(event_refs: &[String], events: &[AgentChatEvent]) -> HashSet<String> {
    event_refs
        .iter()
        .map(|event_ref| {
            let owners = events
                .iter()
                .filter(|event| {
                    event.id == *event_ref
                        || event.metadata["legacy_event_ids"]
                            .as_array()
                            .is_some_and(|aliases| {
                                aliases
                                    .iter()
                                    .any(|alias| alias.as_str() == Some(event_ref))
                            })
                })
                .collect::<Vec<_>>();
            assert!(
                owners.len() <= 1,
                "event reference alias has multiple owners"
            );
            owners
                .first()
                .map_or_else(|| event_ref.clone(), |event| event.id.clone())
        })
        .collect()
}

#[test]
fn archived_codex_mirror_narratives_reconcile_without_crossing_turns() {
    let pair = |turn: &str, seq: u64| {
        let request = AgentChatEvent {
            id: format!("response-item-{turn}"),
            session_id: "agent-1".into(),
            provider: "codex".into(),
            kind: AgentChatEventKind::Message,
            role: Some(AgentChatRole::User),
            text: Some("same retained request".into()),
            title: None,
            status: None,
            turn_id: None,
            source: Some("response_item".into()),
            command: None,
            exit_code: None,
            path: None,
            language: None,
            created_at: None,
            sequence: Some(seq),
            metadata: serde_json::json!({
                "provider_log": true,
                "log_path": "<codex-log>",
                "raw_type": "message",
                "input_origin": "human_input",
                "input_purpose": "request",
                "provider_turn_id": turn,
                "request_root_id": format!("wardian:input:{turn}"),
                "legacy_event_ids": [format!("response-alias-{turn}")],
            }),
        };
        let mut mirror = request.clone();
        mirror.id = format!("event-message-{turn}");
        mirror.source = Some("event_msg".into());
        mirror.sequence = Some(seq + 1);
        mirror.metadata["raw_type"] = serde_json::json!("user_message");
        mirror.metadata["request_root_id"] = serde_json::json!(format!("codex-message:{turn}"));
        mirror.metadata["legacy_event_ids"] = serde_json::json!([format!("event-alias-{turn}")]);

        let mut request_record =
            narrative_from_chat_event(&request, seq).expect("request narrative");
        request_record.event_refs = event_identity_ids(&request)
            .into_iter()
            .map(str::to_string)
            .collect();
        request_record.source_refs = vec![format!("source-response-{turn}")];
        let mut mirror_record =
            narrative_from_chat_event(&mirror, seq + 1).expect("mirror narrative");
        mirror_record.event_refs = event_identity_ids(&mirror)
            .into_iter()
            .map(str::to_string)
            .collect();
        mirror_record.source_refs = vec![format!("source-event-{turn}")];
        (request, mirror, request_record, mirror_record)
    };

    let (request_a, mirror_a, request_record_a, mirror_record_a) = pair("turn-a", 1);
    let (request_b, mirror_b, request_record_b, mirror_record_b) = pair("turn-b", 3);
    for (request, mirror) in [(&request_a, &mirror_a), (&request_b, &mirror_b)] {
        let request_ids = event_identity_ids(request);
        let mirror_ids = event_identity_ids(mirror);
        assert!(!request_ids.iter().any(|id| mirror_ids.contains(id)));
        assert!(crate::providers::chat_transcript::codex_user_mirror_pair(
            request, mirror
        ));
    }

    let mut events = [request_a, mirror_a, request_b, mirror_b];
    let mut records = vec![
        request_record_a,
        mirror_record_a,
        request_record_b,
        mirror_record_b,
    ];
    provenance::refresh_records(&mut records, &events);
    reconcile_codex_user_mirror_records(&mut records, &mut events)
        .expect("coalesce verified request mirrors");

    assert_eq!(
        records.len(),
        2,
        "each native turn has one request narrative"
    );
    assert_eq!(
        records.iter().map(|record| record.seq).collect::<Vec<_>>(),
        vec![1, 3],
        "coalescing preserves narrative sequence identity"
    );
    assert_eq!(
        events[1].metadata["wardian_archive_coalesced_narrative_sequences"],
        serde_json::json!([2])
    );
    assert_eq!(
        events[3].metadata["wardian_archive_coalesced_narrative_sequences"],
        serde_json::json!([4])
    );
    for turn in ["turn-a", "turn-b"] {
        let request_id = format!("response-item-{turn}");
        let mirror_id = format!("event-message-{turn}");
        let request_root = format!("wardian:input:{turn}");
        let mirror_root = format!("codex-message:{turn}");
        let record = records
            .iter()
            .find(|record| record.request_root_id.as_deref() == Some(request_root.as_str()))
            .expect("Wardian response-item root remains canonical");
        assert_eq!(record.event_refs.len(), 4);
        assert!(record.event_refs.contains(&request_id));
        assert!(record.event_refs.contains(&mirror_id));
        assert_eq!(record.source_refs.len(), 2);
        assert!(record
            .source_refs
            .contains(&format!("source-response-{turn}")));
        assert!(record.source_refs.contains(&format!("source-event-{turn}")));
        assert_ne!(
            record.request_root_id.as_deref(),
            Some(mirror_root.as_str())
        );
    }

    let canonical_records = records.clone();
    reconcile_codex_user_mirror_records(&mut records, &mut events).expect("repeat coalescing");
    assert_eq!(records, canonical_records, "reconciliation is idempotent");
}

#[test]
fn replay_event_refs_compare_observations_through_verified_legacy_aliases() {
    let mut observation = event("current-event", "codex", Some("request-root"));
    observation.metadata["legacy_event_ids"] = serde_json::json!(["legacy-event"]);
    let events = [observation];
    let original_refs = vec!["current-event".to_string()];
    let replayed_refs = vec!["current-event".to_string(), "legacy-event".to_string()];
    assert_eq!(
        canonical_event_refs(&original_refs, &events),
        canonical_event_refs(&replayed_refs, &events),
        "verified aliases preserve the same represented provider observations"
    );
    assert_ne!(
        canonical_event_refs(&original_refs, &events),
        canonical_event_refs(&["unmapped-reference".to_string()], &events),
        "unmapped references remain a durable mismatch"
    );
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

#[test]
fn codex_mirror_legacy_alias_requires_native_turn_and_matching_narrative_root() {
    let mirror = |id: &str, provider_turn_id: &str, request_root_id: &str| AgentChatEvent {
        id: id.into(),
        session_id: "agent-1".into(),
        provider: "codex".into(),
        kind: AgentChatEventKind::Message,
        role: Some(AgentChatRole::User),
        text: Some("same synthetic prompt".into()),
        title: None,
        status: None,
        turn_id: None,
        source: Some("event_msg".into()),
        command: None,
        exit_code: None,
        path: None,
        language: None,
        created_at: None,
        sequence: Some(12),
        metadata: serde_json::json!({
            "provider_log": true,
            "log_path": "<codex-log>",
            "provider_session_id": "codex-session",
            "raw_type": "user_message",
            "input_origin": "human_input",
            "input_purpose": "request",
            "provider_turn_id": provider_turn_id,
            "request_root_id": request_root_id,
        }),
    };
    let archived = mirror("legacy-text-hash", "turn-old", "agent-1:12");
    let mut record = narrative_from_chat_event(&archived, 1).expect("legacy mirror narrative");
    record.event_refs = vec![archived.id.clone()];

    let mut next_turn = mirror("turn-scoped-id", "turn-new", "agent-1:24");
    next_turn.metadata["legacy_event_ids"] = serde_json::json!(["legacy-text-hash"]);
    let archived_events = [archived.clone()];
    assert!(
        !provenance::same_observation(&archived, &next_turn),
        "a shared pre-turn hash cannot bind a different native turn"
    );
    assert_eq!(
        matching_record_index(std::slice::from_ref(&record), &next_turn, &archived_events)
            .expect("resolve next-turn narrative owner"),
        None,
        "the legacy alias cannot attach the new turn to the old narrative root"
    );

    let mut next_turn_same_root = next_turn.clone();
    next_turn_same_root.metadata["request_root_id"] = serde_json::json!("agent-1:12");
    assert_eq!(
        matching_record_index(
            std::slice::from_ref(&record),
            &next_turn_same_root,
            &archived_events
        )
        .expect("native turn must also guard aliases when roots collide"),
        None
    );

    let mut same_turn_retry = mirror("turn-scoped-retry-id", "turn-old", "agent-1:12");
    same_turn_retry.metadata["legacy_event_ids"] = serde_json::json!(["legacy-text-hash"]);
    assert!(provenance::same_observation(&archived, &same_turn_retry));
    assert_eq!(
        matching_record_index(
            std::slice::from_ref(&record),
            &same_turn_retry,
            &archived_events
        )
        .expect("resolve same-turn legacy narrative owner"),
        Some(0),
        "same-turn compatibility requires the same durable request root"
    );

    let mut wrong_root_retry = same_turn_retry;
    wrong_root_retry.metadata["request_root_id"] = serde_json::json!("agent-1:25");
    assert!(provenance::same_observation(&archived, &wrong_root_retry));
    assert_eq!(
        matching_record_index(
            std::slice::from_ref(&record),
            &wrong_root_retry,
            &archived_events
        )
        .expect("reject a mismatched narrative root"),
        None
    );

    let mut conflicting_exact_identity = archived.clone();
    conflicting_exact_identity.metadata["provider_turn_id"] =
        serde_json::json!("conflicting-native-turn");
    conflicting_exact_identity.metadata["request_root_id"] =
        serde_json::json!("conflicting-native-root");
    let mut unchanged = vec![archived.clone()];
    assert_eq!(
        provenance::refresh_events(&mut unchanged, &[conflicting_exact_identity])
            .expect_err("the same exact identity with conflicting native evidence fails closed")
            .kind(),
        std::io::ErrorKind::InvalidData
    );
    assert_eq!(unchanged, vec![archived]);
}

#[test]
fn claude_legacy_alias_with_multiple_narrative_owners_fails_closed() {
    let mut current = event("claude-current-raw-line", "claude", None);
    current.metadata["legacy_event_ids"] = serde_json::json!(["shared-legacy-alias"]);
    let mut first = narrative_from_chat_event(&event("claude-old-one", "claude", None), 1).unwrap();
    first.event_refs = vec!["shared-legacy-alias".into()];
    let mut second =
        narrative_from_chat_event(&event("claude-old-two", "claude", None), 2).unwrap();
    second.event_refs = vec!["shared-legacy-alias".into()];
    let mut records = vec![first, second];
    let original_records = records.clone();

    provenance::refresh_records(&mut records, std::slice::from_ref(&current));

    assert_eq!(records, original_records);
    let error = matching_record_index(&records, &current, &[])
        .expect_err("multiple legacy owners must remain ambiguous");
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
}

#[test]
fn codex_assistant_mirror_projection_requires_one_final_native_turn() {
    let observation = |id: &str,
                       source: &str,
                       turn_id: Option<&str>,
                       provider_turn: &str,
                       phase: Option<&str>| {
        let mut metadata = serde_json::json!({
            "provider_log": true,
            "log_path": "<codex-log>",
            "provider_turn_id": provider_turn,
            "raw_type": if source == "event_msg" { "agent_message" } else { "message" },
        });
        if let Some(phase) = phase {
            metadata["provider_phase"] = serde_json::json!(phase);
        }
        AgentChatEvent {
            id: id.into(),
            session_id: "wardian-agent".into(),
            provider: "codex".into(),
            kind: AgentChatEventKind::Message,
            role: Some(AgentChatRole::Assistant),
            text: Some("same final answer".into()),
            title: None,
            status: None,
            turn_id: turn_id.map(str::to_string),
            source: Some(source.into()),
            command: None,
            exit_code: None,
            path: None,
            language: None,
            created_at: None,
            sequence: None,
            metadata,
        }
    };

    let projected = provenance::merge_current_capture(
        Vec::new(),
        vec![
            observation("mirror-a", "event_msg", None, "turn-a", None),
            observation(
                "msg-a",
                "response_item",
                Some("msg-a"),
                "turn-a",
                Some("final_answer"),
            ),
        ],
    )
    .unwrap();
    assert_eq!(projected.len(), 1);
    assert_eq!(projected[0].id, "msg-a");
    assert_eq!(
        projected[0].metadata["provider_observation_ids"]
            .as_array()
            .unwrap()
            .len(),
        2
    );

    let mut watch_mirror = observation("watch-mirror", "event_msg", None, "turn-a", None);
    watch_mirror.metadata["provider_source"] = serde_json::json!("event");
    watch_mirror.metadata["provider_session_id"] = serde_json::json!("watch-session");
    watch_mirror
        .metadata
        .as_object_mut()
        .unwrap()
        .remove("raw_type");
    let mut watch_response = observation(
        "watch-response",
        "response_item",
        Some("watch-response-turn"),
        "turn-a",
        None,
    );
    watch_response.metadata["provider_source"] = serde_json::json!("event");
    watch_response.metadata["provider_session_id"] = serde_json::json!("watch-session");
    watch_response
        .metadata
        .as_object_mut()
        .unwrap()
        .remove("raw_type");
    let watch_group = provenance::merge_current_capture(
        Vec::new(),
        vec![
            observation("mirror-a", "event_msg", None, "turn-a", None),
            observation(
                "msg-a",
                "response_item",
                Some("msg-a"),
                "turn-a",
                Some("final_answer"),
            ),
            watch_mirror.clone(),
            watch_response.clone(),
        ],
    )
    .unwrap();
    assert_eq!(watch_group.len(), 1);
    assert_eq!(watch_group[0].id, "msg-a");
    assert_eq!(
        watch_group[0].metadata["provider_observation_ids"]
            .as_array()
            .unwrap()
            .len(),
        4
    );

    let mut mismatched_watch_response = watch_response;
    mismatched_watch_response.metadata["provider_session_id"] =
        serde_json::json!("different-watch-session");
    let mismatched_watch_group = provenance::merge_current_capture(
        Vec::new(),
        vec![
            observation("mirror-a", "event_msg", None, "turn-a", None),
            observation(
                "msg-a",
                "response_item",
                Some("msg-a"),
                "turn-a",
                Some("final_answer"),
            ),
            watch_mirror,
            mismatched_watch_response,
        ],
    )
    .unwrap();
    assert_eq!(
        mismatched_watch_group.len(),
        4,
        "watch observations with mismatched sessions fail closed"
    );

    let mut artifact_mirror = observation("mirror-large", "event_msg", None, "turn-large", None);
    let mut artifact_completion = observation(
        "completion-large",
        "response_item",
        Some("completion-large"),
        "turn-large",
        Some("final_answer"),
    );
    artifact_mirror.text = None;
    artifact_completion.text = None;
    artifact_mirror.metadata["codex_assistant_text_sha256"] = serde_json::json!("digest-a");
    artifact_completion.metadata["codex_assistant_text_sha256"] = serde_json::json!("digest-a");
    assert_eq!(
        provenance::merge_current_capture(
            Vec::new(),
            vec![artifact_mirror.clone(), artifact_completion.clone()]
        )
        .unwrap()
        .len(),
        1,
        "matching persisted content digests bind text artifacts"
    );
    artifact_completion.metadata["codex_assistant_text_sha256"] = serde_json::json!("digest-b");
    assert_eq!(
        provenance::merge_current_capture(Vec::new(), vec![artifact_mirror, artifact_completion])
            .unwrap()
            .len(),
        2,
        "different artifact digests cannot be paired"
    );

    let distinct_turns = provenance::merge_current_capture(
        Vec::new(),
        vec![
            observation("mirror-a", "event_msg", None, "turn-a", None),
            observation(
                "msg-a",
                "response_item",
                Some("msg-a"),
                "turn-a",
                Some("final_answer"),
            ),
            observation("mirror-b", "event_msg", None, "turn-b", None),
            observation(
                "msg-b",
                "response_item",
                Some("msg-b"),
                "turn-b",
                Some("final_answer"),
            ),
        ],
    )
    .unwrap();
    assert_eq!(distinct_turns.len(), 2);
    assert!(distinct_turns.iter().any(|event| event.id == "msg-a"));
    assert!(distinct_turns.iter().any(|event| event.id == "msg-b"));

    let ambiguous = provenance::merge_current_capture(
        Vec::new(),
        vec![
            observation("mirror-a", "event_msg", None, "turn-a", None),
            observation("mirror-b", "event_msg", None, "turn-a", None),
            observation(
                "msg-a",
                "response_item",
                Some("msg-a"),
                "turn-a",
                Some("final_answer"),
            ),
        ],
    )
    .unwrap();
    assert_eq!(ambiguous.len(), 3);
}

#[test]
fn pi_assistant_mirror_projection_requires_unique_launch_bound_entry() {
    let observation = |id: &str,
                       source: &str,
                       turn_id: &str,
                       provider_log: bool,
                       provider_session_id: Option<&str>,
                       log_path: Option<&str>,
                       text: &str| {
        let mut metadata = serde_json::json!({
            "provider_log": provider_log,
            "log_source": "active_agent_log_path",
            "raw_type": "message",
        });
        if let Some(provider_session_id) = provider_session_id {
            metadata["provider_session_id"] = serde_json::json!(provider_session_id);
            metadata["provider_turn_id"] = serde_json::json!(turn_id);
        }
        if let Some(log_path) = log_path {
            metadata["log_path"] = serde_json::json!(log_path);
        }
        if source == "session_jsonl" {
            metadata["transcript_cursor"] = serde_json::json!(format!("watch:{id}"));
        }
        AgentChatEvent {
            id: id.into(),
            session_id: "dac9e431-f775-4c77-8b8e-0a61c6dba9e4".into(),
            provider: "pi".into(),
            kind: AgentChatEventKind::Message,
            role: Some(AgentChatRole::Assistant),
            text: Some(text.into()),
            title: None,
            status: None,
            turn_id: Some(turn_id.into()),
            source: Some(source.into()),
            command: None,
            exit_code: None,
            path: None,
            language: None,
            created_at: None,
            sequence: None,
            metadata,
        }
    };

    let watch_id = "dac9e431-f775-4c77-8b8e-0a61c6dba9e4:0000000000000004:session_jsonl";
    let native_id =
        "dac9e431-f775-4c77-8b8e-0a61c6dba9e4:provider_log:0f806b9055af218c1f630ec6537aabec";
    let watch = observation(
        watch_id,
        "session_jsonl",
        "118bf261",
        true,
        Some("1f2a5d81-5ca6-46bf-b15a-78c6295d64b7"),
        Some("pi-session.jsonl"),
        "WARDIAN_REAL_DELIVERY_PI_PROMPT_SHORT_setup-94060_1789456156488",
    );
    let native = observation(
        native_id,
        "message",
        "118bf261",
        true,
        None,
        Some("pi-session.jsonl"),
        "WARDIAN_REAL_DELIVERY_PI_PROMPT_SHORT_setup-94060_1789456156488",
    );

    let projected =
        provenance::merge_current_capture(vec![watch.clone(), native.clone()], Vec::new()).unwrap();
    assert_eq!(projected.len(), 1);
    assert_eq!(projected[0].id, native_id);
    let observation_ids = projected[0].metadata["provider_observation_ids"]
        .as_array()
        .unwrap();
    assert_eq!(observation_ids.len(), 2);
    assert!(observation_ids.iter().any(|id| id == watch_id));
    assert!(observation_ids.iter().any(|id| id == native_id));

    let distinct_turns = provenance::merge_current_capture(
        vec![
            watch.clone(),
            native.clone(),
            observation(
                "pi-watch-second-turn",
                "session_jsonl",
                "118bf262",
                true,
                Some("1f2a5d81-5ca6-46bf-b15a-78c6295d64b7"),
                Some("pi-session.jsonl"),
                "WARDIAN_REAL_DELIVERY_PI_PROMPT_SHORT_setup-94060_1789456156488",
            ),
            observation(
                "pi-native-second-turn",
                "message",
                "118bf262",
                true,
                None,
                Some("pi-session.jsonl"),
                "WARDIAN_REAL_DELIVERY_PI_PROMPT_SHORT_setup-94060_1789456156488",
            ),
        ],
        Vec::new(),
    )
    .unwrap();
    assert_eq!(distinct_turns.len(), 2);

    let missing_binding = provenance::merge_current_capture(
        vec![
            observation(
                "pi-unbound-watch",
                "session_jsonl",
                "118bf261",
                false,
                None,
                None,
                "WARDIAN_REAL_DELIVERY_PI_PROMPT_SHORT_setup-94060_1789456156488",
            ),
            native.clone(),
        ],
        Vec::new(),
    )
    .unwrap();
    assert_eq!(missing_binding.len(), 2);

    let mut foreign_watch = watch.clone();
    foreign_watch.metadata["log_path"] = serde_json::json!("other-session.jsonl");
    let foreign_binding =
        provenance::merge_current_capture(vec![foreign_watch, native.clone()], Vec::new()).unwrap();
    assert_eq!(foreign_binding.len(), 2);

    let ambiguous_native = provenance::merge_current_capture(
        vec![
            watch,
            native.clone(),
            observation(
                "pi-native-ambiguous",
                "message",
                "118bf261",
                true,
                None,
                Some("pi-session.jsonl"),
                "WARDIAN_REAL_DELIVERY_PI_PROMPT_SHORT_setup-94060_1789456156488",
            ),
        ],
        Vec::new(),
    )
    .unwrap();
    assert_eq!(ambiguous_native.len(), 3);
}

#[test]
fn retained_codex_delivery_fixture_collapses_only_the_bound_provider_pair() {
    let (_guard, _temp) = isolate();
    let mut current = crate::providers::chat_transcript::normalize_chat_lines(
        "wardian-agent",
        "codex",
        include_str!("../../providers/fixtures/codex-real-delivery-mirror.jsonl").lines(),
    );
    for event in &mut current {
        if event.kind == AgentChatEventKind::Message {
            event.metadata["provider_log"] = serde_json::json!(true);
            event.metadata["log_path"] = serde_json::json!("<codex-log>");
        }
    }
    let mirror = current
        .iter()
        .find(|event| {
            event.role == Some(AgentChatRole::Assistant)
                && event.source.as_deref() == Some("event_msg")
        })
        .cloned()
        .expect("fixture identityless mirror");
    let mut unbound_watch_observation = mirror.clone();
    unbound_watch_observation.id = "watch-event-msg".into();
    unbound_watch_observation.metadata = serde_json::json!({});
    current.push(unbound_watch_observation);

    let projected = provenance::merge_current_capture(Vec::new(), current).unwrap();
    let assistants = projected
        .iter()
        .filter(|event| event.role == Some(AgentChatRole::Assistant))
        .collect::<Vec<_>>();
    assert_eq!(assistants.len(), 2);
    let canonical = assistants
        .iter()
        .find(|event| event.source.as_deref() == Some("response_item"))
        .expect("identified final response");
    assert_eq!(canonical.metadata["provider_phase"], "final_answer");
    assert_eq!(
        canonical.metadata["provider_observation_ids"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert!(assistants.iter().any(|event| event.id == "watch-event-msg"));
}

#[cfg(feature = "retained-codex-replay")]
#[test]
fn retained_codex_logs_replay_to_one_canonical_archive_after_restart() {
    let (_guard, _temp) = isolate();
    let paths = std::env::var("WARDIAN_RETAINED_CODEX_EVENT_ARCHIVES")
        .expect("retained Codex event archive paths")
        .split(';')
        .map(str::to_string)
        .collect::<Vec<_>>();
    assert!(!paths.is_empty());

    let mut replays = Vec::new();
    for (index, path) in paths.iter().enumerate() {
        let archive_text = std::fs::read_to_string(path).expect("read retained event archive");
        let events = archive_text
            .lines()
            .map(|line| serde_json::from_str::<AgentChatEvent>(line).expect("decode event row"))
            .collect::<Vec<_>>();
        assert!(!events.is_empty());
        let agent_id = events[0].session_id.clone();
        assert!(events.iter().all(|event| event.session_id == agent_id));

        let mut raw_logs = std::collections::HashMap::<String, Vec<u8>>::new();
        let mut checked_offsets = 0;
        for event in &events {
            let Some(offset) = event.metadata["provider_log_row_offset"].as_u64() else {
                continue;
            };
            let log_path = event.metadata["log_path"]
                .as_str()
                .expect("retained provider observation log binding");
            let bytes = raw_logs
                .entry(log_path.to_string())
                .or_insert_with(|| std::fs::read(log_path).expect("read bound provider log"));
            let offset = usize::try_from(offset).expect("provider row offset fits memory");
            assert!(offset == 0 || bytes.get(offset - 1) == Some(&b'\n'));
            let row_end = bytes[offset..]
                .iter()
                .position(|byte| *byte == b'\n')
                .map(|relative| offset + relative)
                .unwrap_or(bytes.len());
            let raw_row: serde_json::Value =
                serde_json::from_slice(&bytes[offset..row_end]).expect("decode exact provider row");
            assert_eq!(raw_row["type"].as_str(), event.source.as_deref());
            assert_eq!(
                raw_row["payload"]["type"].as_str(),
                event.metadata["raw_type"].as_str()
            );
            checked_offsets += 1;
        }
        assert!(
            checked_offsets > 0,
            "retained rows bind to exact provider logs"
        );

        let context = ConversationArchiveContext {
            agent_id: agent_id.clone(),
            agent_name: "CoderOne".to_string(),
            agent_class: "Coder".to_string(),
            workspace: "<absolute-workspace-path>".to_string(),
            provider: "codex".to_string(),
            provider_session_ids: Vec::new(),
            provider_source_key: Some(format!("codex:retained-archive-{index}")),
        };
        let archive = ConversationArchiveState::default();
        archive
            .append_chat_events_with_context(context.clone(), &events)
            .expect("append retained Codex observations");

        let conversation_id = archive
            .active_conversation_id_for_test(&agent_id)
            .expect("retained conversation");
        let directory =
            conversation_dir(&agent_id, &conversation_id).expect("retained conversation path");
        let (_, records) = archive
            .show(&conversation_id)
            .expect("read narrative records");
        let context_observation_ids = events
            .iter()
            .filter(|event| event.metadata["input_origin"] == "context_injection")
            .map(|event| event.id.as_str())
            .collect::<std::collections::HashSet<_>>();
        assert!(!context_observation_ids.is_empty());
        let archived_events = archive
            .chat_events_for_agent(&agent_id)
            .expect("read retained Codex event archive");
        let archived_context_ids = archived_events
            .iter()
            .filter(|event| event.metadata["input_origin"] == "context_injection")
            .map(|event| event.id.as_str())
            .collect::<std::collections::HashSet<_>>();
        assert!(context_observation_ids.is_subset(&archived_context_ids));
        assert!(archived_events
            .iter()
            .filter(|event| event.metadata["input_origin"] == "context_injection")
            .all(|event| event.role == Some(AgentChatRole::System)));
        let provider_turns = events
            .iter()
            .filter(|event| {
                event.role == Some(AgentChatRole::User)
                    && event.metadata["input_origin"] == "human_input"
                    && event.metadata["input_purpose"] == "request"
            })
            .filter_map(|event| event.metadata["provider_turn_id"].as_str())
            .collect::<std::collections::HashSet<_>>();
        let canonical_users = records
            .iter()
            .filter(|record| record.role.as_deref() == Some("user"))
            .collect::<Vec<_>>();
        assert_eq!(canonical_users.len(), provider_turns.len());
        let canonical_roots = canonical_users
            .iter()
            .filter_map(|record| record.request_root_id.as_deref())
            .collect::<Vec<_>>();
        assert!(!canonical_roots.is_empty());
        assert!(canonical_users.iter().all(|record| record
            .request_root_id
            .as_deref()
            .is_some_and(|root| root.starts_with("wardian:input:"))));
        for turn_id in &provider_turns {
            let observations = events
                .iter()
                .filter(|event| {
                    event.metadata["provider_turn_id"].as_str() == Some(*turn_id)
                        && event.metadata["input_origin"] == "human_input"
                        && event.metadata["input_purpose"] == "request"
                })
                .collect::<Vec<_>>();
            assert_eq!(observations.len(), 2);
            let record = canonical_users
                .iter()
                .find(|record| {
                    observations
                        .iter()
                        .all(|event| record.event_refs.contains(&event.id))
                })
                .expect("both request observations share one narrative");
            assert_eq!(record.event_refs.len(), 2);
            assert_eq!(record.source_refs.len(), 2);
            let mirror = observations
                .iter()
                .find(|event| event.source.as_deref() == Some("event_msg"))
                .expect("native Codex user mirror");
            let mirror_root = mirror.metadata["provider_mirror_request_root_ids"]
                .as_array()
                .and_then(|roots| roots.first())
                .and_then(serde_json::Value::as_str)
                .or_else(|| mirror.metadata["request_root_id"].as_str())
                .expect("native mirror root provenance");
            assert_ne!(record.request_root_id.as_deref(), Some(mirror_root));
        }

        let final_event_ids = events
            .iter()
            .filter(|event| event.metadata["provider_phase"] == "final_answer")
            .map(|event| event.id.as_str())
            .collect::<std::collections::HashSet<_>>();
        let final_records = records
            .iter()
            .filter(|record| {
                record.role.as_deref() == Some("assistant")
                    && record
                        .event_refs
                        .iter()
                        .any(|event_id| final_event_ids.contains(event_id.as_str()))
            })
            .collect::<Vec<_>>();
        let final_turns = events
            .iter()
            .filter(|event| event.metadata["provider_phase"] == "final_answer")
            .filter_map(|event| event.metadata["provider_turn_id"].as_str())
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(final_records.len(), final_turns.len());
        assert!(final_records
            .iter()
            .all(|record| record.event_refs.len() >= 2));
        let assistant_ref_counts = final_records
            .iter()
            .map(|record| (record.event_refs.len(), record.source_refs.len()))
            .collect::<Vec<_>>();
        let retained_conversation_path =
            std::path::Path::new(path).with_file_name("conversation.jsonl");
        let mut seeded_legacy_split = false;
        if retained_conversation_path.is_file() {
            let retained_records: Vec<ConversationNarrativeRecord> =
                read_jsonl_records(&retained_conversation_path)
                    .expect("read retained narrative archive");
            let retained_user_requests = retained_records
                .iter()
                .filter(|record| {
                    record.role.as_deref() == Some("user")
                        && record.input_origin
                            == Some(
                                wardian_core::conversations::ConversationInputOrigin::HumanInput,
                            )
                        && record.input_purpose.as_deref() == Some("request")
                })
                .count();
            if retained_user_requests > provider_turns.len() {
                assert_eq!(
                    retained_user_requests,
                    provider_turns.len() + 1,
                    "retained archive contains exactly one split request pair"
                );
                let mut reconciled_retained = retained_records.clone();
                let mut reconciliation_events = events.clone();
                provenance::refresh_records(&mut reconciled_retained, &events);
                assert_eq!(
                    reconcile_codex_user_mirror_records(
                        &mut reconciled_retained,
                        &mut reconciliation_events,
                    )
                    .expect("reconcile retained split request"),
                    1,
                    "the retained native pair uniquely owns two request narratives"
                );
                let retained_sequences = retained_records
                    .iter()
                    .map(|record| record.seq)
                    .collect::<std::collections::HashSet<_>>();
                assert!(reconciled_retained
                    .iter()
                    .all(|record| retained_sequences.contains(&record.seq)));
                assert_eq!(reconciled_retained.len() + 1, retained_records.len());
                std::fs::copy(
                    &retained_conversation_path,
                    directory.join("conversation.jsonl"),
                )
                .expect("seed the isolated archive with the retained split narratives");
                for file_name in ["events.jsonl", "sources.jsonl"] {
                    let retained_path = retained_conversation_path.with_file_name(file_name);
                    if retained_path.is_file() {
                        std::fs::copy(&retained_path, directory.join(file_name))
                            .expect("seed the isolated archive with retained source records");
                    }
                }
                seeded_legacy_split = true;
            }
        }
        let provider_turn_count = provider_turns.len();
        drop(provider_turns);
        replays.push((
            context,
            agent_id,
            events,
            records,
            assistant_ref_counts,
            provider_turn_count,
            seeded_legacy_split,
        ));
    }

    for (
        context,
        agent_id,
        events,
        before,
        assistant_ref_counts,
        provider_turn_count,
        seeded_legacy_split,
    ) in replays
    {
        let restarted = ConversationArchiveState::default();
        restarted
            .append_chat_events_with_context(context, &events)
            .expect("replay retained Codex observations after restart");
        let conversation_id = restarted
            .active_conversation_id_for_test(&agent_id)
            .expect("replayed conversation");
        let (_, after) = restarted
            .show(&conversation_id)
            .expect("read replayed narrative records");
        if seeded_legacy_split {
            let canonical_users = after
                .iter()
                .filter(|record| {
                    record.role.as_deref() == Some("user")
                        && record.input_origin
                            == Some(
                                wardian_core::conversations::ConversationInputOrigin::HumanInput,
                            )
                        && record.input_purpose.as_deref() == Some("request")
                })
                .collect::<Vec<_>>();
            assert_eq!(canonical_users.len(), provider_turn_count);
            let replayed_events = restarted
                .chat_events_for_agent(&agent_id)
                .expect("read replayed provider events");
            let mut occupied_sequences = after
                .iter()
                .map(|record| record.seq)
                .collect::<std::collections::HashSet<_>>();
            for event in &replayed_events {
                if let Some(markers) =
                    event.metadata["wardian_archive_coalesced_narrative_sequences"].as_array()
                {
                    occupied_sequences.extend(markers.iter().filter_map(serde_json::Value::as_u64));
                }
            }
            let maximum_sequence = occupied_sequences.iter().copied().max().unwrap_or(0);
            assert_eq!(
                occupied_sequences,
                (1..=maximum_sequence).collect(),
                "verified coalesced sequences reserve only the gaps created by mirror repair"
            );
            for turn_id in events
                .iter()
                .filter(|event| {
                    event.metadata["input_origin"] == "human_input"
                        && event.metadata["input_purpose"] == "request"
                })
                .filter_map(|event| event.metadata["provider_turn_id"].as_str())
                .collect::<std::collections::HashSet<_>>()
            {
                let observations = events
                    .iter()
                    .filter(|event| {
                        event.metadata["provider_turn_id"].as_str() == Some(turn_id)
                            && event.metadata["input_origin"] == "human_input"
                            && event.metadata["input_purpose"] == "request"
                    })
                    .collect::<Vec<_>>();
                let record = canonical_users
                    .iter()
                    .find(|record| {
                        observations
                            .iter()
                            .all(|event| record.event_refs.contains(&event.id))
                    })
                    .expect("retained mirror pair has one narrative after restart");
                assert!(record.source_refs.len() >= 2);
                for event in &observations {
                    if let Some(source) = source_record_from_chat_event(event, record.seq) {
                        assert!(record.source_refs.contains(&source.source_id));
                    }
                }
                assert!(record
                    .request_root_id
                    .as_deref()
                    .is_some_and(|root| root.starts_with("wardian:input:")));
            }
            continue;
        }
        let mut first_difference = None;
        let mut ignored_capture_times = 0;
        for (index, (before_record, after_record)) in before.iter().zip(&after).enumerate() {
            let before_json = serde_json::to_value(before_record).expect("serialize narrative");
            let after_json = serde_json::to_value(after_record).expect("serialize narrative");
            first_difference = [
                "schema",
                "seq",
                "turn_id",
                "kind",
                "role",
                "speaker_type",
                "input_origin",
                "input_purpose",
                "request_root_id",
                "causal_ref",
                "text",
                "tool",
                "status",
                "summary",
                "excerpt",
                "event_refs",
                "source_refs",
                "artifact_refs",
            ]
            .into_iter()
            .find(|field| {
                if *field == "event_refs" {
                    canonical_event_refs(&before_record.event_refs, &events)
                        != canonical_event_refs(&after_record.event_refs, &events)
                } else {
                    before_json.get(*field) != after_json.get(*field)
                }
            })
            .map(|field| (index, field));
            if first_difference.is_some() {
                break;
            }
            if before_record.at != after_record.at {
                let primary_event = before_record
                    .event_refs
                    .first()
                    .and_then(|id| events.iter().find(|event| &event.id == id));
                if primary_event.is_some_and(|event| event.created_at.is_none()) {
                    ignored_capture_times += 1;
                } else {
                    first_difference = Some((index, "at"));
                    break;
                }
            }
        }
        assert!(
            before.len() == after.len() && first_difference.is_none(),
            "fresh-state narrative replay diverged (before={}, after={}, first_difference={:?})",
            before.len(),
            after.len(),
            first_difference
                .map(|(index, field)| format!("record[{index}].{field}"))
                .unwrap_or_else(|| "record_count".to_string())
        );
        eprintln!("fresh-state replay stable; timestamp deltas from untimed source rows: {ignored_capture_times}");
        let replayed_counts = after
            .iter()
            .filter(|record| record.role.as_deref() == Some("assistant"))
            .map(|record| (record.event_refs.len(), record.source_refs.len()))
            .collect::<Vec<_>>();
        assert_eq!(replayed_counts, assistant_ref_counts);
    }
}

#[test]
fn claude_assistant_mirror_projection_requires_unique_native_message() {
    let observation = |id: &str, native: bool, turn_id: &str| AgentChatEvent {
        id: id.into(),
        session_id: "wardian-agent".into(),
        provider: "claude".into(),
        kind: AgentChatEventKind::Message,
        role: Some(AgentChatRole::Assistant),
        text: Some("same Claude answer".into()),
        title: None,
        status: None,
        turn_id: Some(turn_id.into()),
        source: Some("stream_json".into()),
        command: None,
        exit_code: None,
        path: None,
        language: None,
        created_at: None,
        sequence: None,
        metadata: if native {
            serde_json::json!({
                "provider_log": true,
                "log_path": "<claude-log>",
                "raw_type": "assistant",
            })
        } else {
            serde_json::json!({"provider_source": "event"})
        },
    };

    let projected = provenance::merge_current_capture(
        Vec::new(),
        vec![
            observation("watch", false, "msg-1"),
            observation("native", true, "msg-1"),
        ],
    )
    .unwrap();
    assert_eq!(projected.len(), 1);
    assert_eq!(projected[0].id, "native");
    let observation_ids = projected[0].metadata["provider_observation_ids"]
        .as_array()
        .unwrap();
    assert_eq!(observation_ids.len(), 2);
    assert!(observation_ids.iter().any(|id| id == "watch"));
    assert!(observation_ids.iter().any(|id| id == "native"));

    let mut different_content = observation("watch", false, "msg-1");
    different_content.text = Some("different Claude answer".into());
    let content_mismatch = provenance::merge_current_capture(
        Vec::new(),
        vec![different_content, observation("native", true, "msg-1")],
    )
    .unwrap();
    assert_eq!(content_mismatch.len(), 2);

    let distinct_turns = provenance::merge_current_capture(
        Vec::new(),
        vec![
            observation("watch-1", false, "msg-1"),
            observation("native-1", true, "msg-1"),
            observation("watch-2", false, "msg-2"),
            observation("native-2", true, "msg-2"),
        ],
    )
    .unwrap();
    assert_eq!(distinct_turns.len(), 2);

    let ambiguous_native = provenance::merge_current_capture(
        Vec::new(),
        vec![
            observation("watch", false, "msg-1"),
            observation("native-1", true, "msg-1"),
            observation("native-2", true, "msg-1"),
        ],
    )
    .unwrap();
    assert_eq!(ambiguous_native.len(), 3);
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
        // Model IDs and metadata persisted before the provider-log identity
        // migration, even though this helper uses today's normalizer.
        event.turn_id = None;
        event
            .metadata
            .as_object_mut()
            .unwrap()
            .remove("request_root_id");
        for key in [
            crate::providers::chat_transcript::PROVIDER_EVENT_ID_METADATA_KEY,
            crate::providers::chat_transcript::PROVIDER_LOG_ROW_OFFSET_METADATA_KEY,
        ] {
            event.metadata.as_object_mut().unwrap().remove(key);
        }
        event.metadata["provider_log"] = serde_json::json!(true);
        event.metadata["log_path"] = serde_json::json!(path);
        event.id = native_identity::legacy_provider_log_event_id(event, path);
    }
    let mut current = legacy.clone();
    for event in &mut current {
        // Boundary stimulus: project #1167's envelope mapping from the real
        // retained record; this is not a claim to retest the Pi adapter.
        let row = &rows[event.sequence.unwrap() as usize - 1];
        let native_id = row["id"].as_str().unwrap();
        event.turn_id = Some(native_id.into());
        event.metadata[crate::providers::chat_transcript::PROVIDER_EVENT_ID_METADATA_KEY] =
            serde_json::json!(native_id);
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

fn opencode_local_echo_fixture() -> (AgentChatEvent, AgentChatEvent) {
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/opencode-local-echo.json")).unwrap();
    (
        serde_json::from_value(fixture["generated"].clone()).unwrap(),
        serde_json::from_value(fixture["native"].clone()).unwrap(),
    )
}

#[test]
fn opencode_native_projection_reconciles_unique_generated_local_echo() {
    let (_guard, _temp) = isolate();
    let (generated, native) = opencode_local_echo_fixture();

    let merged = provenance::merge_current_capture(vec![native], vec![generated.clone()]).unwrap();

    assert_eq!(merged.len(), 1);
    assert_eq!(merged[0].id, generated.id);
    assert_eq!(merged[0].source.as_deref(), Some("opencode_db"));
    assert_eq!(merged[0].metadata["provider_log"], true);
    assert_eq!(
        merged[0].metadata["opencode_session_id"],
        "opencode-native-session-1"
    );
    assert_eq!(merged[0].turn_id.as_deref(), Some("opencode-message-1"));
    assert_eq!(merged[0].metadata["legacy_event_ids"][0], native_event_id());
}

#[test]
fn opencode_unbound_generated_local_echo_reconciles_with_authoritative_native_session() {
    let (_guard, _temp) = isolate();
    let (mut generated, native) = opencode_local_echo_fixture();
    generated.turn_id = None;

    let merged = provenance::merge_current_capture(vec![native], vec![generated.clone()]).unwrap();

    assert_eq!(merged.len(), 1);
    assert_eq!(merged[0].id, generated.id);
    assert_eq!(merged[0].source.as_deref(), Some("opencode_db"));
    assert_eq!(merged[0].metadata["provider_log"], true);
    assert_eq!(
        merged[0].metadata["opencode_session_id"],
        "opencode-native-session-1"
    );
    assert_eq!(merged[0].turn_id.as_deref(), Some("opencode-message-1"));
}

#[test]
fn opencode_reconciles_when_native_projection_is_already_archived() {
    let (_guard, _temp) = isolate();
    let (generated, native) = opencode_local_echo_fixture();

    let merged = provenance::merge_current_capture(
        vec![native.clone()],
        vec![generated.clone(), native.clone()],
    )
    .unwrap();

    assert_eq!(merged.len(), 1);
    assert_eq!(merged[0].id, generated.id);
    assert_eq!(merged[0].source.as_deref(), Some("opencode_db"));
    assert_eq!(merged[0].metadata["provider_log"], true);
    assert_eq!(merged[0].metadata["legacy_event_ids"][0], native_event_id());
}

fn native_event_id() -> &'static str {
    "agent-opencode-fixture:0000000000000001:opencode_db:part-1"
}

#[test]
fn opencode_distinct_actual_turns_with_identical_text_remain_separate() {
    let (_guard, _temp) = isolate();
    let (generated, native) = opencode_local_echo_fixture();
    let mut second_native = native.clone();
    second_native.id = "agent-opencode-fixture:0000000000000002:opencode_db:part-2".into();
    second_native.turn_id = Some("opencode-message-2".into());
    second_native.created_at = Some("2026-09-13T17:20:35.213Z".into());
    second_native.metadata["part_id"] = serde_json::json!("part-2");
    second_native.metadata["request_root_id"] = serde_json::json!("opencode-message-2");
    let merged =
        provenance::merge_current_capture(vec![native, second_native], vec![generated]).unwrap();

    assert_eq!(merged.len(), 2);
    assert!(merged.iter().any(|event| {
        event.metadata["legacy_event_ids"]
            .as_array()
            .is_some_and(|ids| ids.iter().any(|id| id == native_event_id()))
    }));
    assert!(merged
        .iter()
        .any(|event| event.id.ends_with("opencode_db:part-2")));
}

#[test]
fn opencode_archived_duplicate_cleanup_preserves_distinct_actual_turn() {
    let (_guard, _temp) = isolate();
    let (generated, native) = opencode_local_echo_fixture();
    let mut second_native = native.clone();
    second_native.id = "agent-opencode-fixture:0000000000000002:opencode_db:part-2".into();
    second_native.turn_id = Some("opencode-message-2".into());
    second_native.created_at = Some("2026-09-13T17:20:35.213Z".into());
    second_native.metadata["part_id"] = serde_json::json!("part-2");
    second_native.metadata["request_root_id"] = serde_json::json!("opencode-message-2");

    let merged = provenance::merge_current_capture(
        vec![native.clone(), second_native.clone()],
        vec![generated.clone(), native],
    )
    .unwrap();

    assert_eq!(merged.len(), 2);
    assert!(merged.iter().any(|event| event.id == generated.id));
    assert!(merged.iter().any(|event| event.id == second_native.id));
}

#[test]
fn opencode_whitespace_distinct_inputs_remain_separate() {
    let (_guard, _temp) = isolate();
    let (mut generated, native) = opencode_local_echo_fixture();
    generated.text = Some(" SANITIZED_OPEN_CODE_PROMPT".into());

    let merged =
        provenance::merge_current_capture(vec![native.clone()], vec![generated.clone()]).unwrap();

    assert_eq!(merged.len(), 2);
    assert!(merged.iter().any(|event| event.id == generated.id));
    assert!(merged.iter().any(|event| event.id == native.id));
}

#[test]
fn opencode_ambiguous_unbound_generated_echoes_remain_separate() {
    let (_guard, _temp) = isolate();
    let (mut generated, native) = opencode_local_echo_fixture();
    generated.turn_id = None;
    let mut repeated = generated.clone();
    repeated.id = "generated:conversation:opencode:2".into();
    repeated.sequence = Some(2);
    repeated.created_at = Some("2026-09-13T17:20:36.508Z".into());
    repeated.metadata["request_root_id"] = serde_json::json!("wardian:input:2");

    let merged =
        provenance::merge_current_capture(vec![native.clone()], vec![generated, repeated]).unwrap();

    assert_eq!(merged.len(), 3);
    assert!(merged
        .iter()
        .any(|event| event.id == "generated:conversation:opencode:1"));
    assert!(merged
        .iter()
        .any(|event| event.id == "generated:conversation:opencode:2"));
    assert!(merged.iter().any(|event| event.id == native.id));
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
