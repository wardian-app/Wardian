//! Backend-owned Inbox completion cards.
//!
//! A completed provider turn becomes one `agent_completed` Inbox card that
//! holds the turn's final answer. Every provider publishes through this module
//! so the card is durable before any UI sees it and does not depend on a
//! mounted desktop window re-reading the transcript.
//!
//! A card is keyed by provider evidence for its turn: the Claude prompt ID,
//! the Codex turn ID, or, for providers without a stable turn identity, a hash
//! of the request and its final answer. Two observers of the same turn
//! therefore converge on one card, and a dismissed card stays dismissed.

use crate::state::AppState;
use crate::utils::logging::log_debug;
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Emitter, Manager};
use wardian_core::models::chat::{AgentChatEvent, AgentChatEventKind, AgentChatRole};

/// Matches the frontend's Inbox summary bound; the full answer is kept in
/// `response_text`.
const SUMMARY_MAX_CHARS: usize = 500;

/// Provider commands whose output is not an answer to the user.
const PROVIDER_CONTROL_COMMANDS: &[&str] = &[
    "/login", "/logout", "/compact", "/clear", "/exit", "/help", "/mcp",
];

/// The final answer of one completed provider turn.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TurnCompletion {
    pub(crate) evidence_id: String,
    pub(crate) response_text: String,
    pub(crate) timestamp: i64,
    pub(crate) timestamp_source: &'static str,
}

impl TurnCompletion {
    /// Returns `None` for blank evidence or a blank answer. A turn without a
    /// final answer never becomes an Inbox card.
    pub(crate) fn new(
        evidence_id: &str,
        response_text: &str,
        timestamp: i64,
        timestamp_source: &'static str,
    ) -> Option<Self> {
        let evidence_id = evidence_id.trim();
        if evidence_id.is_empty() || response_text.trim().is_empty() {
            return None;
        }
        Some(Self {
            evidence_id: evidence_id.to_string(),
            response_text: response_text.to_string(),
            timestamp,
            timestamp_source,
        })
    }
}

pub(crate) fn completion_item_id(session_id: &str, evidence_id: &str) -> String {
    format!("agent-completed:{session_id}:{evidence_id}")
}

/// Builds the canonical persisted card. Rebuilding it from the same
/// completion yields the same record, so retries and replays are idempotent.
pub(crate) fn completion_inbox_item(
    session_id: &str,
    agent_name: &str,
    completion: &TurnCompletion,
) -> serde_json::Value {
    serde_json::json!({
        "id": completion_item_id(session_id, &completion.evidence_id),
        "type": "agent_completed",
        "timestamp": completion.timestamp,
        "timestamp_source": completion.timestamp_source,
        "read": false,
        "agent_session_id": session_id,
        "agent_name": agent_name,
        "summary": completion.response_text.trim().chars().take(SUMMARY_MAX_CHARS).collect::<String>(),
        "response_text": completion.response_text,
        "evidence_id": completion.evidence_id,
        "evidence_source": "provider_runtime",
    })
}

/// Returns the persisted card unless it is missing or dismissed.
pub(crate) fn active_completion(
    items: &[serde_json::Value],
    item_id: &str,
) -> Option<serde_json::Value> {
    items
        .iter()
        .find(|item| item.get("id").and_then(serde_json::Value::as_str) == Some(item_id))
        .filter(|item| {
            item.get("type").and_then(serde_json::Value::as_str) == Some("agent_completed")
                && item.get("dismissed").and_then(serde_json::Value::as_bool) != Some(true)
        })
        .cloned()
}

pub(crate) enum PersistedCompletion {
    /// The agent was removed, renamed to blank, changed provider, or replaced
    /// by a newer runtime before the card could be written.
    Stale,
    Committed {
        agent_name: String,
        item_id: String,
    },
}

