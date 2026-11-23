//! Bounded negative proof survives a recent window moving past an unknown row.
use super::*;

const MAX_IDS: usize = 8;
const MAX_BYTES: usize = 4096;

pub(super) fn physical(event: &AgentChatEvent) -> &str {
    event.metadata["chat_source_ref"]
        .as_str()
        .unwrap_or(&event.id)
}

/// A clipped parser context cannot revoke an exact member already certified
/// under this admission. Contradictory evidence is handled before this repair.
pub(super) fn restore(events: &mut [AgentChatEvent], head: &Head, previous: &State) {
    for event in events {
        if event.session_id != head.agent_id
            || event.metadata["chat_source_epoch"].as_str() != head.source_epoch.as_deref()
            || event.metadata["chat_turn_membership_contradiction"].is_string()
        {
            continue;
        }
        let known = previous.roots.iter().find(|(_, known)| {
            known.members.contains(&event.id) || known.members.contains(physical(event))
        });
        let Some((root, known)) = known else { continue };
        if request_root(event).is_some_and(|observed| observed != root)
            || native_turn(event)
                .zip(known.provider_turn.as_deref())
                .is_some_and(|(a, b)| a != b)
        {
            continue;
        }
        let Some(turn) = &known.provider_turn else {
            continue;
        };
        let adapter = match event.provider.as_str() {
            "codex" => "codex_bracket_v1",
            "claude" => "claude_parent_chain_v1",
            _ => continue,
        };
        event.metadata["request_root_id"] = json!(root);
        event.metadata["provider_turn_id"] = json!(turn);
        event.metadata["chat_turn_adapter"] = json!(adapter);
        event.metadata["chat_turn_membership_partial"] = json!(false);
        event.metadata["chat_turn_after_closed"] = json!(false);
    }
}

fn attributed(event: &AgentChatEvent, head: &Head, root: &str) -> bool {
    event.session_id == head.agent_id
        && request_root(event) == Some(root)
        && event.metadata["chat_source_epoch"].as_str() == head.source_epoch.as_deref()
        && adapter(event)
        && native_turn(event).is_some()
        && event.metadata["chat_turn_membership_partial"] != true
        && event.metadata["chat_turn_membership_contradiction"]
            .as_str()
            .is_none()
}

/// The frontier describes source position, never membership or read authority.
pub(super) fn source_order(event: &AgentChatEvent) -> Option<String> {
    presentation_order(event).filter(|order| order.len() <= 1024)
}

pub(super) fn after_boundary(event: &AgentChatEvent, boundary: Option<&str>) -> bool {
    boundary
        .is_some_and(|boundary| source_order(event).is_some_and(|order| order.as_str() >= boundary))
}

fn independent_boundary(
    event: &AgentChatEvent,
    head: &Head,
    root: &str,
    known: &Certified,
) -> bool {
    if event.session_id != head.agent_id
        || event.metadata["chat_source_epoch"].as_str() != head.source_epoch.as_deref()
        || !adapter(event)
        || event.metadata["chat_turn_membership_partial"] == true
        || event.metadata["chat_turn_membership_contradiction"].is_string()
    {
        return false;
    }
    let Some((turn, previous)) = native_turn(event).zip(known.provider_turn.as_deref()) else {
        return false;
    };
    if turn == previous {
        return request_root(event).is_none_or(|observed| observed == root)
            && event.kind == wardian_core::models::chat::AgentChatEventKind::Status
            && event.metadata["chat_turn_membership_complete"] == true
            && matches!(
                event.metadata["raw_type"].as_str(),
                Some("task_complete" | "turn_complete")
            );
    }
    request_root(event) != Some(root)
        && ((request_root(event).is_some()
            && event.role == Some(wardian_core::models::chat::AgentChatRole::User)
            && event.metadata["input_origin"] == "human_input"
            && event.metadata["input_purpose"] == "request")
            || matches!(
                event.metadata["raw_type"].as_str(),
                Some("task_started" | "turn_started")
            ))
}

