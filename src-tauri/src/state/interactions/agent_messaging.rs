//! In-memory projections are published only after v2's canonical DB transaction commits.
use super::*;
use wardian_core::agent_messaging::AgentMessagingError;
use wardian_core::db::agent_messaging as store;

impl InteractionState {
    /// Snapshot provider deliveries for a single pending receive call. Revisions
    /// are process-local wake signals, not durable acknowledgement cursors.
    pub async fn agent_message_provider_revision(&self, recipient: &str) -> u64 {
        self.agent_message_provider_revisions
            .lock()
            .await
            .get(recipient)
            .copied()
            .unwrap_or(0)
    }

    /// Revalidate a previously claimed task after asynchronous owner startup.
    pub async fn validate_agent_message_claim(
        &self,
        claim: &store::TaskClaim,
    ) -> Result<(), AgentMessagingError> {
        let _mutation = self.mutation_lock.lock().await;
        let recipient = claim.record.target_session_ids.first().ok_or_else(|| {
            AgentMessagingError::new("invalid_message", "Message has no recipient.")
        })?;
        if self.deleted_sessions.lock().await.contains(recipient)
            || self
                .current_provider_input_generation(recipient)
                .await
                .unwrap_or(0)
                != claim.generation
            || !store::with_db(|conn| store::owns_claim(conn, claim))?
        {
            return Err(AgentMessagingError::new(
                "stale_claim",
                "Message no longer owns the current delivery boundary.",
            ));
        }
        Ok(())
    }

    /// Claim one notification against the current runtime and deletion boundary.
    pub async fn claim_agent_information(
        &self,
        recipient: &str,
        id: &str,
        generation: u64,
    ) -> Result<Option<store::TaskClaim>, AgentMessagingError> {
        let _mutation = self.mutation_lock.lock().await;
        if self.deleted_sessions.lock().await.contains(recipient)
            || self
                .current_provider_input_generation(recipient)
                .await
                .unwrap_or(0)
                != generation
        {
            return Err(AgentMessagingError::new(
                "stale_claim",
                "Runtime generation changed before information delivery.",
            ));
        }
        store::with_db(|conn| store::claim_information(conn, recipient, id, generation))
    }

    /// Release only on an explicit prewrite transport outcome, never uncertainty.
    pub async fn release_agent_message_before_write(
        &self,
        claim: &store::TaskClaim,
    ) -> Result<(), AgentMessagingError> {
        let _mutation = self.mutation_lock.lock().await;
        store::with_db(|conn| store::release_before_write(conn, claim))
    }

    /// Serialize receiver claims with agent deletion so a stale authenticated
    /// request cannot recreate cursor metadata after canonical deletion commits.
    pub async fn receive_agent_messages(
        &self,
        recipient: &str,
        cursor: Option<&str>,
        ack_cursor: Option<&str>,
        limit: u32,
    ) -> Result<wardian_core::agent_messaging::AgentMessagePage, AgentMessagingError> {
        let _mutation = self.mutation_lock.lock().await;
        if self.deleted_sessions.lock().await.contains(recipient) {
            return Err(AgentMessagingError::new(
                "unauthorized",
                "Receiver was deleted.",
            ));
        }
        store::with_db(|conn| store::receive(conn, recipient, cursor, ack_cursor, limit))
    }

    /// Claim while the caller owns the runtime lifecycle boundary. Generation
    /// selection and durable claim publication share the interaction mutation gate.
    pub async fn claim_agent_task(
        &self,
        recipient: &str,
        expected_generation: u64,
    ) -> Result<Option<store::TaskClaim>, AgentMessagingError> {
        let _mutation = self.mutation_lock.lock().await;
        if self.deleted_sessions.lock().await.contains(recipient)
            || self
                .current_provider_input_generation(recipient)
                .await
                .unwrap_or(0)
                != expected_generation
        {
            return Err(AgentMessagingError::new(
                "stale_claim",
                "Runtime generation changed before delivery.",
            ));
        }
        store::with_db(|conn| store::claim_next_task(conn, recipient, expected_generation))
    }

