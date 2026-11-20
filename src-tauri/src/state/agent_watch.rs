use std::collections::VecDeque;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use tokio::sync::Notify;
use wardian_core::control::{WatchEvent, WatchOutput, WatchTranscript, WatchTranscriptMessage};

use super::terminal_text::strip_terminal_controls;

#[derive(Debug)]
pub struct AgentWatchState {
    agent_id: String,
    max_records: usize,
    max_output_bytes: usize,
    next_sequence: u64,
    live_chat_boundary: Option<(String, u64)>,
    recent_file_source: Option<(crate::commands::chat_recent_database::Source, u64)>,
    records: VecDeque<WatchRecord>,
    notify: Arc<Notify>,
    codex_attachment_ready: Arc<AtomicBool>,
}

impl AgentWatchState {
    pub fn new(agent_id: String, max_records: usize, max_output_bytes: usize) -> Self {
        Self {
            agent_id,
            max_records: max_records.max(1),
            max_output_bytes: max_output_bytes.max(1),
            next_sequence: 0,
            live_chat_boundary: None,
            recent_file_source: None,
            records: VecDeque::new(),
            notify: Arc::new(Notify::new()),
            // Non-Codex and already-attached runtimes are ready by default;
            // fresh Codex publication flips this exact runtime gate closed
            // before the ActiveAgent enters the roster.
            codex_attachment_ready: Arc::new(AtomicBool::new(true)),
        }
    }

    pub fn codex_attachment_ready(&self) -> bool {
        self.codex_attachment_ready.load(Ordering::Acquire)
    }

    pub(crate) fn codex_attachment_ready_flag(&self) -> Arc<AtomicBool> {
        self.codex_attachment_ready.clone()
    }

    pub(crate) fn set_codex_attachment_ready(&self, ready: bool) {
        self.codex_attachment_ready.store(ready, Ordering::Release);
    }

    pub fn latest_cursor(&self) -> String {
        self.cursor_for_sequence(self.next_sequence)
    }

    /// Called for the fresh runtime Watch before any provider observation.
    /// Saved/off Watches have no such receipt and cannot claim live privacy.
    pub(crate) fn bind_live_chat(&mut self, started_at: &str) {
        self.live_chat_boundary = Some((started_at.to_owned(), self.next_sequence));
    }

    pub(crate) fn live_chat_boundary(&self) -> Option<&(String, u64)> {
        self.live_chat_boundary.as_ref()
    }

    /// Only the normal file parser binds its last actual transcript record.
    /// Earlier retained Watch rows acquire no ownership from this receipt.
    pub(crate) fn bind_recent_file_source(
        &mut self,
        source: crate::commands::chat_recent_database::Source,
    ) {
        self.recent_file_source = self.records.iter().rev().find_map(|record| {
            matches!(record.kind, WatchRecordKind::Transcript { .. })
                .then(|| (source.clone(), record.sequence))
        });
    }

    pub(crate) fn recent_file_source(
        &self,
    ) -> Option<&(crate::commands::chat_recent_database::Source, u64)> {
        self.recent_file_source.as_ref()
    }

    pub fn oldest_available_cursor(&self) -> String {
        self.cursor_for_sequence(self.oldest_available_sequence())
    }

    pub fn notifier(&self) -> Arc<Notify> {
        self.notify.clone()
    }

    pub fn push_event(&mut self, kind: &str, payload: serde_json::Value) -> WatchCursor {
        self.push_record(WatchRecordKind::Event {
            kind: kind.to_string(),
            payload,
        })
    }

    pub fn push_output(&mut self, bytes: &[u8]) -> WatchCursor {
        self.push_record(WatchRecordKind::Output {
            bytes: bytes.to_vec(),
        })
    }

    pub fn push_delivery(&mut self, payload: serde_json::Value) -> WatchCursor {
        self.push_record(WatchRecordKind::Delivery { payload })
    }

    pub fn push_transcript(&mut self, message: WatchTranscriptMessage) -> WatchCursor {
        self.push_record(WatchRecordKind::Transcript { message })
    }

    /// File parsers cannot acquire current stdout ownership from a discovered
    /// path or a live runtime boundary. Their rows need their own source receipt.
    pub(crate) fn push_file_transcript(&mut self, message: WatchTranscriptMessage) -> WatchCursor {
        let cursor = self.push_transcript(message);
        if let Some(record) = self.records.back_mut() {
            record.file_derived = true;
        }
        cursor
    }

