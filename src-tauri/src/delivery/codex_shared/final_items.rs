//! Completed agentMessage items are authoritative; repeated item IDs cannot
//! append text or reorder the final-answer choice for the same thread and turn.
use super::*;

#[derive(Debug, Default)]
pub(super) struct CompletedMessages {
    seen: std::collections::HashSet<String>,
    final_answer: Option<String>,
    legacy_answer: Option<String>,
}

impl CompletedMessages {
    pub(super) fn observe(&mut self, item: &Value) {
        if item["type"] != "agentMessage" {
            return;
        }
        let (Some(id), Some(text)) = (item["id"].as_str(), item["text"].as_str()) else {
            return;
        };
        if id.is_empty() || !self.seen.insert(id.to_owned()) {
            return;
        }
        match item["phase"].as_str() {
            Some("final_answer") => self.final_answer = Some(text.to_owned()),
            // Only missing/null phase is the legacy protocol's unknown phase.
            None if item["phase"].is_null() => self.legacy_answer = Some(text.to_owned()),
            _ => {}
        }
    }

    pub(super) fn answer(self) -> String {
        self.final_answer.or(self.legacy_answer).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn last_explicit_final_wins_and_duplicate_items_do_not_reorder() {
        let mut messages = CompletedMessages::default();
        for (id, phase, text) in [
            ("legacy", Value::Null, "legacy"),
            ("first", json!("final_answer"), "first"),
            ("comment", json!("commentary"), "thinking"),
            ("last", json!("final_answer"), "last"),
            ("first", json!("final_answer"), "duplicated first"),
            ("unknown", json!("future_phase"), "future"),
            ("later-legacy", Value::Null, "later legacy"),
        ] {
            messages.observe(&json!({"id":id,"type":"agentMessage","phase":phase,"text":text}));
        }
        messages.observe(&json!({"id":"tool","type":"commandExecution","text":"tool output","phase":"final_answer"}));
        assert_eq!(messages.answer(), "last");
    }

    #[test]
    fn last_legacy_completed_item_is_the_only_fallback() {
        let mut messages = CompletedMessages::default();
        messages.observe(&json!({"id":"first","type":"agentMessage","text":"first"}));
        messages.observe(&json!({"id":"last","type":"agentMessage","phase":null,"text":"last"}));
        messages.observe(
            &json!({"id":"comment","type":"agentMessage","phase":"commentary","text":"comment"}),
        );
        assert_eq!(messages.answer(), "last");
        let mut commentary = CompletedMessages::default();
        commentary.observe(
            &json!({"id":"comment","type":"agentMessage","phase":"commentary","text":"comment"}),
        );
        assert_eq!(commentary.answer(), "");
    }
}
