//! Incremental canonical turn metadata in the existing immutable Store.
//! Admission, source qualification and publication remain with the archive owner.
//! No transcript bodies, raw records, delivery inference or preview state live here.
use std::collections::{BTreeMap, BTreeSet};
use std::io;

use serde::{de::DeserializeOwned, Deserialize, Serialize};
use wardian_core::models::chat::{
    AgentChatEvent, AgentChatEventKind, AgentChatRole, AgentChatStatus,
};
use wardian_core::models::chat_presentation::*;

use super::chat_read_store::{digest, valid_ref, SeekCache, Store};

const ID_BYTES: usize = 512;
const LABEL_BYTES: usize = 160;

pub(super) struct Scope<'a> {
    pub(super) agent_id: &'a str,
    pub(super) conversation_id: &'a str,
    pub(super) source_key: Option<&'a str>,
    pub(super) source_epoch: Option<&'a str>,
    /// The owner proved this request is new to its previous Head, or is building
    /// complete canonical metadata rather than observing only changed rows.
    pub(super) can_activate: bool,
}

/// Only roots belong in the Head. Their AVL values contain bounded metadata;
/// normal page reads never enumerate a turn or open archive evidence.
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub(super) struct Index {
    version: u8,
    scope: Option<String>,
    turns: Option<String>,
    items: Option<String>,
    events: Option<String>,
}

#[derive(Clone, Default, Serialize, Deserialize)]
struct Totals {
    items: u64,
    prompts: u64,
    answers: u64,
    steps: u64,
    failed: u64,
    running: u64,
    action_required: u64,
}

#[derive(Clone, Serialize, Deserialize)]
struct Turn {
    active: bool,
    /// Missing support knowledge in an old checkpoint stays unmigrated.
    #[serde(default)]
    supported: bool,
    #[serde(default)]
    adapter_proven: bool,
    #[serde(default)]
    membership_closed: bool,
    #[serde(default)]
    invalid_proof: bool,
    totals: Totals,
    status: ChatTurnStatus,
    status_order: Option<String>,
    #[serde(default)]
    order_key: Option<String>,
}

impl Default for Turn {
    fn default() -> Self {
        Self {
            active: false,
            supported: true,
            adapter_proven: false,
            membership_closed: false,
            invalid_proof: false,
            totals: Totals::default(),
            status: ChatTurnStatus::Unknown,
            status_order: None,
            order_key: None,
        }
    }
}

impl Turn {
    fn eligible(&self) -> bool {
        self.active && self.supported && self.adapter_proven && self.order_key.is_some()
    }

