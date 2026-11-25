// Fixture-only helpers included by the owning module during tests.

/// A failed enqueue only blocks a page that still needs recent capture.
pub(crate) fn chat_page_needs_recent_capture(page: &AgentChatPage) -> bool {
    page.events.is_empty() && !page.reset && !page.unchanged
}

fn memory_event_belongs_to_conversation(
    event: &wardian_core::memory::MemoryEvent,
    conversation_started_at: Option<&str>,
) -> bool {
    let Some(started_at) = conversation_started_at else {
        return false;
    };
    match (
        chrono::DateTime::parse_from_rfc3339(&event.occurred_at),
        chrono::DateTime::parse_from_rfc3339(started_at),
    ) {
        (Ok(occurred_at), Ok(started_at)) => occurred_at >= started_at,
        _ => event.occurred_at.as_str() >= started_at,
    }
}

#[derive(Default)]
struct RecentWatchTags {
    rows: HashMap<String, (String, String)>,
    cursor: Option<String>,
}

impl RecentWatchTags {
    fn apply(&self, events: &mut [AgentChatEvent]) {
        for event in events {
            if let Some(metadata) = event.metadata.as_object_mut() {
                metadata.remove("chat_recent_key");
                metadata.remove("chat_recent_version");
                // Retention/status changes can reuse a legacy mapper ordinal.
                // Qualify the exact actual Watch snapshot, not that ID alone.
                if let Some((key, version)) = self.rows.get(&event.id).filter(|_| {
                    self.cursor.is_some()
                        && metadata
                            .get("transcript_cursor")
                            .and_then(serde_json::Value::as_str)
                            == self.cursor.as_deref()
                }) {
                    metadata.insert("chat_recent_key".into(), serde_json::json!(key));
                    metadata.insert("chat_recent_version".into(), serde_json::json!(version));
                }
            }
        }
    }
}

