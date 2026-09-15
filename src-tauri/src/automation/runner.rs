use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Mutex;
use std::time::Duration;
use tauri::{Emitter, Manager};
use wardian_core::control::ReplyStatus;
use wardian_core::conversation_lease::ConversationLeaseOwner;
use wardian_core::models::AgentConfig;
use wardian_core::temporary_workers::{
    AutomationWorkerOrigin, RegisterAutomationWorker, TemporaryWorkerRecord, TemporaryWorkerState,
};

type AgentRunFuture<'a> = Pin<Box<dyn Future<Output = Result<String, String>> + Send + 'a>>;
type LiveAgentRunFuture<'a> = Pin<Box<dyn Future<Output = Result<String, String>> + Send + 'a>>;

/// What the executor needs to run one headless agent prompt.
#[derive(Debug, Clone)]
pub struct AgentRunSpec {
    pub node: String,
    pub provider: String,
    pub cwd: PathBuf,
    pub prompt: String,
    /// Process-scoped identity used by a fresh background automation. This may
    /// intentionally differ from the registered agent below so providers do
    /// not resume or write into the visible session's conversation.
    pub session_id: String,
    /// The registered agent whose lifecycle gate, lease, and status belong to
    /// this headless run. Ephemeral provider workers have no registered agent.
    pub agent_session_id: Option<String>,
    pub resume_session: Option<String>,
    pub config_override: Option<AgentConfig>,
    /// Owned by registered background automation paths. It lets the shared
    /// headless process keep the persisted conversation lease alive.
    pub lease_owner: Option<ConversationLeaseOwner>,
    /// Structured automation ownership for an ephemeral worker. Registered
    /// agent conversations leave this unset.
    pub temporary_origin: Option<AutomationWorkerOrigin>,
}

/// What the executor needs to route one prompt into an already-running agent.
#[derive(Debug, Clone)]
pub struct LiveAgentRunSpec {
    /// Trusted execution owner supplied by the host executor, never a managed agent ID.
    pub run_id: String,
    pub node: String,
    pub session_id: String,
    pub prompt: String,
    pub timeout: Duration,
}

/// Boundary between the executor logic and the real headless runtime. Unit tests
/// inject a fake so they never spawn a provider.
pub trait AgentRunner: Send + Sync {
    /// Run the prompt headlessly; return the agent's textual response.
    fn run(&self, spec: AgentRunSpec) -> AgentRunFuture<'_>;
}

/// Boundary for active-agent execution. Unlike headless runs, this uses the
/// canonical v2 task claim and completes only through the correlated reply
/// contract. Idle terminal status and printed reply commands are not
/// completion evidence.
pub trait LiveAgentRunner: Send + Sync {
    fn run_live(&self, spec: LiveAgentRunSpec) -> LiveAgentRunFuture<'_>;
}

/// Real runner: drives `manager::headless::run_headless_with_options` and pulls
/// the `response` field out of the provider-normalized JSON.
pub struct HeadlessAgentRunner;

impl AgentRunner for HeadlessAgentRunner {
    fn run(&self, spec: AgentRunSpec) -> AgentRunFuture<'_> {
        Box::pin(async move {
            let mut worker = begin_temporary_worker(&spec)?;
            let cancellation_marker = cancellation_marker(&spec);
            let run =
                crate::manager::run_headless_with_options(crate::manager::HeadlessRunOptions {
                    cwd: &spec.cwd,
                    prompt: &spec.prompt,
                    wardian_session_id: &spec.session_id,
                    memory_agent_id: spec.agent_session_id.as_deref(),
                    resume_session: spec.resume_session.as_deref(),
                    output_format: "json",
                    provider_name: &spec.provider,
                    config_override: spec.config_override.as_ref(),
                    timeout: crate::manager::DEFAULT_HEADLESS_RUN_TIMEOUT,
                    lease_owner: spec.lease_owner.clone(),
                    cancellation_marker: cancellation_marker.as_deref(),
                })
                .await
                .map(|value| {
                    let provider_session_id = provider_session_id(&value);
                    let response = response_from_headless_value(&value);
                    (response, provider_session_id)
                });
            finish_temporary_worker(worker.as_mut(), &run)?;
            run.map(|(response, _)| response)
                .map_err(|error| error.to_string())
        })
    }
}