    fn membership_complete(&self) -> bool {
        self.adapter_proven && self.membership_closed && !self.invalid_proof
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Membership {
    turn_key: String,
    item_key: String,
}

/// The first accepted source order belongs to each physical observation, not
/// just the currently selected backing. Replays of older result IDs must not
/// acquire a newer order when capture appends them again.
#[derive(Serialize, Deserialize)]
struct Observation {
    #[serde(flatten)]
    membership: Membership,
    #[serde(default)]
    source_order: String,
}

#[derive(Clone, Serialize, Deserialize)]
struct Item {
    membership: Membership,
    order_key: String,
    kind: ChatPresentedItemKind,
    evidence: ChatEvidenceIdentity,
    step: Option<Step>,
    #[serde(default)]
    state_only: bool,
    #[serde(default)]
    legacy_only: bool,
}

#[derive(Clone, Serialize, Deserialize)]
struct Step {
    tool_name: String,
    category: ChatToolCategory,
    description: String,
    invocation: Option<StepRecord>,
    result: Option<StepRecord>,
    #[serde(default)]
    approval: Option<StepRecord>,
    #[serde(default)]
    failed: bool,
}

#[derive(Clone, Serialize, Deserialize)]
struct StepRecord {
    event_id: String,
    status: ChatStepStatus,
    #[serde(default)]
    source_order: String,
}

impl Step {
    fn records(&self) -> impl Iterator<Item = &StepRecord> {
        [
            self.invocation.as_ref(),
            self.approval.as_ref(),
            self.result.as_ref(),
        ]
        .into_iter()
        .flatten()
    }

    fn status(&self) -> ChatStepStatus {
        if self.failed
            || self
                .records()
                .any(|record| record.status == ChatStepStatus::Failed)
        {
            return ChatStepStatus::Failed;
        }
        self.records()
            .max_by(|left, right| record_order(left).cmp(&record_order(right)))
            .map_or(ChatStepStatus::Unknown, |record| record.status.clone())
    }
}

impl Index {
    /// Allows an empty additive index to be omitted without changing old Heads.
    pub(super) fn is_empty(&self) -> bool {
        self.version == 0
            && self.scope.is_none()
            && self.turns.is_none()
            && self.items.is_none()
            && self.events.is_none()
    }

    /// Schema-defined links for the existing Store checkpoint traversal.
    pub(super) fn node_roots(&self) -> impl Iterator<Item = &str> {
        [
            self.turns.as_deref(),
            self.items.as_deref(),
            self.events.as_deref(),
        ]
        .into_iter()
        .flatten()
    }

    /// Observe one qualified canonical candidate BEFORE header clipping.
    /// The caller owns acceptance of `request_root_id` and source coordinates.
    /// Missing request evidence is intentionally not reconstructed from turn IDs.
    ///
    /// Roots advance together only after all writes succeed. Store budgets and
    /// persistence errors propagate; a failed checkpoint cannot publish half an
    /// item/counter update. The returned key may still belong to an inactive turn.
    pub(super) fn observe(
        &mut self,
        store: &mut Store,
        scope: &Scope<'_>,
        event: &AgentChatEvent,
        order_key: &str,
        preferred_step_order: Option<&str>,
    ) -> io::Result<Option<String>> {
        if self.version != 1 && !self.is_empty() {
            return Ok(None);
        }
        if !matches!(event.provider.as_str(), "codex" | "claude" | "unknown") {
            return Ok(None);
        }
        let supported_kind = item_kind(event);
        // Owned generated input may acquire its provider later. Its marker can
        // retain identity, but activation still requires the matching provider.
        let retained_generated = event.provider == "unknown"
            && event.metadata["generated"] == true
            && matches!(
                string(event, "chat_turn_adapter"),
                Some("codex_bracket_v1" | "claude_parent_chain_v1")
            );
        let legacy_only =
            supported_kind.is_none() || !(proven_adapter(event) || retained_generated);
        // This private placeholder is never published as narration. Unsupported
        // accepted-root evidence still owns membership and retires the entire
        // turn to the unchanged legacy renderer.
        let kind = supported_kind.unwrap_or(ChatPresentedItemKind::Narration);
        let Some(root) = string(event, "request_root_id") else {
            return Ok(None);
        };
        if event.session_id != scope.agent_id
            || !bounded_id(&event.id)
            || !bounded_id(root)
            || !bounded_id(order_key)
            || !bounded_id(scope.agent_id)
            || !bounded_id(scope.conversation_id)
            || scope.source_key.is_some_and(|key| !bounded_id(key))
            || scope.source_epoch.is_some_and(|epoch| !bounded_id(epoch))
            || event.metadata["chat_provisional"] == true
            || event.metadata["chat_legacy_identity_ineligible"] == true
            || string(event, "conversation_archive_id")
                .is_some_and(|id| id != scope.conversation_id)
            || string(event, "chat_source_epoch")
                .zip(scope.source_epoch)
                .is_some_and(|(observed, expected)| observed != expected)
        {
            return Ok(None);
        }
        let owner = key(&("turn-owner-v1", scope.agent_id, scope.conversation_id))?;
        if self.scope.as_ref().is_some_and(|known| known != &owner) {
            return Err(io::Error::other("foreign canonical turn index scope"));
        }
        let source = key(&(
            "turn-source-v1",
            &owner,
            scope.source_key,
            scope.source_epoch,
        ))?;
        let turn_key = key(&("turn-v1", &source, root))?;
        let call = matches!(kind, ChatPresentedItemKind::Step)
            .then(|| call_identity(event))
            .flatten();
        let physical = string(event, "chat_source_ref").unwrap_or(&event.id);
        if !bounded_id(physical) {
            return Ok(None);
        }
        let proposed = if kind == ChatPresentedItemKind::Prompt {
            key(&("prompt-v1", &turn_key))?
        } else if let Some(call) = call {
            key(&("step-v1", &turn_key, call))?
        } else {
            key(&("item-v1", &turn_key, physical))?
        };
        let mut cache = SeekCache::default();
        let mut membership = None;
        let mut first_source_order: Option<String> = None;
        for id in [event.id.as_str(), physical] {
            if let Some(known) =
                lookup::<Observation>(store, &self.events, &digest(id.as_bytes()), &mut cache)?
            {
                if known.membership.turn_key != turn_key
                    || membership
                        .as_ref()
                        .is_some_and(|prior| prior != &known.membership)
                    || (call.is_some() && known.membership.item_key != proposed)
                {
                    return Err(io::Error::other("conflicting canonical turn membership"));
                }
                if !known.source_order.is_empty() {
                    if first_source_order
                        .as_ref()
                        .is_some_and(|prior| prior != &known.source_order)
                    {
                        return Err(io::Error::other("conflicting canonical event order"));
                    }
                    first_source_order = Some(known.source_order);
                }
                membership = Some(known.membership);
            }
        }
        let membership = membership.unwrap_or(Membership {
            turn_key: turn_key.clone(),
            item_key: proposed,
        });
        let old: Option<Item> = lookup(store, &self.items, &membership.item_key, &mut cache)?;
        // Existing v1 observations may predate per-event order metadata. Keep
        // their selected record's known order when upgrading that observation.
        let source_order = first_source_order
            .as_deref()
            .or_else(|| {
                old.as_ref()
                    .and_then(|item| item.step.as_ref())
                    .and_then(|step| step.records().find(|record| record.event_id == event.id))
                    .map(|record| record.source_order.as_str())
                    .filter(|order| !order.is_empty())
            })
            .unwrap_or(order_key);
        let mut item = old.clone().unwrap_or_else(|| Item {
            membership: membership.clone(),
            // The item suffix disambiguates equal source-order prefixes without
            // using loaded page positions. Neither key changes on later evidence.
            order_key: format!(
                "{}:{}",
                preferred_step_order
                    .filter(|order| kind == ChatPresentedItemKind::Step && bounded_id(order))
                    .unwrap_or(order_key),
                membership.item_key
            ),
            kind: kind.clone(),
            evidence: ChatEvidenceIdentity::Source {
                source_key: scope
                    .source_key
                    .filter(|key| !key.is_empty())
                    .unwrap_or(&source)
                    .into(),
                source_epoch: scope
                    .source_epoch
                    .filter(|epoch| !epoch.is_empty())
                    .unwrap_or(&source)
                    .into(),
                record_key: physical.into(),
            },
            step: None,
            state_only: state_only(event),
            legacy_only,
        });
        if item.membership != membership
            || (!legacy_only
                && !item.legacy_only
                && ((item.kind == ChatPresentedItemKind::Step)
                    != (kind == ChatPresentedItemKind::Step)
                    || (item.kind == ChatPresentedItemKind::Prompt)
                        != (kind == ChatPresentedItemKind::Prompt)))
        {
            return Err(io::Error::other("canonical turn item changed identity"));
        }
        // A missing or unknown phase in a replay cannot erase proven final
        // evidence. Known commentary/final phases still follow canonical source
        // evidence; idle and text position never determine an answer.
        item.legacy_only |= legacy_only;
        item.state_only |= state_only(event);
        if item.state_only && !item.legacy_only {
            item.kind = ChatPresentedItemKind::Narration;
        } else if !item.legacy_only
            && !(item.kind == ChatPresentedItemKind::Answer
                && kind == ChatPresentedItemKind::Narration
                && !matches!(
                    string(event, "provider_phase"),
                    Some("commentary" | "final_answer")
                ))
        {
            item.kind = kind;
        }
        if !item.legacy_only && item.kind == ChatPresentedItemKind::Step {
            update_step(&mut item, event, source_order);
        }
        let mut turn: Turn = lookup(store, &self.turns, &turn_key, &mut cache)?.unwrap_or_default();
        if let Some(old) = &old {
            turn.totals.remove(old)?;
        }
        turn.totals.add(&item)?;
        turn.supported &= !item.legacy_only;
        if turn
            .order_key
            .as_deref()
            .is_none_or(|known| item.order_key.as_str() < known)
        {
            turn.order_key = Some(item.order_key.clone());
        }
        let adapter_proven = proven_adapter(event);
        let admitted_request = request_prompt(event) && scope.can_activate && adapter_proven;
        turn.active |= admitted_request;
        turn.adapter_proven |= admitted_request;
        turn.membership_closed |=
            adapter_proven && event.metadata["chat_turn_membership_complete"] == true;
        turn.invalid_proof |=
            adapter_proven && event.metadata["chat_turn_membership_partial"] == true;
        if let Some(receipt) = super::chat_turn_closure::for_request(scope, event) {
            turn.membership_closed |= receipt.complete();
            turn.invalid_proof |= receipt.partial();
            let order = receipt.order();
            if turn
                .status_order
                .as_deref()
                .is_none_or(|prior| order.as_str() >= prior)
            {
                turn.status = match receipt.status() {
                    AgentChatStatus::Succeeded => ChatTurnStatus::Completed,
                    AgentChatStatus::Failed => ChatTurnStatus::Failed,
                    AgentChatStatus::Cancelled => ChatTurnStatus::Cancelled,
                    _ => ChatTurnStatus::Unknown,
                };
                turn.status_order = Some(order);
            }
        }
        if !item.legacy_only
            && (event.kind == AgentChatEventKind::Status
                || item.kind == ChatPresentedItemKind::Answer
                || (event.kind == AgentChatEventKind::ToolCall
                    && matches!(
                        string(event, "raw_type"),
                        Some("task_started" | "turn_started")
                    )))
        {
            if let Some(status) = turn_status(event) {
                let source_order = item.order_key.as_str();
                let stale_replay = turn.status_order.as_deref() == Some(source_order)
                    && terminal_turn(&turn.status)
                    && !terminal_turn(&status);
                if !stale_replay
                    && turn
                        .status_order
                        .as_deref()
                        .is_none_or(|prior| source_order >= prior)
                {
                    turn.status = status;
                    turn.status_order = Some(source_order.to_owned());
                }
            }
        }
        let mut next = self.clone();
        next.version = 1;
        next.scope = Some(owner);
        let item_ref = store.put(&item)?;
        next.items = Some(store.insert(&next.items, &membership.item_key, &item_ref)?);
        let turn_ref = store.put(&turn)?;
        next.turns = Some(store.insert(&next.turns, &turn_key, &turn_ref)?);
        let member_ref = store.put(&Observation {
            membership: membership.clone(),
            source_order: source_order.into(),
        })?;
        for id in [event.id.as_str(), physical] {
            next.events = Some(store.insert(&next.events, &digest(id.as_bytes()), &member_ref)?);
        }
        *self = next;
        Ok(Some(membership.item_key))
    }

    /// Construct only this bounded page, using persisted whole-turn totals.
    /// Qualified canonical display rows select one representation per turn.
    /// Unsupported/unmigrated turns retain producer-owned legacy groups in this
    /// same view; unindexed rows share one scope-stable Earlier activity group.
    pub(super) fn page(
        &self,
        store: &mut Store,
        events: &[AgentChatEvent],
        complete: bool,
    ) -> io::Result<Option<ChatPresentationV1>> {
        if self.version != 1 {
            return Ok(None);
        }
        if events.is_empty() || events.len() > CHAT_PRESENTATION_MAX_ITEMS || self.is_empty() {
            return Ok(None);
        }
        let mut cache = SeekCache::default();
        let mut loaded = BTreeMap::<String, (String, Item)>::new();
        let mut state_event_refs = Vec::new();
        let mut turns = BTreeMap::<String, Turn>::new();
        let mut items = BTreeMap::<String, ChatPresentedItem>::new();
        let mut logical_kinds = BTreeMap::<String, ChatPresentedItemKind>::new();
        let mut logical_versions = BTreeMap::<String, BTreeSet<String>>::new();
        let mut bindings = Vec::with_capacity(events.len());
        let mut seen_events = BTreeSet::new();
        for event in events {
            if event.metadata["chat_provisional"] == true
                || !bounded_id(&event.id)
                || !seen_events.insert(event.id.as_str())
            {
                return Ok(None);
            }
            let Some(members) = self.page_members(store, event, &mut cache)? else {
                bindings.push(None);
                continue;
            };
            let mut selected: Option<(String, Item)> = None;
            let mut logical_kind = None;
            let mut versions = BTreeSet::new();
            for member in members {
                if !loaded.contains_key(&member.item_key) {
                    let Some(reference) =
                        store.get_reusing_nodes(&self.items, &member.item_key, &mut cache)?
                    else {
                        return Ok(None);
                    };
                    let item: Item = store.read(&reference)?;
                    loaded.insert(member.item_key.clone(), (reference, item));
                }
                let Some((reference, item)) = loaded.get(&member.item_key) else {
                    return Ok(None);
                };
                versions.insert(reference.clone());
                if item.membership != member
                    || selected.as_ref().is_some_and(|(_, prior)| {
                        prior.membership.turn_key != member.turn_key
                            || !compatible_component_kinds(&prior.kind, &item.kind)
                    })
                {
                    return Ok(None);
                }
                // Membership is already certified by the existing logical
                // owner. Explicit final evidence enriches that component while
                // its earliest persisted item/order retains visible ownership.
                if logical_kind.is_none() || item.kind == ChatPresentedItemKind::Answer {
                    logical_kind = Some(item.kind.clone());
                }
                if selected.as_ref().is_none_or(|(_, prior): &(String, Item)| {
                    (&item.order_key, &item.membership.item_key)
                        < (&prior.order_key, &prior.membership.item_key)
                }) {
                    selected = Some((reference.clone(), item.clone()));
                }
            }
            let Some((_, item)) = selected else {
                return Ok(None);
            };
            let member = &item.membership;
            logical_versions
                .entry(member.item_key.clone())
                .or_default()
                .extend(versions);
            if !turns.contains_key(&member.turn_key) {
                let Some(turn) = lookup::<Turn>(store, &self.turns, &member.turn_key, &mut cache)?
                else {
                    return Ok(None);
                };
                turns.insert(member.turn_key.clone(), turn);
            }
            let Some(turn) = turns.get_mut(&member.turn_key) else {
                return Ok(None);
            };
            turn.supported &= !item.legacy_only && item_kind(event).is_some();
            let kind = logical_kinds
                .entry(member.item_key.clone())
                .or_insert_with(|| logical_kind.clone().unwrap_or_else(|| item.kind.clone()));
            if logical_kind == Some(ChatPresentedItemKind::Answer) {
                *kind = ChatPresentedItemKind::Answer;
            }
            bindings.push(Some(member.item_key.clone()));
        }

        // Resolve the complete bounded page before choosing representations:
        // an unsupported later row must also retire earlier rows of that turn.
        let earlier_key = key(&("earlier-activity-v1", &self.scope))?;
        let mut legacy = BTreeMap::<String, ChatLegacyTurn>::new();
        for (event, binding) in events.iter().zip(bindings) {
            let Some(item_key) = binding else {
                legacy
                    .entry(earlier_key.clone())
                    .or_insert_with(|| ChatLegacyTurn {
                        turn_key: earlier_key.clone(),
                        order_key: "0:earlier".into(),
                        event_refs: Vec::new(),
                        kind: ChatLegacyTurnKind::EarlierActivity,
                        membership_complete: false,
                    })
                    .event_refs
                    .push(event.id.clone());
                continue;
            };
            let Some((reference, item)) = loaded.get(&item_key) else {
                return Ok(None);
            };
            let member = &item.membership;
            let Some(turn) = turns.get(&member.turn_key) else {
                return Ok(None);
            };
            if !turn.eligible() {
                legacy
                    .entry(member.turn_key.clone())
                    .or_insert_with(|| ChatLegacyTurn {
                        turn_key: member.turn_key.clone(),
                        order_key: turn
                            .order_key
                            .clone()
                            .unwrap_or_else(|| format!("0:unmigrated:{}", member.turn_key)),
                        event_refs: Vec::new(),
                        kind: ChatLegacyTurnKind::Unmigrated,
                        membership_complete: false,
                    })
                    .event_refs
                    .push(event.id.clone());
                continue;
            }
            if item.state_only {
                state_event_refs.push(ChatStateEventRef {
                    event_id: event.id.clone(),
                    turn_key: member.turn_key.clone(),
                });
                continue;
            }
            let presented =
                items
                    .entry(member.item_key.clone())
                    .or_insert_with(|| ChatPresentedItem {
                        item_key: member.item_key.clone(),
                        turn_key: member.turn_key.clone(),
                        order_key: item.order_key.clone(),
                        kind: logical_kinds
                            .get(&member.item_key)
                            .cloned()
                            .unwrap_or_else(|| item.kind.clone()),
                        provenance: ChatItemProvenance::Canonical,
                        evidence: item.evidence.clone(),
                        evidence_version: logical_versions
                            .get(&member.item_key)
                            .map(|versions| {
                                digest(
                                    versions
                                        .iter()
                                        .cloned()
                                        .collect::<Vec<_>>()
                                        .join(":")
                                        .as_bytes(),
                                )
                            })
                            .unwrap_or_else(|| reference.clone()),
                        admission_id: None,
                        submission_status: None,
                        event_refs: Vec::new(),
                        preview: None,
                        step: item.step.as_ref().map(|step| present_step(step, turn)),
                    });
            if !presented.event_refs.contains(&event.id) {
                if presented.event_refs.len() == CHAT_PRESENTATION_MAX_ITEMS
                    || !bounded_id(&event.id)
                {
                    return Ok(None);
                }
                presented.event_refs.push(event.id.clone());
            }
            if let Some(step) = &mut presented.step {
                // Only this admitted page's revision-bound reference grants
                // detail authority, never an old reference saved in metadata.
                if let Some(reference) = tail_reference(event) {
                    if event.kind == AgentChatEventKind::ToolResult || step.detail_ref.is_none() {
                        step.detail_ref = Some(reference);
                        step.detail_availability = ChatDetailAvailability::Available;
                    }
                }
            }
        }
        let mut items: Vec<_> = items.into_values().collect();
        items.sort_by(|left, right| {
            left.order_key
                .cmp(&right.order_key)
                .then(left.item_key.cmp(&right.item_key))
        });
        let mut presented_turns = Vec::with_capacity(turns.len());
        let mut ordered_turns = turns
            .into_iter()
            .filter(|(_, turn)| turn.eligible())
            .collect::<Vec<_>>();
        ordered_turns.sort_by(|left, right| {
            left.1
                .order_key
                .cmp(&right.1.order_key)
                .then(left.0.cmp(&right.0))
        });
        for (turn_key, turn) in ordered_turns {
            let Some(order_key) = turn.order_key.clone() else {
                continue;
            };
            let local: Vec<_> = items
                .iter()
                .filter(|local| local.turn_key == turn_key)
                .collect();
            presented_turns.push(ChatPresentedTurn {
                turn_key,
                order_key,
                membership_complete: turn.membership_complete(),
                status: aggregate_status(&turn),
                prompt_item_keys: local
                    .iter()
                    .filter(|item| item.kind == ChatPresentedItemKind::Prompt)
                    .map(|item| item.item_key.clone())
                    .collect(),
                answer_item_keys: local
                    .iter()
                    .filter(|item| item.kind == ChatPresentedItemKind::Answer)
                    .map(|item| item.item_key.clone())
                    .collect(),
                references_complete: complete
                    && turn.membership_complete()
                    && local.len() as u64 == turn.totals.items,
                duration_ms: None,
                reported_changes: None,
                step_count: turn.totals.steps,
                answer_recorded: turn.totals.answers > 0,
                failed_step_count: turn.totals.failed,
                counts_complete: turn.membership_complete(),
            });
        }
        if presented_turns.is_empty() {
            return Ok(None);
        }
        let mut legacy_turns = legacy.into_values().collect::<Vec<_>>();
        legacy_turns.sort_by(|left, right| {
            left.order_key
                .cmp(&right.order_key)
                .then(left.turn_key.cmp(&right.turn_key))
        });
        let owned_events = items
            .iter()
            .map(|item| item.event_refs.len())
            .sum::<usize>()
            + state_event_refs.len()
            + legacy_turns
                .iter()
                .map(|turn| turn.event_refs.len())
                .sum::<usize>();
        if owned_events != events.len()
            || legacy_turns.iter().any(|legacy| {
                presented_turns
                    .iter()
                    .any(|turn| turn.turn_key == legacy.turn_key)
            })
        {
            return Ok(None);
        }
        let page = ChatPresentationV1 {
            legacy_turns,
            state_event_refs,
            turns: presented_turns,
            items,
            detail: None,
        };
        if page.validate().is_err()
            || serde_json::to_vec(&page).map_err(io::Error::other)?.len()
                > CHAT_PRESENTATION_MAX_BYTES
        {
            return Ok(None);
        }
        Ok(Some(page))
    }

    /// Return stored opaque keys for the canonical Row owner after observation.
    /// An inactive turn may have metadata, so this does not grant presentation
    /// authority; page publication still validates Item membership and Turn.
    pub(super) fn event_keys(
        &self,
        store: &mut Store,
        event_id: &str,
    ) -> io::Result<Option<(String, String)>> {
        if self.version != 1 || !bounded_id(event_id) {
            return Ok(None);
        }
        let mut cache = SeekCache::default();
        Ok(lookup::<Membership>(
            store,
            &self.events,
            &digest(event_id.as_bytes()),
            &mut cache,
        )?
        .map(|member| (member.turn_key, member.item_key)))
    }

    /// Resolve a canonical step for the existing detail owner without reading
    /// transcript bodies. Inactive request roots grant no presentation authority.
    pub(super) fn step_key(&self, store: &mut Store, event_id: &str) -> io::Result<Option<String>> {
        Ok(self
            .owned_step(store, event_id)?
            .map(|(item, _)| item.membership.item_key))
    }

    /// Resolve one step using its persisted evidence and proven turn state.
    /// Turn termination stops unresolved work without touching other step rows.
    pub(super) fn step_status(
        &self,
        store: &mut Store,
        event_id: &str,
    ) -> io::Result<Option<ChatStepStatus>> {
        Ok(self
            .owned_step(store, event_id)?
            .and_then(|(item, turn)| item.step.map(|step| resolved_step_status(&step, &turn))))
    }

    /// Resolve the latest owned canonical backing for a saved step reference.
    /// These are archive event IDs, so Head.identities retains all detail and
    /// privacy authority. A result supersedes invocation output within its step.
    /// Approval output is used only before either backing exists.
    pub(super) fn step_output_event(
        &self,
        store: &mut Store,
        event_id: &str,
    ) -> io::Result<Option<String>> {
        Ok(self
            .owned_step(store, event_id)?
            .and_then(|(item, _)| item.step)
            .and_then(|step| step.result.or(step.invocation).or(step.approval))
            .map(|record| record.event_id))
    }

    /// Resolve the current owned invocation's canonical event ID for bounded
    /// input reads. Head.identities and current-root guards retain authority;
    /// missing invocation evidence grants no raw artifact fallback.
    pub(super) fn step_input_event(
        &self,
        store: &mut Store,
        event_id: &str,
    ) -> io::Result<Option<String>> {
        Ok(self
            .owned_step(store, event_id)?
            .and_then(|(item, _)| item.step)
            .and_then(|step| step.invocation)
            .map(|record| record.event_id))
    }

    fn owned_step(&self, store: &mut Store, event_id: &str) -> io::Result<Option<(Item, Turn)>> {
        if self.version != 1 {
            return Ok(None);
        }
        if !bounded_id(event_id) {
            return Ok(None);
        }
        let mut cache = SeekCache::default();
        let Some(member) = lookup::<Membership>(
            store,
            &self.events,
            &digest(event_id.as_bytes()),
            &mut cache,
        )?
        else {
            return Ok(None);
        };
        let Some(item) = lookup::<Item>(store, &self.items, &member.item_key, &mut cache)? else {
            return Ok(None);
        };
        let Some(turn) = lookup::<Turn>(store, &self.turns, &member.turn_key, &mut cache)? else {
            return Ok(None);
        };
        Ok((item.membership == member
            && item.kind == ChatPresentedItemKind::Step
            && !item.legacy_only
            && turn.eligible())
        .then_some((item, turn)))
    }

    fn page_members(
        &self,
        store: &mut Store,
        event: &AgentChatEvent,
        cache: &mut SeekCache,
    ) -> io::Result<Option<Vec<Membership>>> {
        // This field is a producer certificate, not an arbitrary list of text
        // aliases. The caller supplies only admitted canonical display rows.
        let ids = if let Some(members) = event.metadata.get("chat_display_member_ids") {
            let Some(members) = members
                .as_array()
                .filter(|members| !members.is_empty() && members.len() <= 2)
            else {
                return Ok(None);
            };
            let Some(ids) = members
                .iter()
                .map(|id| id.as_str().filter(|id| bounded_id(id)))
                .collect::<Option<Vec<_>>>()
            else {
                return Ok(None);
            };
            ids
        } else {
            if !bounded_id(&event.id) {
                return Ok(None);
            }
            // Only the authenticated Row owner injects these keys, after
            // observe(). Raw-source header construction must never copy them.
            // They avoid an event AVL seek; page() still checks the loaded Item
            // and active Turn. A logical alias certificate takes precedence.
            if let (Some(turn_key), Some(item_key)) = (
                string(event, "chat_turn_key").filter(|key| valid_ref(key)),
                string(event, "chat_turn_item_key").filter(|key| valid_ref(key)),
            ) {
                return Ok(Some(vec![Membership {
                    turn_key: turn_key.into(),
                    item_key: item_key.into(),
                }]));
            }
            for id in [Some(event.id.as_str()), string(event, "chat_source_ref")]
                .into_iter()
                .flatten()
            {
                if let Some(member) = lookup(store, &self.events, &digest(id.as_bytes()), cache)? {
                    return Ok(Some(vec![member]));
                }
            }
            return Ok(None);
        };
        let mut members = Vec::with_capacity(ids.len());
        for id in ids {
            let Some(member) = lookup(store, &self.events, &digest(id.as_bytes()), cache)? else {
                return Ok(None);
            };
            members.push(member);
        }
        Ok(Some(members))
    }
}

fn lookup<T: DeserializeOwned>(
    store: &mut Store,
    root: &Option<String>,
    key: &str,
    cache: &mut SeekCache,
) -> io::Result<Option<T>> {
    store
        .get_reusing_nodes(root, key, cache)?
        .map(|reference| store.read(&reference))
        .transpose()
}

fn key(value: &impl Serialize) -> io::Result<String> {
    Ok(digest(
        &serde_json::to_vec(value).map_err(io::Error::other)?,
    ))
}

fn bounded_id(value: &str) -> bool {
    !value.trim().is_empty() && value.len() <= ID_BYTES
}

fn string<'a>(event: &'a AgentChatEvent, field: &str) -> Option<&'a str> {
    event.metadata[field]
        .as_str()
        .filter(|value| !value.trim().is_empty())
}

