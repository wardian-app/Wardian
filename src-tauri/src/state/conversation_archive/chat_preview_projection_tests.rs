use super::*;
use crate::state::conversation_archive::chat_turn_index::{PreviewOwners, Scope};
use wardian_core::models::chat::{AgentChatEventKind, AgentChatRole, AgentChatStatus};

const ADMISSION: &str = "synthetic-admission";
const ROOT: &str = "synthetic-request-t";
const NEXT_ROOT: &str = "synthetic-request-next";

fn scope() -> Scope<'static> {
    Scope {
        agent_id: "synthetic-agent",
        conversation_id: "synthetic-conversation",
        source_key: Some("synthetic-source"),
        source_epoch: Some("synthetic-epoch"),
        can_activate: false,
    }
}

fn native(ordinal: u64, root: &str, kind: AgentChatEventKind) -> AgentChatEvent {
    serde_json::from_value(json!({
        "id": format!("display-{ordinal}"),
        "session_id": scope().agent_id,
        "provider": "codex",
        "kind": kind,
        "role": "assistant",
        "text": format!("Synthetic content {ordinal}"),
        "sequence": ordinal,
        "metadata": {
            "request_root_id": root,
            "provider_turn_id": format!("provider-{root}"),
            "chat_turn_adapter": "codex_bracket_v1",
            "chat_turn_membership_complete": false,
            "chat_turn_membership_partial": false,
            "chat_provisional": true,
            "chat_source_ref": format!("native-{ordinal}"),
            "chat_source_start": ordinal * 100,
            "chat_source_end": ordinal * 100 + 50,
            "chat_source_epoch": scope().source_epoch,
        },
    }))
    .expect("synthetic native-proven header")
}

fn prompt(ordinal: u64, root: &str) -> AgentChatEvent {
    let mut event = native(ordinal, root, AgentChatEventKind::Message);
    event.role = Some(AgentChatRole::User);
    event.metadata["input_origin"] = json!("human_input");
    event.metadata["input_purpose"] = json!("request");
    event
}

fn unattributed(mut event: AgentChatEvent) -> AgentChatEvent {
    let metadata = event.metadata.as_object_mut().unwrap();
    metadata.remove("request_root_id");
    metadata.remove("provider_turn_id");
    metadata.remove("chat_turn_adapter");
    event
}

fn tool(ordinal: u64, result: bool, call: &str) -> AgentChatEvent {
    let mut event = native(
        ordinal,
        ROOT,
        if result {
            AgentChatEventKind::ToolResult
        } else {
            AgentChatEventKind::ToolCall
        },
    );
    event.metadata["call_id"] = json!(call);
    event.metadata["tool_name"] = json!("exec_command");
    event.metadata["tool_input"] = json!({"description": "Run synthetic tests"});
    event.status = Some(if result {
        AgentChatStatus::Succeeded
    } else {
        AgentChatStatus::Running
    });
    event
}

fn prefix() -> Vec<AgentChatEvent> {
    vec![
        prompt(1, ROOT),
        tool(2, false, "synthetic-call"),
        tool(3, true, "synthetic-call"),
        native(4, ROOT, AgentChatEventKind::Message),
    ]
}

fn terminal_extension() -> Vec<AgentChatEvent> {
    let mut events = prefix();
    events.push(unattributed(native(5, ROOT, AgentChatEventKind::Message)));
    events.push(native(6, ROOT, AgentChatEventKind::Message));
    let mut answer = native(7, ROOT, AgentChatEventKind::Message);
    answer.metadata["provider_phase"] = json!("final_answer");
    answer.status = Some(AgentChatStatus::Succeeded);
    events.push(answer);
    let mut terminal = native(8, ROOT, AgentChatEventKind::Status);
    terminal.metadata["raw_type"] = json!("task_complete");
    terminal.metadata["chat_turn_membership_complete"] = json!(true);
    terminal.status = Some(AgentChatStatus::Succeeded);
    events.push(terminal);
    events
}

struct Fixture {
    _temp: tempfile::TempDir,
    store: Store,
    head: Head,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let store = Store::writer(temp.path());
        Self {
            _temp: temp,
            store,
            head: Head {
                agent_id: scope().agent_id.into(),
                conversation_id: scope().conversation_id.into(),
                source_key: scope().source_key.map(str::to_owned),
                generation: "synthetic-generation".into(),
                source_epoch: scope().source_epoch.map(str::to_owned),
                root: None,
                identities: None,
                logical: Default::default(),
                turns: Default::default(),
                committed_output_bytes: 0,
                capture_stamp: "synthetic-capture".into(),
                progress: "provisional".into(),
                source_coverage: None,
                parent: None,
                changes: Vec::new(),
                recent_start: 0,
                row_count: 0,
                recent_page: None,
                bootstrap: None,
            },
        }
    }

    fn project(
        &mut self,
        events: &mut [AgentChatEvent],
        previous: &State,
        owners: Option<&PreviewOwners>,
    ) -> io::Result<(ChatPresentationV1, State)> {
        let openings = previous.openings();
        let owners_by_root = owners.map(|owners| {
            previous
                .openings()
                .into_iter()
                .chain(
                    events
                        .iter()
                        .filter_map(|event| request_root(event).map(str::to_owned)),
                )
                .map(|root| (root, owners.clone()))
                .collect()
        });
        build(
            Build {
                store: &mut self.store,
                head: &self.head,
                head_ref: "synthetic-head",
                admission: ADMISSION,
                running: true,
                openings: &openings,
                previous,
                owners: owners_by_root.as_ref(),
                canonical: None,
                // A bound snapshot also exposes the all-legacy outcome directly.
                bound: true,
            },
            events,
        )
        .map(|value| {
            let (value, state, _) =
                value.expect("a bound snapshot retains its legacy representation");
            (value, state)
        })
    }

    fn remember(
        &mut self,
        events: &[AgentChatEvent],
        value: &ChatPresentationV1,
        previous: Option<&PreviewOwners>,
    ) -> PreviewOwners {
        let refs: BTreeSet<_> = value
            .items
            .iter()
            .flat_map(|item| &item.event_refs)
            .chain(
                value
                    .state_event_refs
                    .iter()
                    .map(|reference| &reference.event_id),
            )
            .collect();
        let qualified: Vec<_> = events
            .iter()
            .filter(|event| refs.contains(&event.id))
            .cloned()
            .collect();
        self.head
            .turns
            .preview_owners_with_previous(&mut self.store, &scope(), &qualified, previous)
            .unwrap()
            .expect("same-source certified members have owners")
            .remember(&scope(), ADMISSION, value)
            .unwrap()
            .expect("retain only this bound snapshot's certified owners")
    }
}

