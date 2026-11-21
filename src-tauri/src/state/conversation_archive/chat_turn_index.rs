//! Incremental canonical turn metadata in the existing immutable Store.
//! Admission, source qualification and publication remain with the archive owner.
//! No transcript bodies, raw records or delivery inference live here. Preview
//! hints are bounded metadata retained only by the existing in-memory owner.
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

/// Opaque, bounded ownership hints from the caller's exact immutable Head.
/// These retain metadata only and grant no canonical output/detail authority.
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub(super) struct PreviewOwners {
    scope: Option<String>,
    source: Option<(String, String)>,
    admission: Option<String>,
    rows: BTreeMap<String, PreviewOwner>,
    remembered: BTreeMap<String, Item>,
    members: BTreeMap<String, String>,
    /// Display state IDs link only to previously resolved physical owners.
    state_rows: BTreeMap<String, String>,
    /// Global opening order keyed by canonical turn_key, not request_root_id.
    pub(super) turn_orders: BTreeMap<String, String>,
    /// Current-Head opening proof is operation-local, never binding authority.
    #[serde(skip)]
    opening_roots: BTreeSet<String>,
}

#[derive(Clone, Serialize, Deserialize)]
struct PreviewOwner {
    item: Item,
    kind: ChatPresentedItemKind,
    observation: String,
    source_order: String,
    physical_members: Vec<String>,
}

impl PreviewOwners {
    /// These maps index the same retained source backing certificates. Count
    /// the largest logical set rather than duplicate JSON/index representations.
    pub(super) fn record_count(&self) -> usize {
        self.members
            .len()
            .max(self.remembered.len())
            .max(self.state_rows.len())
    }

    /// Roots certified by the same scoped Turn reads used to resolve this slice.
    pub(super) fn proven_openings(&self) -> &BTreeSet<String> {
        &self.opening_roots
    }

    /// Preserve admitted item order within the caller's fenced source admission.
    /// Canonical capture may choose a later call's order for an unpaired result;
    /// matching identity can retain view order without changing current evidence.
    pub(super) fn preserve_item_orders(&self, value: &mut ChatPresentationV1) {
        let Some((source_key, source_epoch)) = self.source.as_ref() else {
            return;
        };
        if self.scope.is_none()
            || self.admission.is_none()
            || self.remembered.len() > CHAT_PRESENTATION_MAX_ITEMS
            || value.items.len() > CHAT_PRESENTATION_MAX_ITEMS
            || value.preview_bridge.as_ref().is_some_and(|bridge| {
                self.admission.as_deref() != Some(bridge.admission_id.as_str())
            })
        {
            return;
        }
        for item in &mut value.items {
            let Some(previous) = self.remembered.get(&item.item_key) else {
                continue;
            };
            if previous.membership.item_key != item.item_key
                || previous.membership.turn_key != item.turn_key
                || item
                    .admission_id
                    .as_ref()
                    .is_some_and(|admission| Some(admission) != self.admission.as_ref())
            {
                continue;
            }
            if matches!(
                (&previous.evidence, &item.evidence),
                (
                    ChatEvidenceIdentity::Source { source_key: old_key, source_epoch: old_epoch, .. },
                    ChatEvidenceIdentity::Source { source_key: current_key, source_epoch: current_epoch, .. }
                ) if old_key == source_key && current_key == source_key
                    && old_epoch == source_epoch && current_epoch == source_epoch
            ) {
                item.order_key.clone_from(&previous.order_key);
            }
        }
    }