fn request_prompt(event: &AgentChatEvent) -> bool {
    event.kind == AgentChatEventKind::Message
        && event.role == Some(AgentChatRole::User)
        && matches!(
            string(event, "input_origin"),
            Some("human_input" | "agent_input")
        )
        && matches!(string(event, "input_purpose"), None | Some("request"))
}

/// Only the adapter for the canonical provider can certify request provenance.
/// Delivery identity, generic metadata and publication readiness cannot do so.
fn proven_adapter(event: &AgentChatEvent) -> bool {
    matches!(
        (event.provider.as_str(), string(event, "chat_turn_adapter")),
        ("codex", Some("codex_bracket_v1")) | ("claude", Some("claude_parent_chain_v1"))
    )
}

fn compatible_component_kinds(left: &ChatPresentedItemKind, right: &ChatPresentedItemKind) -> bool {
    left == right
        || matches!(
            (left, right),
            (
                ChatPresentedItemKind::Narration,
                ChatPresentedItemKind::Answer
            ) | (
                ChatPresentedItemKind::Answer,
                ChatPresentedItemKind::Narration
            )
        )
}

/// The normalizer's explicit context classification is state evidence, not
/// assistant prose. No inference from system text, purpose or visibility occurs.
fn state_only(event: &AgentChatEvent) -> bool {
    event.kind == AgentChatEventKind::Status
        || (event.kind == AgentChatEventKind::ToolCall
            && matches!(
                string(event, "raw_type"),
                Some("task_started" | "turn_started")
            ))
        || (event.kind == AgentChatEventKind::Message
            && matches!(
                string(event, "input_origin"),
                Some("provider_internal" | "context_injection")
            ))
}

