//! In-memory projections are published only after v2's canonical DB transaction commits.
use super::*;
use wardian_core::agent_messaging::AgentMessagingError;
use wardian_core::db::agent_messaging as store;

impl InteractionState {
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

    /// Claim an automatically wakeable new informational message. The store
    /// rechecks the admission marker atomically, so historical rows cannot be
    /// promoted by a racing startup or status wake.
    pub async fn claim_agent_wakeable_information(
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
                "Runtime generation changed before informational wake delivery.",
            ));
        }
        store::with_db(|conn| store::claim_wakeable_information(conn, recipient, id, generation))
    }

    /// Permanently fence a wakeable informational message whose durable
    /// recipient conversation no longer matches the live provider session.
    pub async fn stale_agent_wakeable_information(
        &self,
        id: &str,
    ) -> Result<bool, AgentMessagingError> {
        let _mutation = self.mutation_lock.lock().await;
        store::with_db(|conn| store::stale_wakeable_information(conn, id))
    }

    /// Claim one durable task reply for continuation into its original requester
    /// conversation. This is independent of the FYI push claim path.
    pub async fn claim_agent_reply_continuation(
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
                "Runtime generation changed before reply continuation.",
            ));
        }
        store::with_db(|conn| store::claim_reply_continuation(conn, recipient, id, generation))
    }

    /// Persist queued or blocked reply disposition on the existing delivery row.
    pub async fn set_agent_reply_continuation_state(
        &self,
        id: &str,
        disposition: &str,
    ) -> Result<bool, AgentMessagingError> {
        let _mutation = self.mutation_lock.lock().await;
        store::with_db(|conn| store::set_reply_continuation_state(conn, id, disposition))
    }

    /// Release only on an explicit prewrite transport outcome, never uncertainty.
    pub async fn release_agent_message_before_write(
        &self,
        claim: &store::TaskClaim,
    ) -> Result<(), AgentMessagingError> {
        let _mutation = self.mutation_lock.lock().await;
        store::with_db(|conn| store::release_before_write(conn, claim))?;
        if let Some(recipient) = claim.record.target_session_ids.first() {
            self.notify_agent_mailbox(recipient, false).await;
        }
        Ok(())
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
            self.notify_agent_mailbox(recipient, true).await;
        }
        Ok(())
    }

    /// Admit a followup and its requester's durable conversation binding in the
    /// same canonical transaction, under the caller's lifecycle fence.
    pub async fn admit_agent_message_with_requester_origin(
        &self,
        admission: store::Admission<'_>,
        requester_origin: Option<&store::RequesterOrigin>,
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
        let recipient = admission.recipient;
        let admitted = store::with_db(|conn| {
            store::admit_with_requester_origin(conn, admission, requester_origin)
        })?;
        self.records
            .lock()
            .await
            .insert(admitted.record.id.clone(), admitted.record.clone());
        if !admitted.duplicate {
            self.notify_agent_mailbox(recipient, false).await;
        }
        Ok(admitted)
    }

    /// Atomically admit a managed informational message and its server-owned
    /// wake marker/recipient binding on the existing canonical delivery row.
    pub async fn admit_agent_message_with_wake_on_admission(
        &self,
        admission: store::Admission<'_>,
        recipient_origin: Option<&store::RecipientOrigin>,
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
        let recipient = admission.recipient;
        let admitted = store::with_db(|conn| {
            store::admit_with_wake_on_admission(conn, admission, recipient_origin)
        })?;
        self.records
            .lock()
            .await
            .insert(admitted.record.id.clone(), admitted.record.clone());
        if !admitted.duplicate {
            self.notify_agent_mailbox(recipient, false).await;
        }
        Ok(admitted)
    }

    /// Only the in-process automation runner can select host provenance; control
    /// requests still pass managed-agent authentication and ordinary admission.
    pub(crate) async fn admit_host_automation_task(
        &self,
        run_id: &str,
        node: &str,
        recipient: &str,
        message: &str,
    ) -> Result<store::Admitted, AgentMessagingError> {
        let _mutation = self.mutation_lock.lock().await;
        if self.deleted_sessions.lock().await.contains(recipient) {
            return Err(AgentMessagingError::new(
                "not_found",
                "Recipient was deleted.",
            ));
        }
        let generation = self
            .current_provider_input_generation(recipient)
            .await
            .unwrap_or(0);
        let admitted = store::with_db(|conn| {
            store::admit_host_automation_task(conn, run_id, node, recipient, message, generation)
        })?;
        self.records
            .lock()
            .await
            .insert(admitted.record.id.clone(), admitted.record.clone());
        if !admitted.duplicate {
            self.notify_agent_mailbox(recipient, false).await;
        }
        Ok(admitted)
    }

    pub(crate) async fn check_automation_recipient(
        &self,
        recipient: &str,
    ) -> Result<(), AgentMessagingError> {
        if self.deleted_sessions.lock().await.contains(recipient) {
            return Err(AgentMessagingError::new(
                "not_found",
                "Recipient was deleted.",
            ));
        }
        Ok(())
    }

    /// Host-only informational delivery. Serialized with deletion and all other
    /// interaction mutations; canonical admission completes before cache updates.
    pub(crate) async fn admit_host_automation_message(
        &self,
        run_id: &str,
        node: &str,
        recipient: &str,
        message: &str,
    ) -> Result<store::Admitted, AgentMessagingError> {
        let _mutation = self.mutation_lock.lock().await;
        if self.deleted_sessions.lock().await.contains(recipient) {
            return Err(AgentMessagingError::new(
                "not_found",
                "Recipient was deleted.",
            ));
        }
        let generation = self
            .current_provider_input_generation(recipient)
            .await
            .unwrap_or(0);
        let admitted = store::with_db(|conn| {
            store::admit_host_automation_message(conn, run_id, node, recipient, message, generation)
        })?;
        self.records
            .lock()
            .await
            .insert(admitted.record.id.clone(), admitted.record.clone());
        if !admitted.duplicate {
            self.notify_agent_mailbox(recipient, false).await;
        }
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
        self.cache_agent_reply(&replied).await;
        Ok(replied)
    }

    /// Update projections and wake recipients only after the reply commits.
    /// The caller holds mutation_lock, including automatic and recovered replies.
    pub(super) async fn cache_agent_reply(&self, replied: &store::Replied) {
        let mut records = self.records.lock().await;
        records.insert(replied.task.id.clone(), replied.task.clone());
        records.insert(replied.record.id.clone(), replied.record.clone());
        drop(records);
        self.replies
            .lock()
            .await
            .insert(replied.reply.request_id.clone(), replied.reply.clone());
        if !replied.duplicate {
            for recipient in &replied.record.target_session_ids {
                self.notify_agent_mailbox(recipient, false).await;
            }
        }
    }
}