fn ids(events: &[AgentChatEvent]) -> Vec<String> {
    events.iter().map(|event| event.id.clone()).collect()
}

fn legacy_ids(value: &ChatPresentationV1) -> Vec<String> {
    value
        .legacy_turns
        .iter()
        .flat_map(|turn| turn.event_refs.clone())
        .collect()
}

fn ordered_groups(value: &ChatPresentationV1) -> Vec<Vec<String>> {
    let mut groups: Vec<_> = value
        .turns
        .iter()
        .map(|turn| {
            let refs: Vec<_> = value
                .items
                .iter()
                .filter(|item| item.turn_key == turn.turn_key)
                .flat_map(|item| item.event_refs.clone())
                .collect();
            (turn.order_key.clone(), refs)
        })
        .chain(
            value
                .legacy_turns
                .iter()
                .map(|turn| (turn.order_key.clone(), turn.event_refs.clone())),
        )
        .collect();
    groups.sort_by(|left, right| left.0.cmp(&right.0));
    groups.into_iter().map(|(_, refs)| refs).collect()
}

fn assert_running_prefix(value: &ChatPresentationV1) {
    assert_eq!(value.turns.len(), 1);
    let turn = &value.turns[0];
    assert_eq!(turn.status, ChatTurnStatus::Running);
    assert!(turn.prefix_attributed);
    assert!(!turn.membership_complete);
    assert!(!turn.counts_complete);
    assert!(!turn.answer_recorded);
    assert!(turn.answer_item_keys.is_empty());
    assert_eq!(turn.step_count, 1);
}

#[test]
fn appended_lifecycle_rows_use_physical_order_without_skipping_a_real_gap() {
    for earlier in [true, false] {
        let mut fixture = Fixture::new();
        let mut events = prefix();
        let mut unproven = unattributed(native(0, "earlier-turn", AgentChatEventKind::Status));
        if !earlier {
            unproven.metadata["chat_source_start"] = json!(250);
            unproven.metadata["chat_source_end"] = json!(275);
        }
        let unproven_id = unproven.id.clone();
        // The ordinary reader appends unindexed lifecycle rows after the
        // canonical messages, even when their physical source position is older.
        events.push(unproven);
        let mut terminal = native(5, ROOT, AgentChatEventKind::Status);
        terminal.status = Some(AgentChatStatus::Succeeded);
        terminal.metadata["raw_type"] = json!("task_complete");
        terminal.metadata["chat_turn_membership_complete"] = json!(true);
        let terminal_id = terminal.id.clone();
        events.push(terminal);
        super::super::preview_bridge::order_source_snapshot(&mut events, &fixture.head);
        let (value, _) = fixture
            .project(&mut events, &State::default(), None)
            .unwrap();
        if earlier {
            assert_eq!(value.turns.len(), 1);
            assert_eq!(value.turns[0].status, ChatTurnStatus::Completed);
            assert_eq!(legacy_ids(&value), vec![unproven_id]);
            assert_eq!(
                ordered_groups(&value),
                vec![ids(&events[..1]), ids(&events[1..events.len() - 1])]
            );
            assert!(value
                .state_event_refs
                .iter()
                .any(|reference| reference.event_id == terminal_id));
        } else {
            assert!(value.turns.is_empty());
            assert!(value.items.is_empty());
            assert_eq!(legacy_ids(&value), ids(&events));
        }
    }
}

#[test]
fn a_missing_source_position_preserves_the_unproven_barrier() {
    let fixture = Fixture::new();
    let mut events = vec![
        native(3, ROOT, AgentChatEventKind::Message),
        unattributed(native(2, ROOT, AgentChatEventKind::Message)),
        prompt(1, ROOT),
    ];
    events[1]
        .metadata
        .as_object_mut()
        .unwrap()
        .remove("chat_source_start");
    let original = ids(&events);
    assert!(!super::super::preview_bridge::order_source_snapshot(
        &mut events,
        &fixture.head
    ));
    assert_eq!(ids(&events), original);
}

#[test]
fn canonical_terminal_state_requires_the_exact_published_closing_record() {
    let fixture = Fixture::new();
    let mut terminal = native(8, ROOT, AgentChatEventKind::Status);
    terminal.metadata["raw_type"] = json!("task_complete");
    terminal.metadata["chat_source_ref"] =
        json!("source:synthetic-agent:synthetic-epoch:800:synthetic-digest:0");
    let order = format!(
        "source:{:020}:{:020}:{:020}:{}",
        800,
        850,
        658,
        terminal.metadata["chat_source_ref"].as_str().unwrap()
    );
    let owners = BTreeMap::from([(
        format!("provider-{ROOT}"),
        BTreeSet::from([ROOT.to_owned()]),
    )]);
    assert!(canonical_lifecycle(
        &terminal,
        ROOT,
        &fixture.head,
        &owners,
        Some(&order)
    ));
    for field in [
        "chat_source_ref",
        "chat_source_start",
        "chat_source_end",
        "chat_source_epoch",
        "provider_turn_id",
    ] {
        let mut other = terminal.clone();
        other.metadata[field] = match field {
            "chat_source_start" => json!(801),
            "chat_source_end" => json!(851),
            _ => json!("different-backing"),
        };
        assert!(
            !canonical_lifecycle(&other, ROOT, &fixture.head, &owners, Some(&order)),
            "{field}"
        );
    }
    assert!(!canonical_lifecycle(
        &terminal,
        ROOT,
        &fixture.head,
        &owners,
        None
    ));
    terminal.kind = AgentChatEventKind::Message;
    assert!(!canonical_lifecycle(
        &terminal,
        ROOT,
        &fixture.head,
        &owners,
        Some(&order)
    ));
}

