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
    pub(crate) fn observe(&mut self, raw: &Value, event: Option<&mut AgentChatEvent>) {
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
        }
        if self.saturated {
            return;
        }
        let parent_id = id(&raw["parentUuid"]);
        let parent = parent_id.and_then(|parent| self.links.get(parent)).cloned();
        let tool_root = id(&raw["parent_tool_use_id"])
            .and_then(|tool| self.tools.get(tool))
            .cloned();
        let link = if let Some(root) = request_root {
            Some(Link {
                root,
                sidechain_linked: false,
            })
        } else if is_sidechain {
            match (tool_root, parent) {
                (Some(root), Some(parent)) if root != parent.root => None,
                (Some(root), _) => Some(Link {
                    root,
                    sidechain_linked: true,
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
        if self.links.len() >= MAX_LINKS
            || self.tools.len() >= MAX_LINKS
            || self.roots.len() >= MAX_LINKS
        {
            self.uncertain();
            self.saturated = true;
            self.links.clear();
            self.tools.clear();
            return;
        }
        if let Some(uuid) = uuid {
            if self.links.get(uuid).is_some_and(|known| known != &link) {
                self.uncertain();
                return;
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
                        if self
                            .tools
                            .get(tool)
                            .is_some_and(|known| known != &link.root)
                        {
                            self.uncertain();
                        } else if self.tools.len() < MAX_LINKS {
                            self.tools.insert(tool.into(), link.root.clone());
                        } else {
                            self.uncertain();
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
