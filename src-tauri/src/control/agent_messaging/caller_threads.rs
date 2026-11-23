//! Native helpers report progress; their root retains canonical task ownership.
use super::*;
use std::collections::HashSet;
use wardian_core::temporary_workers::{self, TemporaryWorkerKind};

struct RootSnapshot {
    codex: bool,
    generation: u64,
    origin: Option<store::RecipientOrigin>,
}

// The caller holds the owner lifecycle gate through canonical publication.
async fn root_snapshot(state: &AppState, owner: &str) -> Result<RootSnapshot, ControlError> {
    let config = {
        let agents = state.agents.lock().await;
        agents
            .get(owner)
            .map(|agent| agent.config.clone())
            .ok_or_else(|| {
                ControlError::coded("unauthorized", "Messaging owner no longer exists.")
            })?
    };
    let config = config
        .lock()
        .map_err(|_| ControlError::request_failed("Agent configuration lock poisoned."))?
        .clone();
    let generation = state
        .interactions
        .current_provider_input_generation(owner)
        .await
        .unwrap_or(0);
    let codex = config.provider.eq_ignore_ascii_case("codex");
    let origin = config
        .resume_session
        .as_deref()
        .filter(|root| !root.trim().is_empty())
        .filter(|_| codex && generation > 0)
        .map(|root| store::RecipientOrigin {
            provider: "codex".into(),
            conversation_id: root.into(),
            generation,
        });
    Ok(RootSnapshot {
        codex,
        generation,
        origin,
    })
}

// Existing verified worker parentage supplies the label, never a new caller
// credential. Unknown/expired lineage stays explicitly labelled foreign.
fn child_parent(owner: &str, thread: &str, root: &RootSnapshot) -> Option<String> {
    let root_thread = root.origin.as_ref()?.conversation_id.as_str();
    let workers = temporary_workers::list_for_root(owner).ok()?;
    let mut current = thread;
    let mut direct_parent = None;
    let mut visited = HashSet::new();
    for _ in 0..64 {
        if !visited.insert(current) {
            return None;
        }
        let mut matches = workers.iter().filter(|worker| {
            worker.kind == TemporaryWorkerKind::ProviderChild
                && worker.provider == "codex"
                && worker.root_agent_id.as_deref() == Some(owner)
                && worker.runtime_session_id == owner
                && worker.coverage == "codex_rollout_verified"
                && worker.provider_session_id.as_deref() == Some(current)
        });
        let worker = matches.next()?;
        if matches.next().is_some() {
            return None;
        }
        let parent = worker.parent_provider_session_id.as_deref()?;
        direct_parent.get_or_insert_with(|| parent.to_owned());
        if parent == root_thread {
            return direct_parent;
        }
        current = parent;
    }
    None
}

fn announce_progress(app: Option<&AppHandle>, owner: &str) {
    if let Some(app) = app {
        let _ = app.emit("pair-activity-changed", ());
        super::native::spawn_information(app, owner);
    }
}

/// Return None only for providers whose existing explicit reply contract is
/// unchanged. Missing Codex provenance still passes, with its durable diagnostic.
pub(super) async fn reply(
    app: Option<&AppHandle>,
    state: &AppState,
    owner: &str,
    thread: Option<&str>,
    request_id: &str,
    status: wardian_core::control::ReplyStatus,
    body: &str,
) -> Result<Option<Response>, ControlError> {
    let lifecycle = state.lock_agent_lifecycle(owner).await;
    let root = root_snapshot(state, owner).await?;
    if !root.codex {
        return Ok(None);
    }
    let parent = thread.and_then(|thread| child_parent(owner, thread, &root));
    let result = state
        .interactions
        .reply_agent_message_from_thread(
            owner,
            request_id,
            status,
            body,
            store::ProviderThreadCaller {
                provider_thread_id: thread,
                current_root_thread_id: root
                    .origin
                    .as_ref()
                    .map(|origin| origin.conversation_id.as_str()),
                current_generation: root.generation,
                parent_thread_id: parent.as_deref(),
            },
        )
        .await
        .map_err(control_error)?;
    drop(lifecycle);
    match result {
        store::ProviderThreadReply::Replied(replied) => {
            if let Some(app) = app {
                let _ = app.emit("pair-activity-changed", ());
                if let Some(recipient) = replied.record.target_session_ids.first() {
                    super::spawn_reply_continuation(app, recipient, &replied.record.id);
                }
            }
            Ok(Some(Response::Reply {
                request_id: request_id.into(),
                interaction_id: replied.record.id,
                delivery_state: "stored".into(),
                duplicate: replied.duplicate,
            }))
        }
        store::ProviderThreadReply::Progress(progress) => {
            announce_progress(app, owner);
            Err(ControlError::coded("foreign_provider_thread", "Reply from a forked, foreign or stale-generation thread cannot settle this task. Its body was retained as progress for the owning parent; return the helper result to the parent and let the assigned root reply.")
                .with_details(serde_json::json!({"request_id":request_id, "provider_thread_id":thread, "progress_interaction_id":progress.records.last().map(|record| record.id.as_str())})))
        }
    }
}

/// Foreign native sends cannot impersonate root progress to its requester.
/// Route them to the owner using ordinary conversation-bound information.
pub(super) async fn route_child_progress(
    app: Option<&AppHandle>,
    state: &AppState,
    owner: &str,
    thread: Option<&str>,
    target: &str,
    body: &str,
    key: Option<&str>,
) -> Result<Option<Response>, ControlError> {
    let Some(thread) = thread else {
        return Ok(None);
    };
    let lifecycle = state.lock_agent_lifecycle(owner).await;
    let root = root_snapshot(state, owner).await?;
    if !root.codex {
        return Ok(None);
    }
    let origin = root.origin.as_ref().ok_or_else(|| ControlError::coded(
        "provider_thread_unavailable",
        "Wardian is still identifying the current Codex root. Retry this send after the session is ready.",
    ))?;
    if origin.conversation_id == thread {
        return Ok(None);
    }
    super::resolve_message_recipient(state, owner, target, false).await?;
    let parent = child_parent(owner, thread, &root);
    // A foreign thread may belong to the preceding root. Only existing child
    // lineage to this root can put an uncorrelated send in its provider queue.
    let progress_origin = parent.as_ref().map(|_| origin);
    let progress = state
        .interactions
        .publish_agent_thread_progress(store::ProviderThreadProgressInput {
            owner,
            provider_thread_id: thread,
            parent_thread_id: parent.as_deref(),
            body,
            request_id: None,
            idempotency_key: key,
            recipient_origin: progress_origin,
        })
        .await
        .map_err(control_error)?;
    drop(lifecycle);
    if progress_origin.is_some() {
        announce_progress(app, owner);
    }
    Ok(Some(Response::SendMessage {
        interaction_id: progress.records.last().expect("progress body").id.clone(),
        delivery_state: if progress_origin.is_some() {
            "stored"
        } else {
            "blocked_stale_origin"
        }
        .into(),
        duplicate: progress.duplicate,
    }))
}
