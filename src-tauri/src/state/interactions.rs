use std::collections::HashMap;

use tokio::sync::Mutex;
use wardian_core::control::{
    InteractionBodyRef, InteractionKind, InteractionRecord, InteractionStatus,
    InteractionTriggerPolicy, ProviderInputReadiness, ProviderInputState, ProviderReadyEvidence,
    ReplyStatus, StructuredReply,
};

#[derive(Debug, Default)]
pub struct InteractionState {
    records: Mutex<HashMap<String, InteractionRecord>>,
    replies: Mutex<HashMap<String, StructuredReply>>,
    provider_inputs: Mutex<HashMap<String, ProviderInputState>>,
}

impl InteractionState {
    pub async fn create_task(
        &self,
        sender_session_id: Option<String>,
        target_session_id: String,
        body_ref: InteractionBodyRef,
    ) -> InteractionRecord {
        self.create_task_with_id(
            new_interaction_id(),
            sender_session_id,
            target_session_id,
            body_ref,
        )
        .await
    }

    pub async fn create_task_with_id(
        &self,
        id: String,
        sender_session_id: Option<String>,
        target_session_id: String,
        body_ref: InteractionBodyRef,
    ) -> InteractionRecord {
        let now = now_rfc3339_millis();
        let record = InteractionRecord {
            id,
            kind: InteractionKind::Task,
            sender_session_id,
            target_session_ids: vec![target_session_id],
            status: InteractionStatus::AwaitingReply,
            trigger_policy: InteractionTriggerPolicy::ReplyRequired,
            body_ref,
            parent_interaction_id: None,
            created_at: now.clone(),
            updated_at: now,
            completed_at: None,
        };
        self.records
            .lock()
            .await
            .insert(record.id.clone(), record.clone());
        record
    }

    pub async fn record_provider_input_state(
        &self,
        session_id: &str,
        generation: u64,
        state: ProviderInputReadiness,
        ready_evidence: Option<ProviderReadyEvidence>,
    ) -> ProviderInputState {
        let mut inputs = self.provider_inputs.lock().await;
        if let Some(existing) = inputs.get(session_id) {
            if generation < existing.generation {
                return existing.clone();
            }
        }
        let record = ProviderInputState {
            session_id: session_id.to_string(),
            generation,
            state,
            ready_evidence,
            observed_at: now_rfc3339_millis(),
        };
        inputs.insert(session_id.to_string(), record.clone());
        record
    }

    pub async fn provider_input_state(&self, session_id: &str) -> Option<ProviderInputState> {
        self.provider_inputs.lock().await.get(session_id).cloned()
    }

    pub async fn interaction(&self, id: &str) -> Option<InteractionRecord> {
        self.records.lock().await.get(id).cloned()
    }

    pub async fn complete_task_with_reply(
        &self,
        task_id: &str,
        source_session_id: Option<&str>,
        status: ReplyStatus,
        body: &str,
    ) -> Result<StructuredReply, &'static str> {
        let now = now_rfc3339_millis();
        let structured_reply = {
            let mut records = self.records.lock().await;
            let task = records.get_mut(task_id).ok_or("not_found")?;
            if task.status == InteractionStatus::Completed {
                return Err("duplicate_reply");
            }
            if let Some(source) = source_session_id {
                if !task.target_session_ids.iter().any(|target| target == source) {
                    return Err("unauthorized");
                }
            }
            let target_session_id = task
                .target_session_ids
                .first()
                .cloned()
                .ok_or("not_found")?;
            task.status = InteractionStatus::Completed;
            task.updated_at = now.clone();
            task.completed_at = Some(now.clone());

            let reply = InteractionRecord {
                id: new_interaction_id(),
                kind: InteractionKind::Reply,
                sender_session_id: source_session_id.map(str::to_string),
                target_session_ids: task.sender_session_id.iter().cloned().collect(),
                status: InteractionStatus::Completed,
                trigger_policy: InteractionTriggerPolicy::NotifyOnly,
                body_ref: InteractionBodyRef::Inline {
                    body: body.to_string(),
                },
                parent_interaction_id: Some(task_id.to_string()),
                created_at: now.clone(),
                updated_at: now.clone(),
                completed_at: Some(now.clone()),
            };
            let structured_reply = StructuredReply {
                request_id: task_id.to_string(),
                status,
                body: body.to_string(),
                target_session_id,
                source_session_id: source_session_id.map(str::to_string),
                replied_at: now,
            };
            records.insert(reply.id.clone(), reply.clone());
            structured_reply
        };
        self.replies
            .lock()
            .await
            .insert(task_id.to_string(), structured_reply.clone());
        Ok(structured_reply)
    }

    pub async fn structured_reply(&self, task_id: &str) -> Option<StructuredReply> {
        self.replies.lock().await.get(task_id).cloned()
    }
}