#[derive(Clone)]
pub struct TauriHeadlessAgentRunner {
    app: tauri::AppHandle,
}

impl TauriHeadlessAgentRunner {
    pub fn new(app: tauri::AppHandle) -> Self {
        Self { app }
    }
}

impl AgentRunner for TauriHeadlessAgentRunner {
    fn run(&self, spec: AgentRunSpec) -> AgentRunFuture<'_> {
        Box::pin(async move {
            let state = self.app.state::<crate::state::AppState>();
            let registered_agent_session_id = spec.agent_session_id.clone();
            // Registered background automation runs acquire the persisted lease
            // before they get here. They then share the local lifecycle gate
            // with direct delivery and resume/clear/pause/kill. Fresh runs use
            // a synthetic provider session id, so the gate must key off the
            // registered agent rather than `spec.session_id`.
            let _lifecycle_guard = if spec.lease_owner.is_some() {
                let agent_session_id = registered_agent_session_id.as_deref().ok_or_else(|| {
                    "registered background automation run is missing its agent session id"
                        .to_string()
                })?;
                Some(state.lock_agent_lifecycle(agent_session_id).await)
            } else {
                None
            };
            let (config_override, emit_headless_status) = if spec.lease_owner.is_some() {
                let agent_session_id = registered_agent_session_id
                    .as_deref()
                    .expect("registered background automation id was validated before lock");
                let current_config = {
                    let agents = state.agents.lock().await;
                    let agent = agents.get(agent_session_id).ok_or_else(|| {
                        format!(
                            "agent {} was removed before its background automation could start",
                            agent_session_id
                        )
                    })?;
                    let config = agent
                        .config
                        .lock()
                        .map_err(|_| "agent config lock poisoned".to_string())?
                        .clone();
                    let current_status = agent
                        .current_status
                        .lock()
                        .map_err(|_| "agent status lock poisoned".to_string())?
                        .clone();
                    (config, current_status)
                };
                let (current_config, current_status) = current_config;
                let current_resume = current_config
                    .resume_session
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty());
                let expected_resume = spec
                    .resume_session
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty());
                let resumes_visible_conversation = expected_resume.is_some();
                if current_config.provider != spec.provider
                    || (resumes_visible_conversation
                        && (!current_config.is_off || current_resume != expected_resume))
                {
                    return Err(format!(
                        "agent {} changed before its background automation could start",
                        agent_session_id
                    ));
                }
                let emit_headless_status =
                    current_config.is_off || is_offline_agent_status(&current_status);
                if emit_headless_status {
                    let _ = self.app.emit(
                        "agent-status-updated",
                        serde_json::json!({
                            "session_id": agent_session_id,
                            "current_status": "Headless",
                        }),
                    );
                }
                (Some(current_config), emit_headless_status)
            } else {
                (spec.config_override.clone(), false)
            };
            let mut worker = begin_temporary_worker(&spec)?;
            let cancellation_marker = cancellation_marker(&spec);
            let lease_owner = spec.lease_owner.clone();
            let result = crate::delivery::run_headless_process_prompt(
                &state,
                crate::delivery::HeadlessProcessPromptRequest {
                    node: spec.node,
                    provider: spec.provider,
                    cwd: spec.cwd,
                    prompt: spec.prompt,
                    session_id: spec.session_id,
                    memory_agent_id: registered_agent_session_id.clone(),
                    resume_session: spec.resume_session,
                    config_override,
                    interaction_id: None,
                    timeout: crate::manager::DEFAULT_HEADLESS_RUN_TIMEOUT,
                    lease_owner: spec.lease_owner,
                    cancellation_marker,
                },
            )
            .await
            .map(|result| (result.response, result.provider_session_id));

