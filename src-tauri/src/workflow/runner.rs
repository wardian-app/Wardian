use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Mutex;
use std::time::Duration;
use tauri::Manager;

type AgentRunFuture<'a> = Pin<Box<dyn Future<Output = Result<String, String>> + Send + 'a>>;
type LiveAgentRunFuture<'a> = Pin<Box<dyn Future<Output = Result<String, String>> + Send + 'a>>;

/// What the executor needs to run one headless agent prompt.
#[derive(Debug, Clone)]
pub struct AgentRunSpec {
    pub node: String,
    pub provider: String,
    pub cwd: PathBuf,
    pub prompt: String,
    pub session_id: String,
    pub resume_session: Option<String>,
}

/// What the executor needs to route one prompt into an already-running agent.
#[derive(Debug, Clone)]
pub struct LiveAgentRunSpec {
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
/// existing live PTY and waits for the agent's next terminal status transition.
pub trait LiveAgentRunner: Send + Sync {
    fn run_live(&self, spec: LiveAgentRunSpec) -> LiveAgentRunFuture<'_>;
}

/// Real runner: drives `manager::headless::run_headless_with_options` and pulls
/// the `response` field out of the provider-normalized JSON.
pub struct HeadlessAgentRunner;

impl AgentRunner for HeadlessAgentRunner {
    fn run(&self, spec: AgentRunSpec) -> AgentRunFuture<'_> {
        Box::pin(async move {
            let value =
                crate::manager::run_headless_with_options(crate::manager::HeadlessRunOptions {
                    cwd: &spec.cwd,
                    prompt: &spec.prompt,
                    wardian_session_id: &spec.session_id,
                    resume_session: spec.resume_session.as_deref(),
                    output_format: "json",
                    provider_name: &spec.provider,
                    config_override: None,
                })
                .await?;

            let response = value
                .get("response")
                .and_then(|value| value.as_str())
                .or_else(|| value.get("text").and_then(|value| value.as_str()))
                .map(ToString::to_string)
                .unwrap_or_else(|| value.to_string());

            Ok(response)
        })
    }
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
    let delivery_lock = state.delivery_lock_for(&spec.session_id).await;
    let _delivery_guard = delivery_lock.lock().await;

    let (tx, watch_state, current_status, provider_name, should_mark_processing) = {
        let agents = state.agents.lock().await;
        let agent = agents
            .get(&spec.session_id)
            .ok_or_else(|| format!("Agent {} not found or is off", spec.session_id))?;
        let config = agent
            .config
            .lock()
            .map_err(|_| format!("Agent {} config lock poisoned", spec.session_id))?
            .clone();
        let tx = state
            .input_senders
            .try_read()
            .map_err(|_| "Input channel temporarily locked".to_string())?
            .get(&spec.session_id)
            .cloned()
            .ok_or_else(|| format!("Agent {} not found or is off", spec.session_id))?;
        (
            tx,
            agent.watch_state.clone(),
            agent.current_status.clone(),
            config.provider,
            crate::manager::mark_agent_prompt_started(agent),
        )
    };

    let initial_cursor = watch_state
        .lock()
        .map_err(|_| format!("Agent {} watch state lock poisoned", spec.session_id))?
        .latest_cursor();

    if should_mark_processing {
        crate::manager::set_agent_status(app, &spec.session_id, &current_status, "Processing...");
    }
    state
        .interactions
        .start_provider_input_generation(
            &spec.session_id,
            wardian_core::control::ProviderInputReadiness::Busy,
            None,
        )
        .await;

    crate::utils::terminal_input::submit_prompt_via_sender(&tx, &spec.prompt, &provider_name)
        .await
        .map_err(|error| {
            format!(
                "failed to submit workflow node {} to live agent {}: {error}",
                spec.node, spec.session_id
            )
        })?;

    let snapshot =
        wait_for_live_agent_completion(watch_state, initial_cursor, spec.timeout).await?;
    let response = live_agent_response_from_snapshot(&snapshot);
    if response.trim().is_empty() {
        Err(format!(
            "live agent {} completed workflow node {} without readable output",
            spec.session_id, spec.node
        ))
    } else {
        Ok(response)
    }
}

async fn wait_for_live_agent_completion(
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

        if let Some(status) = latest_terminal_status(&snapshot) {
            match status.as_str() {
                "idle" => return Ok(snapshot),
                "action_required" | "off" | "error" => {
                    return Err(format!(
                        "live agent reached {status} before completing workflow node"
                    ));
                }
                _ => {}
            }
        }

        let elapsed = started.elapsed();
        if elapsed >= timeout {
            return Err("timed out waiting for live agent workflow node to complete".to_string());
        }
        if tokio::time::timeout(timeout - elapsed, notified)
            .await
            .is_err()
        {
            return Err("timed out waiting for live agent workflow node to complete".to_string());
        }
    }
}

fn latest_terminal_status(snapshot: &crate::state::agent_watch::WatchSnapshot) -> Option<String> {
    snapshot.events.iter().rev().find_map(|event| {
        (event.kind == "status")
            .then(|| event.payload.get("status").and_then(|value| value.as_str()))
            .flatten()
            .map(wardian_core::identity::normalize_status)
    })
}

fn live_agent_response_from_snapshot(
    snapshot: &crate::state::agent_watch::WatchSnapshot,
) -> String {
    snapshot
        .transcript
        .messages
        .iter()
        .rev()
        .find(|message| {
            !message.text.trim().is_empty() && !matches!(message.role.as_str(), "user" | "human")
        })
        .map(|message| message.text.clone())
        .or_else(|| {
            (!snapshot.transcript.latest_text.trim().is_empty())
                .then(|| snapshot.transcript.latest_text.clone())
        })
        .unwrap_or_else(|| snapshot.output.text.clone())
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

    #[tokio::test]
    async fn fake_runner_returns_scripted_response_and_records_calls() {
        let runner = FakeAgentRunner::new().with_response("plan", "```json\n{\"ok\":true}\n```");
        let spec = AgentRunSpec {
            node: "plan".into(),
            provider: "mock".into(),
            cwd: std::path::PathBuf::from("."),
            prompt: "do".into(),
            session_id: String::new(),
            resume_session: None,
        };
        let out = runner.run(spec).await.unwrap();
        assert!(out.contains("ok"));
        assert_eq!(runner.calls(), vec!["plan".to_string()]);
    }
}