/// This capability supports only existing admitted prompt, assistant, tool and
/// state shapes. Unsupported evidence stays on the legacy owner unchanged.
fn item_kind(event: &AgentChatEvent) -> Option<ChatPresentedItemKind> {
    if state_only(event) {
        Some(ChatPresentedItemKind::Narration)
    } else if request_prompt(event) {
        Some(ChatPresentedItemKind::Prompt)
    } else if matches!(
        event.kind,
        AgentChatEventKind::ToolCall
            | AgentChatEventKind::ToolResult
            | AgentChatEventKind::Approval
    ) {
        Some(ChatPresentedItemKind::Step)
    } else if event.kind == AgentChatEventKind::Message
        && event.role == Some(AgentChatRole::Assistant)
    {
        Some(if string(event, "provider_phase") == Some("final_answer") {
            ChatPresentedItemKind::Answer
        } else {
            ChatPresentedItemKind::Narration
        })
    } else {
        None
    }
}

/// Return source invocation identity after the owner proves request membership.
pub(super) fn call_identity(event: &AgentChatEvent) -> Option<&str> {
    // Tool-event normalizers use turn_id for invocation identity. It is usable
    // only AFTER accepted request membership, never as a request-root fallback.
    ["call_id", "tool_call_id", "tool_use_id"]
        .into_iter()
        .find_map(|field| string(event, field))
        .or_else(|| {
            event.turn_id.as_deref().filter(|id| {
                string(event, "provider_turn_id") != Some(*id)
                    && string(event, "request_root_id") != Some(*id)
            })
        })
        .filter(|id| bounded_id(id))
}

