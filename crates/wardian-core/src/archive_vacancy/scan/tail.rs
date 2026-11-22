//! Load-only classification of the two approved event-first publication windows.
//! This does not publish, repair, or qualify tail records. Normal recovery owns them.
use super::{conflict, require, typed, Purpose, Snapshot};
use crate::conversation_event_sources::{
    generated_sources_from_record, source_record_from_chat_event,
};
use crate::conversations::{ConversationNarrativeRecord, ConversationSourceRecord};
use crate::models::chat::AgentChatEvent;
use std::{collections::BTreeSet, io};

impl Snapshot {
    pub(super) fn prepare_load_tail(&mut self, unowned: &[String], seq: u64) -> io::Result<()> {
        let Purpose::ExistingLedger {
            committed_head,
            vacancies,
        } = &self.purpose
        else {
            require(unowned.is_empty(), seq, "references_resolved")?;
            // A new approval records this physical head. Historical envelopes
            // would immediately fail the resulting ledger's load boundary.
            return require(
                self.pending_records
                    .keys()
                    .all(|number| *number > self.head),
                seq,
                "pending_record_boundary",
            );
        };
        let boundary = *committed_head;
        // A missing historical row is not one of the later publication windows.
        require(
            self.pending_records.keys().all(|number| *number > boundary),
            seq,
            "pending_record_boundary",
        )?;
        let mut occupied = self
            .records
            .keys()
            .copied()
            .chain(self.reservations.iter().copied())
            .chain(vacancies.iter().copied())
            .collect::<BTreeSet<_>>();
        if !unowned.is_empty() {
            // Explicit occupied members, not span arithmetic, establish that no
            // unrelated historical hole can consume a later native observation.
            let historical_count = occupied.range(1..=boundary).count();
            require(
                u64::try_from(historical_count).ok() == Some(boundary),
                seq,
                "pending_record_boundary",
            )?;
        }
        let floor = self
            .event_owners
            .iter()
            .filter(|(_, number)| **number <= boundary && self.records.contains_key(number))
            .filter_map(|(id, _)| self.events.get(id).map(|row| row.ordinal))
            .max();
        let unowned: BTreeSet<_> = unowned.iter().map(String::as_str).collect();
        let mut order = self
            .events
            .iter()
            .map(|(id, row)| (row.ordinal, id.clone()))
            .collect::<Vec<_>>();
        order.sort_unstable();
        for (ordinal, id) in order {
            let event = &self.events[&id];
            if event.value["metadata"]["generated"] == true {
                if self
                    .event_owners
                    .get(&id)
                    .is_some_and(|number| self.records.contains_key(number))
                {
                    // Already-published rows need no pending-envelope classification.
                    continue;
                }
                let number = event.value["metadata"]["archive_record"]["seq"]
                    .as_u64()
                    .ok_or_else(|| conflict(seq, "structured_evidence"))?;
                if self.pending_records.contains_key(&number) {
                    occupied.insert(number);
                }
            } else if unowned.contains(id.as_str()) {
                require(
                    floor.is_none_or(|floor| ordinal > floor),
                    seq,
                    "pending_record_boundary",
                )?;
                let number = next_tail_slot(&occupied, boundary, seq)?;
                self.pending_native.insert(id, number);
                occupied.insert(number);
            }
        }
        // Source descriptors come from the same pure factories used by normal
        // publication. They never enter physical maps or neighbour closures.
        for (number, row) in &self.pending_records {
            let original: ConversationNarrativeRecord = typed(&row.value, seq)?;
            let mut record = original.clone();
            let sources = generated_sources_from_record(&mut record);
            require(
                record.source_refs == original.source_refs,
                seq,
                "pending_source_reconstructible",
            )?;
            self.pending_sources
                .insert(format!("generated:{}:{}", self.id, number), sources);
        }
        for (id, number) in &self.pending_native {
            let event: AgentChatEvent = typed(&self.events[id].value, seq)?;
            self.pending_sources.insert(
                id.clone(),
                source_record_from_chat_event(&event, *number)
                    .into_iter()
                    .collect(),
            );
        }
        Ok(())
    }

    pub(super) fn pending_source_for_record(&self, source: &str, number: u64) -> bool {
        self.pending_records.contains_key(&number)
            && self.pending_source_for_event(&format!("generated:{}:{}", self.id, number), source)
    }

    pub(super) fn pending_source_for_event(&self, event: &str, source: &str) -> bool {
        self.pending_sources
            .get(event)
            .is_some_and(|sources| sources.iter().any(|row| row.source_id == source))
    }

    pub(super) fn pending_source_row(&self, source: &ConversationSourceRecord) -> bool {
        self.pending_sources
            .values()
            .any(|rows| rows.contains(source))
    }

    pub(super) fn pending_native_event(&self, event: &str) -> bool {
        self.pending_native.contains_key(event)
    }

    pub(super) fn pending_native_sequence(&self, number: u64) -> bool {
        self.pending_native
            .values()
            .any(|pending| *pending == number)
    }
}

// This counter only matches source descriptors during the event-first window;
// it produces no narrative plan and grants no allocation or publication authority.
fn next_tail_slot(occupied: &BTreeSet<u64>, boundary: u64, seq: u64) -> io::Result<u64> {
    let mut next = boundary
        .checked_add(1)
        .ok_or_else(|| conflict(seq, "pending_record_boundary"))?;
    for number in occupied.range(next..) {
        if *number != next {
            break;
        }
        next = next
            .checked_add(1)
            .ok_or_else(|| conflict(seq, "pending_record_boundary"))?;
    }
    Ok(next)
}

pub(super) fn native_narrative(kind: Option<&str>) -> bool {
    matches!(
        kind,
        Some("message" | "tool_call" | "tool_result" | "approval" | "error")
    )
}
