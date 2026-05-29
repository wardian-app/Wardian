pub mod ops;
pub mod output;
pub mod resolve;
pub mod runner;
pub mod runs;

use runner::{AgentRunSpec, AgentRunner};
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use wardian_core::engine::{
    AgentTaskRequest, ChosenPort, DecisionRequest, NotifyRequest, ScriptRequest, ShellRequest,
    StateRequest, StepError, StepExecutor, StepOutput,
};

/// The real StepExecutor: drives headless agents and local side effects for one
/// workflow run.
pub struct LiveStepExecutor {
    runner: Arc<dyn AgentRunner>,
    workspace: PathBuf,
    default_provider: String,
}

impl LiveStepExecutor {
    pub fn new(runner: Arc<dyn AgentRunner>, workspace: PathBuf, default_provider: String) -> Self {
        Self {
            runner,
            workspace,
            default_provider,
        }
    }

    async fn run_prompt(
        &self,
        node: &str,
        agent_ref: &str,
        prompt: String,
    ) -> Result<String, StepError> {
        let resolved = resolve::resolve_agent(agent_ref, &self.workspace, &self.default_provider);
        if !resolved.is_ephemeral {
            crate::utils::logging::log_debug(&format!(
                "[workflow-v2] node {node}: live-agent routing not yet supported; running '{agent_ref}' headless"
            ));
        }

        self.runner
            .run(AgentRunSpec {
                node: node.to_string(),
                provider: resolved.provider,
                cwd: resolved.cwd,
                prompt,
                session_id: resolved.session_id,
                resume_session: resolved.resume_session,
            })
            .await
            .map_err(StepError::new)
    }
}

impl StepExecutor for LiveStepExecutor {
    fn run_agent_task<'life0, 'async_trait>(
        &'life0 self,
        req: AgentTaskRequest,
    ) -> Pin<Box<dyn Future<Output = Result<StepOutput, StepError>> + Send + 'async_trait>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
            let response = self.run_prompt(&req.node, &req.agent, req.prompt).await?;
            Ok(StepOutput(output::extract_structured_output(
                &response,
                req.output_schema.as_deref(),
            )))
        })
    }

    fn run_decision<'life0, 'async_trait>(
        &'life0 self,
        req: DecisionRequest,
    ) -> Pin<Box<dyn Future<Output = Result<ChosenPort, StepError>> + Send + 'async_trait>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
            let choices_line = format!(
                "\n\nRespond with exactly one of: {}",
                req.choices.join(", ")
            );
            let response = self
                .run_prompt(
                    &req.node,
                    &req.agent,
                    format!("{}{}", req.prompt, choices_line),
                )
                .await?;

            if let Some(port) = output::parse_decision_port(&response, &req.choices) {
                return Ok(ChosenPort(port));
            }

            let strict = format!(
                "{}\n\nReply with ONLY one of these exact words: {}",
                req.prompt,
                req.choices.join(", ")
            );
            let response = self.run_prompt(&req.node, &req.agent, strict).await?;
            output::parse_decision_port(&response, &req.choices)
                .map(ChosenPort)
                .ok_or_else(|| {
                    StepError::new(format!(
                        "decision node {} did not choose a declared port",
                        req.node
                    ))
                })
        })
    }

    fn run_shell<'life0, 'async_trait>(
        &'life0 self,
        req: ShellRequest,
    ) -> Pin<Box<dyn Future<Output = Result<StepOutput, StepError>> + Send + 'async_trait>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move { ops::run_shell(&self.workspace, &req).await })
    }

    fn run_script<'life0, 'async_trait>(
        &'life0 self,
        req: ScriptRequest,
    ) -> Pin<Box<dyn Future<Output = Result<StepOutput, StepError>> + Send + 'async_trait>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move { ops::run_script(&self.workspace, &req).await })
    }

    fn notify<'life0, 'async_trait>(
        &'life0 self,
        req: NotifyRequest,
    ) -> Pin<Box<dyn Future<Output = Result<(), StepError>> + Send + 'async_trait>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move { ops::notify(&req) })
    }

    fn state_op<'life0, 'async_trait>(
        &'life0 self,
        req: StateRequest,
    ) -> Pin<Box<dyn Future<Output = Result<StepOutput, StepError>> + Send + 'async_trait>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move { ops::state_op(&req) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workflow_v2::runner::FakeAgentRunner;
    use std::sync::Arc;
    use wardian_core::engine::executor::{AgentTaskRequest, DecisionRequest, StepExecutor};

    fn exec_with(runner: FakeAgentRunner) -> LiveStepExecutor {
        LiveStepExecutor::new(
            Arc::new(runner),
            std::path::PathBuf::from("."),
            "mock".into(),
        )
    }

    #[tokio::test]
    async fn agent_task_extracts_structured_output() {
        let exec =
            exec_with(FakeAgentRunner::new().with_response("plan", "```json\n{\"go\":true}\n```"));
        let out = exec
            .run_agent_task(AgentTaskRequest {
                node: "plan".into(),
                agent: "role:Coder".into(),
                prompt: "p".into(),
                output_schema: None,
            })
            .await
            .unwrap();
        assert_eq!(out.0["go"], true);
    }

    #[tokio::test]
    async fn decision_resolves_to_declared_choice() {
        let exec = exec_with(FakeAgentRunner::new().with_response("router", "I pick deny"));
        let port = exec
            .run_decision(DecisionRequest {
                node: "router".into(),
                agent: "role:x".into(),
                prompt: "p".into(),
                choices: vec!["approve".into(), "deny".into()],
            })
            .await
            .unwrap();
        assert_eq!(port.0, "deny");
    }

    #[tokio::test]
    async fn decision_fails_when_no_choice_after_reprompt() {
        let exec = exec_with(FakeAgentRunner::new().with_response("router", "no idea"));
        let res = exec
            .run_decision(DecisionRequest {
                node: "router".into(),
                agent: "role:x".into(),
                prompt: "p".into(),
                choices: vec!["approve".into(), "deny".into()],
            })
            .await;
        assert!(res.is_err());
    }
}
