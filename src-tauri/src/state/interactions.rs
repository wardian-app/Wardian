use std::collections::{HashMap, HashSet};
use std::sync::Arc;
mod agent_messaging;
mod mailbox_wait;
mod task_turns;

pub use mailbox_wait::AgentMailboxSignal;

use tokio::sync::{watch, Mutex};
use wardian_core::control::{
    DeliveryErrorDetail, DeliveryTransportKind, InboxNotificationDecision, InboxNotificationKind,
    InboxNotificationPayload, InteractionBodyRef, InteractionDeliveryAttemptRecord,
    InteractionKind, InteractionRecord, InteractionStatus, InteractionTriggerPolicy,
    ProviderInputReadiness, ProviderInputState, ProviderReadyEvidence, ReplyStatus,
    StructuredReply,
};

#[derive(Debug, Default)]
pub struct InteractionState {
    mutation_lock: Arc<Mutex<()>>,
    deleted_sessions: Mutex<HashSet<String>>,
    // Crate-visible for remote projection tests that need to model source-lock
    // contention without adding another production synchronization path.
    pub(crate) records: Mutex<HashMap<String, InteractionRecord>>,
    replies: Mutex<HashMap<String, StructuredReply>>,
    provider_generations: Mutex<HashMap<String, u64>>,
    provider_status_observations: Mutex<HashMap<String, u64>>,
    provider_inputs: Mutex<HashMap<String, ProviderInputState>>,
    // Retain senders (including deletion tombstones) so late subscribers see
    // the current baseline even when no receiver existed during publication.
    agent_mailboxes: Mutex<HashMap<String, watch::Sender<AgentMailboxSignal>>>,
}

impl InteractionState {
    pub async fn create_message_durable(
        &self,
        sender_session_id: Option<String>,
        target_session_ids: Vec<String>,
        body_ref: InteractionBodyRef,
    ) -> Result<InteractionRecord, String> {
        let _mutation = self.mutation_lock.lock().await;
        let deleted_sessions = self.deleted_sessions.lock().await;
        if let Some(target_session_id) = target_session_ids
            .iter()
            .find(|target| deleted_sessions.contains(*target))
        {
            return Err(format!("agent has been deleted: {target_session_id}"));
        }
        drop(deleted_sessions);
        let record = message_record(
            new_interaction_id(),
            sender_session_id,
            target_session_ids,
            body_ref,
        );
        wardian_core::db::upsert_interaction_record(&record)
            .map_err(|error| format!("failed to persist interaction: {error}"))?;
        self.records
            .lock()
            .await
            .insert(record.id.clone(), record.clone());
        Ok(record)
    }

    /// Advances the durable lifecycle of one ordinary outbound message.
    ///
    /// A message has one target per interaction, so this state remains an
    /// authoritative answer to whether that target is queued, being sent, or
    /// has reached a terminal transport outcome.
    pub async fn update_message_status_durable(
        &self,
        interaction_id: &str,
        status: InteractionStatus,
    ) -> Result<InteractionRecord, String> {
        let _mutation = self.mutation_lock.lock().await;
        self.update_message_status_durable_locked(interaction_id, status)
            .await
    }

    async fn update_message_status_durable_locked(
        &self,
        interaction_id: &str,
        status: InteractionStatus,
    ) -> Result<InteractionRecord, String> {
        let mut records = self.records.lock().await;
        let current = records
            .get(interaction_id)
            .cloned()
            .ok_or_else(|| format!("message interaction not found: {interaction_id}"))?;
        if current.kind != InteractionKind::Message {
            return Err(format!("interaction is not a message: {interaction_id}"));
        }
        if current.status == status {
            return Ok(current);
        }

        let now = now_rfc3339_millis();
        let mut updated = current;
        updated.status = status;
        updated.updated_at = now.clone();
        updated.completed_at = matches!(
            status,
            InteractionStatus::Delivered | InteractionStatus::Failed
        )
        .then_some(now);
        wardian_core::db::upsert_interaction_record(&updated)
            .map_err(|error| format!("failed to persist message status: {error}"))?;
        records.insert(updated.id.clone(), updated.clone());
        Ok(updated)
    }

    pub async fn create_notification_durable(
        &self,
        sender_session_id: String,
        payload: InboxNotificationPayload,
    ) -> Result<InteractionRecord, &'static str> {
        let _mutation = self.mutation_lock.lock().await;
        let is_approval = matches!(payload.kind, InboxNotificationKind::Approval);
        let now = now_rfc3339_millis();
        let record = InteractionRecord {
            id: new_interaction_id(),
            kind: InteractionKind::Notification,
            sender_session_id: Some(sender_session_id.clone()),
            target_session_ids: Vec::new(),
            status: if is_approval {
                InteractionStatus::AwaitingReply
            } else {
                InteractionStatus::Completed
            },
            trigger_policy: if is_approval {
                InteractionTriggerPolicy::ReplyRequired
            } else {
                InteractionTriggerPolicy::NotifyOnly
            },
            body_ref: InteractionBodyRef::Inline {
                body: serde_json::to_string(&payload).map_err(|_| "invalid_notification")?,
            },
            parent_interaction_id: None,
            created_at: now.clone(),
            updated_at: now.clone(),
            completed_at: (!is_approval).then_some(now.clone()),
        };

        let mut records = self.records.lock().await;
        let expired_records = if is_approval {
            records
                .values()
                .filter(|existing| {
                    existing.kind == InteractionKind::Notification
                        && existing.sender_session_id.as_deref() == Some(sender_session_id.as_str())
                        && existing.status == InteractionStatus::AwaitingReply
                })
                .filter_map(|existing| {
                    let payload = notification_payload(existing)?;
                    is_notification_expired(&payload, &now).then(|| {
                        let mut expired = existing.clone();
                        expired.status = InteractionStatus::Expired;
                        expired.updated_at = now.clone();
                        expired.completed_at = Some(now.clone());
                        expired
                    })
                })
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        let has_open_approval = is_approval
            && records.values().any(|existing| {
                existing.kind == InteractionKind::Notification
                    && existing.sender_session_id.as_deref() == Some(sender_session_id.as_str())
                    && existing.status == InteractionStatus::AwaitingReply
                    && !expired_records
                        .iter()
                        .any(|expired| expired.id == existing.id)
            });
        if has_open_approval {
            return Err("approval_already_open");
        }
        let mut records_to_persist = expired_records.clone();
        records_to_persist.push(record.clone());
        wardian_core::db::upsert_interaction_records(&records_to_persist)
            .map_err(|_| "persistence_failed")?;
        for expired in expired_records {
            records.insert(expired.id.clone(), expired);
        }
        records.insert(record.id.clone(), record.clone());
        Ok(record)
    }