    /// Visit the newest retained transcript records without cloning their bodies.
    /// The second sequence is the unchanged normal transcript mapper's ordinal.
    pub(crate) fn visit_recent_transcript(
        &self,
        current_status: &str,
        mut visit: impl FnMut(u64, u64, bool, &WatchTranscriptMessage),
    ) {
        // An approval row can precede the transcript in the normal mapper. Do
        // not guess that identity without its full approval observation.
        if self.records.len() > 4096
            || wardian_core::identity::normalize_status(current_status) == "action_required"
        {
            return;
        }
        let statuses = self
            .records
            .iter()
            .filter(|record| {
                matches!(&record.kind,
            WatchRecordKind::Event { kind, payload } if kind == "status"
                && payload.get("status").and_then(serde_json::Value::as_str).is_some())
            })
            .count();
        let prefix = statuses.max(usize::from(!current_status.trim().is_empty())) as u64;
        let mut remaining = self.max_output_bytes;
        let mut retained = 0_u64;
        for record in self.records.iter().rev() {
            if let WatchRecordKind::Transcript { message } = &record.kind {
                if remaining == 0 {
                    break;
                }
                retained += 1;
                remaining = remaining.saturating_sub(message.text.len());
            }
        }
        let mut selected = 0_u64;
        for record in self.records.iter().rev() {
            if let WatchRecordKind::Transcript { message } = &record.kind {
                if selected >= retained || selected >= 80 {
                    break;
                }
                visit(
                    record.sequence,
                    prefix + retained - selected,
                    record.file_derived,
                    message,
                );
                selected += 1;
            }
        }
    }

    pub fn snapshot_since(
        &self,
        since: Option<&str>,
        tail_bytes: Option<usize>,
    ) -> Result<WatchSnapshot, WatchStateError> {
        let since_sequence = self.parse_since_sequence(since)?;
        let oldest = self.oldest_available_sequence();
        if since_sequence.saturating_add(1) < oldest {
            return Err(WatchStateError::new(
                "cursor_expired",
                serde_json::json!({
                    "oldest_available_cursor": self.oldest_available_cursor(),
                    "requested_cursor": since,
                }),
            ));
        }

        let records = self
            .records
            .iter()
            .filter(|record| record.sequence > since_sequence);
        let mut events = Vec::new();
        let mut output = Vec::new();
        let mut transcript_messages = Vec::new();
        for record in records {
            match &record.kind {
                WatchRecordKind::Event { kind, payload } => events.push(WatchEvent {
                    cursor: self.cursor_for_sequence(record.sequence),
                    kind: kind.clone(),
                    payload: payload.clone(),
                }),
                WatchRecordKind::Delivery { payload } => events.push(WatchEvent {
                    cursor: self.cursor_for_sequence(record.sequence),
                    kind: "delivery".to_string(),
                    payload: payload.clone(),
                }),
                WatchRecordKind::Output { bytes } => output.extend(bytes),
                WatchRecordKind::Transcript { message } => {
                    transcript_messages.push(message.clone());
                }
            }
        }

        let raw_output = self.snapshot_output(output.clone(), tail_bytes, false);
        let output = self.snapshot_output(output, tail_bytes, true);
        let transcript = self.snapshot_transcript(transcript_messages, tail_bytes);
        Ok(WatchSnapshot {
            cursor: self.latest_cursor(),
            events,
            output,
            raw_output,
            transcript,
        })
    }

    pub fn raw_snapshot_since(
        &self,
        since: Option<&str>,
        tail_bytes: Option<usize>,
    ) -> Result<WatchOutput, WatchStateError> {
        let since_sequence = self.parse_since_sequence(since)?;
        let oldest = self.oldest_available_sequence();
        if since_sequence.saturating_add(1) < oldest {
            return Err(WatchStateError::new(
                "cursor_expired",
                serde_json::json!({
                    "oldest_available_cursor": self.oldest_available_cursor(),
                    "requested_cursor": since,
                }),
            ));
        }

        let mut output = Vec::new();
        for record in self
            .records
            .iter()
            .filter(|record| record.sequence > since_sequence)
        {
            if let WatchRecordKind::Output { bytes } = &record.kind {
                output.extend(bytes);
            }
        }

        Ok(self.snapshot_output(output, tail_bytes, false))
    }

