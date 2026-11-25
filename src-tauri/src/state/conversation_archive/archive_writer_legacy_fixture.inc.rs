// Fixture-only helpers included by the owning module during tests.

pub(crate) fn recent_watch_policy(
    agent_id: &str,
) -> io::Result<impl Fn(&AgentChatEvent, Option<&str>) -> bool> {
    let (state, _) = storage::read_recent_capture_state(agent_id)?;
    Ok(move |event: &AgentChatEvent, source: Option<&str>| {
        state.permits_recent_watch(event, source)
    })
}

pub(crate) fn recent_native_time_policy(
    agent_id: &str,
) -> io::Result<impl Fn(Option<&str>, Option<&str>) -> bool> {
    let (state, _) = storage::read_recent_capture_state(agent_id)?;
    Ok(move |source: Option<&str>, created: Option<&str>| {
        state.permits_recent_native_time(source, created)
    })
}

impl ConversationCaptureState {
    fn permits_recent_native_time(&self, source: Option<&str>, created: Option<&str>) -> bool {
        // A bounded native tail cannot reconstruct legacy sequence-derived
        // identities. An unresolved identity exclusion therefore stays a miss.
        if (source.is_none() && !self.skip_event_ids.is_empty())
            || self.skip_event_scopes.iter().any(|scope| {
                scope.provider_source_key.as_deref() == source && !scope.event_ids.is_empty()
            })
        {
            return false;
        }
        let created = created
            .map(chrono::DateTime::parse_from_rfc3339)
            .transpose();
        let Ok(created) = created else {
            return false;
        };
        let after = |cutoff: Option<&str>| match cutoff {
            None => true,
            Some(cutoff) => created
                .zip(chrono::DateTime::parse_from_rfc3339(cutoff).ok())
                .is_some_and(|(created, cutoff)| created > cutoff),
        };
        if source.is_none() && !after(self.skip_events_at_or_before.as_deref()) {
            return false;
        }
        if self.skip_event_scopes.iter().any(|scope| {
            scope.provider_source_key.as_deref() == source
                && (!after(scope.skip_events_at_or_before.as_deref())
                    || scope.disabled_from.is_some())
        }) {
            return false;
        }
        !self.provider_log_sources.iter().any(|policy| {
            Some(policy.provider_source_key.as_str()) == source
                && (policy.unknown_before_offset.unwrap_or(0) > 0
                    || !policy.disabled_spans.is_empty()
                    || policy.open_disabled_from.is_some())
        })
    }
}

impl ConversationCaptureState {
    fn initial_source_is_unrestricted(
        source: &crate::commands::provider_log_acquisition::ProviderLogCaptureState,
    ) -> bool {
        source.policy_generation == 0
            && source.unknown_before_offset.unwrap_or(0) == 0
            && source.disabled_spans.is_empty()
            && source.open_disabled_from.is_none()
    }
}

impl ConversationCaptureState {
    fn recent_policy_stamp(&self) -> io::Result<String> {
        let sources = self
            .provider_log_sources
            .iter()
            // The first unrestricted observation adds acquisition bookkeeping,
            // not an exclusion for already observed Watch messages. The recent
            // scope pins the actual file identity separately. Keep every real
            // cutoff, disabled interval, and later policy generation in the fence.
            .filter(|source| !Self::initial_source_is_unrestricted(source))
            .map(|source| {
                serde_json::json!({
                    "key": source.provider_source_key, "identity": source.native_identity,
                    "generation": source.policy_generation, "unknown": source.unknown_before_offset,
                    "disabled": source.disabled_spans, "open": source.open_disabled_from,
                })
            })
            .collect::<Vec<_>>();
        Ok(storage::digest(
            &serde_json::to_vec(&serde_json::json!({
                "cutoff": self.skip_events_at_or_before, "ids": self.skip_event_ids,
                "scopes": self.skip_event_scopes, "sources": sources,
            }))
            .map_err(io::Error::other)?,
        ))
    }
}

impl ConversationCaptureState {
    fn permits_recent_watch(&self, event: &AgentChatEvent, source_key: Option<&str>) -> bool {
        let undated_cutoff = event.created_at.is_none()
            && ((source_key.is_none() && self.skip_events_at_or_before.is_some())
                || self.skip_event_scopes.iter().any(|scope| {
                    scope.provider_source_key.as_deref() == source_key
                        && (scope.skip_events_at_or_before.is_some()
                            || scope.disabled_from.is_some())
                }));
        // Apply the original identity first. Watch supplies no byte coordinates,
        // so it cannot prove a native row lies beyond an excluded source span.
        !undated_cutoff
            && !self.should_skip_event(event, source_key)
            && !self.provider_log_sources.iter().any(|source| {
                Some(source.provider_source_key.as_str()) == source_key
                    && (source.unknown_before_offset.unwrap_or(0) > 0
                        || !source.disabled_spans.is_empty()
                        || source.open_disabled_from.is_some())
            })
    }
}

impl ConversationArchiveState {
    pub(crate) fn archive_completion_events(
        &self,
        context: &ConversationArchiveContext,
    ) -> io::Result<Vec<AgentChatEvent>> {
        self.archive_completion_events_with_live(context, &[])
    }
}