    pub async fn inbox_notifications_page(
        &self,
        offset: usize,
        limit: usize,
    ) -> (Vec<InteractionRecord>, bool) {
        let records = self.records.lock().await;
        let mut notifications = records
            .values()
            .filter(|record| record.kind == InteractionKind::Notification)
            .cloned()
            .collect::<Vec<_>>();
        notifications.sort_by(|left, right| {
            right
                .created_at
                .cmp(&left.created_at)
                .then_with(|| right.id.cmp(&left.id))
        });
        let mut page = notifications
            .into_iter()
            .skip(offset)
            .take(limit.saturating_add(1))
            .collect::<Vec<_>>();
        let truncated = page.len() > limit;
        page.truncate(limit);
        (page, truncated)
    }

    pub async fn resolve_notification(
        &self,
        notification_id: &str,
        choice: &str,
    ) -> Result<InboxNotificationDecision, &'static str> {
        let _mutation = self.mutation_lock.lock().await;
        let current = self
            .expire_notification_if_needed_locked(notification_id)
            .await
            .ok_or("not_found")?;
        if current.status == InteractionStatus::Expired {
            return Err("expired");
        }
        let now = now_rfc3339_millis();
        let decision = {
            let mut records = self.records.lock().await;
            let notification = records.get(notification_id).cloned().ok_or("not_found")?;
            if notification.kind != InteractionKind::Notification {
                return Err("not_notification");
            }
            if notification.status != InteractionStatus::AwaitingReply {
                return Err("already_resolved");
            }
            let payload = notification_payload(&notification).ok_or("invalid_notification")?;
            if !matches!(payload.kind, InboxNotificationKind::Approval) {
                return Err("not_approval");
            }
            if !payload.choices.iter().any(|candidate| candidate == choice) {
                return Err("invalid_choice");
            }
            let mut updated_notification = notification;
            updated_notification.status = InteractionStatus::Completed;
            updated_notification.updated_at = now.clone();
            updated_notification.completed_at = Some(now.clone());
            let decision = InboxNotificationDecision {
                choice: choice.to_string(),
                resolved_at: now.clone(),
            };
            let resolution = InteractionRecord {
                id: new_interaction_id(),
                kind: InteractionKind::Reply,
                sender_session_id: None,
                target_session_ids: updated_notification
                    .sender_session_id
                    .iter()
                    .cloned()
                    .collect(),
                status: InteractionStatus::Completed,
                trigger_policy: InteractionTriggerPolicy::NotifyOnly,
                body_ref: InteractionBodyRef::Inline {
                    body: serde_json::to_string(&decision).map_err(|_| "invalid_notification")?,
                },
                parent_interaction_id: Some(notification_id.to_string()),
                created_at: now.clone(),
                updated_at: now.clone(),
                completed_at: Some(now),
            };
            wardian_core::db::upsert_interaction_records(&[
                updated_notification.clone(),
                resolution.clone(),
            ])
            .map_err(|_| "persistence_failed")?;
            records.insert(
                updated_notification.id.clone(),
                updated_notification.clone(),
            );
            records.insert(resolution.id.clone(), resolution.clone());
            decision
        };
        Ok(decision)
    }

    pub async fn notification_decision(
        &self,
        notification_id: &str,
    ) -> Option<InboxNotificationDecision> {
        self.records.lock().await.values().find_map(|record| {
            (record.kind == InteractionKind::Reply
                && record.parent_interaction_id.as_deref() == Some(notification_id))
            .then(|| notification_decision(record))
            .flatten()
        })
    }

    pub async fn expire_notification_if_needed(
        &self,
        notification_id: &str,
    ) -> Option<InteractionRecord> {
        let _mutation = self.mutation_lock.lock().await;
        self.expire_notification_if_needed_locked(notification_id)
            .await
    }

    async fn expire_notification_if_needed_locked(
        &self,
        notification_id: &str,
    ) -> Option<InteractionRecord> {
        let now = now_rfc3339_millis();
        let expired = {
            let mut records = self.records.lock().await;
            let notification = records.get(notification_id).cloned()?;
            if notification.kind != InteractionKind::Notification
                || notification.status != InteractionStatus::AwaitingReply
            {
                return Some(notification.clone());
            }
            let payload = notification_payload(&notification)?;
            if !is_notification_expired(&payload, &now) {
                return Some(notification.clone());
            }
            let mut expired = notification;
            expired.status = InteractionStatus::Expired;
            expired.updated_at = now.clone();
            expired.completed_at = Some(now);
            if wardian_core::db::upsert_interaction_record(&expired).is_err() {
                return None;
            }
            records.insert(expired.id.clone(), expired.clone());
            expired
        };
        Some(expired)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn record_delivery_attempt(
        &self,
        interaction_id: &str,
        target_session_id: &str,
        transport: DeliveryTransportKind,
        generation: u64,
        runtime_state: &str,
        delivery_state: &str,
        delivery_phase: Option<String>,
        observed_state: Option<String>,
        reason: Option<String>,
        error: Option<DeliveryErrorDetail>,
    ) -> InteractionDeliveryAttemptRecord {
        let _mutation = self.mutation_lock.lock().await;
        let attempt = delivery_attempt_record(
            interaction_id,
            target_session_id,
            transport,
            generation,
            runtime_state,
            delivery_state,
            delivery_phase,
            observed_state,
            reason,
            error,
        );
        if !self
            .deleted_sessions
            .lock()
            .await
            .contains(target_session_id)
        {
            let _ = wardian_core::db::upsert_interaction_delivery_attempt(&attempt);
        }
        attempt
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn record_delivery_attempt_durable(
        &self,
        interaction_id: &str,
        target_session_id: &str,
        transport: DeliveryTransportKind,
        generation: u64,
        runtime_state: &str,
        delivery_state: &str,
        delivery_phase: Option<String>,
        observed_state: Option<String>,
        reason: Option<String>,
        error: Option<DeliveryErrorDetail>,
    ) -> Result<InteractionDeliveryAttemptRecord, String> {
        let _mutation = self.mutation_lock.lock().await;
        let attempt = delivery_attempt_record(
            interaction_id,
            target_session_id,
            transport,
            generation,
            runtime_state,
            delivery_state,
            delivery_phase,
            observed_state,
            reason,
            error,
        );
        if self
            .deleted_sessions
            .lock()
            .await
            .contains(target_session_id)
        {
            return Err(format!("agent has been deleted: {target_session_id}"));
        }
        wardian_core::db::upsert_interaction_delivery_attempt(&attempt)
            .map_err(|error| format!("failed to persist delivery attempt: {error}"))?;
        if let Some(status) = Self::message_status_for_delivery_state(delivery_state) {
            let is_message = self
                .records
                .lock()
                .await
                .get(interaction_id)
                .is_some_and(|record| record.kind == InteractionKind::Message);
            if is_message {
                self.update_message_status_durable_locked(interaction_id, status)
                    .await?;
            }
        }
        Ok(attempt)
    }

    fn message_status_for_delivery_state(delivery_state: &str) -> Option<InteractionStatus> {
        match delivery_state {
            "queued" => Some(InteractionStatus::Queued),
            "submit_started" | "submit_sent_unconfirmed" => Some(InteractionStatus::Delivering),
            "submitted"
            | "submit_sent_unverified"
            | "provider_applied"
            | "provider_accepted"
            | "approval_submitted" => Some(InteractionStatus::Delivered),
            "failed" => Some(InteractionStatus::Failed),
            _ => None,
        }
    }

    pub async fn record_provider_input_state(
        &self,
        session_id: &str,
        generation: u64,
        state: ProviderInputReadiness,
        ready_evidence: Option<ProviderReadyEvidence>,
    ) -> ProviderInputState {
        self.record_provider_input_state_with_transition(
            session_id,
            generation,
            state,
            ready_evidence,
        )
        .await
        .0
    }

    /// Records provider readiness and reports whether this write established a
    /// new ready generation. Callers can use the transition flag for one-shot
    /// work without repeatedly sweeping an already-ready provider.
    pub async fn record_provider_input_state_with_transition(
        &self,
        session_id: &str,
        generation: u64,
        state: ProviderInputReadiness,
        ready_evidence: Option<ProviderReadyEvidence>,
    ) -> (ProviderInputState, bool) {
        let _mutation = self.mutation_lock.lock().await;
        if self.deleted_sessions.lock().await.contains(session_id) {
            return (
                provider_input_state_record(session_id, generation, state, ready_evidence),
                false,
            );
        }
        let _observations = self.provider_status_observations.lock().await;
        self.record_provider_input_state_inner(session_id, generation, state, ready_evidence)
            .await
    }

    async fn record_provider_input_state_inner(
        &self,
        session_id: &str,
        generation: u64,
        state: ProviderInputReadiness,
        ready_evidence: Option<ProviderReadyEvidence>,
    ) -> (ProviderInputState, bool) {
        if self.deleted_sessions.lock().await.contains(session_id) {
            return (
                provider_input_state_record(session_id, generation, state, ready_evidence),
                false,
            );
        }
        {
            let mut generations = self.provider_generations.lock().await;
            let current = generations
                .entry(session_id.to_string())
                .or_insert(generation);
            if generation > *current {
                *current = generation;
            }
        }

        let mut inputs = self.provider_inputs.lock().await;
        if let Some(existing) = inputs.get(session_id) {
            if keep_existing_provider_input_state(existing, generation, state, ready_evidence) {
                return (existing.clone(), false);
            }
        }
        let became_ready = state == ProviderInputReadiness::Ready
            && inputs.get(session_id).is_none_or(|existing| {
                existing.generation != generation || existing.state != ProviderInputReadiness::Ready
            });
        let record = ProviderInputState {
            session_id: session_id.to_string(),
            generation,
            state,
            ready_evidence,
            observed_at: now_rfc3339_millis(),
        };
        inputs.insert(session_id.to_string(), record.clone());
        let _ = wardian_core::db::upsert_provider_input_state(&record);
        (record, became_ready)
    }

    pub async fn record_provider_input_status_observation(
        &self,
        session_id: &str,
        status_sequence: u64,
        generation: u64,
        state: ProviderInputReadiness,
        ready_evidence: Option<ProviderReadyEvidence>,
    ) -> ProviderInputState {
        self.record_provider_input_status_observation_with_transition(
            session_id,
            status_sequence,
            generation,
            state,
            ready_evidence,
        )
        .await
        .0
    }

    pub async fn record_provider_input_status_observation_with_transition(
        &self,
        session_id: &str,
        status_sequence: u64,
        generation: u64,
        state: ProviderInputReadiness,
        ready_evidence: Option<ProviderReadyEvidence>,
    ) -> (ProviderInputState, bool) {
        let _mutation = self.mutation_lock.lock().await;
        if self.deleted_sessions.lock().await.contains(session_id) {
            return (
                provider_input_state_record(session_id, generation, state, ready_evidence),
                false,
            );
        }
        let mut observations = self.provider_status_observations.lock().await;
        if matches!(
            observations.get(session_id).copied(),
            Some(current) if status_sequence < current
        ) {
            if let Some(existing) = self.provider_inputs.lock().await.get(session_id).cloned() {
                return (existing, false);
            }
            return (
                provider_input_state_record(session_id, generation, state, ready_evidence),
                false,
            );
        } else {
            observations.insert(session_id.to_string(), status_sequence);
        }

        self.record_provider_input_state_inner(session_id, generation, state, ready_evidence)
            .await
    }

    pub async fn provider_input_state(&self, session_id: &str) -> Option<ProviderInputState> {
        self.provider_inputs.lock().await.get(session_id).cloned()
    }

    pub async fn start_provider_input_generation(
        &self,
        session_id: &str,
        state: ProviderInputReadiness,
        ready_evidence: Option<ProviderReadyEvidence>,
    ) -> ProviderInputState {
        let _mutation = self.mutation_lock.lock().await;
        if self.deleted_sessions.lock().await.contains(session_id) {
            return provider_input_state_record(session_id, 0, state, ready_evidence);
        }
        let _observations = self.provider_status_observations.lock().await;
        let generation = {
            let mut generations = self.provider_generations.lock().await;
            let generation = generations.get(session_id).copied().unwrap_or(0) + 1;
            generations.insert(session_id.to_string(), generation);
            generation
        };
        self.record_provider_input_state_inner(session_id, generation, state, ready_evidence)
            .await
            .0
    }

    pub async fn current_provider_input_generation(&self, session_id: &str) -> Option<u64> {
        self.provider_generations
            .lock()
            .await
            .get(session_id)
            .copied()
    }

    pub async fn clear_provider_input_state_in_memory(&self, session_id: &str) {
        self.provider_status_observations
            .lock()
            .await
            .remove(session_id);
        self.provider_generations.lock().await.remove(session_id);
        self.provider_inputs.lock().await.remove(session_id);
    }

    pub async fn clear_deleted_session(&self, session_id: &str) {
        let _mutation = self.mutation_lock.lock().await;
        self.deleted_sessions.lock().await.remove(session_id);
        self.revive_agent_mailbox(session_id).await;
    }

    pub async fn hydrate_from_persistence(&self) {
        // A new process cannot continue observing an old live turn. Captured
        // terminal outcomes remain publishable without rerunning that turn.
        if let Err(error) = wardian_core::db::agent_messaging::with_db(
            wardian_core::db::agent_messaging::abandon_task_turn_observations,
        ) {
            crate::manager::log_debug(&format!("[WARDIAN] task observation recovery: {error}"));
        }
        if let Err(error) = self.recover_agent_task_results().await {
            crate::manager::log_debug(&format!("[WARDIAN] task result recovery: {error}"));
        }
        if let Ok(records) = wardian_core::db::list_interaction_records() {
            let mut current = self.records.lock().await;
            for record in records {
                current.insert(record.id.clone(), record);
            }
        }
        if let Ok(replies) = wardian_core::db::list_structured_replies() {
            let mut current = self.replies.lock().await;
            for reply in replies {
                current.insert(reply.request_id.clone(), reply);
            }
        }
        if let Ok(inputs) = wardian_core::db::list_provider_input_states() {
            let mut generations = self.provider_generations.lock().await;
            let mut current = self.provider_inputs.lock().await;
            for input in inputs {
                generations.insert(input.session_id.clone(), input.generation);
                current.insert(input.session_id.clone(), input);
            }
        }
    }

    pub async fn interaction(&self, id: &str) -> Option<InteractionRecord> {
        self.records.lock().await.get(id).cloned()
    }

    pub async fn structured_reply(&self, task_id: &str) -> Option<StructuredReply> {
        self.replies.lock().await.get(task_id).cloned()
    }

    /// Deletes an agent's durable interaction state and invalidates the live
    /// task/reply cache under the same mutation gate used by reply completion.
    /// A late provider reply therefore observes `not_found` instead of
    /// recreating a task that was already deleted.
    /// Once admitted, an owned continuation retains the gate through physical
    /// SQLite completion and cache invalidation even if its caller is cancelled.
    pub async fn delete_agent_durable_state(
        self: &Arc<Self>,
        session_id: &str,
    ) -> Result<(), String> {
        let mutation = Arc::clone(&self.mutation_lock).lock_owned().await;
        let owner = Arc::clone(self);
        let session_id = session_id.to_string();
        #[cfg(test)]
        let probe = DELETE_IO_PROBE
            .try_with(|probe| probe.borrow_mut().take())
            .ok()
            .flatten();
        tokio::spawn(async move {
            let _mutation = mutation;
            let database_session_id = session_id.clone();
            tokio::task::spawn_blocking(move || {
                #[cfg(test)]
                if let Some(probe) = probe {
                    let _ = probe.started.send(());
                    let _ = probe.entered.send(());
                    probe.release.recv().map_err(|error| error.to_string())?;
                }
                wardian_core::db::delete_agent(&database_session_id)
                    .map_err(|error| format!("Failed to delete agent state: {error}"))
            })
            .await
            .map_err(|error| format!("Agent state deletion I/O task failed: {error}"))??;
            owner.delete_agent_cached_state(&session_id).await;
            Ok(())
        })
        .await
        .map_err(|error| format!("Agent state deletion continuation failed: {error}"))?
    }

    async fn delete_agent_cached_state(&self, session_id: &str) {
        self.deleted_sessions
            .lock()
            .await
            .insert(session_id.to_string());

        let mut records = self.records.lock().await;
        let mut removed_ids = records
            .iter()
            .filter(|(_, record)| {
                record.sender_session_id.as_deref() == Some(session_id)
                    || record
                        .target_session_ids
                        .iter()
                        .any(|target| target == session_id)
            })
            .map(|(id, _)| id.clone())
            .collect::<HashSet<_>>();
        loop {
            let descendants = records
                .iter()
                .filter(|(id, record)| {
                    !removed_ids.contains(*id)
                        && record
                            .parent_interaction_id
                            .as_deref()
                            .is_some_and(|parent_id| removed_ids.contains(parent_id))
                })
                .map(|(id, _)| id.clone())
                .collect::<Vec<_>>();
            if descendants.is_empty() {
                break;
            }
            removed_ids.extend(descendants);
        }
        records.retain(|id, record| {
            !removed_ids.contains(id)
                && record.sender_session_id.as_deref() != Some(session_id)
                && !record
                    .target_session_ids
                    .iter()
                    .any(|target| target == session_id)
        });
        drop(records);

        self.replies.lock().await.retain(|request_id, reply| {
            !removed_ids.contains(request_id)
                && reply.target_session_id != session_id
                && reply.source_session_id.as_deref() != Some(session_id)
        });
        self.provider_status_observations
            .lock()
            .await
            .remove(session_id);
        self.provider_generations.lock().await.remove(session_id);
        self.provider_inputs.lock().await.remove(session_id);
        self.mark_agent_mailbox_deleted(session_id).await;
    }
}

#[cfg(test)]
pub(crate) struct DeleteIoProbe {
    pub(crate) entered: tokio::sync::oneshot::Sender<()>,
    pub(crate) started: std::sync::mpsc::Sender<()>,
    pub(crate) release: std::sync::mpsc::Receiver<()>,
}

#[cfg(test)]
tokio::task_local! {
    pub(crate) static DELETE_IO_PROBE: std::cell::RefCell<Option<DeleteIoProbe>>;
}

fn provider_input_state_record(
    session_id: &str,
    generation: u64,
    state: ProviderInputReadiness,
    ready_evidence: Option<ProviderReadyEvidence>,
) -> ProviderInputState {
    ProviderInputState {
        session_id: session_id.to_string(),
        generation,
        state,
        ready_evidence,
        observed_at: now_rfc3339_millis(),
    }
}

fn notification_payload(record: &InteractionRecord) -> Option<InboxNotificationPayload> {
    let InteractionBodyRef::Inline { body } = &record.body_ref else {
        return None;
    };
    serde_json::from_str(body).ok()
}

fn notification_decision(record: &InteractionRecord) -> Option<InboxNotificationDecision> {
    let InteractionBodyRef::Inline { body } = &record.body_ref else {
        return None;
    };
    serde_json::from_str(body).ok()
}

fn is_notification_expired(payload: &InboxNotificationPayload, now: &str) -> bool {
    let Some(expires_at) = payload.expires_at.as_deref() else {
        return false;
    };
    let Ok(expires_at) = chrono::DateTime::parse_from_rfc3339(expires_at) else {
        return true;
    };
    let Ok(now) = chrono::DateTime::parse_from_rfc3339(now) else {
        return false;
    };
    expires_at <= now
}

fn new_interaction_id() -> String {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let counter = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let millis = chrono::Utc::now().timestamp_millis();
    format!("int_{millis:013}_{counter:06}")
}

fn message_record(
    id: String,
    sender_session_id: Option<String>,
    target_session_ids: Vec<String>,
    body_ref: InteractionBodyRef,
) -> InteractionRecord {
    let now = now_rfc3339_millis();
    InteractionRecord {
        id,
        kind: InteractionKind::Message,
        sender_session_id,
        target_session_ids,
        status: InteractionStatus::Queued,
        trigger_policy: InteractionTriggerPolicy::StartTurn,
        body_ref,
        parent_interaction_id: None,
        created_at: now.clone(),
        updated_at: now,
        completed_at: None,
    }
}

fn new_delivery_attempt_id() -> String {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let counter = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let millis = chrono::Utc::now().timestamp_millis();
    format!("attempt_{millis:013}_{counter:06}")
}

#[allow(clippy::too_many_arguments)]
fn delivery_attempt_record(
    interaction_id: &str,
    target_session_id: &str,
    transport: DeliveryTransportKind,
    generation: u64,
    runtime_state: &str,
    delivery_state: &str,
    delivery_phase: Option<String>,
    observed_state: Option<String>,
    reason: Option<String>,
    error: Option<DeliveryErrorDetail>,
) -> InteractionDeliveryAttemptRecord {
    let now = now_rfc3339_millis();
    InteractionDeliveryAttemptRecord {
        id: new_delivery_attempt_id(),
        interaction_id: interaction_id.to_string(),
        target_session_id: target_session_id.to_string(),
        transport,
        generation,
        runtime_state: runtime_state.to_string(),
        delivery_state: delivery_state.to_string(),
        delivery_phase,
        observed_state,
        reason,
        error,
        created_at: now.clone(),
        updated_at: now,
    }
}

fn now_rfc3339_millis() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

fn keep_existing_provider_input_state(
    existing: &ProviderInputState,
    generation: u64,
    next_state: ProviderInputReadiness,
    next_evidence: Option<ProviderReadyEvidence>,
) -> bool {
    if generation < existing.generation {
        return true;
    }
    if generation > existing.generation {
        return false;
    }
    if existing.state == next_state && existing.ready_evidence == next_evidence {
        return true;
    }
    if existing.state == next_state
        && existing.ready_evidence == Some(ProviderReadyEvidence::ProviderEvent)
        && next_evidence.is_none()
    {
        return true;
    }
    next_state == ProviderInputReadiness::Ready
        && !matches!(next_evidence, Some(ProviderReadyEvidence::ProviderEvent))
        && matches!(
            existing.state,
            ProviderInputReadiness::Busy | ProviderInputReadiness::ActionRequired
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Admits a v2 task through the production admission path.
    pub(super) async fn admit_task(
        state: &InteractionState,
        sender: &str,
        recipient: &str,
        body: &str,
    ) -> InteractionRecord {
        state
            .admit_agent_message(wardian_core::db::agent_messaging::Admission {
                sender,
                recipient,
                message: body,
                idempotency_key: None,
                task: true,
                generation: 0,
            })
            .await
            .unwrap()
            .record
    }

    #[tokio::test]
    async fn create_message_records_start_turn_interaction() {
        let _guard = crate::utils::wardian_test_env_lock_async().await;
        let home = tempfile::tempdir().unwrap();
        wardian_core::db::init_db_at_path(&home.path().join("state.db")).unwrap();
        let state = InteractionState::default();

        let record = state
            .create_message_durable(
                Some("source-agent".to_string()),
                vec!["target-agent".to_string()],
                InteractionBodyRef::Inline {
                    body: "hello".to_string(),
                },
            )
            .await
            .unwrap();

        assert_eq!(record.kind, InteractionKind::Message);
        assert_eq!(record.status, InteractionStatus::Queued);
        assert_eq!(record.trigger_policy, InteractionTriggerPolicy::StartTurn);
        assert_eq!(record.sender_session_id.as_deref(), Some("source-agent"));
        assert_eq!(record.target_session_ids, vec!["target-agent".to_string()]);
    }

    #[tokio::test]
    async fn durable_delivery_attempts_advance_message_transport_status() {
        let _guard = crate::utils::wardian_test_env_lock_async().await;
        let home = tempfile::tempdir().unwrap();
        wardian_core::db::init_db_at_path(&home.path().join("state.db")).unwrap();
        let state = InteractionState::default();
        let message = state
            .create_message_durable(
                None,
                vec!["target-agent".to_string()],
                InteractionBodyRef::Inline {
                    body: "hello".to_string(),
                },
            )
            .await
            .unwrap();

        state
            .record_delivery_attempt_durable(
                &message.id,
                "target-agent",
                DeliveryTransportKind::LiveSurface,
                1,
                "live_pty_available",
                "submit_started",
                Some("payload_sent".to_string()),
                Some("payload_sent".to_string()),
                None,
                None,
            )
            .await
            .unwrap();
        assert_eq!(
            state.interaction(&message.id).await.unwrap().status,
            InteractionStatus::Delivering
        );

        state
            .record_delivery_attempt_durable(
                &message.id,
                "target-agent",
                DeliveryTransportKind::LiveSurface,
                1,
                "live_pty_available",
                "submit_sent_unconfirmed",
                Some("submit_key_sent".to_string()),
                Some("bytes_sent".to_string()),
                None,
                None,
            )
            .await
            .unwrap();
        let unconfirmed = state.interaction(&message.id).await.unwrap();
        assert_eq!(unconfirmed.status, InteractionStatus::Delivering);
        assert!(unconfirmed.completed_at.is_none());

        state
            .record_delivery_attempt_durable(
                &message.id,
                "target-agent",
                DeliveryTransportKind::LiveSurface,
                1,
                "live_pty_available",
                "provider_accepted",
                Some("turn_started".to_string()),
                Some("turn_started".to_string()),
                None,
                None,
            )
            .await
            .unwrap();
        let delivered = state.interaction(&message.id).await.unwrap();
        assert_eq!(delivered.status, InteractionStatus::Delivered);
        assert!(delivered.completed_at.is_some());
    }

    #[tokio::test]
    async fn expired_approval_does_not_block_a_new_approval_from_the_same_agent() {
        let _guard = crate::utils::wardian_test_env_lock_async().await;
        let home = tempfile::tempdir().unwrap();
        wardian_core::db::init_db_at_path(&home.path().join("state.db")).unwrap();
        let state = InteractionState::default();
        let expired = state
            .create_notification_durable(
                "agent-1".to_string(),
                InboxNotificationPayload {
                    kind: InboxNotificationKind::Approval,
                    title: "Expired approval".to_string(),
                    body: "This must not keep the slot open.".to_string(),
                    proposed_action: Some("Deploy".to_string()),
                    risk: Some("Changes production".to_string()),
                    choices: vec!["Approve".to_string(), "Reject".to_string()],
                    expires_at: Some(
                        (chrono::Utc::now() - chrono::Duration::minutes(1)).to_rfc3339(),
                    ),
                },
            )
            .await
            .unwrap();

        let replacement = state
            .create_notification_durable(
                "agent-1".to_string(),
                InboxNotificationPayload {
                    kind: InboxNotificationKind::Approval,
                    title: "Replacement approval".to_string(),
                    body: "This can use the released slot.".to_string(),
                    proposed_action: Some("Deploy".to_string()),
                    risk: Some("Changes production".to_string()),
                    choices: vec!["Approve".to_string(), "Reject".to_string()],
                    expires_at: Some(
                        (chrono::Utc::now() + chrono::Duration::minutes(5)).to_rfc3339(),
                    ),
                },
            )
            .await
            .unwrap();

        assert_eq!(
            state.interaction(&expired.id).await.unwrap().status,
            InteractionStatus::Expired
        );
        assert_eq!(replacement.status, InteractionStatus::AwaitingReply);
    }

    #[tokio::test]
    async fn record_delivery_attempt_generates_stable_attempt_record() {
        let _guard = crate::utils::wardian_test_env_lock_async().await;
        let home = tempfile::tempdir().unwrap();
        wardian_core::db::init_db_at_path(&home.path().join("state.db")).unwrap();
        let state = InteractionState::default();
        let interaction = state
            .create_message_durable(
                None,
                vec!["agent-1".to_string()],
                InteractionBodyRef::Inline {
                    body: "hello".to_string(),
                },
            )
            .await
            .unwrap();

        let attempt = state
            .record_delivery_attempt(
                &interaction.id,
                "agent-1",
                DeliveryTransportKind::LiveSurface,
                1,
                "live_pty_available",
                "submit_sent_unconfirmed",
                Some("submit_key_sent".to_string()),
                Some("bytes_sent".to_string()),
                None,
                None,
            )
            .await;

        assert!(attempt.id.starts_with("attempt_"));
        assert_eq!(attempt.interaction_id, interaction.id);
        assert_eq!(attempt.transport, DeliveryTransportKind::LiveSurface);
        assert_eq!(attempt.delivery_state, "submit_sent_unconfirmed");
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

    #[tokio::test]
    async fn prompt_readiness_does_not_override_same_generation_busy_or_action_required() {
        let state = InteractionState::default();
        state
            .record_provider_input_state("agent-1", 1, ProviderInputReadiness::Busy, None)
            .await;
        state
            .record_provider_input_state(
                "agent-1",
                1,
                ProviderInputReadiness::Ready,
                Some(ProviderReadyEvidence::PromptDetected),
            )
            .await;

        let busy = state.provider_input_state("agent-1").await.unwrap();
        assert_eq!(busy.state, ProviderInputReadiness::Busy);

        state
            .record_provider_input_state("agent-1", 1, ProviderInputReadiness::ActionRequired, None)
            .await;
        state
            .record_provider_input_state(
                "agent-1",
                1,
                ProviderInputReadiness::Ready,
                Some(ProviderReadyEvidence::TitleDetected),
            )
            .await;

        let action_required = state.provider_input_state("agent-1").await.unwrap();
        assert_eq!(
            action_required.state,
            ProviderInputReadiness::ActionRequired
        );
    }

    #[tokio::test]
    async fn provider_event_readiness_can_complete_same_generation_busy_state() {
        let state = InteractionState::default();
        state
            .record_provider_input_state("agent-1", 1, ProviderInputReadiness::Busy, None)
            .await;
        state
            .record_provider_input_state(
                "agent-1",
                1,
                ProviderInputReadiness::Ready,
                Some(ProviderReadyEvidence::ProviderEvent),
            )
            .await;

        let current = state.provider_input_state("agent-1").await.unwrap();
        assert_eq!(current.state, ProviderInputReadiness::Ready);
        assert_eq!(
            current.ready_evidence,
            Some(ProviderReadyEvidence::ProviderEvent)
        );
    }

    #[tokio::test]
    async fn repeated_provider_readiness_observation_reuses_existing_state() {
        let state = InteractionState::default();
        let initial = state
            .record_provider_input_state(
                "agent-1",
                1,
                ProviderInputReadiness::Ready,
                Some(ProviderReadyEvidence::ProviderEvent),
            )
            .await;

        let repeated = state
            .record_provider_input_state(
                "agent-1",
                1,
                ProviderInputReadiness::Ready,
                Some(ProviderReadyEvidence::ProviderEvent),
            )
            .await;

        assert!(keep_existing_provider_input_state(
            &initial,
            1,
            ProviderInputReadiness::Ready,
            Some(ProviderReadyEvidence::ProviderEvent)
        ));
        assert_eq!(repeated.observed_at, initial.observed_at);
    }

    #[tokio::test]
    async fn readiness_transition_is_reported_once_per_generation() {
        let state = InteractionState::default();
        state
            .record_provider_input_state("agent-1", 1, ProviderInputReadiness::Busy, None)
            .await;

        let (_, first_ready) = state
            .record_provider_input_state_with_transition(
                "agent-1",
                1,
                ProviderInputReadiness::Ready,
                Some(ProviderReadyEvidence::ProviderEvent),
            )
            .await;
        let (_, repeated_ready) = state
            .record_provider_input_state_with_transition(
                "agent-1",
                1,
                ProviderInputReadiness::Ready,
                Some(ProviderReadyEvidence::ProviderEvent),
            )
            .await;
        let (_, next_generation_ready) = state
            .record_provider_input_state_with_transition(
                "agent-1",
                2,
                ProviderInputReadiness::Ready,
                Some(ProviderReadyEvidence::ProviderEvent),
            )
            .await;

        assert!(first_ready);
        assert!(!repeated_ready);
        assert!(next_generation_ready);
    }

    #[tokio::test]
    async fn starting_new_provider_generation_invalidates_previous_ready_state() {
        let state = InteractionState::default();
        state
            .start_provider_input_generation(
                "agent-1",
                ProviderInputReadiness::Ready,
                Some(ProviderReadyEvidence::ProviderEvent),
            )
            .await;
        state
            .start_provider_input_generation("agent-1", ProviderInputReadiness::Booting, None)
            .await;
        state
            .record_provider_input_state(
                "agent-1",
                1,
                ProviderInputReadiness::Ready,
                Some(ProviderReadyEvidence::PromptDetected),
            )
            .await;

        let current = state.provider_input_state("agent-1").await.unwrap();
        assert_eq!(current.generation, 2);
        assert_eq!(current.state, ProviderInputReadiness::Booting);
        assert_eq!(
            state.current_provider_input_generation("agent-1").await,
            Some(2)
        );
    }

    #[tokio::test]
    async fn interactions_and_provider_state_hydrate_from_persistence() {
        let _guard = crate::utils::wardian_test_env_lock_async().await;
        let home = tempfile::tempdir().unwrap();
        wardian_core::db::init_db_at_path(&home.path().join("state.db")).unwrap();

        let session_id = "hydrate-provider-agent-1";
        let state = InteractionState::default();
        let task = admit_task(&state, "planner-1", session_id, "review").await;
        state
            .reply_agent_message(session_id, &task.id, ReplyStatus::Blocked, "blocked")
            .await
            .unwrap();
        state
            .start_provider_input_generation(
                session_id,
                ProviderInputReadiness::Ready,
                Some(ProviderReadyEvidence::ProviderEvent),
            )
            .await;

        let hydrated = InteractionState::default();
        hydrated.hydrate_from_persistence().await;

        assert_eq!(
            hydrated.interaction(&task.id).await.unwrap().status,
            InteractionStatus::Completed
        );
        assert_eq!(
            hydrated.structured_reply(&task.id).await.unwrap().status,
            ReplyStatus::Blocked
        );
        assert_eq!(
            hydrated
                .provider_input_state(session_id)
                .await
                .unwrap()
                .state,
            ProviderInputReadiness::Ready
        );
    }
}

#[cfg(test)]
mod reply_tests {
    use super::tests::admit_task;
    use super::*;
    use wardian_core::control::ReplyStatus;

    #[tokio::test]
    async fn completed_task_exposes_structured_reply_status() {
        let _guard = crate::utils::wardian_test_env_lock_async().await;
        let home = tempfile::tempdir().unwrap();
        wardian_core::db::init_db_at_path(&home.path().join("state.db")).unwrap();
        let state = InteractionState::default();
        let task = admit_task(&state, "planner-1", "agent-1", "review").await;

        state
            .reply_agent_message("agent-1", &task.id, ReplyStatus::Blocked, "blocked")
            .await
            .unwrap();

        assert_eq!(
            state.interaction(&task.id).await.unwrap().status,
            InteractionStatus::Completed
        );
        let reply = state.structured_reply(&task.id).await.unwrap();
        assert_eq!(reply.request_id, task.id);
        assert_eq!(reply.status, ReplyStatus::Blocked);
        assert_eq!(reply.body, "blocked");
        assert_eq!(reply.target_session_id, "agent-1");
        assert_eq!(reply.source_session_id.as_deref(), Some("agent-1"));
    }

    #[tokio::test]
    async fn deleting_agent_invalidates_cached_tasks_before_late_reply() {
        let _guard = crate::utils::wardian_test_env_lock_async().await;
        let home = tempfile::tempdir().unwrap();
        let previous_home = std::env::var_os("WARDIAN_HOME");
        unsafe { std::env::set_var("WARDIAN_HOME", home.path()) };
        wardian_core::db::init_db_at_path(&home.path().join("state.db")).unwrap();

        let state = Arc::new(InteractionState::default());
        let task = admit_task(&state, "agent-delete", "agent-target", "review").await;
        let answered = admit_task(&state, "agent-requester", "agent-delete", "review").await;
        state
            .reply_agent_message("agent-delete", &answered.id, ReplyStatus::Done, "done")
            .await
            .unwrap();
        assert!(state.structured_reply(&answered.id).await.is_some());

        // Older databases can hold a sender-less failure reply. Only its
        // parent task names the deleted agent, so hydration must not let it
        // outlive that agent.
        let now = now_rfc3339_millis();
        let legacy_task = InteractionRecord {
            id: "legacy-task".to_string(),
            kind: InteractionKind::Task,
            sender_session_id: None,
            target_session_ids: vec!["agent-delete".to_string()],
            status: InteractionStatus::Failed,
            trigger_policy: InteractionTriggerPolicy::ReplyRequired,
            body_ref: InteractionBodyRef::Inline {
                body: "review without a sender".to_string(),
            },
            parent_interaction_id: None,
            created_at: now.clone(),
            updated_at: now.clone(),
            completed_at: Some(now.clone()),
        };
        let legacy_reply = InteractionRecord {
            id: "legacy-reply".to_string(),
            kind: InteractionKind::Reply,
            sender_session_id: None,
            target_session_ids: Vec::new(),
            status: InteractionStatus::Completed,
            trigger_policy: InteractionTriggerPolicy::NotifyOnly,
            body_ref: InteractionBodyRef::Inline {
                body: "timed out".to_string(),
            },
            parent_interaction_id: Some(legacy_task.id.clone()),
            created_at: now.clone(),
            updated_at: now.clone(),
            completed_at: Some(now),
        };
        wardian_core::db::upsert_interaction_record(&legacy_task).unwrap();
        wardian_core::db::upsert_interaction_record(&legacy_reply).unwrap();
        state.hydrate_from_persistence().await;
        assert!(state.interaction(&legacy_reply.id).await.is_some());

        state
            .delete_agent_durable_state("agent-delete")
            .await
            .unwrap();
        assert!(state.interaction(&task.id).await.is_none());
        assert!(state.interaction(&answered.id).await.is_none());
        assert!(state.structured_reply(&answered.id).await.is_none());
        assert!(state.interaction(&legacy_task.id).await.is_none());
        assert!(state.interaction(&legacy_reply.id).await.is_none());
        assert!(
            !wardian_core::db::list_interaction_records()
                .unwrap()
                .iter()
                .any(|record| record.parent_interaction_id.as_deref()
                    == Some(legacy_task.id.as_str()))
        );
        assert_eq!(
            state
                .reply_agent_message("agent-target", &task.id, ReplyStatus::Done, "late")
                .await
                .err()
                .unwrap()
                .code,
            "not_found"
        );

        match previous_home {
            Some(value) => unsafe { std::env::set_var("WARDIAN_HOME", value) },
            None => unsafe { std::env::remove_var("WARDIAN_HOME") },
        }
    }

    #[tokio::test]
    async fn deletion_serializes_queued_delivery_and_provider_writes() {
        let _guard = crate::utils::wardian_test_env_lock_async().await;
        let home = tempfile::tempdir().unwrap();
        let previous_home = std::env::var_os("WARDIAN_HOME");
        unsafe { std::env::set_var("WARDIAN_HOME", home.path()) };
        wardian_core::db::init_db_at_path(&home.path().join("state.db")).unwrap();

        let state = std::sync::Arc::new(InteractionState::default());
        let mutation = state.mutation_lock.lock().await;
        let provider_writer = std::sync::Arc::clone(&state);
        let provider_write = tokio::spawn(async move {
            provider_writer
                .record_provider_input_state(
                    "agent-delete",
                    1,
                    ProviderInputReadiness::Ready,
                    Some(ProviderReadyEvidence::ProviderEvent),
                )
                .await;
        });
        tokio::task::yield_now().await;
        let delete_state = std::sync::Arc::clone(&state);
        let deletion = tokio::spawn(async move {
            delete_state
                .delete_agent_durable_state("agent-delete")
                .await
        });
        drop(mutation);
        provider_write.await.unwrap();
        deletion.await.unwrap().unwrap();
        assert!(wardian_core::db::list_provider_input_states()
            .unwrap()
            .iter()
            .all(|record| record.session_id != "agent-delete"));

        let mutation = state.mutation_lock.lock().await;
        let delivery_writer = std::sync::Arc::clone(&state);
        let delivery_write = tokio::spawn(async move {
            delivery_writer
                .record_delivery_attempt(
                    "interaction-delete",
                    "agent-delete",
                    DeliveryTransportKind::LiveSurface,
                    1,
                    "live_pty_available",
                    "failed",
                    Some("test".to_string()),
                    None,
                    Some("test".to_string()),
                    None,
                )
                .await;
        });
        tokio::task::yield_now().await;
        let delete_state = std::sync::Arc::clone(&state);
        let deletion = tokio::spawn(async move {
            delete_state
                .delete_agent_durable_state("agent-delete")
                .await
        });
        drop(mutation);
        delivery_write.await.unwrap();
        deletion.await.unwrap().unwrap();
        assert!(
            wardian_core::db::list_interaction_delivery_attempts("interaction-delete")
                .unwrap()
                .iter()
                .all(|attempt| attempt.target_session_id != "agent-delete")
        );
        assert!(state
            .create_message_durable(
                None,
                vec!["agent-delete".to_string()],
                InteractionBodyRef::Inline {
                    body: "late message".to_string(),
                },
            )
            .await
            .is_err());
        let records_before_late_task = wardian_core::db::list_interaction_records().unwrap();
        let rejected_task = state
            .admit_agent_message(wardian_core::db::agent_messaging::Admission {
                sender: "agent-other",
                recipient: "agent-delete",
                message: "late ask",
                idempotency_key: None,
                task: true,
                generation: 0,
            })
            .await
            .err()
            .unwrap();
        assert_eq!(rejected_task.code, "not_found");
        assert_eq!(
            wardian_core::db::list_interaction_records().unwrap(),
            records_before_late_task
        );

        let provider_state = state
            .record_provider_input_state(
                "agent-delete",
                2,
                ProviderInputReadiness::Ready,
                Some(ProviderReadyEvidence::ProviderEvent),
            )
            .await;
        assert_eq!(provider_state.session_id, "agent-delete");
        state
            .start_provider_input_generation("agent-delete", ProviderInputReadiness::Booting, None)
            .await;
        state
            .record_provider_input_status_observation(
                "agent-delete",
                1,
                3,
                ProviderInputReadiness::Busy,
                None,
            )
            .await;
        let non_durable_attempt = state
            .record_delivery_attempt(
                "interaction-delete",
                "agent-delete",
                DeliveryTransportKind::LiveSurface,
                2,
                "deleted",
                "failed",
                None,
                None,
                Some("late callback".to_string()),
                None,
            )
            .await;
        assert_eq!(non_durable_attempt.target_session_id, "agent-delete");
        assert!(state
            .record_delivery_attempt_durable(
                "interaction-delete",
                "agent-delete",
                DeliveryTransportKind::LiveSurface,
                2,
                "deleted",
                "failed",
                None,
                None,
                Some("late callback".to_string()),
                None,
            )
            .await
            .is_err());
        assert!(state.provider_input_state("agent-delete").await.is_none());
        assert!(wardian_core::db::list_provider_input_states()
            .unwrap()
            .iter()
            .all(|record| record.session_id != "agent-delete"));
        assert!(
            wardian_core::db::list_interaction_delivery_attempts("interaction-delete")
                .unwrap()
                .iter()
                .all(|attempt| attempt.target_session_id != "agent-delete")
        );

        match previous_home {
            Some(value) => unsafe { std::env::set_var("WARDIAN_HOME", value) },
            None => unsafe { std::env::remove_var("WARDIAN_HOME") },
        }
    }
}