            let registry_result = finish_temporary_worker(worker.as_mut(), &result);

            // `run_background_resume` also owns an idempotent persisted-lease
            // guard. Release here, while this runner still owns the local
            // lifecycle gate, so a local resume/clear/pause/remove cannot slip
            // between provider completion and lease cleanup.
            if let Some(owner) = lease_owner {
                match wardian_core::conversation_lease::release_lease_owner_persisted(&owner) {
                    Ok(()) => {
                        if emit_headless_status {
                            if let Some(agent_session_id) = registered_agent_session_id.as_deref() {
                                let restored_status = {
                                    let agents = state.agents.lock().await;
                                    agents.get(agent_session_id).and_then(|agent| {
                                        agent
                                            .current_status
                                            .lock()
                                            .ok()
                                            .map(|status| status.clone())
                                    })
                                };
                                if let Some(restored_status) = restored_status {
                                    let _ = self.app.emit(
                                        "agent-status-updated",
                                        serde_json::json!({
                                            "session_id": agent_session_id,
                                            "current_status": restored_status,
                                        }),
                                    );
                                }
                            }
                        }
                    }
                    Err(error) => {
                        crate::manager::log_debug(&format!(
                        "[automation] headless lifecycle-gate lease release failed for {}: {error}",
                        registered_agent_session_id.as_deref().unwrap_or("<ephemeral>")
                    ))
                    }
                }
            }

            registry_result?;
            result
                .map(|(response, _)| response)
                .map_err(|error| error.to_string())
        })
    }
}

struct TemporaryWorkerLifecycle {
    record: TemporaryWorkerRecord,
    completed: bool,
}

impl Drop for TemporaryWorkerLifecycle {
    fn drop(&mut self) {
        if !self.completed {
            let _ = wardian_core::temporary_workers::mark_unknown(
                &self.record,
                "execution_future_dropped",
                None,
            );
        }
    }
}

fn begin_temporary_worker(spec: &AgentRunSpec) -> Result<Option<TemporaryWorkerLifecycle>, String> {
    let Some(origin) = spec.temporary_origin.as_ref() else {
        return Ok(None);
    };
    let worker =
        wardian_core::temporary_workers::register_automation_worker(RegisterAutomationWorker {
            provider: &spec.provider,
            workspace: &spec.cwd.to_string_lossy(),
            runtime_session_id: &spec.session_id,
            origin,
        })
        .map_err(|error| format!("failed to register temporary worker: {error}"))?;
    let lifecycle = TemporaryWorkerLifecycle {
        record: worker,
        completed: false,
    };
    let started = wardian_core::temporary_workers::mark_running(&lifecycle.record)
        .map_err(|error| format!("failed to start temporary worker registry entry: {error}"))?;
    if !started {
        return Err("temporary worker ownership changed before execution started".to_string());
    }
    Ok(Some(lifecycle))
}

fn finish_temporary_worker(
    worker: Option<&mut TemporaryWorkerLifecycle>,
    result: &Result<(String, Option<String>), crate::manager::HeadlessRunError>,
) -> Result<(), String> {
    let Some(worker) = worker else {
        return Ok(());
    };
    if let Err(error) = result {
        if temporary_worker_error_state(error).is_none() {
            wardian_core::temporary_workers::mark_unknown(
                &worker.record,
                "provider_outcome_uncertain",
                Some(error.message()),
            )
            .map_err(|error| format!("failed to preserve uncertain temporary worker: {error}"))?;
            worker.completed = true;
            return Ok(());
        }
    }
    let (state, outcome, provider_session_id, coverage, error) = match result {
        Ok((_, provider_session_id)) => (
            TemporaryWorkerState::Succeeded,
            Some("completed"),
            provider_session_id.as_deref(),
            if worker.record.provider == "codex" && provider_session_id.is_some() {
                "provider_session_identified"
            } else if provider_session_id.is_some() {
                "provider_observation_adapter_unavailable"
            } else {
                "provider_session_unavailable"
            },
            None,
        ),
        Err(error) if error.kind() == crate::manager::HeadlessRunErrorKind::Cancelled => (
            TemporaryWorkerState::Cancelled,
            Some("cancelled_by_run"),
            None,
            "run_cancellation_acknowledged",
            None,
        ),
        Err(error) => (
            TemporaryWorkerState::Failed,
            Some("provider_failed"),
            None,
            "provider_session_unavailable",
            Some(error.message()),
        ),
    };
    wardian_core::temporary_workers::mark_terminal(
        &worker.record,
        state,
        outcome,
        provider_session_id,
        None,
        coverage,
        error,
    )
    .map_err(|error| format!("failed to complete temporary worker registry entry: {error}"))?;
    worker.completed = true;
    Ok(())
}

