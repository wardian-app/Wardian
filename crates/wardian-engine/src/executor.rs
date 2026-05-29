use crate::error::StepError;
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Mutex;

/// A node's structured output, stored in the registry under `nodes.<id>.output`.
#[derive(Debug, Clone)]
pub struct StepOutput(pub serde_json::Value);
/// The branch port a decision agent chose (must be one of the declared choices).
#[derive(Debug, Clone)]
pub struct ChosenPort(pub String);

#[derive(Debug, Clone)]
pub struct AgentTaskRequest {
    pub node: String,
    pub agent: String,
    pub prompt: String,
    pub output_schema: Option<String>,
}

#[derive(Debug, Clone)]
pub struct DecisionRequest {
    pub node: String,
    pub agent: String,
    pub prompt: String,
    pub choices: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct ShellRequest {
    pub node: String,
    pub command: String,
    pub cwd: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ScriptRequest {
    pub node: String,
    pub runtime: String,
    pub path: String,
}

#[derive(Debug, Clone)]
pub struct NotifyRequest {
    pub node: String,
    pub message: String,
}

#[derive(Debug, Clone)]
pub struct StateRequest {
    pub node: String,
    pub op: String,
    pub entries: serde_json::Value,
}

/// The dependency-inversion boundary: everything side-effecting goes through
/// this. `src-tauri` provides the real impl later; tests use `MockExecutor`.
#[async_trait]
pub trait StepExecutor: Send + Sync {
    async fn run_agent_task(&self, req: AgentTaskRequest) -> Result<StepOutput, StepError>;
    async fn run_decision(&self, req: DecisionRequest) -> Result<ChosenPort, StepError>;
    async fn run_shell(&self, req: ShellRequest) -> Result<StepOutput, StepError>;
    async fn run_script(&self, req: ScriptRequest) -> Result<StepOutput, StepError>;
    async fn notify(&self, req: NotifyRequest) -> Result<(), StepError>;
    async fn state_op(&self, req: StateRequest) -> Result<StepOutput, StepError>;
}

/// Records calls and returns scripted results. Default output is `{}`; default
/// decision is the first choice.
#[derive(Default)]
pub struct MockExecutor {
    task_outputs: HashMap<String, serde_json::Value>,
    decisions: HashMap<String, String>,
    fail_nodes: HashMap<String, String>,
    calls: Mutex<Vec<String>>,
}

impl MockExecutor {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_task_output(mut self, node: &str, out: serde_json::Value) -> Self {
        self.task_outputs.insert(node.into(), out);
        self
    }

    pub fn with_decision(mut self, node: &str, port: &str) -> Self {
        self.decisions.insert(node.into(), port.into());
        self
    }

    pub fn with_failure(mut self, node: &str, err: &str) -> Self {
        self.fail_nodes.insert(node.into(), err.into());
        self
    }

    pub fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }

    fn record(&self, c: String) {
        self.calls.lock().unwrap().push(c);
    }

    fn check_fail(&self, node: &str) -> Result<(), StepError> {
        match self.fail_nodes.get(node) {
            Some(e) => Err(StepError::new(e.clone())),
            None => Ok(()),
        }
    }
}

#[async_trait]
impl StepExecutor for MockExecutor {
    async fn run_agent_task(&self, req: AgentTaskRequest) -> Result<StepOutput, StepError> {
        self.record(format!("task:{}", req.node));
        self.check_fail(&req.node)?;
        Ok(StepOutput(
            self.task_outputs
                .get(&req.node)
                .cloned()
                .unwrap_or(serde_json::json!({})),
        ))
    }

    async fn run_decision(&self, req: DecisionRequest) -> Result<ChosenPort, StepError> {
        self.record(format!("decision:{}", req.node));
        self.check_fail(&req.node)?;
        let port = self
            .decisions
            .get(&req.node)
            .cloned()
            .or_else(|| req.choices.first().cloned())
            .ok_or_else(|| StepError::new("decision has no choices"))?;
        Ok(ChosenPort(port))
    }

    async fn run_shell(&self, req: ShellRequest) -> Result<StepOutput, StepError> {
        self.record(format!("shell:{}", req.node));
        self.check_fail(&req.node)?;
        Ok(StepOutput(serde_json::json!({"exit_code": 0})))
    }

    async fn run_script(&self, req: ScriptRequest) -> Result<StepOutput, StepError> {
        self.record(format!("script:{}", req.node));
        self.check_fail(&req.node)?;
        Ok(StepOutput(serde_json::json!({"exit_code": 0})))
    }

    async fn notify(&self, req: NotifyRequest) -> Result<(), StepError> {
        self.record(format!("notify:{}", req.node));
        self.check_fail(&req.node)
    }

    async fn state_op(&self, req: StateRequest) -> Result<StepOutput, StepError> {
        self.record(format!("state:{}", req.node));
        self.check_fail(&req.node)?;
        Ok(StepOutput(serde_json::json!({})))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn mock_returns_scripted_task_output_and_records_calls() {
        let mock =
            MockExecutor::new().with_task_output("plan", serde_json::json!({"decision": "go"}));
        let out = mock
            .run_agent_task(AgentTaskRequest {
                node: "plan".into(),
                agent: "role:x".into(),
                prompt: "do it".into(),
                output_schema: None,
            })
            .await
            .unwrap();
        assert_eq!(out.0["decision"], "go");
        assert_eq!(mock.calls(), vec!["task:plan".to_string()]);
    }

    #[tokio::test]
    async fn mock_returns_scripted_decision_port() {
        let mock = MockExecutor::new().with_decision("router", "approve");
        let port = mock
            .run_decision(DecisionRequest {
                node: "router".into(),
                agent: "role:x".into(),
                prompt: "p".into(),
                choices: vec!["approve".into(), "deny".into()],
            })
            .await
            .unwrap();
        assert_eq!(port.0, "approve");
    }
}
