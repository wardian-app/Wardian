//! Provider-turn results are host observations, fenced before durable publication.

use super::*;
use wardian_core::agent_messaging::AgentMessagingError;
use wardian_core::db::agent_messaging as store;

impl InteractionState {
    /// Retire old conversation context after the lifecycle owner has joined its
    /// native transports. Admission after this boundary belongs to new work.
    pub async fn fence_agent_conversation_deliveries(
        &self,
        recipient: &str,
    ) -> Result<(), AgentMessagingError> {
        let _mutation = self.mutation_lock.lock().await;
        store::with_db(|conn| store::fence_conversation_deliveries(conn, recipient))
    }

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

    /// Persist verified terminal evidence before attempting the atomic reply.
    /// A failed reply write leaves a recoverable outcome, never runnable work.
    pub async fn complete_agent_task_turn(
        &self,
        binding: &store::TaskTurnBinding,
        status: ReplyStatus,
        body: &str,
    ) -> Result<Option<store::Replied>, AgentMessagingError> {
        let _mutation = self.mutation_lock.lock().await;
        if let Err(error) = self
            .validate_task_turn_generation(&binding.recipient, binding.generation)
            .await
        {
            let _ = store::with_db(|conn| store::mark_task_turn_uncertain(conn, binding));
            return Err(error);
        }
        store::with_db(|conn| store::record_task_turn_outcome(conn, binding, status, body))?;
        let replied = store::with_db(|conn| store::publish_task_turn_outcome(conn, binding))?;
        if let Some(replied) = &replied {
            self.cache_agent_reply(replied).await;
        } else {
            for information in store::with_db(|conn| store::task_turn_information(conn, binding))? {
                self.records
                    .lock()
                    .await
                    .insert(information.id.clone(), information.clone());
                for recipient in &information.target_session_ids {
                    self.notify_agent_mailbox(recipient, false).await;
                }
            }
        }
        Ok(replied)
    }

    /// Read the already committed informational notice and output for delivery
    /// through the normal conversation-bound information route.
    pub async fn agent_task_turn_information(
        &self,
        binding: &store::TaskTurnBinding,
    ) -> Result<Vec<InteractionRecord>, AgentMessagingError> {
        let _mutation = self.mutation_lock.lock().await;
        store::with_db(|conn| store::task_turn_information(conn, binding))
    }

    /// Fence accepted-input evidence with the same runtime/deletion gate as the
    /// outcome. A retired owner cannot lend its receipt to a replacement.
    pub async fn record_agent_task_delivery_receipt(
        &self,
        binding: &store::TaskTurnBinding,
        receipt: &store::TaskDeliveryReceipt,
    ) -> Result<(), AgentMessagingError> {
        let _mutation = self.mutation_lock.lock().await;
        self.validate_task_turn_generation(&binding.recipient, binding.generation)
            .await?;
        store::with_db(|conn| store::record_task_delivery_receipt(conn, binding, receipt))
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