#[test]
fn stored_complete_commentary_target_survives_an_unresolved_native_terminal() {
    use crate::providers::chat_transcript::{
        normalize_chat_lines_with_state, TranscriptNormalizationState,
    };
    let mut fixture = Fixture::new();
    let root = "prompt-128";
    let provider_turn = "synthetic-completed-target";
    let mut body = "Synthetic completed assistant answer. ".to_owned();
    body.push_str(&"x".repeat(950 - body.len()));
    assert_eq!(body.len(), 950);
    let mut raw = vec![
        json!({"type":"session_meta","payload":{"id":"synthetic-session"}}),
        json!({"type":"event_msg","payload":{"type":"task_started","turn_id":provider_turn}}),
        json!({"type":"turn_context","payload":{"turn_id":provider_turn}}),
        json!({"type":"response_item","payload":{"type":"message","id":root,"role":"user","content":[{"type":"input_text","text":"Complete synthetic turn 128."}],"internal_chat_message_metadata_passthrough":{"turn_id":provider_turn,"content_item_kinds":["user.text"]}}}),
    ];
    for index in 0..12 {
        raw.push(json!({"type":"event_msg","payload":{"type":"agent_message","turn_id":provider_turn,"phase":"commentary","message":format!("Synthetic target progress {} of 12.", index + 1)}}));
    }
    raw.extend([
        json!({"type":"response_item","payload":{"type":"message","id":"answer-128","role":"assistant","phase":"final_answer","content":[{"type":"output_text","text":body}],"internal_chat_message_metadata_passthrough":{"turn_id":provider_turn}}}),
        json!({"type":"event_msg","payload":{"type":"task_complete","turn_id":provider_turn,"last_agent_message":body}}),
    ]);
    let mut normalization = TranscriptNormalizationState::default();
    let mut start = 0u64;
    let mut events = Vec::new();
    for value in raw {
        let line = value.to_string();
        let end = start + line.len() as u64 + 1;
        for mut event in normalize_chat_lines_with_state(
            scope().agent_id,
            "codex",
            [line],
            &mut normalization,
            true,
            false,
        )
        .unwrap()
        {
            event.id = format!("synthetic-native:{start}:{end}");
            event.metadata["chat_source_ref"] = json!(event.id);
            event.metadata["chat_source_start"] = json!(start);
            event.metadata["chat_source_end"] = json!(end);
            event.metadata["chat_source_epoch"] = json!(scope().source_epoch);
            events.push(super::header(&event));
        }
        start = end;
    }
    let terminal = events
        .iter()
        .find(|event| event.metadata["raw_type"] == "task_complete")
        .unwrap()
        .clone();
    assert_eq!(terminal.metadata["chat_turn_membership_complete"], true);
    let mut captured: Vec<_> = events
        .iter()
        .filter(|event| {
            event.kind == AgentChatEventKind::Message && event.metadata["request_root_id"] == root
        })
        .cloned()
        .collect();
    assert_eq!(captured.len(), 14);
    let request = captured
        .iter_mut()
        .find(|event| event.role == Some(AgentChatRole::User))
        .unwrap();
    request.metadata["_wardian_turn_closure"] = json!({
        "version":1,"source_key":scope().source_key,"source_epoch":scope().source_epoch,
        "provider":"codex","adapter":"codex_bracket_v1","provider_turn_id":provider_turn,
        "request_root_id":root,"source_ref":terminal.id,"start":terminal.metadata["chat_source_start"],
        "end":terminal.metadata["chat_source_end"],"sequence":658,
        "status":"succeeded","complete":true,"partial":false,
    });
    fixture.head.progress = "ready".into();
    let capture_scope = Scope {
        can_activate: true,
        ..scope()
    };
    for event in &captured {
        let order = presentation_order(event).unwrap();
        assert!(fixture
            .head
            .turns
            .observe(&mut fixture.store, &capture_scope, event, &order, None)
            .unwrap()
            .is_some());
    }
    let canonical = fixture
        .head
        .turns
        .continuation_page(&mut fixture.store, &captured)
        .unwrap()
        .unwrap();
    assert_eq!(canonical.turns.len(), 1);
    assert_eq!(canonical.items.len(), 14);
    assert!(canonical.turns[0].membership_complete && canonical.turns[0].answer_recorded);
    assert_eq!(canonical.turns[0].status, ChatTurnStatus::Completed);
    let answer_id = captured.last().unwrap().id.clone();
    for event in &mut events {
        if event.metadata["raw_type"] == "task_complete" {
            event.metadata["chat_provisional"] = json!(true);
            event.metadata["chat_identity_resolution"] = json!("unresolved");
        }
    }
    let mut reader = Store::new(fixture._temp.path());
    let result = build(
        Build {
            store: &mut reader,
            head: &fixture.head,
            head_ref: "synthetic-head",
            admission: ADMISSION,
            running: false,
            openings: &BTreeSet::new(),
            previous: &State::default(),
            owners: None,
            canonical: Some(&canonical),
            bound: true,
        },
        &mut events,
    )
    .unwrap()
    .unwrap();
    let target_items: Vec<_> = result
        .0
        .items
        .iter()
        .filter(|item| item.event_refs.contains(&answer_id))
        .collect();
    assert_eq!(
        target_items.len(),
        1,
        "a stored complete answer must not become legacy during the bridge fallback"
    );
    assert_eq!(target_items[0].kind, ChatPresentedItemKind::Answer);
    assert_eq!(target_items[0].provenance, ChatItemProvenance::Canonical);
    assert!(result
        .0
        .items
        .iter()
        .all(|item| item.provenance == ChatItemProvenance::Canonical));
    assert!(result
        .0
        .state_event_refs
        .iter()
        .any(|reference| reference.event_id == terminal.id));
    assert!(result.0.validate().is_ok());
}

#[test]
fn running_prefix_stays_certified_when_the_next_prompt_is_unqualified() {
    let mut fixture = Fixture::new();
    let mut events = prefix();
    events.push(unattributed(prompt(5, NEXT_ROOT)));
    let (value, state) = fixture
        .project(&mut events, &State::default(), None)
        .unwrap();

    assert_running_prefix(&value);
    assert_eq!(state.openings(), BTreeSet::from([ROOT.to_string()]));
    assert_eq!(legacy_ids(&value), ids(&events[4..]));
    assert_eq!(
        ordered_groups(&value),
        vec![ids(&events[..4]), ids(&events[4..])]
    );
    assert_eq!(
        value.legacy_turns[0].kind,
        ChatLegacyTurnKind::EarlierActivity
    );
    assert!(value.validate().is_ok());
}

#[test]
fn exact_attribution_upgrades_the_next_prompt_at_its_existing_position() {
    let mut fixture = Fixture::new();
    let mut events = prefix();
    events.push(unattributed(prompt(5, NEXT_ROOT)));
    let (before, state) = fixture
        .project(&mut events, &State::default(), None)
        .unwrap();
    let owners = fixture.remember(&events, &before, None);
    let before_position = ordered_groups(&before);
    events[4] = prompt(5, NEXT_ROOT);
    let (after, state) = fixture.project(&mut events, &state, Some(&owners)).unwrap();

    assert_eq!(after.turns.len(), 2);
    assert_eq!(after.turns[0].turn_key, before.turns[0].turn_key);
    assert_eq!(after.turns[0].order_key, before.turns[0].order_key);
    assert_eq!(after.turns[1].status, ChatTurnStatus::Running);
    assert_eq!(ordered_groups(&after), before_position);
    assert!(after.legacy_turns.is_empty());
    assert_eq!(
        state.openings(),
        BTreeSet::from([ROOT.to_string(), NEXT_ROOT.to_string()])
    );
    let retained: Vec<_> = after
        .items
        .iter()
        .filter(|item| item.turn_key == before.turns[0].turn_key)
        .cloned()
        .collect();
    assert_eq!(retained, before.items);
    assert!(after.validate().is_ok());
}

