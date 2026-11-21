//! Per-turn selection inside one bound source snapshot. Legacy rows never
//! acquire canonical membership by being adjacent to a qualified turn.
use super::*;
use wardian_core::models::chat_presentation::*;

#[path = "chat_preview_gaps.rs"]
mod gaps;

pub(super) type Owners = BTreeMap<String, super::super::chat_turn_index::PreviewOwners>;
type Projection = (ChatPresentationV1, State, Owners);

#[derive(Clone, Default, Serialize)]
pub(super) struct State {
    roots: BTreeMap<String, Certified>,
}

impl State {
    /// Count retained root certificates and their backing/gap identities.
    pub(super) fn record_count(&self) -> usize {
        self.roots
            .values()
            .map(|root| 1 + root.members.len() + root.unresolved.len())
            .sum()
    }

    pub(super) fn openings(&self) -> BTreeSet<String> {
        self.roots.keys().cloned().collect()
    }

    pub(super) fn compact_gap_ids(&mut self) -> bool {
        gaps::compact(self, true)
    }
}

#[derive(Clone, Serialize)]
struct Certified {
    turn: ChatPresentedTurn,
    provider_turn: Option<String>,
    closed_order: Option<String>,
    members: BTreeSet<String>,
    last_source_order: Option<String>,
    boundary_order: Option<String>,
    terminal_seen: bool,
    unresolved: BTreeSet<String>,
    gap_unresolvable: bool,
}

pub(super) struct Build<'a> {
    pub(super) store: &'a mut Store,
    pub(super) head: &'a Head,
    pub(super) head_ref: &'a str,
    pub(super) admission: &'a str,
    pub(super) running: bool,
    pub(super) openings: &'a BTreeSet<String>,
    pub(super) previous: &'a State,
    pub(super) owners: Option<&'a Owners>,
    pub(super) canonical: Option<&'a ChatPresentationV1>,
    pub(super) bound: bool,
}

fn request_root(event: &AgentChatEvent) -> Option<&str> {
    event.metadata["request_root_id"]
        .as_str()
        .filter(|value| !value.is_empty() && value.len() <= 512)
}

fn native_turn(event: &AgentChatEvent) -> Option<&str> {
    event.metadata["provider_turn_id"]
        .as_str()
        .or_else(|| {
            (event.provider == "codex"
                && matches!(
                    event.metadata["raw_type"].as_str(),
                    Some("task_started" | "task_complete")
                ))
            .then_some(event.turn_id.as_deref())
            .flatten()
        })
        .filter(|value| !value.is_empty() && value.len() <= 512)
}

fn adapter(event: &AgentChatEvent) -> bool {
    matches!(
        (
            event.provider.as_str(),
            event.metadata["chat_turn_adapter"].as_str()
        ),
        ("codex", Some("codex_bracket_v1")) | ("claude", Some("claude_parent_chain_v1"))
    )
}

fn terminated(events: &[AgentChatEvent], known: &Certified) -> bool {
    known.terminal_seen
        || events.iter().any(|event| {
            native_turn(event) == known.provider_turn.as_deref()
                && known.provider_turn.is_some()
                && event.kind == wardian_core::models::chat::AgentChatEventKind::Status
                && matches!(
                    event.status,
                    Some(
                        wardian_core::models::chat::AgentChatStatus::Succeeded
                            | wardian_core::models::chat::AgentChatStatus::Failed
                            | wardian_core::models::chat::AgentChatStatus::Cancelled
                    )
                )
        })
}

/// Closure receipts and native display rows can use different normalization
/// sequences. Their physical range and full backing identity must still match.
fn closing_record(event: &AgentChatEvent, order: &str) -> bool {
    let Some(order) = order.strip_prefix("source:") else {
        return false;
    };
    let mut fields = order.splitn(4, ':');
    let start = fields.next().and_then(|value| value.parse::<u64>().ok());
    let end = fields.next().and_then(|value| value.parse::<u64>().ok());
    let sequence = fields.next().and_then(|value| value.parse::<u64>().ok());
    sequence.is_some()
        && start.zip(end).is_some_and(|(start, end)| start < end)
        && event.metadata["chat_source_start"].as_u64() == start
        && event.metadata["chat_source_end"].as_u64() == end
        && fields
            .next()
            .is_some_and(|physical| event.metadata["chat_source_ref"].as_str() == Some(physical))
}