/// Durably upserts the card for `session_id`.
///
/// A failed queue write is retried while the same agent runtime remains
/// current, so a transient filesystem error delays the card instead of losing
/// it. `runtime_generation` fences out completions from a replaced runtime;
/// `None` accepts any current runtime of the agent.
pub(crate) async fn persist_turn_completion(
    state: &AppState,
    session_id: &str,
    provider: &str,
    runtime_generation: Option<u64>,
    completion: &TurnCompletion,
) -> PersistedCompletion {
    let item_id = completion_item_id(session_id, &completion.evidence_id);
    let mut persistence_failures = 0u64;
    loop {
        // Per-agent lifecycle is always acquired before the global queue
        // lock; queue writers never take the lifecycle lock.
        let lifecycle_guard = state.lock_agent_lifecycle(session_id).await;
        let queue_guard = state.queue_io_lock.lock().await;
        let agent_name = {
            let agents = state.agents.lock().await;
            agents.get(session_id).and_then(|agent| {
                if runtime_generation
                    .is_some_and(|generation| agent.runtime_generation != Some(generation))
                {
                    return None;
                }
                agent.config.lock().ok().and_then(|config| {
                    (config.provider == provider && !config.session_name.trim().is_empty())
                        .then(|| config.session_name.clone())
                })
            })
        };
        let Some(agent_name) = agent_name else {
            return PersistedCompletion::Stale;
        };
        let mut items = crate::utils::queue::load_items();
        if !items.iter().any(|item| item["id"] == item_id) {
            items.insert(
                0,
                completion_inbox_item(session_id, &agent_name, completion),
            );
            if let Err(error) = crate::utils::queue::save_items(&items) {
                persistence_failures += 1;
                if persistence_failures == 1 || persistence_failures.is_multiple_of(60) {
                    log_debug(&format!(
                        "[Wardian] Failed to persist {provider} turn completion for {session_id} ({persistence_failures} retries): {error}"
                    ));
                }
                drop(queue_guard);
                drop(lifecycle_guard);
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                continue;
            }
        }
        return PersistedCompletion::Committed {
            agent_name,
            item_id,
        };
    }
}

/// Re-reads the card under the queue lock and projects it to the UI. A
/// dismissal that landed after the write is respected.
pub(crate) async fn emit_active_completion<R: tauri::Runtime>(
    app: &AppHandle<R>,
    state: &AppState,
    session_id: &str,
    agent_name: &str,
    item_id: &str,
) {
    let queue_guard = state.queue_io_lock.lock().await;
    let items = crate::utils::queue::load_items();
    if let Some(item) = active_completion(&items, item_id) {
        let agent_name = item
            .get("agent_name")
            .and_then(serde_json::Value::as_str)
            .unwrap_or(agent_name)
            .to_string();
        let _ = app.emit(
            "agent-turn-completed",
            serde_json::json!({
                "session_id": session_id,
                "agent_name": agent_name,
                "inbox_item": item,
            }),
        );
    }
    drop(queue_guard);
}

/// Persists and projects one completion in the background. The caller's
/// provider observer is never blocked on queue I/O.
pub(crate) fn publish_turn_completion(
    app: &AppHandle,
    session_id: &str,
    provider: &'static str,
    runtime_generation: Option<u64>,
    completion: TurnCompletion,
) {
    let app = app.clone();
    let session_id = session_id.to_string();
    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        if let PersistedCompletion::Committed {
            agent_name,
            item_id,
        } = persist_turn_completion(
            &state,
            &session_id,
            provider,
            runtime_generation,
            &completion,
        )
        .await
        {
            emit_active_completion(&app, &state, &session_id, &agent_name, &item_id).await;
        }
    });
}

/// Extracts a completed Codex turn from a rollout `task_complete` record.
///
/// The rollout's `turn_id` is the same identity the app-server reports for
/// that turn, so this and the live owner observation converge on one card.
/// Records written before `observed_since_ms` are history being re-read
/// (a resumed session reads its rollout from the start) and are ignored.
pub(crate) fn codex_rollout_turn_completion(
    record: &serde_json::Value,
    observed_since_ms: i64,
) -> Option<TurnCompletion> {
    if record.get("type").and_then(serde_json::Value::as_str) != Some("event_msg") {
        return None;
    }
    let payload = record.get("payload")?;
    if payload.get("type").and_then(serde_json::Value::as_str) != Some("task_complete") {
        return None;
    }
    let timestamp = record
        .get("timestamp")
        .and_then(serde_json::Value::as_str)
        .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
        .map(|value| value.timestamp_millis())?;
    if timestamp < observed_since_ms {
        return None;
    }
    TurnCompletion::new(
        payload.get("turn_id").and_then(serde_json::Value::as_str)?,
        payload
            .get("last_agent_message")
            .and_then(serde_json::Value::as_str)?,
        timestamp,
        "provider_log_timestamp",
    )
}