#[test]
fn preprompt_task_started_resolves_only_the_exact_provider_turn() {
    for exact in [true, false] {
        let mut fixture = Fixture::new();
        let mut started = native(0, ROOT, AgentChatEventKind::Status);
        started
            .metadata
            .as_object_mut()
            .unwrap()
            .remove("request_root_id");
        started.metadata["raw_type"] = json!("task_started");
        if !exact {
            started.metadata["provider_turn_id"] = json!("unrelated-provider-turn");
        }
        let start_id = started.id.clone();
        let mut events = vec![started];
        events.extend(prefix());
        let (value, _) = fixture
            .project(&mut events, &State::default(), None)
            .unwrap();

        assert_running_prefix(&value);
        if exact {
            assert_eq!(events[0].metadata["request_root_id"], ROOT);
            assert!(value.legacy_turns.is_empty());
            assert_eq!(value.state_event_refs.len(), 1);
            assert_eq!(value.state_event_refs[0].event_id, start_id);
            assert_eq!(value.state_event_refs[0].turn_key, value.turns[0].turn_key);
        } else {
            assert!(events[0].metadata.get("request_root_id").is_none());
            assert_eq!(legacy_ids(&value), vec![start_id]);
            assert!(value.state_event_refs.is_empty());
        }
        assert!(value.validate().is_ok());
    }
}

#[test]
fn an_unknown_middle_keeps_a_fresh_turn_entirely_legacy() {
    let mut fixture = Fixture::new();
    let mut events = vec![
        prompt(1, ROOT),
        unattributed(native(2, ROOT, AgentChatEventKind::Message)),
        native(3, ROOT, AgentChatEventKind::Message),
    ];
    let (value, state) = fixture
        .project(&mut events, &State::default(), None)
        .unwrap();

    assert!(value.turns.is_empty());
    assert!(value.items.is_empty());
    assert!(value.state_event_refs.is_empty());
    assert!(state.openings().is_empty());
    assert_eq!(value.legacy_turns.len(), 1);
    assert_eq!(legacy_ids(&value), ids(&events));
    assert_eq!(
        value.legacy_turns[0].kind,
        ChatLegacyTurnKind::EarlierActivity
    );
    assert!(value.validate().is_ok());
}

#[test]
fn a_certified_prefix_retains_exact_items_and_source_ordered_legacy_on_refresh() {
    let mut fixture = Fixture::new();
    let mut events = prefix();
    let (before, state) = fixture
        .project(&mut events, &State::default(), None)
        .unwrap();
    let owners = fixture.remember(&events, &before, None);
    events.extend([
        unattributed(native(5, ROOT, AgentChatEventKind::Message)),
        native(6, ROOT, AgentChatEventKind::Message),
        tool(7, false, "extension-call"),
        tool(8, true, "extension-call"),
    ]);
    let (extended, state) = fixture.project(&mut events, &state, Some(&owners)).unwrap();

    assert_running_prefix(&extended);
    assert_eq!(extended.turns, before.turns);
    assert_eq!(extended.items, before.items);
    assert_eq!(legacy_ids(&extended), ids(&events[4..]));
    assert_eq!(
        ordered_groups(&extended),
        vec![ids(&events[..4]), ids(&events[4..])]
    );
    assert!(extended
        .legacy_turns
        .iter()
        .all(|turn| turn.kind == ChatLegacyTurnKind::EarlierActivity));
    let owners = fixture.remember(&events, &extended, Some(&owners));
    let (refreshed, _) = fixture.project(&mut events, &state, Some(&owners)).unwrap();
    assert_eq!(refreshed, extended);
    assert!(refreshed.validate().is_ok());
}

#[test]
fn an_unproven_gap_remains_a_barrier_after_it_and_the_certified_prefix_roll_out() {
    let mut fixture = Fixture::new();
    let mut events = prefix();
    let (before, state) = fixture
        .project(&mut events, &State::default(), None)
        .unwrap();
    let owners = fixture.remember(&events, &before, None);
    events.extend([
        unattributed(native(5, ROOT, AgentChatEventKind::Message)),
        native(6, ROOT, AgentChatEventKind::Message),
        native(7, ROOT, AgentChatEventKind::Message),
    ]);
    let (gapped, state) = fixture.project(&mut events, &state, Some(&owners)).unwrap();
    assert_eq!(gapped.items, before.items);
    assert_eq!(legacy_ids(&gapped), ids(&events[4..]));
    assert!(gapped
        .legacy_turns
        .iter()
        .all(|turn| turn.kind == ChatLegacyTurnKind::EarlierActivity));
    let owners = fixture.remember(&events, &gapped, Some(&owners));

    // The gap and every certified row leave the bounded window. Its old opening
    // certifies no new members across that still-unproven source interval.
    let mut current = events[5..].to_vec();
    let (rolled, state) = fixture
        .project(&mut current, &state, Some(&owners))
        .unwrap();
    assert_eq!(state.openings(), BTreeSet::from([ROOT.to_string()]));
    assert_eq!(rolled.turns.len(), 1);
    assert_eq!(rolled.turns[0].turn_key, before.turns[0].turn_key);
    assert_eq!(rolled.turns[0].order_key, before.turns[0].order_key);
    assert!(!rolled.turns[0].membership_complete);
    assert!(!rolled.turns[0].counts_complete);
    assert!(
        rolled.items.is_empty(),
        "the remembered opening cannot qualify later rows across a hidden gap"
    );
    assert!(rolled.state_event_refs.is_empty());
    assert_eq!(legacy_ids(&rolled), ids(&current));
    assert_eq!(rolled.legacy_turns.len(), 1);
    assert_eq!(
        rolled.legacy_turns[0].kind,
        ChatLegacyTurnKind::EarlierActivity
    );
    assert!(rolled.validate().is_ok());

    let owners = fixture.remember(&current, &rolled, Some(&owners));
    let (refreshed, state) = fixture
        .project(&mut current, &state, Some(&owners))
        .unwrap();
    assert_eq!(refreshed, rolled);

    // Actual attribution of the missing source row clears the barrier.
    events[4] = native(5, ROOT, AgentChatEventKind::Message);
    let (proven, _) = fixture.project(&mut events, &state, Some(&owners)).unwrap();
    assert_eq!(proven.turns.len(), 1);
    assert_eq!(proven.turns[0].turn_key, before.turns[0].turn_key);
    assert_eq!(proven.turns[0].order_key, before.turns[0].order_key);
    assert!(proven.legacy_turns.is_empty());
    assert_eq!(ordered_groups(&proven), vec![ids(&events)]);
    assert!(proven.validate().is_ok());
}

