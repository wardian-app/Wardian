// Fixture-only helpers included by the owning module during tests.

impl Receipt {
    pub(super) fn status(&self) -> &AgentChatStatus {
        &self.status
    }
}

impl Receipt {
    pub(super) fn complete(&self) -> bool {
        self.complete
    }
}

impl Receipt {
    pub(super) fn partial(&self) -> bool {
        self.partial
    }
}

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