/// Builds a Codex completion from the owner's exact terminal turn evidence.
/// Interrupted and failed turns do not carry a final answer.
pub(crate) fn codex_owner_turn_completion(
    turn_id: &str,
    status: &str,
    answer: &str,
) -> Option<TurnCompletion> {
    if status != "completed" {
        return None;
    }
    TurnCompletion::new(
        turn_id,
        answer,
        chrono::Utc::now().timestamp_millis(),
        "provider_event_observed",
    )
}

/// Selects the final answer of a turn that just completed from a provider
/// transcript, for providers without a provider-native completion record.
///
/// The transcript must end with an assistant message that answers a user
/// request. A later user message means the completion raced with another
/// turn, and provider control commands such as `/login` are not requests.
/// Tool activity after the last assistant message means that message was
/// interim prose and the final answer has not reached the transcript yet.
pub(crate) fn transcript_turn_completion(
    session_id: &str,
    events: &[AgentChatEvent],
) -> Option<TurnCompletion> {
    let is_message = |event: &AgentChatEvent| {
        event.kind == AgentChatEventKind::Message
            && event
                .text
                .as_deref()
                .is_some_and(|text| !text.trim().is_empty())
    };
    let final_index = events.iter().rposition(is_message)?;
    if events[final_index + 1..].iter().any(|event| {
        matches!(
            event.kind,
            AgentChatEventKind::ToolCall
                | AgentChatEventKind::ToolResult
                | AgentChatEventKind::Approval
        )
    }) {
        return None;
    }
    let messages = events[..=final_index]
        .iter()
        .filter(|event| is_message(event))
        .collect::<Vec<_>>();
    let (final_answer, earlier) = messages.split_last()?;
    if final_answer.role != Some(AgentChatRole::Assistant) {
        return None;
    }
    let request = earlier
        .iter()
        .rev()
        .find(|event| event.role == Some(AgentChatRole::User))?;
    let request_text = request.text.as_deref().unwrap_or_default();
    let command = request_text
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    if PROVIDER_CONTROL_COMMANDS.contains(&command.as_str()) {
        return None;
    }
    let answer = final_answer.text.as_deref().unwrap_or_default();

    // Transcript event IDs are not stable for every source, and some
    // providers reuse one `turn_id` for a whole thread. A digest of the
    // request and its answer identifies the turn across re-reads.
    let mut hash = Sha256::new();
    for part in [session_id, request_text.trim(), answer.trim()] {
        hash.update(part.as_bytes());
        hash.update(b"\0");
    }
    let digest = hash
        .finalize()
        .iter()
        .take(16)
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    TurnCompletion::new(
        &format!("answer:{digest}"),
        answer,
        chrono::Utc::now().timestamp_millis(),
        "provider_event_observed",
    )
}

/// The agent's provider and live runtime generation, if it still exists.
async fn current_runtime(state: &AppState, session_id: &str) -> Option<(String, Option<u64>)> {
    let agents = state.agents.lock().await;
    let agent = agents.get(session_id)?;
    let provider = agent.config.lock().ok()?.provider.clone();
    Some((provider, agent.runtime_generation))
}