    /// Retain identity metadata for only this admitted presentation. Transcript
    /// text, previews, labels, invocation/result records and detail refs are absent.
    pub(super) fn from_presentation(
        scope: &Scope<'_>,
        admission: &str,
        value: &ChatPresentationV1,
    ) -> io::Result<Option<Self>> {
        let Some((source_key, source_epoch)) = scope.source_key.zip(scope.source_epoch) else {
            return Ok(None);
        };
        if ![
            scope.agent_id,
            scope.conversation_id,
            source_key,
            source_epoch,
            admission,
        ]
        .into_iter()
        .all(bounded_id)
            || value.validate().is_err()
            || value
                .preview_bridge
                .as_ref()
                .is_some_and(|bridge| bridge.admission_id != admission)
        {
            return Ok(None);
        }
        let mut saved = Self {
            scope: Some(owner_identity(scope)?),
            source: Some((source_key.into(), source_epoch.into())),
            admission: Some(admission.into()),
            ..Self::default()
        };
        for turn in &value.turns {
            if !bounded_id(&turn.turn_key) || turn.order_key.len() > 2 * ID_BYTES {
                return Ok(None);
            }
            saved
                .turn_orders
                .insert(turn.turn_key.clone(), turn.order_key.clone());
        }
        for item in &value.items {
            let ChatEvidenceIdentity::Source {
                source_key: key,
                source_epoch: epoch,
                record_key,
            } = &item.evidence
            else {
                return Ok(None);
            };
            if key != source_key
                || epoch != source_epoch
                || !bounded_id(record_key)
                || !bounded_id(&item.item_key)
                || !bounded_id(&item.turn_key)
                || item.order_key.len() > 2 * ID_BYTES
                || item.provenance == ChatItemProvenance::Pending
                || (item.provenance == ChatItemProvenance::Provisional
                    && item.admission_id.as_deref() != Some(admission))
            {
                return Ok(None);
            }
            let owner = Item {
                membership: Membership {
                    item_key: item.item_key.clone(),
                    turn_key: item.turn_key.clone(),
                },
                order_key: item.order_key.clone(),
                kind: item.kind.clone(),
                evidence: item.evidence.clone(),
                step: None,
                state_only: false,
                legacy_only: false,
            };
            if saved
                .members
                .insert(record_key.clone(), item.item_key.clone())
                .is_some_and(|known| known != item.item_key)
            {
                return Ok(None);
            }
            saved.remembered.insert(item.item_key.clone(), owner);
        }
        Ok(Some(saved))
    }

    /// Carry this slice's certified component aliases forward, bounded by the
    /// current admitted items rather than growing a session-wide identity cache.
    pub(super) fn remember(
        &self,
        scope: &Scope<'_>,
        admission: &str,
        value: &ChatPresentationV1,
    ) -> io::Result<Option<Self>> {
        let Some(mut saved) = Self::from_presentation(scope, admission, value)? else {
            return Ok(None);
        };
        if self
            .scope
            .as_ref()
            .is_some_and(|owner| Some(owner) != saved.scope.as_ref())
            || self
                .source
                .as_ref()
                .is_some_and(|source| Some(source) != saved.source.as_ref())
            || self
                .admission
                .as_deref()
                .is_some_and(|known| known != admission)
        {
            return Ok(None);
        }
        for reference in &value.state_event_refs {
            let Some(item) = self
                .rows
                .get(&reference.event_id)
                .map(|hint| &hint.item)
                .or_else(|| {
                    self.state_rows
                        .get(&reference.event_id)
                        .and_then(|key| self.remembered.get(key))
                })
            else {
                // The DTO has no physical state evidence to reconstruct here.
                continue;
            };
            if !item.state_only
                || item.legacy_only
                || item.membership.turn_key != reference.turn_key
                || saved
                    .remembered
                    .get(&item.membership.item_key)
                    .is_some_and(|known| {
                        !known.state_only
                            || known.membership != item.membership
                            || known.order_key != item.order_key
                            || known.evidence != item.evidence
                    })
            {
                return Ok(None);
            }
            let mut owner = item.clone();
            owner.step = None;
            saved
                .state_rows
                .insert(reference.event_id.clone(), item.membership.item_key.clone());
            saved
                .remembered
                .insert(item.membership.item_key.clone(), owner);
        }
        let updated = self
            .rows
            .values()
            .map(|hint| hint.item.membership.item_key.as_str())
            .collect::<BTreeSet<_>>();
        for (physical, item_key) in &self.members {
            if !updated.contains(item_key.as_str())
                && saved.remembered.contains_key(item_key)
                && saved
                    .members
                    .insert(physical.clone(), item_key.clone())
                    .is_some_and(|known| known != *item_key)
            {
                return Ok(None);
            }
        }
        for hint in self.rows.values() {
            let item_key = &hint.item.membership.item_key;
            if !saved.remembered.contains_key(item_key) {
                continue;
            }
            for physical in &hint.physical_members {
                if saved
                    .members
                    .insert(physical.clone(), item_key.clone())
                    .is_some_and(|known| known != *item_key)
                {
                    return Ok(None);
                }
            }
        }
        if saved.remembered.len() > CHAT_PRESENTATION_MAX_ITEMS
            || saved.members.len() > 3 * CHAT_PRESENTATION_MAX_ITEMS
            || saved.state_rows.len() > CHAT_PRESENTATION_MAX_ITEMS
        {
            return Ok(None);
        }
        Ok(Some(saved))
    }
}

