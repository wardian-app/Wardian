//! Provider-turn results are host observations, fenced before durable publication.

use super::*;
use wardian_core::agent_messaging::AgentMessagingError;
use wardian_core::db::agent_messaging as store;

impl InteractionState {
    /// Bind only an acknowledged native Codex task in the current incarnation.
    pub async fn bind_agent_task_turn(
        &self,
        claim: &store::TaskClaim,
        session: &str,
        turn: &str,
        admission_mode: &str,
    ) -> Result<store::TaskTurnBinding, AgentMessagingError> {
        let _mutation = self.mutation_lock.lock().await;
        let recipient =
            claim.record.target_session_ids.first().ok_or_else(|| {
                AgentMessagingError::new("invalid_task", "Task has no recipient.")
            })?;
        self.validate_task_turn_generation(recipient, claim.generation)
            .await?;
        let binding = store::with_db(|conn| {
            store::bind_task_turn(conn, claim, "codex", session, turn, admission_mode)
        })?;
        self.notify_agent_mailbox(recipient, true).await;
        Ok(binding)
    }

    async fn validate_task_turn_generation(
        &self,
        recipient: &str,
        generation: u64,
    ) -> Result<(), AgentMessagingError> {
        if self.deleted_sessions.lock().await.contains(recipient)
            || self
                .current_provider_input_generation(recipient)
                .await
                .unwrap_or(0)
                != generation
        {
            return Err(AgentMessagingError::new(
                "stale_claim",
                "Task result belongs to a replaced or deleted runtime.",
            ));
        }
        Ok(())
    }

    /// Capture an exact finished turn and publish only positively attributed
    /// per-request outcomes. Unattributed evidence keeps ownership and wakes
    /// requesters through informational availability without runnable work.
    pub async fn complete_agent_task_turn(
        &self,
        binding: &store::TaskTurnBinding,
        provider_status: &str,
        answer: &str,
    ) -> Result<(Vec<store::Replied>, Vec<store::Admitted>), AgentMessagingError> {
        let _mutation = self.mutation_lock.lock().await;
        if let Err(error) = self
            .validate_task_turn_generation(&binding.recipient, binding.generation)
            .await
        {
            let _ = store::with_db(|conn| store::mark_task_turn_uncertain(conn, binding));
            return Err(error);
        }
        let recorded = store::with_db(|conn| {
            store::record_task_turn_final(conn, binding, provider_status, answer)
        })?;
        for information in &recorded.information {
            self.records
                .lock()
                .await
                .insert(information.record.id.clone(), information.record.clone());
            if !information.duplicate {
                for recipient in &information.record.target_session_ids {
                    self.notify_agent_mailbox(recipient, false).await;
                }
            }
        }
        let mut replies = Vec::new();
        for binding in recorded.outcomes {
            if let Some(replied) =
                store::with_db(|conn| store::publish_task_turn_outcome(conn, &binding))?
            {
                self.cache_agent_reply(&replied).await;
                replies.push(replied);
            }
        }
        Ok((replies, recorded.information))
    }

    /// Observation loss cannot produce a terminal reply or authorize replay.
    pub async fn mark_agent_task_turn_uncertain(
        &self,
        binding: &store::TaskTurnBinding,
    ) -> Result<(), AgentMessagingError> {
        let _mutation = self.mutation_lock.lock().await;
        store::with_db(|conn| store::mark_task_turn_uncertain(conn, binding))
    }

    /// Recover already captured outcomes without consulting a new provider turn.
    pub async fn recover_agent_task_results(
        &self,
    ) -> Result<Vec<store::Replied>, AgentMessagingError> {
        let _mutation = self.mutation_lock.lock().await;
        let replies = store::with_db(store::recover_task_turn_outcomes)?;
        for replied in &replies {
            self.cache_agent_reply(replied).await;
        }
        Ok(replies)
    }
}
