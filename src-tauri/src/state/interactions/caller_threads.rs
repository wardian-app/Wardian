//! Caller-thread decisions share the canonical interaction mutation gate.
use super::*;
use wardian_core::agent_messaging::AgentMessagingError;
use wardian_core::db::agent_messaging as store;

impl InteractionState {
    /// The control layer holds the owner lifecycle gate from root capture
    /// through this transaction. Recheck the generation under mutation_lock.
    pub(crate) async fn reply_agent_message_from_thread(
        &self,
        sender: &str,
        request_id: &str,
        status: ReplyStatus,
        body: &str,
        mut caller: store::ProviderThreadCaller<'_>,
    ) -> Result<store::ProviderThreadReply, AgentMessagingError> {
        let _mutation = self.mutation_lock.lock().await;
        if self.deleted_sessions.lock().await.contains(sender) {
            return Err(AgentMessagingError::new(
                "unauthorized",
                "Reply sender was deleted.",
            ));
        }
        caller.current_generation = self
            .current_provider_input_generation(sender)
            .await
            .unwrap_or(0);
        let result = store::with_db(|conn| {
            store::reply_with_provider_thread(conn, sender, request_id, status, body, caller)
        })?;
        match &result {
            store::ProviderThreadReply::Replied(replied) => self.cache_agent_reply(replied).await,
            store::ProviderThreadReply::Progress(progress) => {
                self.cache_provider_thread_progress(progress).await
            }
        }
        Ok(result)
    }

    /// Publish labelled child information without a requester terminal result.
    /// Original-conversation binding remains the normal delivery fence.
    pub(crate) async fn publish_agent_thread_progress(
        &self,
        input: store::ProviderThreadProgressInput<'_>,
    ) -> Result<store::ProviderThreadProgress, AgentMessagingError> {
        let _mutation = self.mutation_lock.lock().await;
        if self.deleted_sessions.lock().await.contains(input.owner) {
            return Err(AgentMessagingError::new(
                "unauthorized",
                "Progress owner was deleted.",
            ));
        }
        if let Some(origin) = input.recipient_origin {
            if self.current_provider_input_generation(input.owner).await != Some(origin.generation)
            {
                return Err(AgentMessagingError::new(
                    "stale_claim",
                    "The Codex root changed before its progress could be stored. Retry after the session is ready.",
                ));
            }
        }
        let progress = store::with_db(|conn| store::publish_provider_thread_progress(conn, input))?;
        self.cache_provider_thread_progress(&progress).await;
        Ok(progress)
    }

    async fn cache_provider_thread_progress(&self, progress: &store::ProviderThreadProgress) {
        let mut records = self.records.lock().await;
        for record in &progress.records {
            records.insert(record.id.clone(), record.clone());
        }
        drop(records);
        if !progress.duplicate {
            if let Some(owner) = progress
                .records
                .first()
                .and_then(|record| record.target_session_ids.first())
            {
                self.notify_agent_mailbox(owner, false).await;
            }
        }
    }
}