struct Component {
    item: Item,
    kind: ChatPresentedItemKind,
    versions: BTreeSet<String>,
}

struct PageMembers {
    members: Vec<Membership>,
    source_order: Option<String>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    closed_order: Option<String>,
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
            closed_order: None,
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

    fn closed_source_order(&self) -> Option<&str> {
        (self.eligible()
            && self.membership_complete()
            && matches!(
                self.status,
                ChatTurnStatus::Completed | ChatTurnStatus::Failed | ChatTurnStatus::Cancelled
            ))
        .then(|| {
            self.closed_order
                .as_deref()
                .or(self.status_order.as_deref())
        })
        .flatten()
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
        let Some(Identity {
            owner,
            source,
            turn_key,
            item_key: proposed,
            physical,
            call,
        }) = identity(scope, event, root, &kind, self.scope.as_deref())?
        else {
            return Ok(None);
        };
        let mut cache = SeekCache::default();
        let mut membership = None;
        let mut first_source_order: Option<String> = None;
        let mut physical_seen = false;
        let mut event_seen = false;
        for id in [event.id.as_str(), physical] {
            if let Some(known) =
                lookup::<Observation>(store, &self.events, &digest(id.as_bytes()), &mut cache)?
            {
                physical_seen |= id == physical;
                event_seen |= id == event.id;
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
        // Closed replays retain metadata, but cannot rebind an existing physical
        // record between prompt, step and narrative identities.
        if old.as_ref().is_some_and(|item| {
            item.membership != membership
                || (!legacy_only
                    && !item.legacy_only
                    && ((item.kind == ChatPresentedItemKind::Step)
                        != (kind == ChatPresentedItemKind::Step)
                        || (item.kind == ChatPresentedItemKind::Prompt)
                            != (kind == ChatPresentedItemKind::Prompt)))
        }) {
            return Err(io::Error::other("canonical turn item changed identity"));
        }
        let prior_turn: Option<Turn> = lookup(store, &self.turns, &turn_key, &mut cache)?;
        if let Some(closed_order) = prior_turn.as_ref().and_then(Turn::closed_source_order) {
            if physical_seen {
                let member = membership;
                // Re-identification may add an event alias, but a replay cannot
                // rewrite a closed turn's item metadata, counters or status.
                if !event_seen {
                    let reference = store.put(&Observation {
                        membership: member.clone(),
                        source_order: first_source_order.unwrap_or_else(|| order_key.into()),
                    })?;
                    let mut next = self.clone();
                    next.events = Some(store.insert(
                        &next.events,
                        &digest(event.id.as_bytes()),
                        &reference,
                    )?);
                    *self = next;
                }
                return Ok(Some(member.item_key));
            }
            if source_order_after(order_key, closed_order) {
                return Ok(None);
            }
        }
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
        let mut turn = prior_turn.unwrap_or_default();
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
            if receipt.complete() && !receipt.partial() && turn.closed_order.is_none() {
                turn.closed_order = Some(order.clone());
            }
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
        if turn.closed_order.is_none() && turn.closed_source_order().is_some() {
            turn.closed_order =
                if adapter_proven && event.metadata["chat_turn_membership_complete"] == true {
                    Some(source_order.into())
                } else {
                    turn.status_order.clone()
                };
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
        self.page_inner(store, events, complete, false)
    }

    /// A bound canonical view retains its lifetime while paging into legacy
    /// history. This grants no turn membership to those legacy rows.
    pub(super) fn continuation_page(
        &self,
        store: &mut Store,
        events: &[AgentChatEvent],
    ) -> io::Result<Option<ChatPresentationV1>> {
        self.page_inner(store, events, false, true)
    }

    fn page_inner(
        &self,
        store: &mut Store,
        events: &[AgentChatEvent],
        complete: bool,
        allow_legacy_only: bool,
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
            let Some(PageMembers { members, .. }) = self.page_members(store, event, &mut cache)?
            else {
                bindings.push(None);
                continue;
            };
            let Some(Component {
                item,
                kind: logical_kind,
                versions,
            }) = self.component(store, members, &mut cache, &mut loaded)?
            else {
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
                .or_insert_with(|| logical_kind.clone());
            if logical_kind == ChatPresentedItemKind::Answer {
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
                prefix_attributed: false,
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
        if presented_turns.is_empty() && !allow_legacy_only {
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
            preview_bridge: None,
        };
        if page.validate().is_err()
            || serde_json::to_vec(&page).map_err(io::Error::other)?.len()
                > CHAT_PRESENTATION_MAX_BYTES
        {
            return Ok(None);
        }
        Ok(Some(page))
    }

    /// Return only requested roots' proven terminal source boundaries, reusing
    /// hash-validated immutable seek nodes from the caller's operation-local
    /// cache. Store read limits and source-boundary qualification are unchanged.
    pub(super) fn closed_roots_reusing_nodes(
        &self,
        store: &mut Store,
        scope: &Scope<'_>,
        roots: &BTreeSet<String>,
        cache: &mut SeekCache,
    ) -> io::Result<BTreeMap<String, String>> {
        if roots.len() > CHAT_PRESENTATION_MAX_ITEMS {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "closed root budget exceeded",
            ));
        }
        let mut closed = BTreeMap::new();
        if self.version != 1 {
            return Ok(closed);
        }
        let owner = owner_identity(scope)?;
        if self.scope.as_ref().is_some_and(|known| known != &owner) {
            return Err(io::Error::other("foreign canonical turn index scope"));
        }
        for root in roots.iter().filter(|root| bounded_id(root)) {
            let (_, turn_key) = turn_identity(&owner, scope.source_key, scope.source_epoch, root)?;
            if let Some(turn) = lookup::<Turn>(store, &self.turns, &turn_key, cache)? {
                if let Some(order) = turn.closed_source_order() {
                    closed.insert(root.clone(), order.into());
                }
            }
        }
        Ok(closed)
    }

    /// Compare physical ranges rather than mutable normalizer sequences or
    /// projection IDs. A closing frame itself is never after its own boundary.
    pub(super) fn after_closed_source_order(event: &AgentChatEvent, order: &str) -> bool {
        source_position(order)
            .zip(
                event.metadata["chat_source_start"]
                    .as_u64()
                    .zip(event.metadata["chat_source_end"].as_u64()),
            )
            .is_some_and(|(closed, observed)| observed > closed)
    }

    /// Reuse only a previous producer-certified owner from this same source
    /// admission when a newly coalesced physical mirror has not been captured.
    /// The caller's operation-local cache reuses hash-validated immutable seek
    /// nodes across turn lookups while preserving the Store's hard read limits.
    pub(super) fn preview_owners_reusing_nodes(
        &self,
        store: &mut Store,
        scope: &Scope<'_>,
        events: &[AgentChatEvent],
        previous: Option<&PreviewOwners>,
        cache: &mut SeekCache,
    ) -> io::Result<Option<PreviewOwners>> {
        if events.len() > CHAT_PRESENTATION_MAX_ITEMS || (self.version != 1 && !self.is_empty()) {
            return Ok(None);
        }
        let owner = owner_identity(scope)?;
        if self.scope.as_ref().is_some_and(|known| known != &owner) {
            return Err(io::Error::other("foreign canonical turn index scope"));
        }
        let source = scope
            .source_key
            .zip(scope.source_epoch)
            .map(|(key, epoch)| (key.to_owned(), epoch.to_owned()));
        if previous.is_some_and(|previous| {
            previous.scope.as_ref() != Some(&owner)
                || previous.source != source
                || previous.admission.is_none()
                || previous.remembered.len() > CHAT_PRESENTATION_MAX_ITEMS
                || previous.members.len() > 3 * CHAT_PRESENTATION_MAX_ITEMS
                || previous.state_rows.len() > CHAT_PRESENTATION_MAX_ITEMS
                || previous.turn_orders.len() > CHAT_PRESENTATION_MAX_ITEMS
        }) {
            return Ok(None);
        }
        let mut owners = PreviewOwners {
            scope: Some(owner),
            source,
            admission: previous.and_then(|previous| previous.admission.clone()),
            rows: BTreeMap::new(),
            members: previous.map_or_else(BTreeMap::new, |previous| previous.members.clone()),
            ..PreviewOwners::default()
        };
        let mut loaded = BTreeMap::new();
        let mut root_turns = BTreeMap::<String, Option<Turn>>::new();
        for event in events {
            let Some(observation) = preview_observation(event)? else {
                return Ok(None);
            };
            let Some(kind) = item_kind(event) else {
                return Ok(None);
            };
            let root = string(event, "request_root_id").expect("qualified observation");
            let Some(derived) = identity(scope, event, root, &kind, owners.scope.as_deref())?
            else {
                return Ok(None);
            };
            if !root_turns.contains_key(&derived.turn_key) {
                let turn = lookup::<Turn>(store, &self.turns, &derived.turn_key, cache)?;
                root_turns.insert(derived.turn_key.clone(), turn);
            }
            let turn = root_turns.get(&derived.turn_key).and_then(Option::as_ref);
            if turn.is_some_and(|turn| {
                turn.active && turn.supported && turn.adapter_proven && !turn.invalid_proof
            }) {
                owners.opening_roots.insert(root.into());
            }
            let certified_before = previous
                .is_some_and(|previous| previous.turn_orders.contains_key(&derived.turn_key));
            let stored_order = turn
                .filter(|turn| turn.eligible() && !turn.invalid_proof)
                .and_then(|turn| turn.order_key.as_ref())
                .or_else(|| {
                    previous.and_then(|previous| previous.turn_orders.get(&derived.turn_key))
                });
            if let Some(order) = stored_order {
                owners
                    .turn_orders
                    .insert(derived.turn_key.clone(), order.clone());
            }
            let members = self.page_members(store, event, cache)?;
            let mut source_order = members
                .as_ref()
                .and_then(|members| members.source_order.clone());
            let (component, reused) = if let Some(PageMembers { members, .. }) = members {
                let Some(component) = self.component(store, members, cache, &mut loaded)? else {
                    return Ok(None);
                };
                (Some(component), false)
            } else {
                let component = if event.metadata.get("chat_display_member_ids").is_some() {
                    None
                } else {
                    lookup::<Item>(store, &self.items, &derived.item_key, cache)?.map(|item| {
                        Component {
                            kind: item.kind.clone(),
                            item,
                            versions: BTreeSet::new(),
                        }
                    })
                };
                if component.is_some() {
                    (component, false)
                } else if let Some(previous) = previous {
                    (
                        self.remembered_component(
                            store,
                            scope,
                            event,
                            previous,
                            cache,
                            &mut loaded,
                        )?,
                        true,
                    )
                } else {
                    (None, false)
                }
            };
            let Some(component) = component else {
                if event.metadata.get("chat_display_member_ids").is_some()
                    || turn.is_some_and(|turn| !turn.supported || turn.invalid_proof)
                {
                    return Ok(None);
                }
                continue;
            };
            // A newly unsupported extension cannot retract exact old members,
            // or use that exception to admit a new physical mirror/component.
            let certified_member = previous.is_some_and(|previous| {
                let item_key = &component.item.membership.item_key;
                previous.members.get(derived.physical) == Some(item_key)
                    && physical_members(event)
                        .iter()
                        .all(|physical| previous.members.get(physical) == Some(item_key))
            });
            let expected_source = ChatEvidenceIdentity::Source {
                source_key: scope.source_key.unwrap_or(&derived.source).into(),
                source_epoch: scope.source_epoch.unwrap_or(&derived.source).into(),
                record_key: match &component.item.evidence {
                    ChatEvidenceIdentity::Source { record_key, .. } => record_key.clone(),
                    _ => return Ok(None),
                },
            };
            if component.item.membership.turn_key != derived.turn_key
                || component.item.evidence != expected_source
                || component.item.legacy_only
                || !compatible_component_kinds(&component.kind, &kind)
                || (matches!(
                    kind,
                    ChatPresentedItemKind::Prompt | ChatPresentedItemKind::Step
                ) && component.item.membership.item_key != derived.item_key)
                || turn.is_some_and(|turn| {
                    ((!turn.supported || turn.invalid_proof) && !certified_member)
                        || (!turn.eligible() && !certified_before)
                })
                || (turn.is_none() && !reused)
            {
                return Ok(None);
            }
            // An authenticated singleton Row already loaded its owning Item.
            // Reuse native order only when that persisted owner proves the same
            // recipe; old orders, aliases and re-identifications still seek it.
            if source_order.is_none()
                && event.metadata["_wardian_canonical_row"] == true
                && event.metadata.get("chat_display_member_ids").is_none()
                && event.id == derived.physical
                && scope.source_key.is_some()
                && scope.source_epoch.is_some()
                && string(event, "chat_source_epoch") == scope.source_epoch
                && native_member_start(scope, derived.physical).is_some_and(|start| {
                    event.metadata["chat_source_start"].as_u64() == Some(start)
                })
                && string(event, "chat_turn_key")
                    == Some(component.item.membership.turn_key.as_str())
                && string(event, "chat_turn_item_key")
                    == Some(component.item.membership.item_key.as_str())
                && matches!(&component.item.evidence,
                    ChatEvidenceIdentity::Source { record_key, .. } if record_key == derived.physical)
            {
                source_order = super::chat_read::presentation_order(event).filter(|order| {
                    component.item.order_key
                        == format!("{order}:{}", component.item.membership.item_key)
                });
            }
            if source_order.is_none() {
                for id in [event.id.as_str(), derived.physical] {
                    if let Some(known) =
                        lookup::<Observation>(store, &self.events, &digest(id.as_bytes()), cache)?
                    {
                        if !known.source_order.is_empty() {
                            source_order = Some(known.source_order);
                            break;
                        }
                    }
                }
            }
            let Some(source_order) =
                source_order.or_else(|| super::chat_read::presentation_order(event))
            else {
                return Ok(None);
            };
            if owners
                .rows
                .insert(
                    event.id.clone(),
                    PreviewOwner {
                        item: component.item,
                        kind: component.kind,
                        observation,
                        source_order,
                        physical_members: physical_members(event),
                    },
                )
                .is_some()
            {
                return Ok(None);
            }
        }
        Ok(Some(owners))
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

    fn remembered_component(
        &self,
        store: &mut Store,
        scope: &Scope<'_>,
        event: &AgentChatEvent,
        previous: &PreviewOwners,
        cache: &mut SeekCache,
        loaded: &mut BTreeMap<String, (String, Item)>,
    ) -> io::Result<Option<Component>> {
        let members = physical_members(event);
        let mut remembered: Option<&Item> = None;
        let mut known = Vec::new();
        for id in &members {
            if let Some(key) = previous.members.get(id) {
                if let Some(candidate) = previous.remembered.get(key) {
                    if remembered.is_some_and(|prior| {
                        prior.membership.turn_key != candidate.membership.turn_key
                            || !compatible_component_kinds(&prior.kind, &candidate.kind)
                    }) {
                        return Ok(None);
                    }
                    if remembered.is_none_or(|prior| {
                        (&candidate.order_key, &candidate.membership.item_key)
                            < (&prior.order_key, &prior.membership.item_key)
                    }) {
                        remembered = Some(candidate);
                    }
                }
            }
            if let Some(member) =
                lookup::<Membership>(store, &self.events, &digest(id.as_bytes()), cache)?
            {
                known.push(member);
            }
        }
        let Some(owner) = remembered else {
            return Ok(None);
        };
        let Some((owner_start, _)) = source_position(&owner.order_key) else {
            return Ok(None);
        };
        for id in &members {
            if previous.members.get(id) == Some(&owner.membership.item_key) {
                continue;
            }
            // The new representative has exact coordinates from acquisition.
            // Other unknown members require the native source-ref parser; text
            // order and normalizer sequence never establish precedence.
            let start = if let Some(known) = previous
                .members
                .get(id)
                .and_then(|key| previous.remembered.get(key))
            {
                source_position(&known.order_key).map(|(start, _)| start)
            } else if string(event, "chat_source_ref") == Some(id.as_str()) {
                event.metadata["chat_source_start"].as_u64()
            } else {
                native_member_start(scope, id)
            };
            if start.is_none_or(|start| start <= owner_start) {
                return Ok(None);
            }
        }
        let canonical = if known.is_empty() {
            None
        } else {
            let Some(component) = self.component(store, known, cache, loaded)? else {
                return Ok(None);
            };
            if component.item.membership.turn_key != owner.membership.turn_key
                || !compatible_component_kinds(&component.kind, &owner.kind)
                || (component.item.membership.item_key != owner.membership.item_key
                    && source_position(&component.item.order_key)
                        .is_none_or(|(start, _)| start <= owner_start))
            {
                return Ok(None);
            }
            Some(component)
        };
        let current_kind = item_kind(event);
        if current_kind
            .as_ref()
            .is_none_or(|kind| !compatible_component_kinds(kind, &owner.kind))
        {
            return Ok(None);
        }
        let kind = if current_kind == Some(ChatPresentedItemKind::Answer)
            || canonical
                .as_ref()
                .is_some_and(|component| component.kind == ChatPresentedItemKind::Answer)
        {
            ChatPresentedItemKind::Answer
        } else {
            owner.kind.clone()
        };
        Ok(Some(Component {
            item: owner.clone(),
            kind,
            versions: BTreeSet::new(),
        }))
    }

    fn component(
        &self,
        store: &mut Store,
        members: Vec<Membership>,
        cache: &mut SeekCache,
        loaded: &mut BTreeMap<String, (String, Item)>,
    ) -> io::Result<Option<Component>> {
        let mut selected: Option<Item> = None;
        let mut kind = None;
        let mut versions = BTreeSet::new();
        for member in members {
            if !loaded.contains_key(&member.item_key) {
                let Some(reference) =
                    store.get_reusing_nodes(&self.items, &member.item_key, cache)?
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
                || selected.as_ref().is_some_and(|prior| {
                    prior.membership.turn_key != member.turn_key
                        || !compatible_component_kinds(&prior.kind, &item.kind)
                })
            {
                return Ok(None);
            }
            // Canonical and provisional display rows choose the same earliest
            // persisted owner while explicit final evidence enriches its kind.
            if kind.is_none() || item.kind == ChatPresentedItemKind::Answer {
                kind = Some(item.kind.clone());
            }
            if selected.as_ref().is_none_or(|prior| {
                (&item.order_key, &item.membership.item_key)
                    < (&prior.order_key, &prior.membership.item_key)
            }) {
                selected = Some(item.clone());
            }
        }
        Ok(selected.zip(kind).map(|(item, kind)| Component {
            item,
            kind,
            versions,
        }))
    }

    fn page_members(
        &self,
        store: &mut Store,
        event: &AgentChatEvent,
        cache: &mut SeekCache,
    ) -> io::Result<Option<PageMembers>> {
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
                return Ok(Some(PageMembers {
                    members: vec![Membership {
                        turn_key: turn_key.into(),
                        item_key: item_key.into(),
                    }],
                    source_order: None,
                }));
            }
            for id in [Some(event.id.as_str()), string(event, "chat_source_ref")]
                .into_iter()
                .flatten()
            {
                if let Some(observation) =
                    lookup::<Observation>(store, &self.events, &digest(id.as_bytes()), cache)?
                {
                    return Ok(Some(PageMembers {
                        members: vec![observation.membership],
                        source_order: (!observation.source_order.is_empty())
                            .then_some(observation.source_order),
                    }));
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
        // Logical aliases retain their existing display-row order lookup.
        Ok(Some(PageMembers {
            members,
            source_order: None,
        }))
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

struct Identity<'a> {
    owner: String,
    source: String,
    turn_key: String,
    item_key: String,
    physical: &'a str,
    call: Option<&'a str>,
}

/// Bind opaque ownership hints to the exact displayed physical/component row,
/// excluding mutable normalization sequence and display text from identity.
fn physical_members(event: &AgentChatEvent) -> Vec<String> {
    event.metadata["chat_display_member_ids"]
        .as_array()
        .map(|members| {
            members
                .iter()
                .filter_map(|id| id.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_else(|| {
            string(event, "chat_source_ref")
                .map(str::to_owned)
                .into_iter()
                .collect()
        })
}

fn source_position(order: &str) -> Option<(u64, u64)> {
    let mut fields = order.strip_prefix("source:")?.split(':');
    let start = fields.next()?.parse().ok()?;
    let end = fields.next()?.parse().ok()?;
    (start < end).then_some((start, end))
}

fn source_order_after(observed: &str, closed: &str) -> bool {
    source_position(observed)
        .zip(source_position(closed))
        .map_or_else(|| observed > closed, |(observed, closed)| observed > closed)
}

fn native_member_start(scope: &Scope<'_>, id: &str) -> Option<u64> {
    if !super::chat_source_index::valid_source_id(scope.agent_id, id) {
        return None;
    }
    let prefix = format!("source:{}:", scope.agent_id);
    let mut fields = id.strip_prefix(&prefix)?.split(':');
    if Some(fields.next()?) != scope.source_epoch {
        return None;
    }
    fields.next()?.parse().ok()
}

fn preview_observation(event: &AgentChatEvent) -> io::Result<Option<String>> {
    let Some(root) = string(event, "request_root_id").filter(|root| bounded_id(root)) else {
        return Ok(None);
    };
    let Some(physical) = string(event, "chat_source_ref").filter(|id| bounded_id(id)) else {
        return Ok(None);
    };
    if !bounded_id(&event.id) {
        return Ok(None);
    }
    if event
        .metadata
        .get("chat_display_physical_id")
        .is_some_and(|id| id.as_str() != Some(physical))
    {
        return Ok(None);
    }
    if let Some(members) = event.metadata.get("chat_display_member_ids") {
        if !members.as_array().is_some_and(|members| {
            !members.is_empty()
                && members.len() <= 2
                && members.iter().all(|id| id.as_str().is_some_and(bounded_id))
                && members.iter().any(|id| id.as_str() == Some(physical))
        }) {
            return Ok(None);
        }
    } else if event.metadata.get("chat_display_physical_id").is_some() {
        return Ok(None);
    }
    key(&(
        "preview-owner-v1",
        &event.id,
        root,
        physical,
        &event.metadata["chat_display_member_ids"],
        call_identity(event),
    ))
    .map(Some)
}

/// Keep physical and invocation identity identical across canonical observation
/// and bounded previews. Qualification remains with each caller; this retains
/// the canonical event-ID fallback and foreign-scope error ordering unchanged.
fn identity<'a>(
    scope: &Scope<'_>,
    event: &'a AgentChatEvent,
    root: &str,
    kind: &ChatPresentedItemKind,
    known_owner: Option<&str>,
) -> io::Result<Option<Identity<'a>>> {
    let owner = owner_identity(scope)?;
    if known_owner.is_some_and(|known| known != owner) {
        return Err(io::Error::other("foreign canonical turn index scope"));
    }
    let (source, turn_key) = turn_identity(&owner, scope.source_key, scope.source_epoch, root)?;
    let call = (*kind == ChatPresentedItemKind::Step)
        .then(|| call_identity(event))
        .flatten();
    let physical = string(event, "chat_source_ref").unwrap_or(&event.id);
    if !bounded_id(physical) {
        return Ok(None);
    }
    let item_key = if *kind == ChatPresentedItemKind::Prompt {
        key(&("prompt-v1", &turn_key))?
    } else if let Some(call) = call {
        key(&("step-v1", &turn_key, call))?
    } else {
        key(&("item-v1", &turn_key, physical))?
    };
    Ok(Some(Identity {
        owner,
        source,
        turn_key,
        item_key,
        physical,
        call,
    }))
}

fn owner_identity(scope: &Scope<'_>) -> io::Result<String> {
    key(&("turn-owner-v1", scope.agent_id, scope.conversation_id))
}

fn turn_identity(
    owner: &str,
    source_key: Option<&str>,
    source_epoch: Option<&str>,
    root: &str,
) -> io::Result<(String, String)> {
    let source = source_identity(owner, source_key, source_epoch)?;
    let turn_key = key(&("turn-v1", &source, root))?;
    Ok((source, turn_key))
}

fn source_identity(
    owner: &str,
    source_key: Option<&str>,
    source_epoch: Option<&str>,
) -> io::Result<String> {
    key(&("turn-source-v1", owner, source_key, source_epoch))
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

#[path = "chat_turn_preview.rs"]
mod chat_turn_preview;
pub(super) use chat_turn_preview::preview;