fn new_interaction_id() -> String {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let counter = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let millis = chrono::Utc::now().timestamp_millis();
    format!("int_{millis:013}_{counter:06}")
}

fn now_rfc3339_millis() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn task_interaction_starts_awaiting_reply() {
        let state = InteractionState::default();

        let record = state
            .create_task(
                Some("source-1".to_string()),
                "agent-1".to_string(),
                InteractionBodyRef::Inline {
                    body: "review this".to_string(),
                },
            )
            .await;

        assert!(record.id.starts_with("int_"));
        assert_eq!(record.kind, InteractionKind::Task);
        assert_eq!(record.status, InteractionStatus::AwaitingReply);
        assert_eq!(
            record.trigger_policy,
            InteractionTriggerPolicy::ReplyRequired
        );
    }

    #[tokio::test]
    async fn stale_provider_readiness_generation_is_ignored() {
        let state = InteractionState::default();
        state
            .record_provider_input_state(
                "agent-1",
                4,
                ProviderInputReadiness::Ready,
                Some(ProviderReadyEvidence::PromptDetected),
            )
            .await;
        state
            .record_provider_input_state(
                "agent-1",
                3,
                ProviderInputReadiness::Ready,
                Some(ProviderReadyEvidence::ManualStatus),
            )
            .await;

        let current = state.provider_input_state("agent-1").await.unwrap();
        assert_eq!(current.generation, 4);
        assert_eq!(
            current.ready_evidence,
            Some(ProviderReadyEvidence::PromptDetected)
        );
    }
}

#[cfg(test)]
mod reply_tests {
    use super::*;
    use wardian_core::control::ReplyStatus;

    #[tokio::test]
    async fn reply_completes_parent_task_once() {
        let state = InteractionState::default();
        let task = state
            .create_task(
                None,
                "agent-1".to_string(),
                InteractionBodyRef::Inline {
                    body: "review".to_string(),
                },
            )
            .await;

        let structured_reply = state
            .complete_task_with_reply(&task.id, Some("agent-1"), ReplyStatus::Done, "finished")
            .await
            .unwrap();

        assert_eq!(structured_reply.request_id, task.id);
        let completed = state.interaction(&task.id).await.unwrap();
        assert_eq!(completed.status, InteractionStatus::Completed);

        let duplicate = state
            .complete_task_with_reply(&task.id, Some("agent-1"), ReplyStatus::Done, "again")
            .await
            .unwrap_err();
        assert_eq!(duplicate, "duplicate_reply");
    }

    #[tokio::test]
    async fn completed_task_exposes_structured_reply_status() {
        let state = InteractionState::default();
        let task = state
            .create_task(
                None,
                "agent-1".to_string(),
                InteractionBodyRef::Inline {
                    body: "review".to_string(),
                },
            )
            .await;

        state
            .complete_task_with_reply(&task.id, Some("agent-1"), ReplyStatus::Blocked, "blocked")
            .await
            .unwrap();

        let reply = state.structured_reply(&task.id).await.unwrap();
        assert_eq!(reply.request_id, task.id);
        assert_eq!(reply.status, ReplyStatus::Blocked);
        assert_eq!(reply.body, "blocked");
        assert_eq!(reply.target_session_id, "agent-1");
        assert_eq!(reply.source_session_id.as_deref(), Some("agent-1"));
    }
}