fn publish_watch_recent(
    state: &AppState,
    snapshot: &AgentArchiveCaptureSnapshot,
) -> std::io::Result<RecentWatchTags> {
    use crate::state::conversation_archive::chat_read;
    if !state
        .conversation_capture_policy_lock
        .background_admission_open()
    {
        return Err(std::io::Error::other(
            "recent admission waits for lifecycle boundary",
        ));
    }
    // A path discovered for this runtime does not describe every Watch row's
    // origin. Only actual file-derived rows need this exact parser receipt.
    let file_watch = if let Some(path) = &snapshot.log_path {
        let receipt = snapshot
            .watch_state
            .lock()
            .map_err(|_| std::io::Error::other("watch poisoned"))?
            .recent_file_source()
            .cloned();
        match receipt {
            Some((source, sequence)) if source.matches_path(path) && source.valid()? => {
                Some((source, sequence))
            }
            _ => None,
        }
    } else {
        None
    };
    let (current, runtime) =
        chat_read_snapshot_for_state(state, &snapshot.session_id).map_err(std::io::Error::other)?;
    if !Arc::ptr_eq(&current.watch_state, &snapshot.watch_state)
        || conversation_archive_context_from_snapshot(&current).provider_source_key
            != conversation_archive_context_from_snapshot(snapshot).provider_source_key
    {
        return Err(std::io::Error::other("recent producer retired"));
    }
    let owner = &state.conversation_archive.chat_projection;
    if owner.has_recent_native(snapshot, &runtime) {
        return Ok(RecentWatchTags::default());
    }
    let admission = owner.begin_recent_with_source(
        snapshot,
        &runtime,
        file_watch.as_ref().map(|(source, _)| source.clone()),
    )?;
    let token = &admission.0;
    let policy = crate::state::conversation_archive::recent_watch_policy(&snapshot.session_id)?;
    let (global_logging, _) =
        crate::utils::shell::recent_conversation_logging().map_err(std::io::Error::other)?;
    let disabled =
        effective_conversation_logging(global_logging, snapshot.agent_conversation_logging)
            == ConversationLoggingSetting::Disabled;
    let live_started = state
        .conversation_archive
        .live_conversation_started_at(&snapshot.session_id)?;
    let source_key = conversation_archive_context_from_snapshot(snapshot).provider_source_key;
    let mut headers = Vec::new();
    let mut tags = HashMap::new();
    let observed_cursor = {
        let watch = snapshot
            .watch_state
            .lock()
            .map_err(|_| std::io::Error::other("watch poisoned"))?;
        if !watch.codex_attachment_ready() {
            return Err(std::io::Error::other("recent watch attachment pending"));
        }
        let live = watch.live_chat_boundary().filter(|(started, _)| {
            live_started.as_ref() == Some(started)
                && (snapshot.resume_session.is_none()
                    || provider_log_source_is_fresh(
                        &snapshot.provider,
                        snapshot.resume_session.as_deref(),
                        snapshot.fresh_provider_session_id.as_deref(),
                    ))
        });
        if disabled && watch.live_chat_boundary().is_some() && live.is_none() {
            return Err(std::io::Error::other("recent live boundary retired"));
        }
        let transcript = WatchTranscript {
            cursor: watch.latest_cursor(),
            messages: Vec::new(),
            latest_text: String::new(),
            truncated: false,
            omitted_bytes: 0,
        };
        watch.visit_recent_transcript(
            &snapshot.current_status,
            |native_sequence, sequence, file_derived, message| {
                if file_derived
                    && !file_watch
                        .as_ref()
                        .is_some_and(|(_, bound)| native_sequence == *bound)
                {
                    return;
                }
                if message.role.len() > 64
                    || message.provider.len() > 32
                    || message.turn_id.as_ref().is_some_and(|id| id.len() > 256)
                    || message
                        .source
                        .as_ref()
                        .is_some_and(|source| source.len() > 128)
                    || (message.provider != "mock" && message.provider_provenance.is_some())
                {
                    return;
                }
                if message.source.as_deref() == Some("opencode_db") {
                    return;
                }
                // Policy uses only the original mapper ID and metadata, never text.
                // Do not clone a potentially huge Watch body just to classify it.
                let shallow = WatchTranscriptMessage {
                    role: message.role.clone(),
                    text: String::new(),
                    provider: message.provider.clone(),
                    turn_id: message.turn_id.clone(),
                    source: message.source.clone(),
                    provider_provenance: None,
                };
                let mut event = message_event_from_transcript(
                    &snapshot.session_id,
                    &snapshot.provider,
                    sequence,
                    &shallow,
                    &transcript,
                );
                let durable_permitted = policy(&event, source_key.as_deref());
                let owned_live = disabled
                    && !file_derived
                    && live.is_some_and(|(_, floor)| native_sequence > *floor)
                    && message.source.as_deref() != Some("opencode_db");
                if event.provider != snapshot.provider
                    || (!durable_permitted && !owned_live)
                    || (disabled && live.is_some_and(|(_, floor)| native_sequence <= *floor))
                {
                    return;
                }
                let mut end = message.text.len().min(chat_read::PREVIEW_BYTES);
                while !message.text.is_char_boundary(end) {
                    end -= 1;
                }
                event.text = Some(message.text[..end].to_owned());
                let original_id = event.id.clone();
                let key = format!("{token}:watch:{native_sequence}");
                let version = crate::commands::chat_recent_seed::hash(key.as_bytes());
                tags.insert(original_id, (key.clone(), version.clone()));
                event.id = format!("preview:{token}:{native_sequence}");
                event.sequence = Some(native_sequence);
                event.metadata["chat_recent_key"] = serde_json::json!(key);
                event.metadata["chat_recent_version"] = serde_json::json!(version);
                headers.push(chat_read::header(&event));
            },
        );
        transcript.cursor
    };
    headers.reverse();
    let (after, after_runtime) =
        chat_read_snapshot_for_state(state, &snapshot.session_id).map_err(std::io::Error::other)?;
    if !state
        .conversation_capture_policy_lock
        .background_admission_open()
        || !Arc::ptr_eq(&runtime, &after_runtime)
        || !Arc::ptr_eq(&after.watch_state, &snapshot.watch_state)
        || !owner.publish_recent(&after, &runtime, &admission, headers)?
    {
        return Err(std::io::Error::other("recent producer publication retired"));
    }
    owner.note_recent_coverage(&admission, &tags)?;
    owner.note_recent_watch(&admission, observed_cursor.clone())?;
    Ok(RecentWatchTags {
        rows: tags,
        cursor: Some(observed_cursor),
    })
}