/// Publishes a completion for providers that report only a turn boundary.
///
/// The transcript can trail the boundary event by a few hundred
/// milliseconds, so the final answer is polled briefly before giving up. A
/// candidate that is already an Inbox card belongs to an earlier turn, so
/// polling continues. The runtime that reported the boundary fences every
/// read and the write: a replacement runtime's transcript never answers it.
pub(crate) async fn publish_transcript_turn_completion(app: AppHandle, session_id: String) {
    let state = app.state::<AppState>();
    let Some((provider, runtime_generation)) = current_runtime(&state, &session_id).await else {
        return;
    };
    // Claude and Codex publish from provider-native turn records.
    if matches!(provider.as_str(), "claude" | "codex") {
        return;
    }
    for attempt in 0..6 {
        if attempt > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
        if current_runtime(&state, &session_id).await
            != Some((provider.clone(), runtime_generation))
        {
            return;
        }
        let Ok(events) =
            crate::commands::chat::load_agent_chat_transcript_for_state(&state, session_id.clone())
                .await
        else {
            continue;
        };
        let Some(completion) = transcript_turn_completion(&session_id, &events) else {
            continue;
        };
        let item_id = completion_item_id(&session_id, &completion.evidence_id);
        let already_recorded = {
            let _queue_guard = state.queue_io_lock.lock().await;
            crate::utils::queue::load_items()
                .iter()
                .any(|item| item["id"] == item_id)
        };
        if already_recorded {
            continue;
        }
        if let PersistedCompletion::Committed {
            agent_name,
            item_id,
        } = persist_turn_completion(
            &state,
            &session_id,
            &provider,
            runtime_generation,
            &completion,
        )
        .await
        {
            emit_active_completion(&app, &state, &session_id, &agent_name, &item_id).await;
        }
        return;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(role: AgentChatRole, text: &str) -> AgentChatEvent {
        AgentChatEvent {
            id: format!("agent-1:{text}"),
            session_id: "agent-1".to_string(),
            provider: "gemini".to_string(),
            kind: AgentChatEventKind::Message,
            role: Some(role),
            text: Some(text.to_string()),
            title: None,
            status: None,
            turn_id: Some("thread-wide-id".to_string()),
            source: Some("watch".to_string()),
            command: None,
            exit_code: None,
            path: None,
            language: None,
            created_at: None,
            sequence: None,
            metadata: serde_json::Value::Null,
        }
    }

    #[test]
    fn completion_item_keeps_the_full_answer_and_a_stable_identity() {
        let completion = TurnCompletion::new(
            "claude-message-7",
            "  finished the requested work  ",
            100,
            "hook_outbox_mtime",
        )
        .expect("answer");
        let first = completion_inbox_item("agent-1", "Claude", &completion);
        let retry = completion_inbox_item("agent-1", "Claude", &completion);

        assert_eq!(first["id"], "agent-completed:agent-1:claude-message-7");
        assert_eq!(first["evidence_id"], "claude-message-7");
        assert_eq!(first["evidence_source"], "provider_runtime");
        assert_eq!(first["summary"], "finished the requested work");
        assert_eq!(first["response_text"], "  finished the requested work  ");
        assert_eq!(first["timestamp_source"], "hook_outbox_mtime");
        assert_eq!(first, retry);

        let long = "x".repeat(SUMMARY_MAX_CHARS + 50);
        let long_item = completion_inbox_item(
            "agent-1",
            "Claude",
            &TurnCompletion::new("p", &long, 1, "hook_outbox_mtime").expect("answer"),
        );
        assert_eq!(
            long_item["summary"]
                .as_str()
                .map(|text| text.chars().count()),
            Some(SUMMARY_MAX_CHARS)
        );
        assert_eq!(long_item["response_text"], long);
    }

    #[test]
    fn turns_without_an_answer_or_identity_never_become_cards() {
        assert!(TurnCompletion::new("turn-1", "   ", 1, "provider_event_observed").is_none());
        assert!(TurnCompletion::new(" ", "answer", 1, "provider_event_observed").is_none());
    }

    #[test]
    fn dismissed_completion_is_not_emitted_from_a_stale_snapshot() {
        let item = serde_json::json!({
            "id": "agent-completed:agent-1:prompt-1",
            "type": "agent_completed",
            "dismissed": true,
            "read": true,
        });
        assert!(active_completion(
            std::slice::from_ref(&item),
            "agent-completed:agent-1:prompt-1",
        )
        .is_none());

        let active = serde_json::json!({
            "id": "agent-completed:agent-1:prompt-1",
            "type": "agent_completed",
            "dismissed": false,
        });
        assert_eq!(
            active_completion(
                std::slice::from_ref(&active),
                "agent-completed:agent-1:prompt-1",
            ),
            Some(active),
        );
    }

    #[test]
    fn codex_rollout_completion_uses_the_turn_identity_and_final_answer() {
        let record = serde_json::json!({
            "timestamp": "2026-10-04T21:10:19.500Z",
            "type": "event_msg",
            "payload": {
                "type": "task_complete",
                "turn_id": "01a108c0-dc49-7d70-b9e3-362bce231a39",
                "last_agent_message": "The audit found no change is justified.",
            },
        });
        let observed_since = chrono::DateTime::parse_from_rfc3339("2026-10-04T21:00:00Z")
            .expect("time")
            .timestamp_millis();

        let completion =
            codex_rollout_turn_completion(&record, observed_since).expect("completed turn");
        assert_eq!(
            completion.evidence_id,
            "01a108c0-dc49-7d70-b9e3-362bce231a39"
        );
        assert_eq!(
            completion.response_text,
            "The audit found no change is justified."
        );
        assert_eq!(completion.timestamp_source, "provider_log_timestamp");
        assert_eq!(completion.timestamp, observed_since + 619_500);

        // The live owner reports the same turn under the same identity.
        let owner = codex_owner_turn_completion(
            "01a108c0-dc49-7d70-b9e3-362bce231a39",
            "completed",
            "The audit found no change is justified.",
        )
        .expect("owner completion");
        assert_eq!(
            completion_item_id("agent-1", &owner.evidence_id),
            completion_item_id("agent-1", &completion.evidence_id)
        );
    }

    #[test]
    fn codex_rollout_history_and_unanswered_turns_are_ignored() {
        let record = |timestamp: &str, answer: serde_json::Value| {
            serde_json::json!({
                "timestamp": timestamp,
                "type": "event_msg",
                "payload": { "type": "task_complete", "turn_id": "turn-1", "last_agent_message": answer },
            })
        };
        let observed_since = chrono::DateTime::parse_from_rfc3339("2026-10-04T21:00:00Z")
            .expect("time")
            .timestamp_millis();

        assert!(codex_rollout_turn_completion(
            &record("2026-10-04T20:59:59Z", "answer".into()),
            observed_since
        )
        .is_none());
        assert!(codex_rollout_turn_completion(
            &record("2026-10-04T21:00:01Z", serde_json::Value::Null),
            observed_since
        )
        .is_none());
        assert!(codex_rollout_turn_completion(
            &serde_json::json!({"type": "event_msg", "payload": {"type": "task_complete", "turn_id": "turn-1", "last_agent_message": "answer"}}),
            observed_since
        )
        .is_none());
        assert!(codex_owner_turn_completion("turn-1", "interrupted", "partial").is_none());
        assert!(codex_owner_turn_completion("turn-1", "failed", "").is_none());
    }

    #[test]
    fn transcript_completion_identifies_each_turn_by_its_request_and_answer() {
        let first = transcript_turn_completion(
            "agent-1",
            &[
                message(AgentChatRole::User, "What changed?"),
                message(AgentChatRole::Assistant, "Working on it"),
                message(AgentChatRole::Assistant, "Two files changed."),
            ],
        )
        .expect("answer");
        assert_eq!(first.response_text, "Two files changed.");
        assert!(first.evidence_id.starts_with("answer:"));

        // Same thread-wide turn_id, different request: a different card.
        let second = transcript_turn_completion(
            "agent-1",
            &[
                message(AgentChatRole::User, "What changed?"),
                message(AgentChatRole::Assistant, "Two files changed."),
                message(AgentChatRole::User, "Run the tests"),
                message(AgentChatRole::Assistant, "All tests pass."),
            ],
        )
        .expect("answer");
        assert_ne!(first.evidence_id, second.evidence_id);

        // Re-reading the same turn yields the same card.
        let reread = transcript_turn_completion(
            "agent-1",
            &[
                message(AgentChatRole::User, "What changed?"),
                message(AgentChatRole::Assistant, "Two files changed."),
            ],
        )
        .expect("answer");
        assert_eq!(first.evidence_id, reread.evidence_id);
    }

    #[test]
    fn transcript_completion_waits_past_interim_prose() {
        let mut tool_call = message(AgentChatRole::Assistant, "");
        tool_call.kind = AgentChatEventKind::ToolCall;
        tool_call.text = None;
        let events = [
            message(AgentChatRole::User, "Run the tests"),
            message(AgentChatRole::Assistant, "Running the suite now."),
            tool_call,
        ];
        // The final answer has not reached the transcript yet.
        assert!(transcript_turn_completion("agent-1", &events).is_none());

        let mut caught_up = events.to_vec();
        caught_up.push(message(AgentChatRole::Assistant, "All tests pass."));
        assert_eq!(
            transcript_turn_completion("agent-1", &caught_up)
                .expect("final answer")
                .response_text,
            "All tests pass."
        );
    }

    #[tokio::test]
    async fn persistence_is_idempotent_and_fenced_to_the_reporting_runtime() {
        use crate::state::{ActiveAgent, AgentWatchState};
        use std::sync::{Arc, Mutex};

        let _home = crate::control::test_support::TestWardianHome::new_async().await;
        let state = AppState::new();
        state.agents.lock().await.insert(
            "agent-1".into(),
            ActiveAgent {
                config: Arc::new(Mutex::new(wardian_core::models::AgentConfig {
                    session_id: "agent-1".into(),
                    session_name: "Gemini Agent".into(),
                    provider: "gemini".into(),
                    ..Default::default()
                })),
                child_process: None,
                background_processes: Vec::new(),
                memory_capability: None,
                runtime_generation: Some(2),
                process_id: None,
                query_count: Arc::new(Mutex::new(0)),
                init_timestamp: Arc::new(Mutex::new(None)),
                last_query_timestamp: Arc::new(Mutex::new(None)),
                current_status: Arc::new(Mutex::new("Idle".into())),
                last_status_at: Arc::new(Mutex::new(None)),
                watch_state: Arc::new(Mutex::new(AgentWatchState::new(
                    "agent-1".to_string(),
                    4096,
                    262_144,
                ))),
                terminal_title: Arc::new(Mutex::new(String::new())),
                last_output_at: Arc::new(Mutex::new(None)),
                log_path: Arc::new(Mutex::new(None)),
                log_last_modified: Arc::new(Mutex::new(None)),
                #[cfg(windows)]
                job_object: None,
            },
        );
        let completion =
            TurnCompletion::new("answer:1", "Done.", 1, "provider_event_observed").expect("answer");

        // A completion reported by a replaced runtime is never written.
        assert!(matches!(
            persist_turn_completion(&state, "agent-1", "gemini", Some(1), &completion).await,
            PersistedCompletion::Stale
        ));
        assert!(crate::utils::queue::load_items().is_empty());

        for _ in 0..2 {
            assert!(matches!(
                persist_turn_completion(&state, "agent-1", "gemini", Some(2), &completion).await,
                PersistedCompletion::Committed { .. }
            ));
        }
        let items = crate::utils::queue::load_items();
        assert_eq!(items.len(), 1, "a re-observed turn keeps one card");
        assert_eq!(items[0]["id"], "agent-completed:agent-1:answer:1");
        assert_eq!(items[0]["agent_name"], "Gemini Agent");
    }

    #[test]
    fn transcript_completion_requires_a_final_answer_to_a_user_request() {
        assert!(transcript_turn_completion(
            "agent-1",
            &[
                message(AgentChatRole::Assistant, "Done."),
                message(AgentChatRole::User, "Next question"),
            ],
        )
        .is_none());
        assert!(transcript_turn_completion(
            "agent-1",
            &[message(AgentChatRole::Assistant, "Ready when you are.")],
        )
        .is_none());
        assert!(transcript_turn_completion(
            "agent-1",
            &[
                message(AgentChatRole::User, "/login"),
                message(AgentChatRole::Assistant, "Login successful."),
            ],
        )
        .is_none());
    }
}