fn temporary_worker_error_state(
    error: &crate::manager::HeadlessRunError,
) -> Option<TemporaryWorkerState> {
    match error.kind() {
        crate::manager::HeadlessRunErrorKind::DefiniteFailure => Some(TemporaryWorkerState::Failed),
        crate::manager::HeadlessRunErrorKind::Uncertain => None,
        crate::manager::HeadlessRunErrorKind::Cancelled => Some(TemporaryWorkerState::Cancelled),
    }
}

fn cancellation_marker(spec: &AgentRunSpec) -> Option<PathBuf> {
    let origin = spec.temporary_origin.as_ref()?;
    wardian_core::paths::automation_run_dir(&origin.blueprint_id, &origin.run_id)
        .map(|run_root| run_root.join("cancel.marker"))
}

fn provider_session_id(value: &serde_json::Value) -> Option<String> {
    value
        .get("thread_id")
        .or_else(|| value.get("session_id"))
        .and_then(|value| value.as_str())
        .map(str::to_string)
}

fn response_from_headless_value(value: &serde_json::Value) -> String {
    value
        .get("response")
        .and_then(|value| value.as_str())
        .or_else(|| value.get("text").and_then(|value| value.as_str()))
        .map(ToString::to_string)
        .unwrap_or_else(|| value.to_string())
}

fn is_offline_agent_status(status: &str) -> bool {
    matches!(
        wardian_core::identity::normalize_status(status).as_str(),
        "off" | "error"
    )
}

#[derive(Clone)]
pub struct TauriLiveAgentRunner {
    app: tauri::AppHandle,
}

impl TauriLiveAgentRunner {
    pub fn new(app: tauri::AppHandle) -> Self {
        Self { app }
    }
}

impl LiveAgentRunner for TauriLiveAgentRunner {
    fn run_live(&self, spec: LiveAgentRunSpec) -> LiveAgentRunFuture<'_> {
        Box::pin(async move { run_live_agent_prompt(&self.app, spec).await })
    }
}

async fn run_live_agent_prompt(
    app: &tauri::AppHandle,
    spec: LiveAgentRunSpec,
) -> Result<String, String> {
    let state = app.state::<crate::state::AppState>();
    let watch_state = {
        let agents = state.agents.lock().await;
        agents
            .get(&spec.session_id)
            .ok_or_else(|| format!("Agent {} not found", spec.session_id))?
            .watch_state
            .clone()
    };
    let cursor = watch_state
        .lock()
        .map_err(|_| "watch state lock poisoned")?
        .latest_cursor();
    let task = state
        .interactions
        .admit_host_automation_task(&spec.run_id, &spec.node, &spec.session_id, &spec.prompt)
        .await
        .map_err(|error| error.to_string())?;
    crate::control::spawn_agent_messaging_after_restore(app, &spec.session_id);
    // A timeout is a waiter outcome, not recipient-authored completion, cancellation,
    // or permission to replay uncertain work. The exact task remains inspectable.
    let reply = wait_for_live_agent_reply(
        &state,
        watch_state,
        cursor,
        &task.record.id,
        &spec.session_id,
        spec.timeout,
    )
    .await
    .map_err(|error| {
        format!(
            "automation run {} node {} task {}: {error}",
            spec.run_id, spec.node, task.record.id
        )
    })?;
    automation_reply_result(&spec.node, reply)
}

