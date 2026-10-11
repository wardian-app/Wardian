//! Private saved-reviewer preparation before any ordinary TUI or input runtime.
use super::super::{json, version, CodexSharedClient, CodexSharedError, Value, STARTUP_TIMEOUT};
use super::{attachment, policy::ExpectedPolicy};
use std::sync::atomic::Ordering;
use tokio::sync::broadcast;

#[cfg(test)]
#[path = "initial_checkpoint_tests.rs"]
mod tests;

/// Only startup-relevant frames are retained; terminal/model payloads are not.
#[derive(Clone, Debug)]
pub(crate) enum Event {
    Notification {
        sequence: u64,
        method: String,
        params: Value,
    },
    Disconnected,
}

impl Event {
    pub(crate) fn from_value(value: &Value, sequence: u64) -> Option<Self> {
        let method = value["method"].as_str()?;
        if !matches!(
            method,
            "thread/settings/updated"
                | "thread/closed"
                | "thread/status/changed"
                | "turn/started"
                | "turn/completed"
                | "error"
        ) {
            return None;
        }
        Some(Self::Notification {
            sequence,
            method: method.into(),
            params: value["params"].clone(),
        })
    }

    pub(crate) fn blocks_update(&self) -> bool {
        match self {
            Self::Disconnected => true,
            Self::Notification { method, params, .. } => {
                matches!(
                    method.as_str(),
                    "thread/closed" | "turn/started" | "turn/completed" | "error"
                ) || (method == "thread/status/changed" && params["status"]["type"] == "active")
            }
        }
    }
}

fn private_client(client: &CodexSharedClient) -> Result<(), CodexSharedError> {
    let state = client.observation.borrow();
    if state.thread_id.is_some() || state.closed || state.stopped {
        return Err(CodexSharedError::unsupported(
            "initial checkpoint requires a live unbound owner",
        ));
    }
    Ok(())
}

fn quiet_response(response: &Value, id: &str) -> Result<(), CodexSharedError> {
    if response["thread"]["id"].as_str() != Some(id)
        || response["thread"]["canAcceptDirectInput"] != true
        || response["thread"]["status"]["type"] != "idle"
        || response["thread"]["turns"]
            .as_array()
            .is_some_and(|turns| turns.iter().any(|turn| turn["status"] == "inProgress"))
    {
        return Err(CodexSharedError::unsupported(
            "initial checkpoint requires the exact idle direct-input thread",
        ));
    }
    Ok(())
}

fn event_result(
    result: Result<Event, broadcast::error::RecvError>,
) -> Result<Event, CodexSharedError> {
    result.map_err(|_| {
        CodexSharedError::uncertain("initial checkpoint notification lost; not replayed")
    })
}

fn guard_event(
    event: &Event,
    id: &str,
    expect_closed: bool,
    require_user: bool,
) -> Result<(), CodexSharedError> {
    let Event::Notification { method, params, .. } = event else {
        return Err(CodexSharedError::uncertain(
            "initial checkpoint connection closed; not replayed",
        ));
    };
    if method == "error" {
        return Err(CodexSharedError::uncertain(
            "initial checkpoint reported an asynchronous error; not replayed",
        ));
    }
    if params["threadId"].as_str() != Some(id) {
        return Ok(());
    }
    if require_user
        && method == "thread/settings/updated"
        && params["threadSettings"]["approvalsReviewer"] != "user"
    {
        return Err(CodexSharedError::uncertain(
            "conflicting initial reviewer notification",
        ));
    }
    if matches!(method.as_str(), "turn/started" | "turn/completed")
        || (method == "thread/status/changed" && params["status"]["type"] == "active")
        || (method == "thread/closed" && !expect_closed)
    {
        return Err(CodexSharedError::uncertain(
            "initial checkpoint thread became busy or closed; not replayed",
        ));
    }
    Ok(())
}

fn drain(
    events: &mut broadcast::Receiver<Event>,
    id: &str,
    require_user: bool,
) -> Result<(), CodexSharedError> {
    loop {
        match events.try_recv() {
            Ok(event) => guard_event(&event, id, false, require_user)?,
            Err(broadcast::error::TryRecvError::Empty) => return Ok(()),
            Err(_) => {
                return Err(CodexSharedError::uncertain(
                    "initial checkpoint notification lost; not replayed",
                ))
            }
        }
    }
}