/// A bounded owner prelude shared with normal selection dispatch. It neither
/// waits for capture/archive work. Native source discovery stays in this
/// background/blocking producer, never in selection request dispatch.
pub(crate) fn publish_recent_for_state(
    state: &AppState,
    session_id: &str,
    expected_runtime: Option<&Arc<Mutex<String>>>,
) -> std::io::Result<()> {
    let (snapshot, runtime) =
        chat_read_snapshot_for_state(state, session_id).map_err(std::io::Error::other)?;
    if expected_runtime.is_some_and(|expected| !Arc::ptr_eq(expected, &runtime)) {
        return Err(std::io::Error::other("recent request runtime retired"));
    }
    if publish_database_recent(state, &snapshot, &runtime)? {
        return Ok(());
    }
    publish_watch_recent(state, &snapshot).map(|_| ())
}

fn publish_database_recent(
    state: &AppState,
    snapshot: &AgentArchiveCaptureSnapshot,
    runtime: &Arc<Mutex<String>>,
) -> std::io::Result<bool> {
    use super::chat_recent_database;
    if snapshot.provider == "antigravity" {
        return publish_antigravity_recent(state, snapshot, runtime);
    }
    if snapshot.provider != "opencode" {
        return Ok(false);
    }
    let Some(native_session) = opencode_session_id(
        &snapshot.session_id,
        snapshot.resume_session.as_deref(),
        snapshot.fresh_provider_session_id.as_deref(),
    ) else {
        return Ok(false);
    };
    let Some(path) = opencode_database_path() else {
        return Ok(false);
    };
    let (logging, _) =
        crate::utils::shell::recent_conversation_logging().map_err(std::io::Error::other)?;
    if effective_conversation_logging(logging, snapshot.agent_conversation_logging)
        == ConversationLoggingSetting::Disabled
    {
        return Ok(false); // A database cannot use a current Watch's LIVE receipt.
    }
    if !state
        .conversation_capture_policy_lock
        .background_admission_open()
    {
        return Err(std::io::Error::other(
            "recent database admission waits for lifecycle boundary",
        ));
    }
    let owner = &state.conversation_archive.chat_projection;
    let source = chat_recent_database::Source::open(&path)?;
    let receipt = owner.begin_recent_with_source(snapshot, runtime, Some(source.clone()))?;
    let result = (|| {
        let policy =
            crate::state::conversation_archive::recent_native_time_policy(&snapshot.session_id)?;
        let source_key = conversation_archive_context_from_snapshot(snapshot).provider_source_key;
        let batch = chat_recent_database::opencode(&path, &native_session)?;
        if batch.source != source {
            return Err(std::io::Error::other(
                "recent database source changed during acquisition",
            ));
        }
        let mut headers = Vec::new();
        for row in batch.rows {
            let created = row
                .part
                .part_time_created
                .or(row.part.message_time_created)
                .and_then(chrono::DateTime::from_timestamp_millis)
                .map(|time| time.to_rfc3339());
            // This native time/identity policy intentionally withholds an
            // unresolved legacy-ID cutoff. No guessed sequence reaches it.
            if !policy(source_key.as_deref(), created.as_deref()) {
                continue;
            }
            let identity = format!("preview:{}:{}", receipt.0, row.key);
            let Some(mut event) = opencode_db_part_to_chat_event_with_identity(
                &snapshot.session_id,
                &native_session,
                None,
                row.part,
                None,
                Some(identity),
            )
            .map_err(std::io::Error::other)?
            else {
                continue;
            };
            event.created_at = created;
            normalize_chat_event_visible_text(&mut event);
            if event.text.as_ref().is_none_or(|text| text.is_empty()) {
                continue;
            }
            event.metadata["chat_recent_key"] =
                serde_json::json!(format!("{}:{}", receipt.0, row.key));
            event.metadata["chat_recent_version"] = serde_json::json!(row.version);
            let mut header = crate::state::conversation_archive::chat_read::header(&event);
            // These are the actual bounded native keys, not reconstructed
            // canonical identities or a sequence in a truncated DB window.
            for key in ["opencode_session_id", "part_id"] {
                if let Some(value) = event.metadata.get(key) {
                    header.metadata[key] = value.clone();
                }
            }
            headers.push(header);
        }
        let (after, after_runtime) = chat_read_snapshot_for_state(state, &snapshot.session_id)
            .map_err(std::io::Error::other)?;
        if !Arc::ptr_eq(runtime, &after_runtime)
            || !Arc::ptr_eq(&snapshot.watch_state, &after.watch_state)
            || !state
                .conversation_capture_policy_lock
                .background_admission_open()
            || !owner.publish_recent(&after, runtime, &receipt, headers)?
        {
            return Err(std::io::Error::other("recent database publication retired"));
        }
        let _metrics = batch.metrics;
        Ok(true)
    })();
    if result.is_err() {
        owner.abandon_recent(&receipt);
    }
    result
}