/// Own bracket rows are state, not extra work items. A terminal row can reuse a
/// closed turn only when it is the exact published closure record.
fn canonical_lifecycle(
    event: &AgentChatEvent,
    root: &str,
    head: &Head,
    provider_roots: &BTreeMap<String, BTreeSet<String>>,
    closed_order: Option<&str>,
) -> bool {
    event.provider == "codex"
        && event.session_id == head.agent_id
        && event.metadata["chat_source_epoch"].as_str() == head.source_epoch.as_deref()
        && head.source_epoch.is_some()
        && event.metadata["chat_source_ref"]
            .as_str()
            .is_some_and(|reference| !reference.is_empty())
        && event.metadata["chat_source_start"]
            .as_u64()
            .zip(event.metadata["chat_source_end"].as_u64())
            .is_some_and(|(start, end)| start < end)
        && adapter(event)
        && request_root(event) == Some(root)
        && native_turn(event)
            .and_then(|turn| provider_roots.get(turn))
            .is_some_and(|roots| roots.len() == 1 && roots.contains(root))
        && match event.metadata["raw_type"].as_str() {
            Some("task_started") => matches!(
                event.kind,
                wardian_core::models::chat::AgentChatEventKind::ToolCall
                    | wardian_core::models::chat::AgentChatEventKind::Status
            ),
            Some("task_complete") => {
                event.kind == wardian_core::models::chat::AgentChatEventKind::Status
                    && closed_order.is_some_and(|order| closing_record(event, order))
            }
            _ => false,
        }
}

fn empty_value() -> ChatPresentationV1 {
    ChatPresentationV1 {
        turns: Vec::new(),
        items: Vec::new(),
        state_event_refs: Vec::new(),
        legacy_turns: Vec::new(),
        detail: None,
        preview_bridge: None,
    }
}