fn update_step(item: &mut Item, event: &AgentChatEvent, source_order: &str) {
    let (tool_name, category, description) = label(event);
    let step = item.step.get_or_insert_with(|| Step {
        tool_name: tool_name.clone(),
        category: category.clone(),
        description: description.clone(),
        invocation: None,
        result: None,
        approval: None,
        failed: false,
    });
    let mut record = StepRecord {
        event_id: event.id.clone(),
        status: step_status(event),
        source_order: source_order.into(),
    };
    // Failure is evidence about this invocation even if a newer physical record
    // supplies the current output. A real retry has a different owning step.
    let failed = record.status == ChatStepStatus::Failed
        || step
            .records()
            .any(|record| record.status == ChatStepStatus::Failed);
    step.failed |= failed;
    let old = match event.kind {
        AgentChatEventKind::ToolCall => &step.invocation,
        AgentChatEventKind::Approval => &step.approval,
        _ => &step.result,
    };
    if let Some(old) = old {
        if old.event_id == event.id {
            record.source_order.clone_from(&old.source_order);
            if old.status == ChatStepStatus::Failed
                || (terminal_step(&old.status) && !terminal_step(&record.status))
            {
                record.status = old.status.clone();
            }
        }
        if record_order(old) > record_order(&record) && old.event_id != event.id {
            return;
        }
    }
    if event.kind == AgentChatEventKind::ToolCall
        || (event.kind == AgentChatEventKind::Approval && step.invocation.is_none())
    {
        step.tool_name = tool_name;
        step.category = category;
        step.description = description;
    }
    match event.kind {
        AgentChatEventKind::ToolCall => step.invocation = Some(record),
        AgentChatEventKind::Approval => step.approval = Some(record),
        _ => step.result = Some(record),
    }
}