    /// Late receipts from a replaced generation cannot publish provider visibility.
    /// Their original claim remains unavailable for automatic replay.
    pub async fn finish_agent_task(
        &self,
        claim: &store::TaskClaim,
        outcome: &str,
    ) -> Result<(), AgentMessagingError> {
        let _mutation = self.mutation_lock.lock().await;
        let recipient =
            claim.record.target_session_ids.first().ok_or_else(|| {
                AgentMessagingError::new("invalid_message", "Task has no recipient.")
            })?;
        if self.deleted_sessions.lock().await.contains(recipient)
            || self
                .current_provider_input_generation(recipient)
                .await
                .unwrap_or(0)
                != claim.generation
        {
            store::with_db(|conn| store::finish_claim(conn, claim, "uncertain"))?;
            return Err(AgentMessagingError::new(
                "stale_claim",
                "Runtime generation changed before receipt.",
            ));
        }
        store::with_db(|conn| store::finish_claim(conn, claim, outcome))?;
        if matches!(
            outcome,
            "provider_accepted" | "provider_visible" | "provider_completed"
        ) {
            let mut revisions = self.agent_message_provider_revisions.lock().await;
            let revision = revisions.entry(recipient.clone()).or_default();
            *revision = revision.wrapping_add(1);
        }
        Ok(())
    }

    /// Admit one typed message/task without provider I/O or legacy mailbox insertion.
    pub async fn admit_agent_message(
        &self,
        admission: store::Admission<'_>,
    ) -> Result<store::Admitted, AgentMessagingError> {
        let _mutation = self.mutation_lock.lock().await;
        let deleted = self.deleted_sessions.lock().await;
        if deleted.contains(admission.sender) || deleted.contains(admission.recipient) {
            return Err(AgentMessagingError::new(
                "not_found",
                "Sender or recipient was deleted.",
            ));
        }
        drop(deleted);
        let admitted = store::with_db(|conn| store::admit(conn, admission))?;
        self.records
            .lock()
            .await
            .insert(admitted.record.id.clone(), admitted.record.clone());
        Ok(admitted)
    }

    /// Authorized v2 completion is atomic in storage and then updates both caches.
    pub async fn reply_agent_message(
        &self,
        sender: &str,
        request_id: &str,
        status: ReplyStatus,
        message: &str,
    ) -> Result<store::Replied, AgentMessagingError> {
        self.publish_agent_reply(sender, request_id, status, message, None)
            .await
    }

    /// Complete the exact persisted scheduler claim after a definite failure
    /// before provider submission, even if lease expiry allowed runtime replacement.
    /// The transaction still requires its token, stored generation, and dispatching
    /// ownership. Never recreate deleted state or convert uncertainty into a reply.
    pub async fn fail_agent_startup(
        &self,
        claim: &store::TaskClaim,
    ) -> Result<store::Replied, AgentMessagingError> {
        let recipient =
            claim.record.target_session_ids.first().ok_or_else(|| {
                AgentMessagingError::new("invalid_message", "Task has no recipient.")
            })?;
        self.publish_agent_reply(
            recipient,
            &claim.record.id,
            ReplyStatus::Failed,
            "",
            Some(claim),
        )
        .await
    }

    async fn publish_agent_reply(
        &self,
        sender: &str,
        request_id: &str,
        status: ReplyStatus,
        message: &str,
        claim: Option<&store::TaskClaim>,
    ) -> Result<store::Replied, AgentMessagingError> {
        let _mutation = self.mutation_lock.lock().await;
        if self.deleted_sessions.lock().await.contains(sender) {
            return Err(AgentMessagingError::new(
                "unauthorized",
                "Reply sender was deleted.",
            ));
        }
        let replied = if let Some(claim) = claim {
            // Internal pre-submission settlement owns the captured DB claim,
            // not the replacement runtime. The transaction fences token reuse.
            store::with_db(|conn| store::reply_startup_failure(conn, claim))?
        } else {
            store::with_db(|conn| store::reply(conn, sender, request_id, status, message))?
        };
        let mut records = self.records.lock().await;
        records.insert(replied.task.id.clone(), replied.task.clone());
        records.insert(replied.record.id.clone(), replied.record.clone());
        drop(records);
        self.replies
            .lock()
            .await
            .insert(request_id.into(), replied.reply.clone());
        Ok(replied)
    }
}