/// Resolve exact lifecycle IDs, select each turn once, and keep unqualified
/// activity in this same producer envelope. Returns no initial qualification
/// when the entire snapshot is legacy.
pub(super) fn build(
    build: Build<'_>,
    events: &mut [AgentChatEvent],
) -> io::Result<Option<Projection>> {
    let scope = super::super::chat_turn_index::Scope {
        agent_id: &build.head.agent_id,
        conversation_id: &build.head.conversation_id,
        source_key: build.head.source_key.as_deref(),
        source_epoch: build.head.source_epoch.as_deref(),
        can_activate: false,
    };
    gaps::restore(events, build.head, build.previous);
    let mut provider_roots = BTreeMap::<String, BTreeSet<String>>::new();
    for (root, known) in &build.previous.roots {
        if let Some(turn) = &known.provider_turn {
            provider_roots
                .entry(turn.clone())
                .or_default()
                .insert(root.clone());
        }
    }
    for event in events
        .iter()
        .filter(|event| adapter(event) && event.metadata["chat_turn_membership_partial"] != true)
    {
        if let Some((root, turn)) = request_root(event).zip(native_turn(event)) {
            provider_roots
                .entry(turn.to_owned())
                .or_default()
                .insert(root.to_owned());
        }
    }
    // Only an exact same-source provider ID can own a pre-prompt lifecycle row.
    for event in events.iter_mut() {
        if event.provider == "codex"
            && event.metadata["raw_type"] == "task_started"
            && adapter(event)
            && request_root(event).is_none()
        {
            let linked = native_turn(event)
                .and_then(|turn| provider_roots.get(turn))
                .filter(|roots| roots.len() == 1)
                .and_then(|roots| roots.first())
                .cloned();
            if let Some(root) = linked {
                event.metadata["request_root_id"] = json!(root);
            }
        }
    }
    let roots: BTreeSet<_> = events
        .iter()
        .filter_map(|event| request_root(event).map(str::to_owned))
        .collect();
    // One snapshot can contain many unqualified turns. Reuse immutable seek
    // nodes across their lookups instead of spending the read budget on repeats.
    let mut seeks = SeekCache::default();
    let mut closed =
        build
            .head
            .turns
            .closed_roots_reusing_nodes(build.store, &scope, &roots, &mut seeks)?;
    for (root, known) in &build.previous.roots {
        if let Some(order) = &known.closed_order {
            closed.entry(root.clone()).or_insert_with(|| order.clone());
        }
    }
    for event in events.iter() {
        let Some(kind) = event.metadata["chat_turn_membership_contradiction"].as_str() else {
            continue;
        };
        if !matches!(
            kind,
            "codex_provider_turn_conflict"
                | "codex_request_root_conflict"
                | "claude_uuid_ancestry_conflict"
        ) {
            continue;
        }
        let Some(root) = event.metadata["chat_turn_membership_contradicted_root"].as_str() else {
            continue;
        };
        if let Some(known) = build
            .previous
            .roots
            .get(root)
            .filter(|known| !known.turn.membership_complete)
        {
            eprintln!(
                "Chat preview source proof retracted: turn_key={} kind={kind}",
                known.turn.turn_key
            );
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "certified membership contradiction",
            ));
        }
    }
    // Reuse this producer snapshot's prepared payload. Only missing closed
    // metadata needs a bulk lookup; running prefixes use their owner proof.
    let canonical = if let Some(value) = build.canonical {
        Some(value.clone())
    } else {
        let closed_events: Vec<_> = events
            .iter()
            .filter(|event| {
                request_root(event).is_some_and(|root| closed.contains_key(root))
                    && event.metadata["chat_provisional"] != true
                    && event.metadata["chat_identity_resolution"] != "unresolved"
            })
            .cloned()
            .collect();
        if closed_events.is_empty() {
            None
        } else {
            build
                .head
                .turns
                .continuation_page(build.store, &closed_events)?
        }
    };
    let mut groups = BTreeMap::<String, Vec<usize>>::new();
    let mut selected = Vec::with_capacity(events.len());
    for (index, event) in events.iter().enumerate() {
        let owner = request_root(event)
            .filter(|root| {
                event.metadata["chat_turn_after_closed"] != true
                    && closed.get(*root).is_none_or(|order| {
                        !super::super::chat_turn_index::Index::after_closed_source_order(
                            event, order,
                        )
                    })
            })
            .map(str::to_owned);
        if let Some(root) = &owner {
            groups.entry(root.clone()).or_default().push(index);
        }
        selected.push(owner);
    }
    let mut value = empty_value();
    let mut state = State::default();
    let mut pending_gaps = BTreeMap::new();
    for (root, known) in &build.previous.roots {
        pending_gaps.insert(
            root.clone(),
            gaps::unresolved(build.store, build.head, root, Some(known), events)?,
        );
    }
    let mut remembered_owners = Owners::new();
    let latest = events.iter().rev().find_map(native_turn).map(str::to_owned);
    let mut owned = BTreeSet::new();
    for (root, indices) in groups {
        let start = *indices.first().unwrap();
        let known = build.previous.roots.get(&root);
        let end = events
            .iter()
            .enumerate()
            .rev()
            .find(|(index, event)| {
                *index >= start
                    && (selected[*index].as_deref() == Some(root.as_str())
                        || known
                            .and_then(|known| known.provider_turn.as_deref())
                            .is_some_and(|turn| native_turn(event) == Some(turn)))
            })
            .map_or(*indices.last().unwrap(), |(index, _)| index);
        let (mut unresolved, mut gap_unresolvable, boundary_order) =
            pending_gaps.get(&root).cloned().unwrap_or_default();
        // An unlinked row between members taints the entire candidate turn.
        let unproven = |index: usize| {
            (selected[index].is_none()
                || !adapter(&events[index])
                || events[index].metadata["chat_turn_membership_partial"] == true)
                && events[index].metadata["generated"] != true
                && !gaps::after_boundary(&events[index], boundary_order.as_deref())
        };
        if known.is_some_and(|known| !known.turn.membership_complete) {
            for event in (start..=end)
                .filter(|index| unproven(*index))
                .map(|index| &events[index])
            {
                gaps::add(&mut unresolved, &mut gap_unresolvable, event);
            }
        }
        let gap = gap_unresolvable || !unresolved.is_empty() || (start..=end).any(unproven);
        pending_gaps.insert(
            root.clone(),
            (unresolved.clone(), gap_unresolvable, boundary_order.clone()),
        );
        let mut group: Vec<_> = indices.iter().map(|index| events[*index].clone()).collect();
        let partial = group
            .iter()
            .any(|event| event.metadata["chat_turn_membership_partial"] == true);
        let mut projected = None;
        let previous_owners = build.owners.and_then(|owners| owners.get(&root));
        let mut resolved_owners = None;
        let terminal_seen = build
            .previous
            .roots
            .get(&root)
            .is_some_and(|known| terminated(events, known));
        if !gap && !partial {
            let lifecycle: BTreeSet<_> = group
                .iter()
                .filter(|event| {
                    canonical_lifecycle(
                        event,
                        &root,
                        build.head,
                        &provider_roots,
                        closed.get(&root).map(String::as_str),
                    )
                })
                .map(|event| event.id.as_str())
                .collect();
            let canonical_group = group.iter().all(|event| {
                lifecycle.contains(event.id.as_str())
                    || (event.metadata["chat_provisional"] != true
                        && event.metadata["chat_identity_resolution"] != "unresolved")
            });
            if canonical_group {
                let canonical_events: Vec<_> = group
                    .iter()
                    .filter(|event| !lifecycle.contains(event.id.as_str()))
                    .cloned()
                    .collect();
                projected = canonical
                    .clone()
                    .and_then(|value| {
                        turn_read::window(value, &canonical_events, Some(build.head_ref))
                    })
                    .filter(|value| value.turns.len() == 1 && value.turns[0].membership_complete)
                    .filter(|value| value.legacy_turns.is_empty());
                if let Some(value) = &mut projected {
                    for event in group
                        .iter()
                        .filter(|event| lifecycle.contains(event.id.as_str()))
                    {
                        if !value
                            .state_event_refs
                            .iter()
                            .any(|reference| reference.event_id == event.id)
                        {
                            value.state_event_refs.push(ChatStateEventRef {
                                event_id: event.id.clone(),
                                turn_key: value.turns[0].turn_key.clone(),
                            });
                        }
                    }
                }
            }
            if projected.is_none() {
                if let Some(owners) = build.head.turns.preview_owners_reusing_nodes(
                    build.store,
                    &scope,
                    &group,
                    previous_owners,
                    &mut seeks,
                )? {
                    let active = build.running
                        && !terminal_seen
                        && group.iter().find_map(native_turn).is_none_or(|turn| {
                            latest.as_deref().is_none_or(|latest| turn == latest)
                        });
                    let openings = build
                        .openings
                        .union(owners.proven_openings())
                        .cloned()
                        .collect();
                    projected = super::super::chat_turn_index::preview(
                        &scope,
                        &group,
                        build.admission,
                        &openings,
                        active,
                        &owners,
                    )?;
                    resolved_owners = Some(owners);
                }
            }
        }
        if projected.is_none() {
            if let Some(known) = build.previous.roots.get(&root) {
                // Preserve the certified prefix while displaying every new,
                // unproven row as legacy activity in this same source snapshot.
                group.retain(|event| {
                    known.members.contains(&event.id)
                        || event.metadata["chat_source_ref"]
                            .as_str()
                            .is_some_and(|id| known.members.contains(id))
                });
                if let Some(owners) = build.head.turns.preview_owners_reusing_nodes(
                    build.store,
                    &scope,
                    &group,
                    previous_owners,
                    &mut seeks,
                )? {
                    let openings = build
                        .openings
                        .union(owners.proven_openings())
                        .cloned()
                        .collect();
                    projected = super::super::chat_turn_index::preview(
                        &scope,
                        &group,
                        build.admission,
                        &openings,
                        build.running && !known.turn.membership_complete && !terminal_seen,
                        &owners,
                    )?;
                    resolved_owners = Some(owners);
                }
            }
        }
        if let Some(mut projected) = projected {
            if let Some(previous) = previous_owners {
                previous.preserve_item_orders(&mut projected);
                projected.items.sort_by(|left, right| {
                    left.order_key
                        .cmp(&right.order_key)
                        .then(left.item_key.cmp(&right.item_key))
                });
            }
            // Closed rows keep admitted display order too. Their durable index
            // remains unchanged; this bounded certificate belongs to the view.
            if let Some(remembered) = resolved_owners
                .as_ref()
                .or(previous_owners)
                .map(|owners| owners.remember(&scope, build.admission, &projected))
                .transpose()?
                .flatten()
            {
                remembered_owners.insert(root.clone(), remembered);
            }
            if let Some(turn) = projected.turns.first() {
                let closed_order = turn
                    .membership_complete
                    .then(|| {
                        group
                            .iter()
                            .filter(|event| event.metadata["chat_turn_membership_complete"] == true)
                            .filter_map(presentation_order)
                            .max()
                    })
                    .flatten()
                    .or_else(|| closed.get(&root).cloned());
                state.roots.insert(
                    root.clone(),
                    Certified {
                        turn: turn.clone(),
                        provider_turn: group.iter().find_map(native_turn).map(str::to_owned),
                        closed_order,
                        last_source_order: group
                            .iter()
                            .filter_map(gaps::source_order)
                            .max()
                            .max(known.and_then(|known| known.last_source_order.clone())),
                        boundary_order,
                        members: group
                            .iter()
                            .flat_map(|event| {
                                std::iter::once(event.id.clone()).chain(
                                    event.metadata["chat_source_ref"]
                                        .as_str()
                                        .map(str::to_owned),
                                )
                            })
                            .collect(),
                        terminal_seen: terminal_seen || turn.membership_complete,
                        unresolved,
                        gap_unresolvable,
                    },
                );
            }
            owned.extend(group.iter().map(|event| event.id.clone()));
            value.turns.append(&mut projected.turns);
            value.items.append(&mut projected.items);
            value
                .state_event_refs
                .append(&mut projected.state_event_refs);
        }
    }
    // A certificate with no rows in this bounded window does not become a
    // legacy turn. Later unproven activity carries no claim to its membership.
    for (root, known) in &build.previous.roots {
        if state.roots.contains_key(root) || known.turn.membership_complete {
            continue;
        }
        let mut known = known.clone();
        if let Some((unresolved, sticky, boundary)) = pending_gaps.remove(root) {
            known.unresolved = unresolved;
            known.gap_unresolvable = sticky;
            known.boundary_order = boundary;
        }
        known.terminal_seen = terminated(events, &known);
        known.turn.prompt_item_keys.clear();
        known.turn.answer_item_keys.clear();
        known.turn.references_complete = false;
        known.turn.counts_complete = false;
        known.turn.answer_recorded = false;
        if known.terminal_seen
            || !build.running
            || known
                .provider_turn
                .as_deref()
                .zip(latest.as_deref())
                .is_some_and(|(old, latest)| old != latest)
        {
            known.turn.status = ChatTurnStatus::Incomplete;
        }
        value.turns.push(known.turn.clone());
        state.roots.insert(root.clone(), known);
    }
    // The first certified prefix may itself end in unknown activity. Retain
    // that negative evidence now, without assigning the suffix to the turn.
    for (root, known) in &mut state.roots {
        if !build.previous.roots.contains_key(root) {
            let (unresolved, sticky, boundary) =
                gaps::unresolved(build.store, build.head, root, Some(known), events)?;
            known.unresolved = unresolved;
            known.gap_unresolvable = sticky;
            known.boundary_order = boundary;
        }
    }
    if let Some(previous) = build.owners {
        for (root, known) in &state.roots {
            if !known.turn.membership_complete && !remembered_owners.contains_key(root) {
                if let Some(owners) = previous.get(root) {
                    remembered_owners.insert(root.clone(), owners.clone());
                }
            }
        }
    }
    let mut legacy = Vec::new();
    let orders: BTreeMap<_, _> = events
        .iter()
        .filter_map(|event| presentation_order(event).map(|order| (event.id.clone(), order)))
        .collect();
    let flush = |rows: &mut Vec<String>, value: &mut ChatPresentationV1| {
        if rows.is_empty() {
            return;
        }
        let first = &rows[0];
        let order_key = orders.get(first).cloned().unwrap_or_else(|| first.clone());
        value.legacy_turns.push(ChatLegacyTurn {
            turn_key: format!(
                "legacy:{}",
                digest(format!("{}:{}:{first}", build.head.agent_id, build.admission).as_bytes())
            ),
            order_key,
            event_refs: std::mem::take(rows),
            kind: ChatLegacyTurnKind::EarlierActivity,
            membership_complete: false,
        });
    };
    for event in events.iter_mut() {
        if owned.contains(&event.id) {
            flush(&mut legacy, &mut value);
        } else {
            if let Some(metadata) = event.metadata.as_object_mut() {
                metadata.remove("chat_turn_key");
                metadata.remove("chat_turn_item_key");
            }
            legacy.push(event.id.clone());
        }
    }
    flush(&mut legacy, &mut value);
    if !build.bound && value.items.is_empty() {
        return Ok(None);
    }
    value
        .turns
        .sort_by(|left, right| left.order_key.cmp(&right.order_key));
    value.validate().map_err(io::Error::other)?;
    gaps::compact(&mut state, false);
    if serde_json::to_vec(&state).map_err(io::Error::other)?.len() > 64 * 1024 {
        gaps::compact(&mut state, true);
    }
    if state.roots.len() > PAGE_ROWS
        || serde_json::to_vec(&state).map_err(io::Error::other)?.len() > 64 * 1024
    {
        return Err(io::Error::other("preview certification budget exceeded"));
    }
    Ok(Some((value, state, remembered_owners)))
}

#[cfg(test)]
#[path = "chat_preview_projection_tests.rs"]
mod tests;
