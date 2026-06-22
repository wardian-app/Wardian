use serde::{Deserialize, Serialize};
use wardian_core::control::{ApprovalAction, MessageInputMode, MessageOrigin, QueuePolicy};

const MAX_TERMINAL_RECORDS_PER_TARGET: usize = 64;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MailboxMessageDraft {
    pub interaction_id: String,
    pub target_session_id: String,
    pub body: String,
    pub input_mode: MessageInputMode,
    pub queue_policy: QueuePolicy,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_action: Option<ApprovalAction>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<MessageOrigin>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MailboxMessageRecord {
    pub id: String,
    pub interaction_id: String,
    pub target_session_id: String,
    pub body: String,
    pub input_mode: MessageInputMode,
    pub queue_policy: QueuePolicy,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_action: Option<ApprovalAction>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<MessageOrigin>,
    pub created_at: String,
    pub status: MailboxMessageStatus,
    pub phase: MailboxDeliveryPhase,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MailboxMessageStatus {
    Pending,
    InFlight,
    Delivered,
    Failed,
}

impl MailboxMessageStatus {
    fn is_terminal(self) -> bool {
        matches!(self, Self::Delivered | Self::Failed)
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MailboxDeliveryPhase {
    Queued,
    Dispatching,
    Submitted,
    Terminal,
}

#[derive(Debug, Default)]
pub struct MailboxState {
    records: Vec<MailboxMessageRecord>,
    last_millis: i64,
    counter: u64,
}

impl MailboxState {
    pub fn enqueue(&mut self, draft: MailboxMessageDraft) -> MailboxMessageRecord {
        let created_at = now_rfc3339_millis();
        let id = self.next_message_id();
        let record = MailboxMessageRecord {
            id,
            interaction_id: draft.interaction_id,
            target_session_id: draft.target_session_id,
            body: draft.body,
            input_mode: draft.input_mode,
            queue_policy: draft.queue_policy,
            approval_action: draft.approval_action,
            origin: draft.origin,
            created_at,
            status: MailboxMessageStatus::Pending,
            phase: MailboxDeliveryPhase::Queued,
        };
        self.records.push(record.clone());
        record
    }

    pub fn all(&self) -> Vec<MailboxMessageRecord> {
        self.records.clone()
    }

    pub fn list_for_target(&self, target_session_id: &str) -> Vec<MailboxMessageRecord> {
        self.records
            .iter()
            .filter(|record| record.target_session_id == target_session_id)
            .cloned()
            .collect()
    }

    pub fn take_next_pending_for_target(
        &mut self,
        target_session_id: &str,
    ) -> Option<MailboxMessageRecord> {
        let record = self.records.iter_mut().find(|record| {
            record.target_session_id == target_session_id
                && record.status == MailboxMessageStatus::Pending
        })?;
        record.status = MailboxMessageStatus::InFlight;
        record.phase = MailboxDeliveryPhase::Dispatching;
        Some(record.clone())
    }

    pub fn mark_delivered(&mut self, id: &str) -> Option<MailboxMessageRecord> {
        self.mark_terminal(id, MailboxMessageStatus::Delivered)
    }

    pub fn mark_failed(&mut self, id: &str) -> Option<MailboxMessageRecord> {
        self.mark_terminal(id, MailboxMessageStatus::Failed)
    }

    pub fn mark_pending(&mut self, id: &str) -> Option<MailboxMessageRecord> {
        let record = self.records.iter_mut().find(|record| record.id == id)?;
        record.status = MailboxMessageStatus::Pending;
        record.phase = MailboxDeliveryPhase::Queued;
        Some(record.clone())
    }

    pub fn remove_for_target(&mut self, target_session_id: &str) -> usize {
        let original_len = self.records.len();
        self.records
            .retain(|record| record.target_session_id != target_session_id);
        original_len - self.records.len()
    }

    fn mark_terminal(
        &mut self,
        id: &str,
        status: MailboxMessageStatus,
    ) -> Option<MailboxMessageRecord> {
        let updated = {
            let record = self.records.iter_mut().find(|record| record.id == id)?;
            record.status = status;
            record.phase = MailboxDeliveryPhase::Terminal;
            record.clone()
        };
        self.compact_terminal_records_for_target(&updated.target_session_id);
        Some(updated)
    }

    fn compact_terminal_records_for_target(&mut self, target_session_id: &str) {
        let terminal_count = self
            .records
            .iter()
            .filter(|record| {
                record.target_session_id == target_session_id && record.status.is_terminal()
            })
            .count();
        let mut remove_count = terminal_count.saturating_sub(MAX_TERMINAL_RECORDS_PER_TARGET);
        if remove_count == 0 {
            return;
        }

        self.records.retain(|record| {
            if remove_count > 0
                && record.target_session_id == target_session_id
                && record.status.is_terminal()
            {
                remove_count -= 1;
                false
            } else {
                true
            }
        });
    }

    fn next_message_id(&mut self) -> String {
        let now_millis = chrono::Utc::now().timestamp_millis();
        let millis = now_millis.max(self.last_millis);
        if millis == self.last_millis {
            self.counter = self.counter.saturating_add(1);
        } else {
            self.last_millis = millis;
            self.counter = 0;
        }

        format!("msg_{millis:013}_{:06}", self.counter)
    }
}

fn now_rfc3339_millis() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use wardian_core::control::{ApprovalAction, MessageInputMode, MessageOrigin, QueuePolicy};

    fn message_for(target_session_id: &str, body: &str) -> MailboxMessageDraft {
        MailboxMessageDraft {
            interaction_id: format!("int_{target_session_id}_{body}"),
            target_session_id: target_session_id.to_string(),
            body: body.to_string(),
            input_mode: MessageInputMode::Message,
            queue_policy: QueuePolicy::QueueIfBusy,
            approval_action: None,
            origin: None,
        }
    }

    #[test]
    fn enqueueing_message_records_pending_mailbox_entry() {
        let mut mailbox = MailboxState::default();

        let record = mailbox.enqueue(message_for("agent-1", "hello"));

        assert_eq!(record.target_session_id, "agent-1");
        assert_eq!(record.body, "hello");
        assert_eq!(record.status, MailboxMessageStatus::Pending);
        assert_eq!(record.phase, MailboxDeliveryPhase::Queued);
        assert!(record.id.starts_with("msg_"));
        assert_eq!(mailbox.all().len(), 1);
    }

    #[test]
    fn queued_message_preserves_interaction_id() {
        let mut mailbox = MailboxState::default();

        let record = mailbox.enqueue(message_for("agent-1", "hello"));

        assert_eq!(record.interaction_id, "int_agent-1_hello");
    }

    #[test]
    fn message_ids_are_monotonic_with_stable_shape() {
        let mut mailbox = MailboxState::default();

        let first = mailbox.enqueue(message_for("agent-1", "one"));
        let second = mailbox.enqueue(message_for("agent-1", "two"));

        assert!(first.id.starts_with("msg_"));
        assert!(second.id.starts_with("msg_"));
        assert_ne!(first.id, second.id);
        assert!(
            first.id < second.id,
            "ids should sort in enqueue order: {} then {}",
            first.id,
            second.id
        );
    }

    #[test]
    fn listing_can_filter_by_target_session_id() {
        let mut mailbox = MailboxState::default();
        mailbox.enqueue(message_for("agent-1", "one"));
        mailbox.enqueue(message_for("agent-2", "two"));
        mailbox.enqueue(message_for("agent-1", "three"));

        let agent_one = mailbox.list_for_target("agent-1");

        assert_eq!(agent_one.len(), 2);
        assert!(agent_one
            .iter()
            .all(|record| record.target_session_id == "agent-1"));
        assert_eq!(agent_one[0].body, "one");
        assert_eq!(agent_one[1].body, "three");
    }

    #[test]
    fn enqueue_preserves_approval_action_and_origin_metadata() {
        let mut mailbox = MailboxState::default();
        let origin = MessageOrigin::WardianAgent {
            session_id: "source-agent".to_string(),
        };
        let approval_action = ApprovalAction::Select {
            option: "allow_once".to_string(),
        };

        let record = mailbox.enqueue(MailboxMessageDraft {
            interaction_id: "int_approval".to_string(),
            target_session_id: "agent-1".to_string(),
            body: "approve".to_string(),
            input_mode: MessageInputMode::ApprovalAction,
            queue_policy: QueuePolicy::MailboxOnly,
            approval_action: Some(approval_action.clone()),
            origin: Some(origin.clone()),
        });

        assert_eq!(record.input_mode, MessageInputMode::ApprovalAction);
        assert_eq!(record.queue_policy, QueuePolicy::MailboxOnly);
        assert_eq!(record.approval_action, Some(approval_action));
        assert_eq!(record.origin, Some(origin));
    }

    #[test]
    fn taking_next_pending_marks_only_first_target_message_in_flight() {
        let mut mailbox = MailboxState::default();
        let first = mailbox.enqueue(message_for("agent-1", "one"));
        let second = mailbox.enqueue(message_for("agent-1", "two"));
        mailbox.enqueue(message_for("agent-2", "other"));

        let taken = mailbox.take_next_pending_for_target("agent-1").unwrap();

        assert_eq!(taken.id, first.id);
        assert_eq!(taken.status, MailboxMessageStatus::InFlight);
        assert_eq!(taken.phase, MailboxDeliveryPhase::Dispatching);
        let agent_one = mailbox.list_for_target("agent-1");
        assert_eq!(agent_one[0].status, MailboxMessageStatus::InFlight);
        assert_eq!(agent_one[1].id, second.id);
        assert_eq!(agent_one[1].status, MailboxMessageStatus::Pending);
    }

    #[test]
    fn terminal_markers_preserve_records_and_update_phase() {
        let mut mailbox = MailboxState::default();
        let delivered = mailbox.enqueue(message_for("agent-1", "one"));
        let failed = mailbox.enqueue(message_for("agent-1", "two"));

        let delivered = mailbox.mark_delivered(&delivered.id).unwrap();
        let failed = mailbox.mark_failed(&failed.id).unwrap();

        assert_eq!(delivered.status, MailboxMessageStatus::Delivered);
        assert_eq!(delivered.phase, MailboxDeliveryPhase::Terminal);
        assert_eq!(failed.status, MailboxMessageStatus::Failed);
        assert_eq!(failed.phase, MailboxDeliveryPhase::Terminal);
        assert_eq!(mailbox.all().len(), 2);
    }

    #[test]
    fn terminal_compaction_keeps_only_recent_terminal_records_per_target() {
        let mut mailbox = MailboxState::default();
        let first = mailbox.enqueue(message_for("agent-1", "first"));
        mailbox.mark_delivered(&first.id).unwrap();

        for index in 0..70 {
            let record = mailbox.enqueue(message_for("agent-1", &format!("message-{index}")));
            mailbox.mark_delivered(&record.id).unwrap();
        }

        let records = mailbox.list_for_target("agent-1");
        assert_eq!(records.len(), 64);
        assert!(records.iter().all(|record| record.body != "first"));
    }

    #[test]
    fn mark_pending_requeues_in_flight_message_for_retry() {
        let mut mailbox = MailboxState::default();
        let record = mailbox.enqueue(message_for("agent-1", "one"));
        mailbox.take_next_pending_for_target("agent-1").unwrap();

        let requeued = mailbox.mark_pending(&record.id).unwrap();

        assert_eq!(requeued.status, MailboxMessageStatus::Pending);
        assert_eq!(requeued.phase, MailboxDeliveryPhase::Queued);
        let pending = mailbox.take_next_pending_for_target("agent-1").unwrap();
        assert_eq!(pending.id, record.id);
    }
}
