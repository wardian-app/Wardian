//! Claude UUID ancestry is the membership proof. Encounter order, prose and
//! timestamps never join records. Unresolved edges only suppress completeness.
use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use wardian_core::models::chat::{
    AgentChatEvent, AgentChatEventKind, AgentChatRole, AgentChatStatus,
};

const MAX_LINKS: usize = 512;
const ID_BYTES: usize = 512;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Link {
    root: String,
    sidechain_linked: bool,
    #[serde(default)]
    parent_uuid: Option<String>,
    #[serde(default)]
    parent_observed: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct ClaudeState {
    session: Option<String>,
    links: BTreeMap<String, Link>,
    tools: BTreeMap<String, String>,
    roots: BTreeSet<String>,
    partial: BTreeSet<String>,
    saturated: bool,
    // A terminal row retires this chain's authority to retract membership.
    closed: bool,
    contradicted_root: Option<String>,
}

fn id(value: &Value) -> Option<&str> {
    value
        .as_str()
        .filter(|id| !id.is_empty() && id.len() <= ID_BYTES)
}

impl ClaudeState {
    fn uncertain(&mut self) {
        // An unresolved source record has no owning turn. Conservatively deny
        // full counts to existing roots without assigning that record to one.
        self.partial.extend(self.roots.iter().cloned());
    }

    /// Consume each native row once, including rows with no visible event.
    /// An exhausted continuation is deliberately incapable of granting proof.
    pub(crate) fn observe(&mut self, raw: &Value, mut event: Option<&mut AgentChatEvent>) {
        if let Some(event) = event.as_deref_mut() {
            super::clear_contradiction(event);
            if let Some(metadata) = event.metadata.as_object_mut() {
                metadata.remove("chat_turn_after_closed");
            }
        }
        let was_closed = self.closed;
        self.observe_record(raw, event.as_deref_mut());
        if let Some((event, root)) = event.zip(self.contradicted_root.as_deref()) {
            if !was_closed {
                super::stamp_contradiction(event, "claude_uuid_ancestry_conflict", root, None);
                event.metadata["chat_turn_membership_complete"] = json!(false);
                event.metadata["chat_turn_membership_partial"] = json!(true);
            }
        }
    }

    fn observe_record(&mut self, raw: &Value, event: Option<&mut AgentChatEvent>) {
        if let Some(session) = id(&raw["sessionId"]) {
            if self
                .session
                .as_deref()
                .is_some_and(|known| known != session)
            {
                *self = Self::default();
            }
            self.session = Some(session.into());
        }
        let uuid = id(&raw["uuid"]);
        let parent_id = id(&raw["parentUuid"]);
        let parent_observed = raw
            .get("parentUuid")
            .is_some_and(|value| value.is_null() || id(value).is_some());
        let is_sidechain = raw["isSidechain"] == true;
        let request_root = event
            .as_ref()
            .filter(|event| {
                !is_sidechain
                    && event.kind == AgentChatEventKind::Message
                    && event.role == Some(AgentChatRole::User)
                    && matches!(
                        event.metadata["input_origin"].as_str(),
                        Some("human_input" | "agent_input")
                    )
                    && matches!(
                        event.metadata["input_purpose"].as_str(),
                        None | Some("request")
                    )
            })
            .and_then(|event| uuid.and_then(|_| id(&event.metadata["request_root_id"])))
            .map(str::to_owned);
        if self.closed
            && request_root
                .as_ref()
                .is_none_or(|root| self.roots.contains(root))
            && (event.as_ref().is_some_and(|event| {
                ["request_root_id", "provider_turn_id"]
                    .into_iter()
                    .any(|key| {
                        id(&event.metadata[key]).is_some_and(|root| self.roots.contains(root))
                    })
            }) || [uuid, parent_id].into_iter().flatten().any(|uuid| {
                self.links
                    .get(uuid)
                    .is_some_and(|link| self.roots.contains(&link.root))
            }) || id(&raw["parent_tool_use_id"])
                .and_then(|tool| self.tools.get(tool))
                .is_some_and(|root| self.roots.contains(root)))
        {
            if let Some(event) = event {
                if let Some(metadata) = event.metadata.as_object_mut() {
                    metadata.remove("chat_turn_adapter");
                }
                event.metadata["chat_turn_after_closed"] = json!(true);
                event.metadata["chat_turn_membership_complete"] = json!(false);
                event.metadata["chat_turn_membership_partial"] = json!(false);
            }
            return;
        }
        if !self.closed {
            if let Some(known) = uuid.and_then(|uuid| self.links.get(uuid)) {
                // Absence of a parent field is a truncated observation. A new
                // explicit edge for this already proven UUID is a rewrite,
                // even when both parents resolve to the same request root.
                if (known.parent_observed
                    && parent_observed
                    && known.parent_uuid.as_deref() != parent_id)
                    || request_root
                        .as_deref()
                        .is_some_and(|root| root != known.root)
                {
                    self.contradicted_root
                        .get_or_insert_with(|| known.root.clone());
                    self.uncertain();
                    return;
                }
            }
        }
        if request_root
            .as_ref()
            .is_some_and(|root| !self.roots.contains(root))
        {
            // A real, independently identified prompt starts its own ancestry
            // root. Retiring older links bounds long-lived sources; late old
            // branches whose ancestry was retired receive no new proof.
            self.links.clear();
            self.tools.clear();
            self.roots.clear();
            self.partial.clear();
            self.saturated = false;
            self.closed = false;
            self.contradicted_root = None;
        }
        if self.saturated {
            return;
        }
        let parent = parent_id.and_then(|parent| self.links.get(parent)).cloned();
        let tool_root = id(&raw["parent_tool_use_id"])
            .and_then(|tool| self.tools.get(tool))
            .cloned();
        let link = if let Some(root) = request_root {
            Some(Link {
                root,
                sidechain_linked: false,
                parent_uuid: None,
                parent_observed: false,
            })
        } else if is_sidechain {
            match (tool_root, parent) {
                (Some(root), Some(parent)) if root != parent.root => None,
                (Some(root), _) => Some(Link {
                    root,
                    sidechain_linked: true,
                    parent_uuid: None,
                    parent_observed: false,
                }),
                (None, Some(parent)) if parent.sidechain_linked => Some(parent),
                _ => None,
            }
        } else {
            parent.filter(|parent| !parent.sidechain_linked)
        };
        let Some(link) = link else {
            // Known bookkeeping without a visible event neither contributes
            // work nor supplies a fabricated edge. Broken visible rows do.
            if event.is_some() || uuid.is_some() || parent_id.is_some() || is_sidechain {
                self.uncertain();
            }
            return;
        };
        let mut link = link;
        link.parent_uuid = parent_id.map(str::to_owned);
        link.parent_observed = parent_observed;
        if (self.links.len() >= MAX_LINKS
            && uuid.is_some_and(|uuid| !self.links.contains_key(uuid)))
            || (self.roots.len() >= MAX_LINKS && !self.roots.contains(&link.root))
        {
            self.uncertain();
            self.saturated = true;
            self.links.clear();
            self.tools.clear();
            return;
        }
        if let Some(uuid) = uuid {
            if let Some(known) = self.links.get(uuid) {
                if known.root != link.root || known.sidechain_linked != link.sidechain_linked {
                    if !self.closed {
                        self.contradicted_root
                            .get_or_insert_with(|| known.root.clone());
                    }
                    self.uncertain();
                    return;
                }
                // A replay without its edge cannot erase earlier evidence.
                if !link.parent_observed {
                    link.parent_uuid.clone_from(&known.parent_uuid);
                    link.parent_observed = known.parent_observed;
                }
            }
            self.links.insert(uuid.into(), link.clone());
        }
        self.roots.insert(link.root.clone());
        let mut work_count = 0;
        if let Some(content) = raw["message"]["content"].as_array() {
            for block in content {
                if matches!(block["type"].as_str(), Some("tool_use" | "tool_result")) {
                    work_count += 1;
                }
                if block["type"] == "tool_use" {
                    if let Some(tool) = id(&block["id"]) {
                        match self.tools.get(tool) {
                            Some(known) if known != &link.root => self.uncertain(),
                            Some(_) => {}
                            None if self.tools.len() < MAX_LINKS => {
                                self.tools.insert(tool.into(), link.root.clone());
                            }
                            None => self.uncertain(),
                        }
                    }
                }
            }
        }
        // The current legacy normalizer emits one event per native row. A row
        // containing several invocations or results cannot prove complete work.
        if work_count > 1 {
            self.partial.insert(link.root.clone());
        }
        let Some(event) = event else {
            return;
        };
        if id(&event.metadata["request_root_id"]).is_some_and(|known| known != link.root) {
            self.uncertain();
            return;
        }
        event.metadata["request_root_id"] = json!(link.root);
        event.metadata["provider_turn_id"] = json!(link.root);
        event.metadata["chat_turn_adapter"] = json!("claude_parent_chain_v1");
        let terminal = !is_sidechain
            && (raw["message"]["stop_reason"] == "end_turn"
                || (raw["type"] == "result" && event.kind == AgentChatEventKind::Status));
        let partial = self.partial.contains(&link.root);
        event.metadata["chat_turn_membership_complete"] = json!(terminal && !partial);
        event.metadata["chat_turn_membership_partial"] = json!(partial);
        if terminal {
            self.closed = true;
        }
        if terminal
            && event.kind == AgentChatEventKind::Message
            && event.role == Some(AgentChatRole::Assistant)
        {
            event.metadata["provider_phase"] = json!("final_answer");
            event.status = Some(AgentChatStatus::Succeeded);
        }
    }
}

#[cfg(test)]
#[path = "claude_tests.rs"]
mod tests;