fn automation_reply_result(
    node: &str,
    reply: wardian_core::control::StructuredReply,
) -> Result<String, String> {
    match reply.status {
        ReplyStatus::Done => Ok(reply.body),
        ReplyStatus::Blocked | ReplyStatus::Failed => Err(format!(
            "automation node {node} task {}: {}",
            reply.request_id, reply.body
        )),
    }
}

async fn wait_for_live_agent_reply(
    app_state: &crate::state::AppState,
    watch_state: std::sync::Arc<Mutex<crate::state::AgentWatchState>>,
    mut since: String,
    request_id: &str,
    _target_session_id: &str,
    timeout: Duration,
) -> Result<wardian_core::control::StructuredReply, String> {
    let started = std::time::Instant::now();
    loop {
        if let Some(reply) = app_state.interactions.structured_reply(request_id).await {
            return Ok(reply);
        }
        let snapshot = {
            let guard = watch_state
                .lock()
                .map_err(|_| "watch state lock poisoned".to_string())?;
            match guard.snapshot_since(Some(&since), Some(128 * 1024)) {
                Ok(snapshot) => snapshot,
                Err(error) if error.code() == "cursor_expired" => guard
                    .snapshot_since(None, Some(128 * 1024))
                    .map_err(|error| format!("watch state error: {}", error.code()))?,
                Err(error) => return Err(format!("watch state error: {}", error.code())),
            }
        };
        since = snapshot.cursor.clone();
        if let Some(error) = live_agent_terminal_failure(&snapshot) {
            return Err(error);
        }
        let elapsed = started.elapsed();
        if elapsed >= timeout {
            return Err(format!(
                "timed out waiting for live agent automation reply {request_id}"
            ));
        }
        tokio::time::sleep((timeout - elapsed).min(Duration::from_millis(25))).await;
    }
}

#[cfg(test)]
async fn wait_for_live_agent_status_transition_for_regression(
    state: std::sync::Arc<Mutex<crate::state::AgentWatchState>>,
    since: String,
    timeout: Duration,
) -> Result<crate::state::agent_watch::WatchSnapshot, String> {
    let started = std::time::Instant::now();
    let notify = state
        .lock()
        .map_err(|_| "watch state lock poisoned".to_string())?
        .notifier();

    loop {
        let notified = notify.notified();
        let snapshot = {
            let guard = state
                .lock()
                .map_err(|_| "watch state lock poisoned".to_string())?;
            guard
                .snapshot_since(Some(&since), Some(128 * 1024))
                .map_err(|error| format!("watch state error: {}", error.code()))?
        };

        if let Some(error) = live_agent_terminal_failure(&snapshot) {
            return Err(error);
        }

        let elapsed = started.elapsed();
        if elapsed >= timeout {
            return Err("timed out waiting for live agent automation node to complete".to_string());
        }
        if tokio::time::timeout(timeout - elapsed, notified)
            .await
            .is_err()
        {
            return Err("timed out waiting for live agent automation node to complete".to_string());
        }
    }
}

fn live_agent_terminal_failure(
    snapshot: &crate::state::agent_watch::WatchSnapshot,
) -> Option<String> {
    latest_terminal_status(snapshot).and_then(|status| match status.as_str() {
        "action_required" | "off" | "error" => Some(format!(
            "live agent reached {status} before completing automation node"
        )),
        _ => None,
    })
}

fn latest_terminal_status(snapshot: &crate::state::agent_watch::WatchSnapshot) -> Option<String> {
    snapshot.events.iter().rev().find_map(|event| {
        (event.kind == "status")
            .then(|| event.payload.get("status").and_then(|value| value.as_str()))
            .flatten()
            .map(wardian_core::identity::normalize_status)
    })
}

/// Test double: scripted responses keyed by node id; records call order.
#[derive(Default)]
pub struct FakeAgentRunner {
    responses: HashMap<String, String>,
    calls: Mutex<Vec<String>>,
}

