use std::collections::VecDeque;
use wardian_core::models::{TerminalBrokerEvent, TerminalBrokerEventKind};

pub(super) const MAX_REPLAY_EVENTS: usize = 4_096;
pub(super) const MAX_REPLAY_RAW_BYTES: usize = 1_048_576;
pub(super) const MAX_BATCH_EVENTS: u16 = 256;
pub(super) const MAX_BATCH_BYTES: u32 = 262_144;

pub(super) struct ReplayRing {
    events: VecDeque<TerminalBrokerEvent>,
    raw_bytes: usize,
}

impl ReplayRing {
    pub(super) fn new() -> Self {
        Self {
            events: VecDeque::new(),
            raw_bytes: 0,
        }
    }

    pub(super) fn push(&mut self, event: TerminalBrokerEvent) {
        let event_bytes = raw_bytes(&event);
        if event_bytes > MAX_REPLAY_RAW_BYTES {
            self.events.clear();
            self.raw_bytes = 0;
            return;
        }
        self.raw_bytes = self.raw_bytes.saturating_add(event_bytes);
        self.events.push_back(event);
        while self.events.len() > MAX_REPLAY_EVENTS || self.raw_bytes > MAX_REPLAY_RAW_BYTES {
            if let Some(discarded) = self.events.pop_front() {
                self.raw_bytes = self.raw_bytes.saturating_sub(raw_bytes(&discarded));
            } else {
                break;
            }
        }
    }

    pub(super) fn available_from_sequence(&self, latest_sequence: u64) -> u64 {
        self.events
            .front()
            .map(|event| event.sequence)
            .unwrap_or_else(|| latest_sequence.saturating_add(1))
    }

    pub(super) fn read_after(
        &self,
        after_sequence: u64,
        requested_events: u16,
        requested_bytes: u32,
    ) -> Vec<TerminalBrokerEvent> {
        let max_events = requested_events.clamp(1, MAX_BATCH_EVENTS) as usize;
        let max_bytes = requested_bytes.clamp(1, MAX_BATCH_BYTES) as usize;
        let mut selected = Vec::new();
        let mut selected_bytes = 0usize;

        for event in self
            .events
            .iter()
            .filter(|event| event.sequence > after_sequence)
        {
            if selected.len() == max_events {
                break;
            }
            let event_bytes = raw_bytes(event);
            if selected_bytes.saturating_add(event_bytes) > max_bytes {
                break;
            }
            selected_bytes = selected_bytes.saturating_add(event_bytes);
            selected.push(event.clone());
        }
        selected
    }
}

fn raw_bytes(event: &TerminalBrokerEvent) -> usize {
    match &event.event {
        TerminalBrokerEventKind::Output { bytes } => bytes.len(),
        _ => 0,
    }
}