impl ConversationArchiveState {
    /// Derived history admission shares archive serialization, but never binds
    /// the mutable writer handle or changes canonical archive files.
    pub(crate) fn bootstrap_saved_chat(
        &self,
        context: &ConversationArchiveContext,
    ) -> io::Result<bool> {
        let agent_lock = agent_lock_for(&self.agent_locks, &context.agent_id)?;
        let _guard = lock_agent_archive(&agent_lock)?;
        self.chat_projection.bootstrap_saved(context)
    }
}

impl ConversationArchiveState {
    /// Build an old projection only in fixtures that exercise migration readers.
    fn publish_chat_candidate(&self, mut candidate: chat_read::Candidate) -> io::Result<()> {
        let capture = read_capture_state(&candidate.context.agent_id)?;
        compatibility_claims::restore_candidate(&mut candidate, &capture)?;

        candidate.source_epoch = capture
            .provider_log_sources
            .iter()
            .find(|source| {
                candidate.context.provider_source_key.as_deref()
                    == Some(source.provider_source_key.as_str())
            })
            .map(|source| {
                crate::commands::chat_recent_seed::hash(
                    &serde_json::to_vec(&source.native_identity).unwrap_or_default(),
                )
            });
        // Legacy rows and partially appended batches are not upgraded by a
        // subsequent generated write. Native rows need their committed source
        // coordinate; generated rows need this owned conversation identity.
        let generated_prefix = format!("generated:{}:", candidate.conversation_id);
        for event in &mut candidate.events {
            if event.metadata["generated"] == true {
                // Legacy text-selected links are evidence in the archive, but
                // cannot become verified aliases in the display projection.
                if let Some(metadata) = event.metadata.as_object_mut() {
                    for key in [
                        "chat_source_ref",
                        "chat_source_start",
                        "chat_source_end",
                        "chat_source_epoch",
                        "legacy_event_ids",
                        "request_root_id",
                    ] {
                        metadata.remove(key);
                    }
                    metadata.insert(
                        "chat_identity_resolution".into(),
                        serde_json::json!("unresolved"),
                    );
                }
            }
        }
        let archived_events = std::mem::take(&mut candidate.events);
        let display_events = archived_events
            .into_iter()
            .filter(|event| {
                event.session_id == candidate.context.agent_id
                    && (event.provider == candidate.context.provider
                        || chat_read::is_owned_unknown_input(&candidate, event))
            })
            .collect();
        candidate.events = display_events;
        candidate.events.retain(|event| {
            if event.metadata["generated"] == true {
                return event.id.starts_with(&generated_prefix)
                    && candidate.generated_ids.contains(&event.id);
            }
            if event.metadata["provider_log"] != true {
                return true;
            }
            if matches!(candidate.context.provider.as_str(), "codex" | "pi")
                && !event.metadata["chat_source_ref"].is_string()
            {
                // Keep historical physical envelopes usable. Only the separate
                // qualified relation index may supply display correspondence.
                return true;
            }
            if matches!(
                candidate.context.provider.as_str(),
                "opencode" | "antigravity"
            ) && candidate.verified_ids.contains(&event.id)
            {
                return true;
            }
            capture.provider_log_sources.iter().any(|source| {
                candidate.context.provider_source_key.as_deref()
                    == Some(source.provider_source_key.as_str())
                    && event.metadata["chat_source_epoch"].as_str()
                        == Some(
                            crate::commands::chat_recent_seed::hash(
                                &serde_json::to_vec(&source.native_identity).unwrap_or_default(),
                            )
                            .as_str(),
                        )
                    && event.metadata["chat_source_start"]
                        .as_u64()
                        .zip(event.metadata["chat_source_end"].as_u64())
                        .is_some_and(|(start, end)| {
                            start < end
                                && end <= source.committed_offset
                                && start >= source.unknown_before_offset.unwrap_or(0)
                                && source
                                    .open_disabled_from
                                    .is_none_or(|disabled| end <= disabled)
                                && !source
                                    .disabled_spans
                                    .iter()
                                    .any(|span| start < span.end && end > span.start)
                        })
            })
        });
        let stamp = capture
            .provider_log_sources
            .iter()
            .map(|source| {
                serde_json::json!({
                    "source": source.provider_source_key, "identity": source.native_identity,
                    "offset": source.committed_offset, "policy": source.policy_generation,
                    "disabled": source.disabled_spans, "open_disabled": source.open_disabled_from,
                })
            })
            .collect::<Vec<_>>();
        self.chat_projection.committed(
            candidate,
            crate::commands::chat_recent_seed::hash(
                &serde_json::to_vec(&stamp).map_err(io::Error::other)?,
            ),
        )
    }
}

impl ConversationArchiveState {
    pub fn set_active_for_test(&self, agent_id: &str, handle: ActiveConversationHandle) {
        self.active
            .lock()
            .expect("active conversation lock")
            .insert(agent_id.to_string(), handle);
    }
}

impl ConversationArchiveState {
    pub fn fail_next_rollover_after_close_for_test(&self) {
        self.fail_next_rollover_after_close
            .store(true, Ordering::SeqCst);
    }
}

impl ConversationArchiveState {
    pub(crate) fn fail_after_chat_event_append_for_test(&self) {
        self.fail_after_chat_event_append
            .store(true, Ordering::SeqCst);
    }
}

impl ConversationArchiveState {
    pub fn active_conversation_id_for_test(&self, agent_id: &str) -> Option<String> {
        self.active_conversation_id(agent_id)
            .expect("active conversation lock")
    }
}
