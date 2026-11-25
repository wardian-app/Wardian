use std::collections::{BTreeMap, HashSet};

use wardian_core::conversations::ConversationTurnStatus;
use wardian_core::models::chat::{AgentChatEventKind as Kind, AgentChatRole, AgentChatStatus};
use wardian_core::models::chat_presentation::*;

use super::{digest, hash_json, io, AgentChatEvent, ConversationTurnRecord};

pub(super) struct Occurrence {
    pub(super) owner: String,
    pub(super) key: String,
    pub(super) sequence: u64,
    pub(super) covered_sequence: Option<u64>,
    pub(super) event: AgentChatEvent,
    pub(super) turn: ConversationTurnRecord,
    pub(super) body: Option<String>,
    pub(super) body_unavailable: bool,
}

pub(super) fn call_id(event: &AgentChatEvent) -> Option<&str> {
    let mut ids = ["call_id", "tool_call_id", "tool_use_id"]
        .iter()
        .filter_map(|key| event.metadata[*key].as_str())
        .filter(|id| !id.is_empty());
    let id = ids.next()?;
    ids.all(|other| other == id).then_some(id)
}

pub(super) fn step_status(event: &AgentChatEvent) -> ChatStepStatus {
    if event.exit_code.is_some_and(|code| code != 0) {
        return ChatStepStatus::Failed;
    }
    match event.status {
        Some(AgentChatStatus::Failed) => ChatStepStatus::Failed,
        Some(AgentChatStatus::Cancelled) => ChatStepStatus::Cancelled,
        Some(AgentChatStatus::ActionRequired) => ChatStepStatus::ActionRequired,
        Some(AgentChatStatus::Running | AgentChatStatus::Processing) => ChatStepStatus::Running,
        _ => ChatStepStatus::Completed,
    }
}

pub(super) fn item_key(owned: &str, event: &AgentChatEvent) -> String {
    format!(
        "archive-item:{}",
        digest(format!("{owned}:{}", event.id).as_bytes())
    )
}

pub(super) fn encode(value: ChatPresentationV1) -> io::Result<ChatPresentation> {
    let envelope = ChatPresentation {
        version: CHAT_PRESENTATION_VERSION,
        payload: serde_json::to_value(value).map_err(io::Error::other)?,
    };
    // Check the wire budget and structural contract before a page publishes.
    envelope.decode_v1().map_err(io::Error::other)?;
    Ok(envelope)
}

pub(super) fn envelope() -> ChatPresentationV1 {
    ChatPresentationV1 {
        turns: Vec::new(),
        items: Vec::new(),
        state_event_refs: Vec::new(),
        legacy_turns: Vec::new(),
        detail: None,
        preview_bridge: None,
    }
}

fn item(
    row: &Occurrence,
    key: &str,
    kind: ChatPresentedItemKind,
    step: Option<ChatPresentedStep>,
    refs: Vec<String>,
) -> ChatPresentedItem {
    ChatPresentedItem {
        item_key: item_key(&row.owner, &row.event),
        turn_key: key.into(),
        order_key: format!("{:020}:{}", row.sequence, row.key),
        kind,
        provenance: ChatItemProvenance::Canonical,
        evidence: ChatEvidenceIdentity::Source {
            source_key: format!("archive:{}", row.turn.conversation_id),
            source_epoch: row.owner.clone(),
            record_key: row.key.clone(),
        },
        evidence_version: row.key.clone(),
        admission_id: None,
        submission_status: None,
        event_refs: refs,
        preview: None,
        step,
    }
}

fn step_reference(
    call: &Occurrence,
    result: &Occurrence,
    base: &str,
    displayed: &str,
) -> io::Result<String> {
    let tail = |row: &Occurrence| row.key.split(':').skip(2).collect::<Vec<_>>().join(":");
    let body = result
        .body
        .clone()
        .unwrap_or(hash_json(&result.event.text)?);
    let base = base.rsplit(':').next().unwrap_or_default();
    let displayed = displayed.rsplit(':').next().unwrap_or_default();
    Ok(format!(
        "step:archive1:{}:{}:{}:{body}:{base}:{displayed}",
        call.owner,
        tail(call),
        tail(result)
    ))
}