impl FakeAgentRunner {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_response(mut self, node: &str, response: &str) -> Self {
        self.responses.insert(node.into(), response.into());
        self
    }

    pub fn calls(&self) -> Vec<String> {
        self.calls.lock().expect("fake runner calls lock").clone()
    }
}

impl AgentRunner for FakeAgentRunner {
    fn run(&self, spec: AgentRunSpec) -> AgentRunFuture<'_> {
        Box::pin(async move {
            self.calls
                .lock()
                .expect("fake runner calls lock")
                .push(spec.node.clone());

            Ok(self
                .responses
                .get(&spec.node)
                .cloned()
                .unwrap_or_else(|| "{}".into()))
        })
    }
}

/// Test double for active-agent execution.
#[derive(Default)]
pub struct FakeLiveAgentRunner {
    responses: HashMap<String, String>,
    calls: Mutex<Vec<String>>,
}

impl FakeLiveAgentRunner {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_response(mut self, session_id: &str, response: &str) -> Self {
        self.responses.insert(session_id.into(), response.into());
        self
    }

    pub fn calls(&self) -> Vec<String> {
        self.calls
            .lock()
            .expect("fake live runner calls lock")
            .clone()
    }
}

impl LiveAgentRunner for FakeLiveAgentRunner {
    fn run_live(&self, spec: LiveAgentRunSpec) -> LiveAgentRunFuture<'_> {
        Box::pin(async move {
            self.calls
                .lock()
                .expect("fake live runner calls lock")
                .push(spec.session_id.clone());

            Ok(self
                .responses
                .get(&spec.session_id)
                .cloned()
                .unwrap_or_else(|| "{}".into()))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn worker_error_classification_distinguishes_failure_uncertainty_and_cancellation() {
        let failed = crate::manager::HeadlessRunError::definite("provider rejected launch");
        let uncertain = crate::manager::HeadlessRunError::uncertain("connection lost after submit");
        let cancelled = crate::manager::HeadlessRunError::cancelled("owned run cancelled");

        assert_eq!(
            temporary_worker_error_state(&failed),
            Some(TemporaryWorkerState::Failed)
        );
        assert_eq!(temporary_worker_error_state(&uncertain), None);
        assert_eq!(
            temporary_worker_error_state(&cancelled),
            Some(TemporaryWorkerState::Cancelled)
        );
    }

    #[tokio::test]
    async fn fake_runner_returns_scripted_response_and_records_calls() {
        let runner = FakeAgentRunner::new().with_response("plan", "```json\n{\"ok\":true}\n```");
        let spec = AgentRunSpec {
            node: "plan".into(),
            provider: "mock".into(),
            cwd: std::path::PathBuf::from("."),
            prompt: "do".into(),
            session_id: String::new(),
            agent_session_id: None,
            resume_session: None,
            config_override: None,
            lease_owner: None,
            temporary_origin: None,
        };
        let out = runner.run(spec).await.unwrap();
        assert!(out.contains("ok"));
        assert_eq!(runner.calls(), vec!["plan".to_string()]);
    }

    #[tokio::test]
    async fn live_agent_wait_does_not_complete_on_idle_status_alone() {
        let state = Arc::new(Mutex::new(crate::state::AgentWatchState::new(
            "agent-1".to_string(),
            16,
            4096,
        )));
        let since = state.lock().expect("watch state lock").latest_cursor();
        {
            let mut guard = state.lock().expect("watch state lock");
            guard.push_output(b"stale terminal repaint text");
            guard.push_event("status", serde_json::json!({ "status": "idle" }));
        }

        let result = wait_for_live_agent_status_transition_for_regression(
            state,
            since,
            Duration::from_millis(10),
        )
        .await;

        assert_eq!(
            result.expect_err("idle status alone must not complete a live automation node"),
            "timed out waiting for live agent automation node to complete"
        );
    }

    #[tokio::test]
    async fn live_agent_reply_wait_completes_only_after_structured_reply() {
        let _home = crate::control::test_support::TestWardianHome::new_async().await;
        let app_state = crate::state::AppState::new();
        let watch_state = Arc::new(Mutex::new(crate::state::AgentWatchState::new(
            "agent-1".to_string(),
            16,
            4096,
        )));
        let since = watch_state
            .lock()
            .expect("watch state lock")
            .latest_cursor();
        {
            let mut guard = watch_state.lock().expect("watch state lock");
            guard.push_output(b"stale terminal repaint text");
            guard.push_event("status", serde_json::json!({ "status": "idle" }));
        }
        let task = app_state
            .interactions
            .admit_host_automation_task("wf_test_reply", "test-node", "agent-1", "write the file")
            .await
            .unwrap()
            .record;
        let task_id = task.id.clone();

        let wait = wait_for_live_agent_reply(
            &app_state,
            watch_state,
            since,
            &task_id,
            "agent-1",
            Duration::from_secs(1),
        );
        let complete = async {
            tokio::time::sleep(Duration::from_millis(25)).await;
            app_state
                .interactions
                .reply_agent_message("agent-1", &task_id, ReplyStatus::Done, "{\"ok\":true}")
                .await
                .expect("complete automation task")
        };

        let (reply, _) = tokio::join!(wait, complete);

        let reply = reply.expect("structured reply should complete automation node");
        assert_eq!(reply.status, ReplyStatus::Done);
        assert_eq!(reply.body, "{\"ok\":true}");
    }

    #[tokio::test]
    async fn live_agent_reply_wait_ignores_printed_reply_command_after_idle() {
        let _home = crate::control::test_support::TestWardianHome::new_async().await;
        let app_state = crate::state::AppState::new();
        let watch_state = Arc::new(Mutex::new(crate::state::AgentWatchState::new(
            "agent-1".to_string(),
            16,
            4096,
        )));
        let since = watch_state
            .lock()
            .expect("watch state lock")
            .latest_cursor();
        let task = app_state
            .interactions
            .admit_host_automation_task("wf_test_marker", "test-node", "agent-1", "write the file")
            .await
            .unwrap()
            .record;
        let task_id = task.id.clone();
        {
            let mut guard = watch_state.lock().expect("watch state lock");
            guard.push_transcript(wardian_core::control::WatchTranscriptMessage {
                role: "assistant".to_string(),
                text: format!(
                    "Final automation output\n\nwardian reply {task_id} --status done --stdin"
                ),
                provider: "gemini".to_string(),
                turn_id: None,
                source: Some("gemini_log".to_string()),
                provider_provenance: None,
            });
            guard.push_event("status", serde_json::json!({ "status": "idle" }));
        }

        let error = wait_for_live_agent_reply(
            &app_state,
            watch_state,
            since,
            &task_id,
            "agent-1",
            Duration::from_millis(10),
        )
        .await
        .expect_err("printed reply command should not complete automation task");

        assert!(error.contains("timed out waiting for live agent automation reply"));
        assert!(app_state
            .interactions
            .structured_reply(&task_id)
            .await
            .is_none());
    }

    #[tokio::test]
    async fn live_agent_reply_wait_survives_watch_cursor_rollover() {
        let _home = crate::control::test_support::TestWardianHome::new_async().await;
        let app_state = crate::state::AppState::new();
        let watch_state = Arc::new(Mutex::new(crate::state::AgentWatchState::new(
            "agent-1".to_string(),
            2,
            4096,
        )));
        let since = watch_state
            .lock()
            .expect("watch state lock")
            .latest_cursor();
        let task = app_state
            .interactions
            .admit_host_automation_task(
                "wf_test_cursor_rollover",
                "test-node",
                "agent-1",
                "write the file",
            )
            .await
            .unwrap()
            .record;
        let task_id = task.id.clone();
        {
            let mut guard = watch_state.lock().expect("watch state lock");
            guard.push_event("status", serde_json::json!({ "status": "processing" }));
            guard.push_transcript(wardian_core::control::WatchTranscriptMessage {
                role: "assistant".to_string(),
                text: "working".to_string(),
                provider: "mock".to_string(),
                turn_id: None,
                source: None,
                provider_provenance: None,
            });
            guard.push_output(b"still working");
        }

        let wait = wait_for_live_agent_reply(
            &app_state,
            watch_state,
            since,
            &task_id,
            "agent-1",
            Duration::from_secs(1),
        );
        let complete = async {
            tokio::time::sleep(Duration::from_millis(25)).await;
            app_state
                .interactions
                .reply_agent_message("agent-1", &task_id, ReplyStatus::Done, "{\"ok\":true}")
                .await
                .expect("complete automation task")
        };

        let (reply, _) = tokio::join!(wait, complete);

        let reply = reply.expect("cursor rollover should not fail structured reply waits");
        assert_eq!(reply.status, ReplyStatus::Done);
        assert_eq!(reply.body, "{\"ok\":true}");
    }
    #[tokio::test]
    async fn canonical_automation_timeout_preserves_uncertainty_and_accepts_late_reply() {
        let _home = crate::control::test_support::TestWardianHome::new_async().await;
        let state = crate::state::AppState::new();
        let task = state
            .interactions
            .admit_host_automation_task("timeout-run", "node", "receiver", "work")
            .await
            .unwrap();
        let claim = state
            .interactions
            .claim_agent_task("receiver", 0)
            .await
            .unwrap()
            .unwrap();
        state
            .interactions
            .finish_agent_task(&claim, "uncertain")
            .await
            .unwrap();
        let watch = Arc::new(Mutex::new(crate::state::AgentWatchState::new(
            "receiver".into(),
            16,
            4096,
        )));
        let cursor = watch.lock().unwrap().latest_cursor();
        let before = state
            .interactions
            .interaction(&task.record.id)
            .await
            .unwrap();
        let error = wait_for_live_agent_reply(
            &state,
            watch.clone(),
            cursor.clone(),
            &task.record.id,
            "receiver",
            Duration::ZERO,
        )
        .await
        .unwrap_err();
        assert!(error.contains(&task.record.id));
        assert_eq!(
            state
                .interactions
                .interaction(&task.record.id)
                .await
                .unwrap(),
            before
        );
        assert!(state
            .interactions
            .structured_reply(&task.record.id)
            .await
            .is_none());
        assert!(
            state
                .interactions
                .claim_agent_task("receiver", 0)
                .await
                .unwrap()
                .is_none(),
            "timeout cannot replay uncertain work"
        );
        assert!(state
            .interactions
            .receive_agent_messages("receiver", None, None, 100)
            .await
            .unwrap()
            .messages
            .is_empty());
        state
            .interactions
            .reply_agent_message(
                "receiver",
                &task.record.id,
                ReplyStatus::Done,
                "late correlated result",
            )
            .await
            .unwrap();
        let reply = wait_for_live_agent_reply(
            &state,
            watch,
            cursor,
            &task.record.id,
            "receiver",
            Duration::ZERO,
        )
        .await
        .unwrap();
        assert_eq!(
            automation_reply_result("node", reply).unwrap(),
            "late correlated result"
        );
    }

    #[tokio::test]
    async fn canonical_automation_reply_status_controls_node_result() {
        let _home = crate::control::test_support::TestWardianHome::new_async().await;
        let state = crate::state::AppState::new();
        for status in [ReplyStatus::Done, ReplyStatus::Blocked, ReplyStatus::Failed] {
            let task = state
                .interactions
                .admit_host_automation_task("status-run", "node", "receiver", "work")
                .await
                .unwrap();
            let replied = state
                .interactions
                .reply_agent_message("receiver", &task.record.id, status.clone(), "node output")
                .await
                .unwrap();
            let result = automation_reply_result("node", replied.reply);
            match status {
                ReplyStatus::Done => assert_eq!(result.unwrap(), "node output"),
                ReplyStatus::Blocked | ReplyStatus::Failed => {
                    let error = result.unwrap_err();
                    assert!(error.contains(&task.record.id));
                    assert!(error.contains("node output"));
                }
            }
        }
    }
}