#[test]
fn a_suffix_gap_survives_three_observations_with_or_without_visible_certified_members() {
    for prefix_visible in [true, false] {
        let mut fixture = Fixture::new();
        let mut certified = prefix();
        let (before, state) = fixture
            .project(&mut certified, &State::default(), None)
            .unwrap();
        let owners = fixture.remember(&certified, &before, None);
        let frontier = presentation_order(certified.last().unwrap());

        // There is no later attributed T row to bracket this observation.
        let unknown = unattributed(native(5, ROOT, AgentChatEventKind::Message));
        let mut second = if prefix_visible {
            certified
        } else {
            Vec::new()
        };
        second.push(unknown.clone());
        let (gapped, state) = fixture.project(&mut second, &state, Some(&owners)).unwrap();
        assert_eq!(
            state.roots[ROOT].unresolved,
            BTreeSet::from(["native-5".into()])
        );
        assert_eq!(state.roots[ROOT].last_source_order, frontier);
        assert!(!state.roots[ROOT].gap_unresolvable);
        assert_eq!(legacy_ids(&gapped), vec![unknown.id]);
        if prefix_visible {
            assert_eq!(gapped.items, before.items);
        } else {
            assert!(gapped.items.is_empty());
        }

        // Both the uncertainty and every certified member now leave the view.
        let mut third = vec![native(6, ROOT, AgentChatEventKind::Message)];
        let (rolled, state) = fixture.project(&mut third, &state, Some(&owners)).unwrap();
        assert_eq!(
            state.roots[ROOT].unresolved,
            BTreeSet::from(["native-5".into()])
        );
        assert_eq!(state.roots[ROOT].last_source_order, frontier);
        assert!(!state.roots[ROOT].members.contains("native-6"));
        assert!(rolled.items.is_empty());
        assert!(rolled.state_event_refs.is_empty());
        assert_eq!(legacy_ids(&rolled), ids(&third));
        assert_eq!(rolled.turns[0].turn_key, before.turns[0].turn_key);
        assert!(!rolled.turns[0].membership_complete);
        assert!(!rolled.turns[0].counts_complete);
        assert!(!rolled.turns[0].answer_recorded);
        assert!(rolled.validate().is_ok());
    }
}

#[test]
fn a_proven_next_turn_opening_keeps_its_uncertainty_out_of_the_previous_prefix() {
    let mut fixture = Fixture::new();
    let mut events = prefix();
    let (before, state) = fixture
        .project(&mut events, &State::default(), None)
        .unwrap();
    let owners = fixture.remember(&events, &before, None);
    let next = prompt(5, NEXT_ROOT);
    let boundary = presentation_order(&next);
    events.extend([
        next,
        unattributed(native(6, NEXT_ROOT, AgentChatEventKind::Message)),
    ]);
    let (partitioned, state) = fixture.project(&mut events, &state, Some(&owners)).unwrap();
    assert!(state.roots[ROOT].unresolved.is_empty());
    assert_eq!(state.roots[ROOT].boundary_order, boundary);
    assert_eq!(
        state.roots[NEXT_ROOT].unresolved,
        BTreeSet::from(["native-6".into()])
    );
    let retained: Vec<_> = partitioned
        .items
        .iter()
        .filter(|item| item.turn_key == before.turns[0].turn_key)
        .cloned()
        .collect();
    assert_eq!(retained, before.items);
    assert_eq!(legacy_ids(&partitioned), ids(&events[5..]));
    let owners = fixture.remember(&events, &partitioned, Some(&owners));

    // The independent opening rolls out too. Its boundary remains a source
    // fact, without granting either turn ownership of this unrooted activity.
    let mut current = vec![unattributed(native(
        7,
        NEXT_ROOT,
        AgentChatEventKind::Message,
    ))];
    let (rolled, state) = fixture
        .project(&mut current, &state, Some(&owners))
        .unwrap();
    assert!(state.roots[ROOT].unresolved.is_empty());
    assert_eq!(state.roots[ROOT].boundary_order, boundary);
    assert_eq!(
        state.roots[NEXT_ROOT].unresolved,
        BTreeSet::from(["native-6".into(), "native-7".into()])
    );
    assert!(rolled.items.is_empty());
    assert_eq!(legacy_ids(&rolled), ids(&current));
    assert!(rolled.validate().is_ok());
}

#[test]
fn a_terminal_inside_an_extension_gap_stops_working_without_claiming_an_answer() {
    let mut fixture = Fixture::new();
    let mut events = prefix();
    let (before, state) = fixture
        .project(&mut events, &State::default(), None)
        .unwrap();
    let owners = fixture.remember(&events, &before, None);
    let mut events = terminal_extension();
    let (value, _) = fixture.project(&mut events, &state, Some(&owners)).unwrap();

    assert_eq!(value.turns.len(), 1);
    let turn = &value.turns[0];
    assert_eq!(turn.turn_key, before.turns[0].turn_key);
    assert_eq!(turn.order_key, before.turns[0].order_key);
    assert_eq!(turn.status, ChatTurnStatus::Incomplete);
    assert!(turn.prefix_attributed);
    assert!(!turn.membership_complete);
    assert!(!turn.counts_complete);
    assert!(!turn.references_complete);
    assert!(!turn.answer_recorded);
    assert!(turn.answer_item_keys.is_empty());
    assert_eq!(turn.step_count, before.turns[0].step_count);
    assert_eq!(value.items, before.items);
    assert!(value
        .items
        .iter()
        .all(|item| item.kind != ChatPresentedItemKind::Answer));
    assert_eq!(legacy_ids(&value), ids(&events[4..]));
    assert!(value.validate().is_ok());
}