fn record_order(record: &StepRecord) -> (&str, &str) {
    (&record.source_order, &record.event_id)
}

fn resolved_step_status(step: &Step, turn: &Turn) -> ChatStepStatus {
    let status = step.status();
    if !matches!(
        status,
        ChatStepStatus::Running | ChatStepStatus::ActionRequired | ChatStepStatus::Unknown
    ) {
        return status;
    }
    match turn.status {
        ChatTurnStatus::Cancelled => ChatStepStatus::Cancelled,
        ChatTurnStatus::Completed | ChatTurnStatus::Failed | ChatTurnStatus::Incomplete => {
            ChatStepStatus::Incomplete
        }
        _ => status,
    }
}

fn step_status(event: &AgentChatEvent) -> ChatStepStatus {
    if event.exit_code.is_some_and(|code| code != 0) {
        return ChatStepStatus::Failed;
    }
    match event.status {
        Some(AgentChatStatus::Running | AgentChatStatus::Processing) => ChatStepStatus::Running,
        Some(AgentChatStatus::Succeeded) => ChatStepStatus::Completed,
        Some(AgentChatStatus::Failed) => ChatStepStatus::Failed,
        Some(AgentChatStatus::Cancelled) => ChatStepStatus::Cancelled,
        Some(AgentChatStatus::ActionRequired) => ChatStepStatus::ActionRequired,
        _ => ChatStepStatus::Unknown,
    }
}

