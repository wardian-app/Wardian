use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Mutex;

type AgentRunFuture<'a> = Pin<Box<dyn Future<Output = Result<String, String>> + Send + 'a>>;

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

/// Boundary between the executor logic and the real headless runtime. Unit tests
/// inject a fake so they never spawn a provider.
pub trait AgentRunner: Send + Sync {
    /// Run the prompt headlessly; return the agent's textual response.
    fn run(&self, spec: AgentRunSpec) -> AgentRunFuture<'_>;
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