fn publish_antigravity_recent(
    state: &AppState,
    snapshot: &AgentArchiveCaptureSnapshot,
    runtime: &Arc<Mutex<String>>,
) -> std::io::Result<bool> {
    use super::chat_recent_database;
    let Some(session) = snapshot
        .resume_session
        .as_deref()
        .or(snapshot.fresh_provider_session_id.as_deref())
    else {
        return Ok(false);
    };
    let Some(path) = snapshot
        .log_path
        .as_deref()
        .filter(|path| path.extension().is_some_and(|extension| extension == "db"))
    else {
        return Ok(false);
    };
    if path.file_stem().and_then(|stem| stem.to_str()) != Some(session) {
        return Err(std::io::Error::other(
            "recent Antigravity source session mismatch",
        ));
    }
    let (logging, _) =
        crate::utils::shell::recent_conversation_logging().map_err(std::io::Error::other)?;
    if effective_conversation_logging(logging, snapshot.agent_conversation_logging)
        == ConversationLoggingSetting::Disabled
    {
        return Ok(false);
    }
    if !state
        .conversation_capture_policy_lock
        .background_admission_open()
    {
        return Err(std::io::Error::other(
            "recent Antigravity admission waits for lifecycle boundary",
        ));
    }
    let owner = &state.conversation_archive.chat_projection;
    let source = chat_recent_database::Source::open(path)?;
    let receipt = owner.begin_recent_with_source(snapshot, runtime, Some(source.clone()))?;
    let result = (|| {
        let policy =
            crate::state::conversation_archive::recent_native_time_policy(&snapshot.session_id)?;
        let source_key = conversation_archive_context_from_snapshot(snapshot).provider_source_key;
        let batch = chat_recent_database::antigravity(path, session)?;
        if batch.source != source {
            return Err(std::io::Error::other(
                "recent Antigravity source changed during acquisition",
            ));
        }
        let mut headers = Vec::new();
        for row in batch.rows {
            if !policy(source_key.as_deref(), None) {
                continue;
            }
            let id = format!("preview:{}:{}", receipt.0, row.key);
            let mut event = antigravity_database_event(
                &snapshot.session_id,
                &snapshot.provider,
                path,
                None,
                row.message,
                Some(id),
            )
            .map_err(std::io::Error::other)?;
            normalize_chat_event_visible_text(&mut event);
            if event.text.as_ref().is_none_or(|text| text.is_empty()) {
                continue;
            }
            event.metadata["chat_recent_key"] =
                serde_json::json!(format!("{}:{}", receipt.0, row.key));
            event.metadata["chat_recent_version"] = serde_json::json!(row.version);
            let mut header = crate::state::conversation_archive::chat_read::header(&event);
            header.metadata["step_index"] = event.metadata["step_index"].clone();
            headers.push(header);
        }
        let (after, after_runtime) = chat_read_snapshot_for_state(state, &snapshot.session_id)
            .map_err(std::io::Error::other)?;
        if !Arc::ptr_eq(runtime, &after_runtime)
            || !Arc::ptr_eq(&snapshot.watch_state, &after.watch_state)
            || !state
                .conversation_capture_policy_lock
                .background_admission_open()
            || !owner.publish_recent(&after, runtime, &receipt, headers)?
        {
            return Err(std::io::Error::other(
                "recent Antigravity publication retired",
            ));
        }
        let _metrics = batch.metrics;
        Ok(true)
    })();
    if result.is_err() {
        owner.abandon_recent(&receipt);
    }
    result
}
