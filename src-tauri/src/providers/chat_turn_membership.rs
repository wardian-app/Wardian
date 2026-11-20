//! Source-owned turn membership. These adapters consume typed records before
//! clipping; presentation never reconstructs their evidence from visible text.
mod claude;
mod codex;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use wardian_core::models::chat::AgentChatEvent;

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
