//! Source-owned turn membership. These adapters consume typed records before
//! clipping; presentation never reconstructs their evidence from visible text.
mod claude;
mod codex;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use wardian_core::models::chat::AgentChatEvent;

fn clear_contradiction(event: &mut AgentChatEvent) {
    if let Some(metadata) = event.metadata.as_object_mut() {
        metadata.remove("chat_turn_membership_contradiction");
        metadata.remove("chat_turn_membership_contradicted_root");
        metadata.remove("chat_turn_membership_expected_provider_turn");
    }
}

/// Emit only a fixed reason and previously certified identities. Source content
/// and newly encountered identities must never become diagnostic prose.
fn stamp_contradiction(
    event: &mut AgentChatEvent,
    kind: &'static str,
    root: &str,
    provider_turn: Option<&str>,
) {
    event.metadata["chat_turn_membership_contradiction"] = serde_json::json!(kind);
    event.metadata["chat_turn_membership_contradicted_root"] = serde_json::json!(root);
    if let Some(turn) = provider_turn {
        event.metadata["chat_turn_membership_expected_provider_turn"] = serde_json::json!(turn);
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct State {
    claude: claude::ClaudeState,
    codex: codex::CodexState,
}

impl State {
    /// The normalizer owns one state per native source and persists it with its
    /// existing acquisition cursor. Unsupported providers receive no capability.
    pub(crate) fn observe(
        &mut self,
        provider: &str,
        raw: &Value,
        event: Option<&mut AgentChatEvent>,
    ) {
        match provider {
            "claude" => self.claude.observe(raw, event),
            "codex" => self.codex.observe(raw, event),
            _ => {}
        }
    }
}
