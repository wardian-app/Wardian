// Fixture-only helpers included by the owning module during tests.

impl AgentWatchState {
    pub(crate) fn live_chat_boundary(&self) -> Option<&(String, u64)> {
        self.live_chat_boundary.as_ref()
    }
}

impl AgentWatchState {
    pub(crate) fn recent_file_source(
        &self,
    ) -> Option<&(crate::commands::chat_recent_database::Source, u64)> {
        self.recent_file_source.as_ref()
    }
}

impl AgentWatchState {
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
}