pub(super) fn window(
    rows: &[Occurrence],
    base: &str,
    displayed: &str,
    events: &mut [AgentChatEvent],
) -> io::Result<Option<ChatPresentation>> {
    let mut turns = BTreeMap::<(&str, u64), Vec<&Occurrence>>::new();
    for row in rows {
        turns
            .entry((&row.owner, row.turn.seq_start))
            .or_default()
            .push(row);
    }
    let mut value = envelope();
    for ((owner, _), rows) in turns {
        let turn = &rows[0].turn;
        let closed = matches!(
            turn.status,
            ConversationTurnStatus::Responded | ConversationTurnStatus::Interrupted
        );
        if !matches!(rows[0].event.provider.as_str(), "codex" | "claude")
            || !matches!(
                turn.status,
                ConversationTurnStatus::Responded
                    | ConversationTurnStatus::InProgress
                    | ConversationTurnStatus::PendingResponse
                    | ConversationTurnStatus::Interrupted
            )
        {
            continue;
        }
        // A partial window or an unrepresented narrative row does not grant
        // turn-membership authority. The ordinary event renderer remains usable.
        let seqs = rows
            .iter()
            .flat_map(|row| std::iter::once(row.sequence).chain(row.covered_sequence))
            .collect::<HashSet<_>>();
        if seqs.len() as u64 != turn.counts.records
            || !seqs.contains(&turn.seq_start)
            || !seqs.contains(&turn.seq_end)
            || rows.iter().any(|row| row.turn != *turn)
        {
            continue;
        }
        let requests = rows
            .iter()
            .filter(|row| {
                row.sequence == turn.request.seq
                    && row.event.kind == Kind::Message
                    && row.event.role == Some(AgentChatRole::User)
            })
            .count();
        let answers = rows
            .iter()
            .filter(|row| {
                turn.assistant_result
                    .as_ref()
                    .is_some_and(|result| result.seq == row.sequence)
                    && row.event.kind == Kind::Message
                    && row.event.role == Some(AgentChatRole::Assistant)
            })
            .count();
        if requests != 1
            || answers != usize::from(turn.assistant_result.is_some())
            || (!closed && answers != 0)
        {
            continue;
        }
        let key = format!(
            "archive-turn:{}",
            digest(format!("{owner}:{}", turn.seq_start).as_bytes())
        );
        let mut items = Vec::new();
        let mut state = Vec::new();
        let mut paired = HashSet::new();
        let mut invalid = false;
        for row in &rows {
            if row.event.kind != Kind::ToolCall {
                continue;
            }
            let Some(id) = call_id(&row.event) else {
                invalid = true;
                break;
            };
            if rows
                .iter()
                .filter(|other| {
                    other.event.kind == Kind::ToolCall && call_id(&other.event) == Some(id)
                })
                .count()
                != 1
            {
                invalid = true;
                break;
            }
            let mut outputs = rows.iter().filter(|other| {
                other.event.kind == Kind::ToolResult
                    && call_id(&other.event) == Some(id)
                    && other.sequence > row.sequence
            });
            let output = outputs.next().copied();
            if outputs.next().is_some() {
                invalid = true;
                break;
            }
            if let Some(other) = output {
                paired.insert(other.event.id.as_str());
            }
            let (tool_name, category, description) =
                super::super::chat_turn_index::label(&row.event);
            let status = output.map_or(ChatStepStatus::Running, |row| step_status(&row.event));
            let pending = matches!(
                status,
                ChatStepStatus::Running | ChatStepStatus::ActionRequired
            );
            let available = output.filter(|r| {
                !pending && !r.body_unavailable && (r.body.is_some() || r.event.text.is_some())
            });
            let step = ChatPresentedStep {
                tool_name,
                category,
                description,
                status,
                summary: None,
                invocation_ref: Some(row.event.id.clone()),
                result_ref: output.map(|other| other.event.id.clone()),
                approval_ref: None,
                detail_availability: if available.is_some() {
                    ChatDetailAvailability::Available
                } else if pending {
                    ChatDetailAvailability::Pending
                } else {
                    ChatDetailAvailability::Unavailable
                },
                detail_ref: available
                    .map(|other| step_reference(row, other, base, displayed))
                    .transpose()?,
            };
            let refs = std::iter::once(row.event.id.clone())
                .chain(output.map(|other| other.event.id.clone()))
                .collect();
            items.push(item(
                row,
                &key,
                ChatPresentedItemKind::Step,
                Some(step),
                refs,
            ));
        }
        if invalid
            || rows.iter().any(|row| {
                row.event.kind == Kind::ToolResult && !paired.contains(row.event.id.as_str())
            })
        {
            continue;
        }
        for row in &rows {
            if matches!(row.event.kind, Kind::ToolCall | Kind::ToolResult) {
                continue;
            }
            let kind = if row.sequence == turn.request.seq
                && row.event.role == Some(AgentChatRole::User)
            {
                ChatPresentedItemKind::Prompt
            } else if turn
                .assistant_result
                .as_ref()
                .is_some_and(|result| result.seq == row.sequence)
                && row.event.role == Some(AgentChatRole::Assistant)
            {
                ChatPresentedItemKind::Answer
            } else if row.event.kind == Kind::Message
                && row.event.role == Some(AgentChatRole::Assistant)
            {
                ChatPresentedItemKind::Narration
            } else {
                state.push(ChatStateEventRef {
                    event_id: row.event.id.clone(),
                    turn_key: key.clone(),
                });
                continue;
            };
            items.push(item(row, &key, kind, None, vec![row.event.id.clone()]));
        }
        let step_count = items.iter().filter(|item| item.step.is_some()).count() as u64;
        let failed_step_count = items
            .iter()
            .filter(|item| {
                item.step
                    .as_ref()
                    .is_some_and(|step| step.status == ChatStepStatus::Failed)
            })
            .count() as u64;
        if step_count != turn.counts.tool_calls {
            continue;
        }
        let status = match turn.status {
            ConversationTurnStatus::Responded => ChatTurnStatus::Completed,
            ConversationTurnStatus::Interrupted => ChatTurnStatus::Incomplete,
            _ => ChatTurnStatus::Running,
        };
        value.turns.push(ChatPresentedTurn {
            turn_key: key.clone(),
            order_key: format!("{:020}", turn.seq_start),
            membership_complete: closed,
            prefix_attributed: !closed,
            status,
            prompt_item_keys: items
                .iter()
                .filter(|item| item.kind == ChatPresentedItemKind::Prompt)
                .map(|item| item.item_key.clone())
                .collect(),
            answer_item_keys: items
                .iter()
                .filter(|item| item.kind == ChatPresentedItemKind::Answer)
                .map(|item| item.item_key.clone())
                .collect(),
            references_complete: closed,
            duration_ms: None,
            reported_changes: None,
            step_count,
            answer_recorded: answers > 0,
            failed_step_count,
            counts_complete: closed,
        });
        for event in events
            .iter_mut()
            .filter(|event| rows.iter().any(|row| row.event.id == event.id))
        {
            event.metadata["chat_turn_key"] = serde_json::json!(key);
        }
        value.items.extend(items);
        value.state_event_refs.extend(state);
    }
    if value.items.is_empty() {
        Ok(None)
    } else {
        encode(value).map(Some)
    }
}