fn turn_status(event: &AgentChatEvent) -> Option<ChatTurnStatus> {
    match event.status {
        Some(AgentChatStatus::Running | AgentChatStatus::Processing) => {
            Some(ChatTurnStatus::Running)
        }
        Some(AgentChatStatus::Succeeded) => Some(ChatTurnStatus::Completed),
        Some(AgentChatStatus::Failed) => Some(ChatTurnStatus::Failed),
        Some(AgentChatStatus::Cancelled) => Some(ChatTurnStatus::Cancelled),
        Some(AgentChatStatus::ActionRequired) => Some(ChatTurnStatus::ActionRequired),
        // Idle is agent state and supplies no turn termination evidence.
        _ => None,
    }
}

fn aggregate_status(turn: &Turn) -> ChatTurnStatus {
    if terminal_turn(&turn.status) {
        turn.status.clone()
    } else if turn.totals.action_required > 0 {
        ChatTurnStatus::ActionRequired
    } else if turn.totals.running > 0 {
        ChatTurnStatus::Running
    } else {
        turn.status.clone()
    }
}

fn terminal_turn(status: &ChatTurnStatus) -> bool {
    matches!(
        status,
        ChatTurnStatus::Completed
            | ChatTurnStatus::Failed
            | ChatTurnStatus::Cancelled
            | ChatTurnStatus::Incomplete
    )
}

fn terminal_step(status: &ChatStepStatus) -> bool {
    matches!(
        status,
        ChatStepStatus::Completed
            | ChatStepStatus::Failed
            | ChatStepStatus::Cancelled
            | ChatStepStatus::Incomplete
    )
}

impl Totals {
    fn contribution(item: &Item) -> Self {
        if item.legacy_only || item.state_only {
            return Self::default();
        }
        let status = item.step.as_ref().map(Step::status);
        Self {
            items: u64::from(!item.state_only),
            prompts: u64::from(item.kind == ChatPresentedItemKind::Prompt),
            answers: u64::from(item.kind == ChatPresentedItemKind::Answer),
            steps: u64::from(item.kind == ChatPresentedItemKind::Step),
            failed: u64::from(status == Some(ChatStepStatus::Failed)),
            running: u64::from(status == Some(ChatStepStatus::Running)),
            action_required: u64::from(status == Some(ChatStepStatus::ActionRequired)),
        }
    }