#[test]
fn proving_all_extension_members_completes_the_same_turn_with_stable_keys_and_order() {
    let mut fixture = Fixture::new();
    let mut events = prefix();
    let (before, state) = fixture
        .project(&mut events, &State::default(), None)
        .unwrap();
    let owners = fixture.remember(&events, &before, None);
    let mut events = terminal_extension();
    let (gapped, state) = fixture.project(&mut events, &state, Some(&owners)).unwrap();
    let owners = fixture.remember(&events, &gapped, Some(&owners));
    events[4] = native(5, ROOT, AgentChatEventKind::Message);
    let (complete, _) = fixture.project(&mut events, &state, Some(&owners)).unwrap();

    assert_eq!(complete.turns.len(), 1);
    let turn = &complete.turns[0];
    assert_eq!(turn.turn_key, before.turns[0].turn_key);
    assert_eq!(turn.order_key, before.turns[0].order_key);
    assert!(turn.membership_complete);
    assert_eq!(turn.status, ChatTurnStatus::Completed);
    assert!(turn.answer_recorded);
    assert_eq!(turn.step_count, before.turns[0].step_count);
    assert!(complete.legacy_turns.is_empty());
    let answer = complete
        .items
        .iter()
        .find(|item| item.kind == ChatPresentedItemKind::Answer)
        .expect("the proven closed turn records its answer");
    assert_eq!(answer.event_refs, vec![events[6].id.clone()]);
    assert_eq!(turn.answer_item_keys, vec![answer.item_key.clone()]);
    for old in &before.items {
        let current = complete
            .items
            .iter()
            .find(|item| item.item_key == old.item_key)
            .expect("certified prefix item remains in place");
        assert_eq!(current, old);
    }
    let mut fresh = Fixture::new();
    let (direct, _) = fresh.project(&mut events, &State::default(), None).unwrap();
    assert_eq!(complete.items, direct.items);
    assert_eq!(complete.turns, direct.turns);
    assert!(complete.validate().is_ok());
}

#[test]
fn a_partial_terminal_stops_working_even_when_the_terminal_root_is_missing() {
    let mut fixture = Fixture::new();
    let mut events = prefix();
    let (before, state) = fixture
        .project(&mut events, &State::default(), None)
        .unwrap();
    let owners = fixture.remember(&events, &before, None);
    let mut extended = terminal_extension();
    let terminal = extended.last_mut().unwrap();
    terminal.turn_id = terminal.metadata["provider_turn_id"]
        .as_str()
        .map(str::to_owned);
    terminal.metadata["request_root_id"] = json!(null);
    terminal.metadata["provider_turn_id"] = json!(null);
    terminal.metadata["chat_turn_membership_complete"] = json!(false);
    terminal.metadata["chat_turn_membership_partial"] = json!(true);
    let (gapped, state) = fixture
        .project(&mut extended, &state, Some(&owners))
        .unwrap();
    assert_eq!(gapped.turns[0].status, ChatTurnStatus::Incomplete);
    assert!(!gapped.turns[0].counts_complete && gapped.turns[0].answer_item_keys.is_empty());
    assert_eq!(legacy_ids(&gapped), ids(&extended[4..]));
    let owners = fixture.remember(&extended, &gapped, Some(&owners));
    let (again, _) = fixture.project(&mut events, &state, Some(&owners)).unwrap();
    assert_eq!(again.turns[0].status, ChatTurnStatus::Incomplete);
    assert_eq!(again.turns[0].turn_key, before.turns[0].turn_key);
    assert!(again.turns[0].answer_item_keys.is_empty());
}

#[test]
fn an_actual_certified_membership_contradiction_is_invalid_data() {
    for kind in [
        "codex_provider_turn_conflict",
        "codex_request_root_conflict",
    ] {
        let mut fixture = Fixture::new();
        let mut events = prefix();
        let (before, state) = fixture
            .project(&mut events, &State::default(), None)
            .unwrap();
        let owners = fixture.remember(&events, &before, None);
        let mut diagnostic = native(5, ROOT, AgentChatEventKind::Status);
        diagnostic.metadata["chat_turn_membership_partial"] = json!(true);
        diagnostic.metadata["chat_turn_membership_contradiction"] = json!(kind);
        diagnostic.metadata["chat_turn_membership_contradicted_root"] = json!(ROOT);
        events.push(diagnostic);
        let error = match fixture.project(&mut events, &state, Some(&owners)) {
            Err(error) => error,
            Ok(_) => panic!("an explicit {kind} must retract the contradicted certificate"),
        };
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }
}

#[test]
fn honest_unpaired_and_early_results_do_not_retract_a_certified_prefix() {
    for later_invocation in [false, true] {
        let mut fixture = Fixture::new();
        let mut events = prefix();
        let (before, state) = fixture
            .project(&mut events, &State::default(), None)
            .unwrap();
        let owners = fixture.remember(&events, &before, None);
        let mut early = tool(5, true, "unpaired-call");
        early.metadata["chat_turn_membership_partial"] = json!(true);
        assert!(early
            .metadata
            .get("chat_turn_membership_contradiction")
            .is_none());
        events.push(early);
        if later_invocation {
            events.push(tool(6, false, "unpaired-call"));
        }
        let (value, state) = fixture
            .project(&mut events, &state, Some(&owners))
            .expect("missing or early invocation evidence is uncertainty, not a contradiction");

        assert_running_prefix(&value);
        assert_eq!(value.turns, before.turns);
        assert_eq!(value.items, before.items);
        assert_eq!(legacy_ids(&value), ids(&events[4..]));
        assert_eq!(state.openings(), BTreeSet::from([ROOT.to_string()]));
        assert!(value.validate().is_ok());
    }
}

