//! Pure, bounded presentation of a source-attributed prefix. The caller owns
//! the exact Head/admission fence and certifies any openings outside this slice.
use super::*;

struct PreviewTurn<'a> {
    turn: Turn,
    provider: &'a str,
    provider_turn: &'a str,
}

struct PreviewItem {
    item: Item,
    event_refs: Vec<String>,
    content_id: String,
}

/// Present only this admitted slice, without reading history or granting any
/// canonical detail authority. Unknown middle rows reject the entire prefix.
/// `known_openings` must come from this same source/admission or canonical index.
/// `owners` must be resolved for these exact display rows at the caller's Head;
/// merged components cannot fall back to their currently displayed physical row.
pub(in super::super) fn preview(
    scope: &Scope<'_>,
    events: &[AgentChatEvent],
    admission: &str,
    known_openings: &BTreeSet<String>,
    running: bool,
    owners: &PreviewOwners,
) -> io::Result<Option<ChatPresentationV1>> {
    let Some((source_key, source_epoch)) = scope.source_key.zip(scope.source_epoch) else {
        return Ok(None);
    };
    if owners
        .admission
        .as_deref()
        .is_some_and(|known| known != admission)
        || owners
            .source
            .as_ref()
            .is_some_and(|(key, epoch)| key != source_key || epoch != source_epoch)
    {
        return Ok(None);
    }
    if events.is_empty()
        || events.len() > CHAT_PRESENTATION_MAX_ITEMS
        || ![
            scope.agent_id,
            scope.conversation_id,
            source_key,
            source_epoch,
            admission,
        ]
        .into_iter()
        .all(bounded_id)
    {
        return Ok(None);
    }
    let mut ordered = Vec::with_capacity(events.len());
    let mut by_id = BTreeMap::new();
    for event in events {
        let Some(root) = string(event, "request_root_id").filter(|root| bounded_id(root)) else {
            return Ok(None);
        };
        let Some(provider_turn) = string(event, "provider_turn_id").filter(|id| bounded_id(id))
        else {
            return Ok(None);
        };
        if event.session_id != scope.agent_id
            || !bounded_id(&event.id)
            || !proven_adapter(event)
            || string(event, "chat_source_epoch") != Some(source_epoch)
            || !string(event, "chat_source_ref").is_some_and(bounded_id)
            || !event.metadata["chat_source_start"]
                .as_u64()
                .zip(event.metadata["chat_source_end"].as_u64())
                .is_some_and(|(start, end)| start < end)
            || event.metadata["chat_turn_membership_partial"] == true
            || event.metadata["chat_turn_membership_contradiction"] == true
            || event.metadata["chat_legacy_identity_ineligible"] == true
            || string(event, "conversation_archive_id")
                .is_some_and(|id| id != scope.conversation_id)
            || by_id.insert(event.id.as_str(), event).is_some()
        {
            return Ok(None);
        }
        let Some(mut kind) = item_kind(event) else {
            return Ok(None);
        };
        let hint = owners.rows.get(&event.id);
        if event.metadata.get("chat_display_member_ids").is_some() && hint.is_none() {
            return Ok(None);
        }
        if let Some(hint) = hint {
            if preview_observation(event)?.as_deref() != Some(hint.observation.as_str())
                || !compatible_component_kinds(&hint.kind, &kind)
            {
                return Ok(None);
            }
            kind = hint.kind.clone();
        }
        let Some(order) = hint
            .map(|hint| hint.source_order.clone())
            .or_else(|| super::super::chat_read::presentation_order(event))
            .filter(|order| bounded_id(order))
        else {
            return Ok(None);
        };
        ordered.push((order, root, provider_turn, kind, event, hint));
    }
    ordered.sort_by(|left, right| left.0.cmp(&right.0));
    let mut turns = BTreeMap::<String, PreviewTurn<'_>>::new();
    let mut items = BTreeMap::<String, PreviewItem>::new();
    let mut physical_members = BTreeMap::<&str, Membership>::new();
    for (order, root, provider_turn, kind, event, hint) in ordered {
        let Some(identity) = identity(scope, event, root, &kind, None)? else {
            return Ok(None);
        };
        if owners
            .scope
            .as_ref()
            .is_some_and(|owner| owner != &identity.owner)
        {
            return Ok(None);
        }
        let membership = hint
            .map(|hint| hint.item.membership.clone())
            .unwrap_or_else(|| Membership {
                turn_key: identity.turn_key.clone(),
                item_key: identity.item_key.clone(),
            });
        if membership.turn_key != identity.turn_key {
            return Ok(None);
        }
        if physical_members
            .insert(identity.physical, membership.clone())
            .is_some_and(|known| known != membership)
        {
            return Ok(None);
        }
        let entry = turns
            .entry(identity.turn_key.clone())
            .or_insert_with(|| PreviewTurn {
                turn: Turn {
                    active: known_openings.contains(root),
                    order_key: owners.turn_orders.get(&identity.turn_key).cloned(),
                    status: if running {
                        ChatTurnStatus::Running
                    } else {
                        ChatTurnStatus::Incomplete
                    },
                    ..Turn::default()
                },
                provider: &event.provider,
                provider_turn,
            });
        if entry.provider != event.provider || entry.provider_turn != provider_turn {
            return Ok(None);
        }
        entry.turn.active |= request_prompt(event);
        let order_key = hint
            .map(|hint| hint.item.order_key.clone())
            .unwrap_or_else(|| format!("{order}:{}", membership.item_key));
        if request_prompt(event) && entry.turn.order_key.is_none() {
            entry.turn.order_key = Some(order_key.clone());
        }
        if let Some(status) = closed_status(event) {
            entry.turn.membership_closed = true;
            entry.turn.status = status;
        }
        let presented = items
            .entry(membership.item_key.clone())
            .or_insert_with(|| PreviewItem {
                item: Item {
                    membership,
                    order_key,
                    kind: kind.clone(),
                    evidence: hint
                        .map(|hint| hint.item.evidence.clone())
                        .unwrap_or_else(|| ChatEvidenceIdentity::Source {
                            source_key: source_key.into(),
                            source_epoch: source_epoch.into(),
                            record_key: identity.physical.into(),
                        }),
                    step: None,
                    state_only: false,
                    legacy_only: false,
                },
                event_refs: Vec::new(),
                content_id: event.id.clone(),
            });
        if !compatible_component_kinds(&presented.item.kind, &kind) {
            return Ok(None);
        }
        presented.item.state_only |=
            state_only(event) || hint.is_some_and(|hint| hint.item.state_only);
        if presented.item.state_only {
            presented.item.kind = ChatPresentedItemKind::Narration;
        } else if kind == ChatPresentedItemKind::Step {
            update_step(&mut presented.item, event, &order);
        } else {
            presented.item.kind = kind;
        }
        presented.content_id.clone_from(&event.id);
        presented.event_refs.push(event.id.clone());
    }
    if turns
        .values()
        .any(|entry| !entry.turn.active || entry.turn.order_key.is_none())
    {
        return Ok(None);
    }
    let mut state_event_refs = Vec::new();
    let mut presented_items = Vec::new();
    for (_, mut entry) in items {
        let turn = &mut turns
            .get_mut(&entry.item.membership.turn_key)
            .expect("owned turn")
            .turn;
        if entry.item.state_only {
            state_event_refs.extend(entry.event_refs.into_iter().map(|event_id| {
                ChatStateEventRef {
                    event_id,
                    turn_key: entry.item.membership.turn_key.clone(),
                }
            }));
            continue;
        }
        if !turn.membership_closed && entry.item.kind == ChatPresentedItemKind::Answer {
            entry.item.kind = ChatPresentedItemKind::Narration;
        }
        turn.totals.add(&entry.item)?;
        let mut step = entry
            .item
            .step
            .as_ref()
            .map(|step| present_step(step, turn));
        if let Some(step) = &mut step {
            step.invocation_ref = None;
            step.result_ref = None;
            step.approval_ref = None;
            step.detail_ref = None;
            step.detail_availability = ChatDetailAvailability::Pending;
            step.summary = None;
        }
        let content_id = entry
            .item
            .step
            .as_ref()
            .and_then(|step| {
                step.result
                    .as_ref()
                    .or(step.invocation.as_ref())
                    .or(step.approval.as_ref())
            })
            .map_or(entry.content_id.as_str(), |record| record.event_id.as_str());
        let content = by_id.get(content_id).expect("admitted content");
        let text = bounded_text(content.text.as_deref().unwrap_or_default());
        presented_items.push(ChatPresentedItem {
            evidence_version: key(&("preview-evidence-v1", admission, &entry.item, &text))?,
            item_key: entry.item.membership.item_key,
            turn_key: entry.item.membership.turn_key,
            order_key: entry.item.order_key,
            kind: entry.item.kind,
            provenance: ChatItemProvenance::Provisional,
            evidence: entry.item.evidence,
            admission_id: Some(admission.into()),
            submission_status: None,
            event_refs: entry.event_refs,
            preview: Some(ChatItemPreview {
                text,
                attachment_refs: Vec::new(),
            }),
            step,
        });
    }
    presented_items.sort_by(|left, right| {
        (&left.order_key, &left.item_key).cmp(&(&right.order_key, &right.item_key))
    });
    let mut presented_turns = Vec::new();
    for (turn_key, entry) in turns {
        let local = || {
            presented_items
                .iter()
                .filter(|item| item.turn_key == turn_key)
        };
        presented_turns.push(ChatPresentedTurn {
            order_key: entry.turn.order_key.expect("admitted order"),
            prefix_attributed: true,
            membership_complete: entry.turn.membership_closed,
            status: entry.turn.status,
            prompt_item_keys: local()
                .filter(|item| item.kind == ChatPresentedItemKind::Prompt)
                .map(|item| item.item_key.clone())
                .collect(),
            answer_item_keys: local()
                .filter(|item| item.kind == ChatPresentedItemKind::Answer)
                .map(|item| item.item_key.clone())
                .collect(),
            // Closure proves membership, not that a bounded suffix loaded every
            // earlier item. Neither preview counts nor references are totals.
            references_complete: false,
            duration_ms: None,
            reported_changes: None,
            step_count: entry.turn.totals.steps,
            answer_recorded: entry.turn.totals.answers > 0,
            failed_step_count: entry.turn.totals.failed,
            counts_complete: false,
            turn_key,
        });
    }
    presented_turns.sort_by(|left, right| {
        (&left.order_key, &left.turn_key).cmp(&(&right.order_key, &right.turn_key))
    });
    let page = ChatPresentationV1 {
        turns: presented_turns,
        items: presented_items,
        state_event_refs,
        legacy_turns: Vec::new(),
        detail: None,
        preview_bridge: None,
    };
    if page.validate().is_err()
        || serde_json::to_vec(&page).map_err(io::Error::other)?.len() > CHAT_PRESENTATION_MAX_BYTES
    {
        return Ok(None);
    }
    Ok(Some(page))
}

fn bounded_text(text: &str) -> String {
    let mut end = text.len().min(1024);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].into()
}

/// An adapter's complete flag on a source terminal is the closure proof. Tool
/// success, final-looking prose and process idleness do not close a request.
fn closed_status(event: &AgentChatEvent) -> Option<ChatTurnStatus> {
    if event.metadata["chat_turn_membership_complete"] != true {
        return None;
    }
    let terminal = match event.provider.as_str() {
        "codex" => {
            event.kind == AgentChatEventKind::Status
                && string(event, "raw_type") == Some("task_complete")
        }
        "claude" => {
            (event.kind == AgentChatEventKind::Status
                && matches!(string(event, "raw_type"), Some("result" | "assistant")))
                || (event.kind == AgentChatEventKind::Message
                    && event.role == Some(AgentChatRole::Assistant)
                    && string(event, "provider_phase") == Some("final_answer"))
        }
        _ => false,
    };
    terminal
        .then(|| turn_status(event))
        .flatten()
        .filter(|status| {
            matches!(
                status,
                ChatTurnStatus::Completed | ChatTurnStatus::Failed | ChatTurnStatus::Cancelled
            )
        })
}

#[cfg(test)]
#[path = "chat_turn_preview_tests.rs"]
mod tests;