/// This single deadline covers checkpoint, eviction and empty-cache evidence.
/// The caller owns the fresh daemon/startup lease and has not launched a TUI.
/// A matching closed event is eviction evidence, not a persistence claim. Stock
/// rejects a retained live writer on the next cold load; final TUI attachment
/// must independently validate the persisted User policy before publication.
pub(super) async fn prepare_saved_resume(
    client: &CodexSharedClient,
    policy: &ExpectedPolicy,
    id: &str,
    alive: &mut impl FnMut() -> Result<(), CodexSharedError>,
    deadline: tokio::time::Instant,
    observed_version: &str,
) -> Result<(), CodexSharedError> {
    version::require_initial_checkpoint_version(observed_version)?;
    let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
    if remaining.is_zero() {
        return Err(CodexSharedError::unsupported(
            "initial checkpoint startup budget exhausted before write",
        ));
    }
    prepare_with_timeout(client, policy, id, alive, remaining).await
}

async fn prepare_with_timeout(
    client: &CodexSharedClient,
    policy: &ExpectedPolicy,
    id: &str,
    alive: &mut impl FnMut() -> Result<(), CodexSharedError>,
    timeout: std::time::Duration,
) -> Result<(), CodexSharedError> {
    tokio::time::timeout(timeout, async {
        private_client(client)?;
        alive()?;
        let mut events = client.initial_notifications.subscribe();
        let activity = client.initial_activity_sequence.load(Ordering::Acquire);
        let response = client.resume_metadata(policy.initial_resume_params(id)).await?;
        private_client(client)?;
        alive()?;
        quiet_response(&response, id)?;
        let needs_update = policy.initial_reviewer_needs_update(&response)?;
        drain(&mut events, id, !needs_update)?;
        if client.initial_activity_sequence.load(Ordering::Acquire) != activity {
            return Err(CodexSharedError::unsupported("initial checkpoint activity changed before settings"));
        }
        if needs_update {
            let fence = client.settings_sequence.load(Ordering::Acquire);
            let request = client.request_with_fences(
                "thread/settings/update",
                json!({"threadId": id, "approvalsReviewer": "user"}),
                Some(STARTUP_TIMEOUT), None, Some(activity),
            );
            tokio::pin!(request);
            let mut acknowledged = false;
            let mut applied = false;
            while !acknowledged || !applied {
                private_client(client)?;
                alive()?;
                tokio::select! {
                    result = &mut request, if !acknowledged => {
                        if result? != json!({}) {
                            return Err(CodexSharedError::uncertain("unexpected initial settings acknowledgement"));
                        }
                        acknowledged = true;
                    }
                    event = events.recv() => {
                        let event = event_result(event)?;
                        guard_event(&event, id, false, true)?;
                        if let Event::Notification { sequence, method, params } = event {
                            if sequence > fence && method == "thread/settings/updated"
                                && params["threadId"].as_str() == Some(id) {
                                if params["threadSettings"]["approvalsReviewer"] != "user" {
                                    return Err(CodexSharedError::uncertain("conflicting initial reviewer notification"));
                                }
                                applied = true;
                            }
                        }
                    }
                }
            }
            let response = client.resume_metadata(json!({"threadId": id})).await?;
            quiet_response(&response, id)?;
            policy.validate(&response)?;
        }
        private_client(client)?;
        alive()?;
        drain(&mut events, id, true)?;
        if client.initial_activity_sequence.load(Ordering::Acquire) != activity {
            return Err(CodexSharedError::uncertain("initial checkpoint activity changed before eviction"));
        }
        let fence = client.settings_sequence.load(Ordering::Acquire);
        let request = client.request_with_fences(
            "thread/unsubscribe", json!({"threadId": id}),
            Some(STARTUP_TIMEOUT), None, Some(activity),
        );
        tokio::pin!(request);
        let mut acknowledged = false;
        let mut closed = false;
        while !acknowledged || !closed {
            private_client(client)?;
            alive()?;
            tokio::select! {
                result = &mut request, if !acknowledged => {
                    if result?["status"] != "unsubscribed" {
                        return Err(CodexSharedError::unsupported("initial preloader was not the subscribed owner"));
                    }
                    acknowledged = true;
                }
                event = events.recv() => {
                    let event = event_result(event)?;
                    guard_event(&event, id, true, true)?;
                    if let Event::Notification { sequence, method, params } = event {
                        closed |= sequence > fence && method == "thread/closed"
                            && params["threadId"].as_str() == Some(id);
                    }
                }
            }
        }
        attachment::require_empty(client).await?;
        private_client(client)?;
        alive()?;
        Ok(())
    }).await.map_err(|_| CodexSharedError::uncertain("initial reviewer checkpoint timed out; not replayed"))?
}