#[test]
fn a_rolled_gap_resolves_only_through_matching_identity_and_same_head_row_membership() {
    let mut fixture = Fixture::new();
    let mut events = prefix();
    let (before, state) = fixture
        .project(&mut events, &State::default(), None)
        .unwrap();
    let owners = fixture.remember(&events, &before, None);
    events.extend([
        unattributed(native(5, ROOT, AgentChatEventKind::Message)),
        native(6, ROOT, AgentChatEventKind::Message),
        native(7, ROOT, AgentChatEventKind::Message),
    ]);
    let (gapped, state) = fixture.project(&mut events, &state, Some(&owners)).unwrap();
    let owners = fixture.remember(&events, &gapped, Some(&owners));
    let mut current = events[5..].to_vec();
    let current_ids = ids(&current);
    let pending_ids = BTreeSet::from(["native-5".to_string()]);
    let assert_pending = |value: &ChatPresentationV1, state: &State| {
        let certified = state.roots.get(ROOT).expect("retain the certified turn");
        assert_eq!(certified.unresolved, pending_ids);
        assert!(!certified.gap_unresolvable);
        assert_eq!(value.turns.len(), 1);
        assert_eq!(value.turns[0].turn_key, before.turns[0].turn_key);
        assert_eq!(value.turns[0].order_key, before.turns[0].order_key);
        assert!(value.items.is_empty());
        assert_eq!(legacy_ids(value), current_ids);
        assert!(value.validate().is_ok());
    };
    let (rolled, state) = fixture
        .project(&mut current, &state, Some(&owners))
        .unwrap();
    assert_pending(&rolled, &state);

    let row = Row {
        agent_id: scope().agent_id.into(),
        conversation_id: scope().conversation_id.into(),
        key: format!("{:020}", 5),
        event: native(5, ROOT, AgentChatEventKind::Message),
        body: None,
        relation: None,
        removed_display_ids: Vec::new(),
    };
    let reference = fixture.store.put(&row).unwrap();
    let identity = digest(b"native-5");
    fixture.head.identities = Some(
        fixture
            .store
            .insert(&fixture.head.identities, &identity, &reference)
            .unwrap(),
    );
    // An identity entry left over from a prior Head cannot prove membership.
    let (identity_only, pending) = fixture
        .project(&mut current, &state, Some(&owners))
        .unwrap();
    assert_pending(&identity_only, &pending);

    let mut stale = row.clone();
    stale.event.text = Some("Older synthetic row version".into());
    let stale_reference = fixture.store.put(&stale).unwrap();
    fixture.head.root = Some(
        fixture
            .store
            .insert(&fixture.head.root, &row.key, &stale_reference)
            .unwrap(),
    );
    let (different_version, pending) = fixture
        .project(&mut current, &state, Some(&owners))
        .unwrap();
    assert_pending(&different_version, &pending);

    for invalid in [
        "row_agent",
        "event_agent",
        "conversation",
        "epoch",
        "request_root",
        "adapter",
        "partial",
        "provider_turn",
        "physical",
    ] {
        let mut candidate = row.clone();
        match invalid {
            "row_agent" => candidate.agent_id = "foreign-agent".into(),
            "event_agent" => candidate.event.session_id = "foreign-agent".into(),
            "conversation" => candidate.conversation_id = "foreign-conversation".into(),
            "epoch" => candidate.event.metadata["chat_source_epoch"] = json!("foreign-epoch"),
            "request_root" => candidate.event.metadata["request_root_id"] = json!(NEXT_ROOT),
            "adapter" => candidate.event.metadata["chat_turn_adapter"] = json!("unsupported"),
            "partial" => candidate.event.metadata["chat_turn_membership_partial"] = json!(true),
            "provider_turn" => {
                candidate
                    .event
                    .metadata
                    .as_object_mut()
                    .unwrap()
                    .remove("provider_turn_id");
            }
            "physical" => candidate.event.metadata["chat_source_ref"] = json!("foreign-physical"),
            _ => unreachable!(),
        }
        let invalid_reference = fixture.store.put(&candidate).unwrap();
        fixture.head.identities = Some(
            fixture
                .store
                .insert(&fixture.head.identities, &identity, &invalid_reference)
                .unwrap(),
        );
        fixture.head.root = Some(
            fixture
                .store
                .insert(&fixture.head.root, &candidate.key, &invalid_reference)
                .unwrap(),
        );
        let (invalid_proof, pending) = fixture
            .project(&mut current, &state, Some(&owners))
            .unwrap();
        assert_pending(&invalid_proof, &pending);
    }

    fixture.head.identities = Some(
        fixture
            .store
            .insert(&fixture.head.identities, &identity, &reference)
            .unwrap(),
    );
    fixture.head.root = Some(
        fixture
            .store
            .insert(&fixture.head.root, &row.key, &reference)
            .unwrap(),
    );
    let (resolved, state) = fixture
        .project(&mut current, &state, Some(&owners))
        .unwrap();
    assert!(state.roots[ROOT].unresolved.is_empty());
    assert!(!state.roots[ROOT].gap_unresolvable);
    assert_eq!(resolved.turns.len(), 1);
    assert_eq!(resolved.turns[0].turn_key, before.turns[0].turn_key);
    assert_eq!(resolved.turns[0].order_key, before.turns[0].order_key);
    assert!(resolved.legacy_turns.is_empty());
    assert_eq!(ordered_groups(&resolved), vec![current_ids]);
    assert!(resolved.validate().is_ok());
}