    fn push_record(&mut self, kind: WatchRecordKind) -> WatchCursor {
        self.next_sequence = self.next_sequence.saturating_add(1);
        let sequence = self.next_sequence;
        self.records.push_back(WatchRecord {
            sequence,
            kind,
            file_derived: false,
        });
        while self.records.len() > self.max_records {
            self.records.pop_front();
        }
        self.trim_output_records();
        self.notify.notify_waiters();
        WatchCursor {
            agent_id: self.agent_id.clone(),
            sequence,
        }
    }

    fn trim_output_records(&mut self) {
        let mut output_bytes = self.retained_output_bytes();
        while output_bytes > self.max_output_bytes {
            let Some(index) = self
                .records
                .iter()
                .position(|record| matches!(record.kind, WatchRecordKind::Output { .. }))
            else {
                break;
            };
            if let Some(record) = self.records.remove(index) {
                if let WatchRecordKind::Output { bytes } = record.kind {
                    output_bytes = output_bytes.saturating_sub(bytes.len());
                }
            }
        }
    }

    fn retained_output_bytes(&self) -> usize {
        self.records
            .iter()
            .filter_map(|record| match &record.kind {
                WatchRecordKind::Output { bytes } => Some(bytes.len()),
                _ => None,
            })
            .sum()
    }

    fn snapshot_output(
        &self,
        bytes: Vec<u8>,
        tail_bytes: Option<usize>,
        sanitize: bool,
    ) -> WatchOutput {
        let limit = tail_bytes.unwrap_or(bytes.len()).min(bytes.len());
        let start = utf8_tail_start(&bytes, limit);
        let omitted_bytes = start;
        let text = String::from_utf8_lossy(&bytes[start..]).to_string();
        let text = if sanitize {
            strip_terminal_controls(&text)
        } else {
            text
        };
        WatchOutput {
            cursor: self.latest_cursor(),
            text,
            truncated: omitted_bytes > 0,
            omitted_bytes,
        }
    }

    fn snapshot_transcript(
        &self,
        messages: Vec<WatchTranscriptMessage>,
        tail_bytes: Option<usize>,
    ) -> WatchTranscript {
        let limit = tail_bytes.unwrap_or(self.max_output_bytes);
        let mut remaining = limit;
        let mut omitted_bytes = 0usize;
        let mut retained = Vec::new();

        for mut message in messages.into_iter().rev() {
            let bytes = message.text.as_bytes();
            if bytes.len() <= remaining {
                remaining = remaining.saturating_sub(bytes.len());
                retained.push(message);
                continue;
            }

            if remaining > 0 {
                let start = utf8_tail_start(bytes, remaining);
                omitted_bytes = omitted_bytes.saturating_add(start);
                message.text = String::from_utf8_lossy(&bytes[start..]).to_string();
                retained.push(message);
                remaining = 0;
            } else {
                omitted_bytes = omitted_bytes.saturating_add(bytes.len());
            }
        }

        retained.reverse();
        let latest_text = retained
            .iter()
            .rev()
            .find(|message| !message.text.trim().is_empty())
            .map(|message| message.text.clone())
            .unwrap_or_default();
        WatchTranscript {
            cursor: self.latest_cursor(),
            messages: retained,
            latest_text,
            truncated: omitted_bytes > 0,
            omitted_bytes,
        }
    }

    fn parse_since_sequence(&self, since: Option<&str>) -> Result<u64, WatchStateError> {
        let Some(cursor) = since else {
            return Ok(self.oldest_available_sequence().saturating_sub(1));
        };
        let Some((agent_id, sequence)) = cursor.split_once(':') else {
            return Err(WatchStateError::new(
                "invalid_cursor",
                serde_json::json!({ "cursor": cursor }),
            ));
        };
        if agent_id != self.agent_id {
            return Err(WatchStateError::new(
                "invalid_cursor",
                serde_json::json!({ "cursor": cursor }),
            ));
        }
        u64::from_str_radix(sequence, 16).map_err(|_| {
            WatchStateError::new("invalid_cursor", serde_json::json!({ "cursor": cursor }))
        })
    }

    fn oldest_available_sequence(&self) -> u64 {
        self.records
            .front()
            .map(|record| record.sequence)
            .unwrap_or(self.next_sequence)
    }