    fn remove(&mut self, item: &Item) -> io::Result<()> {
        let old = Self::contribution(item);
        for (count, delta) in [
            (&mut self.items, old.items),
            (&mut self.prompts, old.prompts),
            (&mut self.answers, old.answers),
            (&mut self.steps, old.steps),
            (&mut self.failed, old.failed),
            (&mut self.running, old.running),
            (&mut self.action_required, old.action_required),
        ] {
            *count = count
                .checked_sub(delta)
                .ok_or_else(|| io::Error::other("inconsistent canonical turn totals"))?;
        }
        Ok(())
    }

    fn add(&mut self, item: &Item) -> io::Result<()> {
        let new = Self::contribution(item);
        for (count, delta) in [
            (&mut self.items, new.items),
            (&mut self.prompts, new.prompts),
            (&mut self.answers, new.answers),
            (&mut self.steps, new.steps),
            (&mut self.failed, new.failed),
            (&mut self.running, new.running),
            (&mut self.action_required, new.action_required),
        ] {
            *count = count
                .checked_add(delta)
                .ok_or_else(|| io::Error::other("canonical turn count overflow"))?;
        }
        Ok(())
    }
}

fn present_step(step: &Step, turn: &Turn) -> ChatPresentedStep {
    ChatPresentedStep {
        tool_name: step.tool_name.clone(),
        category: step.category.clone(),
        description: step.description.clone(),
        status: resolved_step_status(step, turn),
        summary: step.result.as_ref().map(|_| "Result available".into()),
        invocation_ref: step
            .invocation
            .as_ref()
            .map(|record| record.event_id.clone()),
        result_ref: step.result.as_ref().map(|record| record.event_id.clone()),
        approval_ref: step.approval.as_ref().map(|record| record.event_id.clone()),
        detail_availability: ChatDetailAvailability::Pending,
        detail_ref: None,
    }
}

fn tail_reference(event: &AgentChatEvent) -> Option<String> {
    string(event, "chat_step_detail_ref")
        .filter(|reference| reference.len() <= ID_BYTES && reference.ends_with(":tail"))
        .map(str::to_owned)
        .or_else(|| {
            string(event, "chat_detail_ref")
                .filter(|reference| reference.len() <= ID_BYTES)
                .and_then(|reference| reference.strip_suffix(":0"))
                .map(|reference| format!("{reference}:tail"))
        })
}

fn safe_label(value: &str) -> Option<&str> {
    let value = value.trim();
    (!value.is_empty()
        && value.len() <= LABEL_BYTES
        && !value
            .chars()
            .any(|ch| ch.is_control() || matches!(ch, '{' | '}' | '[' | ']')))
    .then_some(value)
}

fn label(event: &AgentChatEvent) -> (String, ChatToolCategory, String) {
    let tool = string(event, "tool_name")
        .or_else(|| {
            (event.kind == AgentChatEventKind::ToolCall)
                .then_some(event.title.as_deref())
                .flatten()
        })
        .filter(|tool| {
            tool.len() <= 64
                && tool
                    .bytes()
                    .all(|ch| ch.is_ascii_alphanumeric() || b"_-.:".contains(&ch))
                && !matches!(
                    *tool,
                    "function_call"
                        | "custom_tool_call"
                        | "tool_call"
                        | "tool_use"
                        | "tool_result"
                        | "function_call_output"
                        | "custom_tool_call_output"
                        | "assistant"
                        | "result"
                )
        })
        .unwrap_or("tool");
    let normalized = tool.to_ascii_lowercase();
    let category = match normalized.as_str() {
        "bash" | "shell" | "shell_command" | "exec_command" | "exec" | "run_command"
        | "terminal" => ChatToolCategory::Shell,
        "read" | "read_file" | "readfile" | "view_file" => ChatToolCategory::Read,
        "edit"
        | "write"
        | "edit_file"
        | "write_file"
        | "apply_patch"
        | "multiedit"
        | "write_to_file"
        | "replace_file_content" => ChatToolCategory::Edit,
        "browser" | "navigate" | "list_tabs" | "open_tab" | "browser_snapshot" => {
            ChatToolCategory::Browser
        }
        _ => ChatToolCategory::Tool,
    };
    let input = &event.metadata["tool_input"];
    let basename = event
        .path
        .as_deref()
        .or_else(|| string(event, "file_path"))
        .or_else(|| {
            ["file_path", "path", "AbsolutePath", "FilePath"]
                .into_iter()
                .find_map(|field| input[field].as_str())
        })
        .and_then(|path| path.rsplit(['/', '\\']).next())
        .filter(|path| !matches!(*path, "." | ".."))
        .and_then(safe_label);
    let description = match category {
        ChatToolCategory::Read => {
            basename.map_or_else(|| "Read file".into(), |name| format!("Read {name}"))
        }
        ChatToolCategory::Edit => {
            basename.map_or_else(|| "Update files".into(), |name| format!("Update {name}"))
        }
        ChatToolCategory::Shell => input["description"]
            .as_str()
            .and_then(safe_label)
            .map(str::to_owned)
            .unwrap_or_else(|| {
                let executable = event
                    .command
                    .as_deref()
                    .and_then(|command| command.split_whitespace().next())
                    .and_then(|path| path.rsplit(['/', '\\']).next())
                    .filter(|name| {
                        name.len() <= 48
                            && name
                                .bytes()
                                .all(|ch| ch.is_ascii_alphanumeric() || b"_.-".contains(&ch))
                    });
                executable.map_or_else(|| "Run command".into(), |name| format!("Run {name}"))
            }),
        ChatToolCategory::Browser
            if normalized == "list_tabs" || input["action"] == "list_tabs" =>
        {
            "Inspect open tabs".into()
        }
        ChatToolCategory::Browser => "Inspect browser".into(),
        ChatToolCategory::Tool => format!("Run {tool}"),
    };
    (tool.into(), category, description)
}

#[cfg(test)]
#[path = "chat_turn_index_tests.rs"]
mod tests;