#[test]
fn overflowing_gap_selectors_stays_unresolvable_after_later_proof_and_closure() {
    let mut fixture = Fixture::new();
    let mut events = prefix();
    let (before, state) = fixture
        .project(&mut events, &State::default(), None)
        .unwrap();
    let owners = fixture.remember(&events, &before, None);
    for ordinal in 5..14 {
        let mut unknown = unattributed(native(ordinal, ROOT, AgentChatEventKind::Message));
        let physical = format!("gap-{ordinal:04}-{}", "x".repeat(503));
        assert_eq!(physical.len(), 512);
        unknown.metadata["chat_source_ref"] = json!(physical);
        events.push(unknown);
    }
    assert_eq!(events.len() - 4, 9);
    assert_eq!(
        events[4..]
            .iter()
            .map(|event| event.metadata["chat_source_ref"].as_str().unwrap().len())
            .sum::<usize>(),
        4608
    );
    events.push(native(14, ROOT, AgentChatEventKind::Message));
    let (overflowed, state) = fixture.project(&mut events, &state, Some(&owners)).unwrap();
    assert!(state.roots[ROOT].gap_unresolvable);
    assert!(state.roots[ROOT].unresolved.is_empty());
    assert_eq!(overflowed.items, before.items);
    assert_eq!(overflowed.turns, before.turns);
    assert_eq!(legacy_ids(&overflowed), ids(&events[4..]));
    assert!(overflowed
        .legacy_turns
        .iter()
        .all(|turn| turn.kind == ChatLegacyTurnKind::EarlierActivity));
    let owners = fixture.remember(&events, &overflowed, Some(&owners));

    // Preserve every physical ID while supplying full later attribution.
    for event in &mut events[4..] {
        event.metadata["request_root_id"] = json!(ROOT);
        event.metadata["provider_turn_id"] = json!(format!("provider-{ROOT}"));
        event.metadata["chat_turn_adapter"] = json!("codex_bracket_v1");
        event.metadata["chat_turn_membership_partial"] = json!(false);
    }
    let mut answer = native(15, ROOT, AgentChatEventKind::Message);
    answer.metadata["provider_phase"] = json!("final_answer");
    answer.status = Some(AgentChatStatus::Succeeded);
    events.push(answer);
    let mut terminal = native(16, ROOT, AgentChatEventKind::Status);
    terminal.metadata["raw_type"] = json!("task_complete");
    terminal.metadata["chat_turn_membership_complete"] = json!(true);
    terminal.status = Some(AgentChatStatus::Succeeded);
    events.push(terminal);
    let (closed, state) = fixture.project(&mut events, &state, Some(&owners)).unwrap();
    assert!(state.roots[ROOT].gap_unresolvable);
    assert!(state.roots[ROOT].unresolved.is_empty());
    assert_eq!(closed.turns.len(), 1);
    let turn = &closed.turns[0];
    assert_eq!(turn.turn_key, before.turns[0].turn_key);
    assert_eq!(turn.order_key, before.turns[0].order_key);
    assert_eq!(turn.status, ChatTurnStatus::Incomplete);
    assert!(!turn.membership_complete);
    assert!(!turn.counts_complete);
    assert!(!turn.answer_recorded);
    assert!(turn.answer_item_keys.is_empty());
    assert_eq!(closed.items, before.items);
    assert_eq!(legacy_ids(&closed), ids(&events[4..]));
    assert!(closed
        .legacy_turns
        .iter()
        .all(|turn| turn.kind == ChatLegacyTurnKind::EarlierActivity));
    assert!(closed.validate().is_ok());
    let (refreshed, _) = fixture.project(&mut events, &state, Some(&owners)).unwrap();
    assert_eq!(refreshed, closed);

    // A fresh reader inherits the same admission's certificate, including its
    // overflow marker. Fully attributed current rows cannot erase that history.
    // Maximal selectors also overflow fallback display-order strings. Publish
    // bounded source-coordinate certificates through the index before either
    // admission reads them; preserve exact order proof for the original prefix.
    let capture_scope = Scope {
        can_activate: true,
        ..scope()
    };
    for event in &events {
        let mut captured = event.clone();
        captured.metadata["chat_provisional"] = json!(false);
        let order = presentation_order(&captured)
            .filter(|order| order.len() <= 512)
            .unwrap_or_else(|| {
                format!(
                    "source:{:020}:{:020}:{:020}",
                    captured.metadata["chat_source_start"].as_u64().unwrap(),
                    captured.metadata["chat_source_end"].as_u64().unwrap(),
                    captured.sequence.unwrap()
                )
            });
        assert!(
            fixture
                .head
                .turns
                .observe(&mut fixture.store, &capture_scope, &captured, &order, None,)
                .unwrap()
                .is_some(),
            "each fully proven row has an indexed owner"
        );
    }
    let shared_owners = Owners::from([(ROOT.to_owned(), owners)]);
    for (reader, bound) in [("fresh", false), ("current", true)] {
        let openings = state.openings();
        let mut current_events = events.clone();
        let (view, next_state, _) = build(
            Build {
                store: &mut fixture.store,
                head: &fixture.head,
                head_ref: "synthetic-head",
                admission: ADMISSION,
                running: true,
                openings: &openings,
                previous: &state,
                owners: Some(&shared_owners),
                canonical: None,
                bound,
            },
            &mut current_events,
        )
        .unwrap()
        .expect("either reader shares the existing admission's certified prefix");
        assert_eq!(
            view, closed,
            "{reader} reader cannot re-evaluate the same A"
        );
        assert!(next_state.roots[ROOT].gap_unresolvable, "{reader} reader");
        assert!(next_state.roots[ROOT].unresolved.is_empty());
    }

    // Retiring A permits re-evaluation only with a new admission and fresh
    // certification/owners. Keep the exact same fully proven source rows.
    let new_admission = "synthetic-new-admission";
    let fresh_state = State::default();
    let fresh_owners = Owners::new();
    let openings = fresh_state.openings();
    let (qualified, new_state, _) = build(
        Build {
            store: &mut fixture.store,
            head: &fixture.head,
            head_ref: "synthetic-head",
            admission: new_admission,
            running: true,
            openings: &openings,
            previous: &fresh_state,
            owners: Some(&fresh_owners),
            canonical: None,
            bound: false,
        },
        &mut events,
    )
    .unwrap()
    .expect("a new admission re-evaluates the now fully proven source");
    assert!(!new_state.roots[ROOT].gap_unresolvable);
    assert!(new_state.roots[ROOT].unresolved.is_empty());
    assert_eq!(qualified.turns.len(), 1);
    assert!(qualified.turns[0].membership_complete);
    assert_eq!(qualified.turns[0].status, ChatTurnStatus::Completed);
    assert!(qualified.turns[0].answer_recorded);
    assert!(!qualified.turns[0].answer_item_keys.is_empty());
    assert!(qualified.legacy_turns.is_empty());
    assert!(qualified.items.len() > before.items.len());
    assert!(qualified
        .items
        .iter()
        .all(|item| item.admission_id.as_deref() == Some(new_admission)));
    assert!(qualified.validate().is_ok());
}

#[test]
fn exact_certified_physical_members_recover_clipped_parser_context_with_stable_keys() {
    for rebased_display_ids in [false, true] {
        let mut fixture = Fixture::new();
        let mut events = prefix();
        let (before, state) = fixture
            .project(&mut events, &State::default(), None)
            .unwrap();
        let owners = fixture.remember(&events, &before, None);
        let mut clipped: Vec<_> = events.into_iter().map(unattributed).collect();
        for event in &mut clipped {
            if rebased_display_ids {
                event.id = format!("rebased-{}", event.id);
            }
            event.metadata["chat_turn_membership_partial"] = json!(true);
            event.metadata["chat_turn_after_closed"] = json!(true);
            assert!(event.metadata.get("request_root_id").is_none());
            assert!(event.metadata.get("chat_turn_adapter").is_none());
            assert_eq!(
                event.metadata["chat_source_epoch"],
                scope().source_epoch.unwrap()
            );
        }
        let (repaired, state) = fixture
            .project(&mut clipped, &state, Some(&owners))
            .unwrap();
        assert_running_prefix(&repaired);
        assert_eq!(state.openings(), BTreeSet::from([ROOT.to_string()]));
        assert_eq!(repaired.turns, before.turns);
        assert!(repaired.legacy_turns.is_empty());
        assert_eq!(repaired.items.len(), before.items.len());
        for (restored, certified) in repaired.items.iter().zip(&before.items) {
            assert_eq!(restored.item_key, certified.item_key);
            assert_eq!(restored.turn_key, certified.turn_key);
            assert_eq!(restored.order_key, certified.order_key);
            assert_eq!(restored.kind, certified.kind);
            assert_eq!(restored.evidence, certified.evidence);
        }
        for event in &clipped {
            assert_eq!(event.metadata["request_root_id"], ROOT);
            assert_eq!(
                event.metadata["provider_turn_id"],
                format!("provider-{ROOT}")
            );
            assert_eq!(event.metadata["chat_turn_adapter"], "codex_bracket_v1");
            assert_eq!(event.metadata["chat_turn_membership_partial"], false);
            assert_eq!(event.metadata["chat_turn_after_closed"], false);
        }
        assert_eq!(ordered_groups(&repaired), vec![ids(&clipped)]);
        assert!(repaired.validate().is_ok());
    }
}
