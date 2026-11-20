//! Structural checks for the additive Chat turn capability.
//! The existing projection owner retains source, scope and revision authority.

use std::collections::{HashMap, HashSet};

use crate::models::chat_presentation::*;

impl ChatPresentation {
    /// Unknown versions fall back as a whole before interpreting any payload.
    pub fn decode_v1(&self) -> Result<Option<ChatPresentationV1>, String> {
        if self.version != CHAT_PRESENTATION_VERSION {
            return Ok(None);
        }
        if serde_json::to_vec(self)
            .map_err(|error| error.to_string())?
            .len()
            > CHAT_PRESENTATION_MAX_BYTES
        {
            return Err("turn presentation byte budget exceeded".into());
        }
        let value: ChatPresentationV1 =
            serde_json::from_value(self.payload.clone()).map_err(|error| error.to_string())?;
        value.validate()?;
        Ok(Some(value))
    }
}

impl ChatPresentationV1 {
    /// Check the bounded structural contract before atomic page publication.
    /// Source authority and scope/revision validation remain with the owner.
    pub fn validate(&self) -> Result<(), String> {
        if self.items.len() > CHAT_PRESENTATION_MAX_ITEMS
            || self.turns.len() + self.legacy_turns.len() > CHAT_PRESENTATION_MAX_ITEMS
        {
            return Err("turn presentation header budget exceeded".into());
        }
        let turns: HashMap<_, _> = self
            .turns
            .iter()
            .map(|turn| (&turn.turn_key, turn))
            .collect();
        let items: HashMap<_, _> = self
            .items
            .iter()
            .map(|item| (&item.item_key, item))
            .collect();
        if turns.len() != self.turns.len() || items.len() != self.items.len() {
            return Err("duplicate turn presentation key".into());
        }
        let mut orders = HashSet::new();
        let mut event_refs = HashSet::new();
        for item in &self.items {
            let valid_identity = match &item.evidence {
                ChatEvidenceIdentity::Source {
                    source_key,
                    source_epoch,
                    record_key,
                } => [source_key, source_epoch, record_key]
                    .iter()
                    .all(|value| !value.is_empty()),
                ChatEvidenceIdentity::Submission { submission_id } => !submission_id.is_empty(),
            };
            if item.item_key.is_empty()
                || item.turn_key.is_empty()
                || item.order_key.is_empty()
                || !turns.contains_key(&item.turn_key)
                || !orders.insert((&item.turn_key, &item.order_key))
                || item.event_refs.len() > CHAT_PRESENTATION_MAX_ITEMS
                || (item.kind == ChatPresentedItemKind::Step) != item.step.is_some()
                || item.evidence_version.is_empty()
                || !valid_identity
                || item.event_refs.iter().any(String::is_empty)
                || item
                    .preview
                    .as_ref()
                    .is_some_and(|preview| preview.text.len() > 1024)
            {
                return Err("invalid turn presentation item".into());
            }
            if item
                .event_refs
                .iter()
                .any(|id| !event_refs.insert(id.as_str()))
            {
                return Err("event has multiple turn presentation owners".into());
            }
            if item.provenance == ChatItemProvenance::Canonical {
                if item.event_refs.is_empty() || item.admission_id.is_some() {
                    return Err("invalid canonical presentation backing".into());
                }
            } else if item.admission_id.as_ref().is_none_or(String::is_empty)
                || item.step.as_ref().is_some_and(|step| {
                    step.detail_ref.is_some()
                        || step.detail_availability == ChatDetailAvailability::Available
                })
            {
                return Err("provisional item has no admission or grants detail authority".into());
            }
            if item.provenance == ChatItemProvenance::Pending
                && (item.kind != ChatPresentedItemKind::Prompt
                    || !matches!(item.evidence, ChatEvidenceIdentity::Submission { .. })
                    || item.submission_status.is_none()
                    || !item.event_refs.is_empty())
            {
                return Err("invalid pending prompt".into());
            }
            if item.provenance != ChatItemProvenance::Canonical && item.preview.is_none() {
                return Err("provisional item requires bounded admitted content".into());
            }
        }
        if self.state_event_refs.iter().any(|reference| {
            reference.event_id.is_empty()
                || !turns.contains_key(&reference.turn_key)
                || !event_refs.insert(reference.event_id.as_str())
        }) || event_refs.len() > CHAT_PRESENTATION_MAX_ITEMS
        {
            return Err("invalid turn state references or header budget".into());
        }
        let mut group_keys = turns.keys().copied().collect::<HashSet<_>>();
        for legacy in &self.legacy_turns {
            if legacy.turn_key.is_empty()
                || legacy.order_key.is_empty()
                || !group_keys.insert(&legacy.turn_key)
                || legacy.event_refs.is_empty()
                || (legacy.kind == ChatLegacyTurnKind::EarlierActivity
                    && legacy.membership_complete)
                || legacy
                    .event_refs
                    .iter()
                    .any(|id| id.is_empty() || !event_refs.insert(id.as_str()))
            {
                return Err("mixed or invalid legacy turn presentation".into());
            }
        }
        if event_refs.len() > CHAT_PRESENTATION_MAX_ITEMS {
            return Err("turn presentation header budget exceeded".into());
        }
        for turn in &self.turns {
            let loaded_steps = self
                .items
                .iter()
                .filter(|item| {
                    item.turn_key == turn.turn_key && item.kind == ChatPresentedItemKind::Step
                })
                .count() as u64;
            let loaded_failures = self
                .items
                .iter()
                .filter(|item| {
                    item.turn_key == turn.turn_key
                        && item
                            .step
                            .as_ref()
                            .is_some_and(|step| step.status == ChatStepStatus::Failed)
                })
                .count() as u64;
            let reference_matches = |key: &String, kind: ChatPresentedItemKind| {
                items
                    .get(key)
                    .is_some_and(|item| item.turn_key == turn.turn_key && item.kind == kind)
            };
            let unique_refs: HashSet<_> = turn
                .prompt_item_keys
                .iter()
                .chain(&turn.answer_item_keys)
                .collect();
            if turn.turn_key.is_empty()
                || turn.order_key.is_empty()
                || turn.step_count < loaded_steps
                || (!turn.answer_recorded && !turn.answer_item_keys.is_empty())
                || turn.failed_step_count < loaded_failures
                || turn.failed_step_count > turn.step_count
                || unique_refs.len() != turn.prompt_item_keys.len() + turn.answer_item_keys.len()
                || !turn
                    .prompt_item_keys
                    .iter()
                    .all(|key| reference_matches(key, ChatPresentedItemKind::Prompt))
                || !turn
                    .answer_item_keys
                    .iter()
                    .all(|key| reference_matches(key, ChatPresentedItemKind::Answer))
            {
                return Err("invalid turn presentation references or counts".into());
            }
        }
        if let Some(detail) = &self.detail {
            if detail.step_key.is_empty()
                || detail.revision.is_empty()
                || detail.text.len() > CHAT_STEP_OUTPUT_BYTES
                || detail
                    .input_text
                    .as_ref()
                    .is_some_and(|text| text.len() > CHAT_STEP_INPUT_BYTES)
                || (detail.input_truncated && detail.input_text.is_none())
                || detail.result_summary.as_ref().is_some_and(|summary| {
                    summary.len() > 160 || summary.chars().any(char::is_control)
                })
                || (detail.pending && detail.unavailable)
                || (detail.complete
                    && (detail.pending || detail.unavailable || detail.truncated_before))
                || detail
                    .total_bytes
                    .is_some_and(|bytes| bytes < detail.text.len() as u64)
                || (detail.truncated_before
                    && detail
                        .total_bytes
                        .is_some_and(|bytes| bytes <= detail.text.len() as u64))
                || (detail.format == ChatOutputFormat::Json
                    && (!detail.complete
                        || serde_json::from_str::<serde_json::Value>(&detail.text).is_err()))
            {
                return Err("invalid bounded step detail".into());
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
