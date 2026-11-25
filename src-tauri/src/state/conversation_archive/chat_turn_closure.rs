//! Capture-owned lifecycle evidence enriches an existing request without
//! creating a narrative row. Generic provider/Watch metadata cannot mint it.
use std::io;

use serde::{Deserialize, Serialize};
use wardian_core::models::chat::{
    AgentChatEvent, AgentChatEventKind, AgentChatRole, AgentChatStatus,
};

use super::{CapturePreparation, ConversationArchiveContext};

pub(super) const METADATA_KEY: &str = "_wardian_turn_closure";

#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub(super) struct Receipt {
    version: u8,
    source_key: String,
    source_epoch: String,
    provider: String,
    adapter: String,
    provider_turn_id: String,
    request_root_id: String,
    source_ref: String,
    start: u64,
    end: u64,
    sequence: u64,
    status: AgentChatStatus,
    complete: bool,
    partial: bool,
}

fn text<'a>(event: &'a AgentChatEvent, key: &str) -> Option<&'a str> {
    event.metadata[key]
        .as_str()
        .filter(|value| !value.is_empty() && value.len() <= 512)
}

/// Only a current native acquisition receipt may carry closing adapter facts
/// past the archive's deliberate omission of Status from narrative records.
pub(super) fn prepare(
    context: &ConversationArchiveContext,
    events: &[AgentChatEvent],
    capture: Option<&CapturePreparation<'_>>,
) -> io::Result<Vec<Receipt>> {
    let Some(capture) = capture else {
        return Ok(Vec::new());
    };
    let Some(proof) = capture.source_proof else {
        return Ok(Vec::new());
    };
    if context.provider_source_key.as_deref() != Some(&capture.next.provider_source_key)
        || !proof.matches_capture(capture.previous, capture.next)?
    {
        return Ok(Vec::new());
    }
    let epoch = crate::commands::chat_recent_seed::hash(
        &serde_json::to_vec(&capture.next.native_identity).map_err(io::Error::other)?,
    );
    Ok(events
        .iter()
        .filter_map(|event| {
            let adapter = text(event, "chat_turn_adapter")?;
            if event.session_id != context.agent_id
                || event.kind != AgentChatEventKind::Status
                || !matches!(
                    (event.provider.as_str(), adapter),
                    ("codex", "codex_bracket_v1") | ("claude", "claude_parent_chain_v1")
                )
                || !proof.proves(event)
                || capture
                    .state
                    .should_skip_event(event, Some(&capture.next.provider_source_key))
                || text(event, "chat_source_epoch") != Some(epoch.as_str())
            {
                return None;
            }
            let status = event.status.clone()?;
            if !matches!(
                status,
                AgentChatStatus::Succeeded | AgentChatStatus::Failed | AgentChatStatus::Cancelled
            ) {
                return None;
            }
            Some(Receipt {
                version: 1,
                source_key: capture.next.provider_source_key.clone(),
                source_epoch: epoch.clone(),
                provider: event.provider.clone(),
                adapter: adapter.into(),
                provider_turn_id: text(event, "provider_turn_id")?.into(),
                request_root_id: text(event, "request_root_id")?.into(),
                source_ref: text(event, "chat_source_ref")?.into(),
                start: event.metadata["chat_source_start"].as_u64()?,
                end: event.metadata["chat_source_end"].as_u64()?,
                sequence: event.sequence.unwrap_or(0),
                status,
                complete: event.metadata["chat_turn_membership_complete"] == true,
                partial: event.metadata["chat_turn_membership_partial"] == true,
            })
        })
        .collect())
}

impl Receipt {
    fn matches_request(&self, event: &AgentChatEvent) -> bool {
        self.version == 1
            && event.kind == AgentChatEventKind::Message
            && event.role == Some(AgentChatRole::User)
            && matches!(
                text(event, "input_origin"),
                Some("human_input" | "agent_input")
            )
            && matches!(text(event, "input_purpose"), None | Some("request"))
            && event.provider == self.provider
            && text(event, "chat_turn_adapter") == Some(self.adapter.as_str())
            && text(event, "chat_source_epoch") == Some(self.source_epoch.as_str())
            && text(event, "provider_turn_id") == Some(self.provider_turn_id.as_str())
            && text(event, "request_root_id") == Some(self.request_root_id.as_str())
            && text(event, "chat_source_ref").is_some()
            && event.metadata["chat_source_start"]
                .as_u64()
                .is_some_and(|start| start < self.start)
            && self.start < self.end
    }

    pub(super) fn order(&self) -> String {
        format!(
            "source:{:020}:{:020}:{:020}:{}",
            self.start, self.end, self.sequence, self.source_ref
        )
    }

    #[cfg(test)]
    pub(super) fn status(&self) -> &AgentChatStatus {
        &self.status
    }
    #[cfg(test)]
    pub(super) fn complete(&self) -> bool {
        self.complete
    }
    #[cfg(test)]
    pub(super) fn partial(&self) -> bool {
        self.partial
    }
}

/// Attach only within an entirely admitted source interval. Privacy gaps never
/// become complete membership merely because the closing row was admitted.
pub(super) fn attach(
    receipts: &[Receipt],
    events: &mut [AgentChatEvent],
    capture: Option<&CapturePreparation<'_>>,
) -> io::Result<bool> {
    let Some(capture) = capture else {
        return Ok(false);
    };
    let mut changed = false;
    for event in events {
        for receipt in receipts {
            if !receipt.matches_request(event) {
                continue;
            }
            let start = event.metadata["chat_source_start"].as_u64().unwrap_or(0);
            let next = capture.next;
            if start < next.unknown_before_offset.unwrap_or(0)
                || next
                    .open_disabled_from
                    .is_some_and(|disabled| receipt.end > disabled)
                || next
                    .disabled_spans
                    .iter()
                    .any(|span| start < span.end && receipt.end > span.start)
                || capture
                    .state
                    .should_skip_event(event, Some(&receipt.source_key))
            {
                continue;
            }
            let old = serde_json::from_value::<Receipt>(event.metadata[METADATA_KEY].clone()).ok();
            if old
                .as_ref()
                .is_some_and(|old| old.order() >= receipt.order())
            {
                continue;
            }
            event.metadata[METADATA_KEY] =
                serde_json::to_value(receipt).map_err(io::Error::other)?;
            changed = true;
        }
    }
    Ok(changed)
}

#[cfg(test)]
pub(super) fn for_request(
    scope: &super::chat_turn_index::Scope<'_>,
    event: &AgentChatEvent,
) -> Option<Receipt> {
    let receipt: Receipt = serde_json::from_value(event.metadata[METADATA_KEY].clone()).ok()?;
    (receipt.matches_request(event)
        && scope.source_key == Some(receipt.source_key.as_str())
        && scope
            .source_epoch
            .is_none_or(|epoch| epoch == receipt.source_epoch))
    .then_some(receipt)
}