/// Current source proof or exact membership at this immutable Head can resolve
/// a gap. Looking up a physical ID grants no detail, paging or ACK authority.
pub(super) fn unresolved(
    store: &mut Store,
    head: &Head,
    root: &str,
    previous: Option<&Certified>,
    closed_order: Option<&str>,
    events: &[AgentChatEvent],
) -> io::Result<(BTreeSet<String>, bool, Option<String>)> {
    let Some(previous) = previous else {
        return Ok((BTreeSet::new(), false, None));
    };
    let boundary = previous
        .boundary_order
        .iter()
        .chain(previous.closed_order.iter())
        .cloned()
        .chain(closed_order.map(str::to_owned))
        .chain(
            events
                .iter()
                .filter(|event| independent_boundary(event, head, root, previous))
                .filter_map(source_order)
                .filter(|order| {
                    previous
                        .last_source_order
                        .as_ref()
                        .is_none_or(|last| order > last)
                }),
        )
        .min();
    if previous.turn.membership_complete {
        return Ok((BTreeSet::new(), false, boundary));
    }
    if previous.gap_unresolvable {
        return Ok((BTreeSet::new(), true, boundary));
    }
    let mut remaining = BTreeSet::new();
    let mut cache = SeekCache::default();
    for id in &previous.unresolved {
        if events.iter().any(|event| {
            (physical(event) == id || event.id == *id)
                && event.session_id == head.agent_id
                && event.metadata["chat_source_epoch"].as_str() == head.source_epoch.as_deref()
                && (attributed(event, head, root) || after_boundary(event, boundary.as_deref()))
        }) {
            continue;
        }
        let mut proven = false;
        if let Some(reference) =
            store.get_reusing_nodes(&head.identities, &digest(id.as_bytes()), &mut cache)?
        {
            let row: Row = store.read(&reference)?;
            proven = row.agent_id == head.agent_id
                && row.conversation_id == head.conversation_id
                && (physical(&row.event) == id || row.event.id == *id)
                && row.event.metadata["chat_source_epoch"].as_str() == head.source_epoch.as_deref()
                && (attributed(&row.event, head, root)
                    || after_boundary(&row.event, boundary.as_deref()))
                && store
                    .get_reusing_nodes(&head.root, &row.key, &mut cache)?
                    .as_deref()
                    == Some(reference.as_str());
        }
        if !proven {
            remaining.insert(id.clone());
        }
    }
    let mut sticky = false;
    // A suffix remains uncertain even without another attributed row, or when
    // every certified member has rolled out. Source position scopes that debt;
    // an independent opening/closure is the only upper boundary.
    for event in events {
        let uncertain = (request_root(event).is_none()
            || !adapter(event)
            || event.metadata["chat_turn_membership_partial"] == true
            || event.metadata["chat_turn_after_closed"] == true)
            && event.metadata["generated"] != true;
        let later = source_order(event).as_ref().is_none_or(|order| {
            previous
                .last_source_order
                .as_ref()
                .is_none_or(|last| order > last)
        });
        if uncertain && later && !after_boundary(event, boundary.as_deref()) {
            add(&mut remaining, &mut sticky, event);
        }
    }
    Ok((remaining, sticky, boundary))
}

pub(super) fn add(ids: &mut BTreeSet<String>, sticky: &mut bool, event: &AgentChatEvent) {
    if *sticky {
        return;
    }
    let id = physical(event);
    if id.is_empty() || id.len() > 512 {
        ids.clear();
        *sticky = true;
        return;
    }
    ids.insert(id.to_owned());
    if ids.len() > MAX_IDS || ids.iter().map(String::len).sum::<usize>() > MAX_BYTES {
        ids.clear();
        *sticky = true;
    }
}

/// Overflow forgets selectors, never the unresolved state or the bound view.
pub(super) fn compact(state: &mut State, force: bool) -> bool {
    let count: usize = state
        .roots
        .values()
        .map(|known| known.unresolved.len())
        .sum();
    let bytes: usize = state
        .roots
        .values()
        .flat_map(|known| &known.unresolved)
        .map(String::len)
        .sum();
    if !force && count <= MAX_IDS && bytes <= MAX_BYTES {
        return false;
    }
    let mut changed = false;
    for known in state
        .roots
        .values_mut()
        .filter(|known| !known.unresolved.is_empty())
    {
        known.unresolved.clear();
        known.gap_unresolvable = true;
        changed = true;
    }
    changed
}