    fn cursor_for_sequence(&self, sequence: u64) -> String {
        format!("{}:{sequence:016x}", self.agent_id)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatchCursor {
    agent_id: String,
    sequence: u64,
}

impl WatchCursor {
    pub fn sequence(&self) -> u64 {
        self.sequence
    }
}

#[derive(Debug, Clone)]
pub struct WatchSnapshot {
    pub cursor: String,
    pub events: Vec<WatchEvent>,
    pub output: WatchOutput,
    pub raw_output: WatchOutput,
    pub transcript: WatchTranscript,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WatchStateError {
    code: &'static str,
    details: serde_json::Value,
}

impl WatchStateError {
    fn new(code: &'static str, details: serde_json::Value) -> Self {
        Self { code, details }
    }

    pub fn code(&self) -> &'static str {
        self.code
    }

    pub fn details(&self) -> &serde_json::Value {
        &self.details
    }
}

#[derive(Debug, Clone)]
struct WatchRecord {
    sequence: u64,
    kind: WatchRecordKind,
    file_derived: bool,
}

#[derive(Debug, Clone)]
enum WatchRecordKind {
    Event {
        kind: String,
        payload: serde_json::Value,
    },
    Output {
        bytes: Vec<u8>,
    },
    Transcript {
        message: WatchTranscriptMessage,
    },
    Delivery {
        payload: serde_json::Value,
    },
}

fn utf8_tail_start(bytes: &[u8], limit: usize) -> usize {
    if limit >= bytes.len() {
        return 0;
    }

    let mut start = bytes.len() - limit;
    while start < bytes.len() && std::str::from_utf8(&bytes[start..]).is_err() {
        start += 1;
    }
    start
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recent_watch_origin_is_per_record_without_changing_public_transcript() {
        let mut state = AgentWatchState::new("agent-1".into(), 16, 1024);
        let message = crate::providers::transcript::extract_transcript_message(
            "mock",
            r#"{"type":"model","content":"same body","turn_id":"turn"}"#,
        )
        .unwrap();
        state.push_transcript(message.clone());
        state.push_file_transcript(message);
        let mut origins = Vec::new();
        state.visit_recent_transcript("Off", |sequence, ordinal, file_derived, message| {
            origins.push((sequence, ordinal, file_derived, message.text.clone()));
        });
        assert_eq!(
            origins,
            [
                (2, 3, true, "same body".into()),
                (1, 2, false, "same body".into())
            ]
        );
        let transcript = state.snapshot_since(None, Some(1024)).unwrap().transcript;
        assert_eq!(transcript.messages.len(), 2);
        assert!(transcript
            .messages
            .iter()
            .all(|message| message.text == "same body"));
    }

    #[test]
    fn watch_state_returns_output_since_cursor_without_draining() {
        let mut state = AgentWatchState::new("agent-1".to_string(), 16, 1024);
        let start = state.latest_cursor();

        state.push_output("hello".as_bytes());
        let first = state.snapshot_since(Some(&start), Some(1024)).unwrap();
        let second = state.snapshot_since(Some(&start), Some(1024)).unwrap();

        assert_eq!(first.output.text, "hello");
        assert_eq!(second.output.text, "hello");
    }

    #[test]
    fn watch_state_sanitizes_default_output_and_keeps_raw_snapshot_opt_in() {
        let mut state = AgentWatchState::new("agent-1".to_string(), 16, 1024);
        state.push_output("\u{1b}[31mred\u{1b}[0m".as_bytes());

        let snapshot = state.snapshot_since(None, Some(1024)).unwrap();
        let raw = state.raw_snapshot_since(None, Some(1024)).unwrap();

        assert_eq!(snapshot.output.text, "red");
        assert_eq!(raw.text, "\u{1b}[31mred\u{1b}[0m");
    }

    #[test]
    fn watch_state_returns_transcript_messages_since_cursor() {
        let mut state = AgentWatchState::new("agent-1".to_string(), 16, 1024);
        let start = state.latest_cursor();
        state.push_transcript(wardian_core::control::WatchTranscriptMessage {
            role: "assistant".to_string(),
            text: "final answer".to_string(),
            provider: "mock".to_string(),
            turn_id: Some("turn-1".to_string()),
            source: Some("model".to_string()),
            provider_provenance: None,
        });

        let snapshot = state.snapshot_since(Some(&start), Some(1024)).unwrap();

        assert_eq!(snapshot.transcript.messages.len(), 1);
        assert_eq!(snapshot.transcript.latest_text, "final answer");
    }

    #[test]
    fn watch_state_tail_bounds_transcript_text() {
        let mut state = AgentWatchState::new("agent-1".to_string(), 16, 1024);
        state.push_transcript(wardian_core::control::WatchTranscriptMessage {
            role: "assistant".to_string(),
            text: "alpha beta gamma".to_string(),
            provider: "mock".to_string(),
            turn_id: Some("turn-1".to_string()),
            source: Some("model".to_string()),
            provider_provenance: None,
        });

        let snapshot = state.snapshot_since(None, Some(5)).unwrap();

        assert_eq!(snapshot.transcript.messages.len(), 1);
        assert_eq!(snapshot.transcript.latest_text, "gamma");
        assert!(snapshot.transcript.truncated);
        assert!(snapshot.transcript.omitted_bytes > 0);
    }

    #[test]
    fn watch_state_default_transcript_uses_retention_byte_limit() {
        let mut state = AgentWatchState::new("agent-1".to_string(), 16, 6);
        state.push_transcript(wardian_core::control::WatchTranscriptMessage {
            role: "assistant".to_string(),
            text: "alpha beta".to_string(),
            provider: "mock".to_string(),
            turn_id: Some("turn-1".to_string()),
            source: Some("model".to_string()),
            provider_provenance: None,
        });

        let snapshot = state.snapshot_since(None, None).unwrap();

        assert_eq!(snapshot.transcript.latest_text, "a beta");
        assert!(snapshot.transcript.truncated);
    }

    #[test]
    fn watch_state_rejects_expired_cursor() {
        let mut state = AgentWatchState::new("agent-1".to_string(), 2, 1024);
        let old = state.latest_cursor();
        state.push_event("status", serde_json::json!({"status":"processing"}));
        state.push_event("status", serde_json::json!({"status":"idle"}));
        state.push_event("status", serde_json::json!({"status":"processing"}));

        let error = state.snapshot_since(Some(&old), Some(1024)).unwrap_err();

        assert_eq!(error.code(), "cursor_expired");
    }

    #[test]
    fn watch_state_tail_preserves_utf8_boundary_and_reports_omitted_bytes() {
        let mut state = AgentWatchState::new("agent-1".to_string(), 16, 1024);
        state.push_output("alpha beta".as_bytes());

        let snapshot = state.snapshot_since(None, Some(6)).unwrap();

        assert!(snapshot.output.truncated);
        assert!(snapshot.output.omitted_bytes > 0);
        assert!(std::str::from_utf8(snapshot.output.text.as_bytes()).is_ok());
    }

    #[test]
    fn watch_state_rejects_cursor_from_another_agent() {
        let state = AgentWatchState::new("agent-1".to_string(), 16, 1024);
        let error = state
            .snapshot_since(Some("agent-2:0000000000000000"), Some(1024))
            .unwrap_err();

        assert_eq!(error.code(), "invalid_cursor");
    }

    #[test]
    fn watch_state_orders_status_output_and_delivery_on_one_cursor_sequence() {
        let mut state = AgentWatchState::new("agent-1".to_string(), 16, 1024);

        let status = state.push_event("status", serde_json::json!({"status":"processing"}));
        let output = state.push_output("hello".as_bytes());
        let transcript = state.push_transcript(wardian_core::control::WatchTranscriptMessage {
            role: "assistant".to_string(),
            text: "hello".to_string(),
            provider: "mock".to_string(),
            turn_id: None,
            source: None,
            provider_provenance: None,
        });
        let delivery = state.push_delivery(serde_json::json!({"delivery_state":"submitted"}));

        assert!(status.sequence() < output.sequence());
        assert!(output.sequence() < transcript.sequence());
        assert!(transcript.sequence() < delivery.sequence());
    }

    #[test]
    fn expired_cursor_error_includes_oldest_available_cursor() {
        let mut state = AgentWatchState::new("agent-1".to_string(), 2, 1024);
        let old = state.latest_cursor();
        state.push_event("status", serde_json::json!({"status":"processing"}));
        state.push_event("status", serde_json::json!({"status":"idle"}));
        state.push_event("status", serde_json::json!({"status":"processing"}));

        let error = state.snapshot_since(Some(&old), Some(1024)).unwrap_err();

        assert_eq!(error.code(), "cursor_expired");
        assert!(error.details()["oldest_available_cursor"]
            .as_str()
            .is_some());
    }
}