/// Test-only compatibility helpers for durable mailbox regression fixtures.
#[cfg(test)]
mod test_support {
    use super::*;

    impl InteractionState {
        /// Serialize receiver claims with deletion for legacy regression fixtures.
        pub(crate) async fn receive_agent_messages(
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

        /// Read mailbox activity without claiming messages in legacy fixtures.
        pub(crate) async fn has_unacknowledged_agent_messages(
            &self,
            recipient: &str,
        ) -> Result<bool, AgentMessagingError> {
            let _mutation = self.mutation_lock.lock().await;
            if self.deleted_sessions.lock().await.contains(recipient) {
                return Err(AgentMessagingError::new(
                    "unauthorized",
                    "Receiver was deleted.",
                ));
            }
            store::with_db(|conn| {
                Ok(conn.query_row(
                    "SELECT EXISTS(
                        SELECT 1 FROM agent_message_availability a
                        LEFT JOIN agent_message_delivery d ON d.interaction_id=a.interaction_id
                        WHERE a.recipient=?1
                        AND a.sequence > COALESCE(
                            (SELECT sequence FROM agent_message_ack WHERE recipient=?1),0
                        )
                        AND (d.owner IS NULL OR d.owner IN ('stored','pending','receiver_available'))
                    )",
                    [recipient],
                    |row| row.get(0),
                )?)
            })
        }

        /// Preserve test admission fixtures without exposing the originless API.
        pub(crate) async fn admit_agent_message(
            &self,
            admission: store::Admission<'_>,
        ) -> Result<store::Admitted, AgentMessagingError> {
            self.admit_agent_message_with_requester_origin(admission, None)
                .await
        }
    }
}
