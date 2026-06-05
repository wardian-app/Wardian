pub mod ops;
pub mod output;
pub mod resolve;
pub mod runner;
pub mod runs;
pub mod schedule;

use resolve::{AgentBinding, AgentRouteInput, PlannedAgentRoute};
use runner::{AgentRunSpec, AgentRunner, LiveAgentRunSpec, LiveAgentRunner};
use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use wardian_core::engine::{
    AgentTaskRequest, ChosenPort, DecisionRequest, NotifyRequest, ScriptRequest, ShellRequest,
    StateRequest, StepError, StepExecutor, StepOutput,
};
use wardian_core::models::{InvocationKind, WorkflowAssignments, WorkflowRoleAssignment};

/// The real StepExecutor: drives headless agents and local side effects for one
/// workflow run.
pub struct LiveStepExecutor {
    runner: Arc<dyn AgentRunner>,
    live_runner: Option<Arc<dyn LiveAgentRunner>>,
    workspace: PathBuf,
    default_provider: String,
    bindings: HashMap<String, String>,
    assignments: WorkflowAssignments,
    agent_catalog: HashMap<String, AgentBinding>,
    owner_id: String,
}

impl LiveStepExecutor {
    pub fn new(
        runner: Arc<dyn AgentRunner>,
        workspace: PathBuf,
        default_provider: String,
        bindings: HashMap<String, String>,
        agent_catalog: HashMap<String, AgentBinding>,
    ) -> Self {
        let assignments = wardian_core::workflow::assignment::normalize_assignments(
            None,
            &bindings,
            InvocationKind::Manual,
        );
        Self::new_with_assignments_and_live_runner(
            runner,
            None,
            workspace,
            default_provider,
            bindings,
            assignments,
            agent_catalog,
        )
    }

    pub fn new_with_live_runner(
        runner: Arc<dyn AgentRunner>,
        live_runner: Option<Arc<dyn LiveAgentRunner>>,
        workspace: PathBuf,
        default_provider: String,
        bindings: HashMap<String, String>,
        agent_catalog: HashMap<String, AgentBinding>,
    ) -> Self {
        let assignments = wardian_core::workflow::assignment::normalize_assignments(
            None,
            &bindings,
            InvocationKind::Manual,
        );
        Self::new_with_assignments_and_live_runner(
            runner,
            live_runner,
            workspace,
            default_provider,
            bindings,
            assignments,
            agent_catalog,
        )
    }

    pub fn new_with_assignments_and_live_runner(
        runner: Arc<dyn AgentRunner>,
        live_runner: Option<Arc<dyn LiveAgentRunner>>,
        workspace: PathBuf,
        default_provider: String,
        bindings: HashMap<String, String>,
        assignments: WorkflowAssignments,
        agent_catalog: HashMap<String, AgentBinding>,
    ) -> Self {
        Self {
            runner,
            live_runner,
            workspace,
            default_provider,
            bindings,
            assignments,
            agent_catalog,
            owner_id: "workflow/manual".to_string(),
        }
    }

    pub fn with_owner_id(mut self, owner_id: String) -> Self {
        self.owner_id = owner_id;
        self
    }

    async fn run_prompt(
        &self,
        node: &str,
        agent_ref: &str,
        prompt: String,
    ) -> Result<String, StepError> {
        let role = assignment_role_name(agent_ref);
        if let Some(assignment) = self.assignments.get(&role) {
            return self
                .run_assigned_prompt(node, agent_ref, &role, assignment, prompt)
                .await;
        }

        let resolved = resolve::resolve_agent_with_catalog(
            agent_ref,
            &self.workspace,
            &self.default_provider,
            &self.bindings,
            &self.agent_catalog,
        );
        if !resolved.is_ephemeral && !resolved.session_id.trim().is_empty() {
            let agent = AgentBinding {
                session_id: resolved.session_id.clone(),
                provider: resolved.provider.clone(),
                cwd: resolved.cwd.clone(),
                resume_session: resolved.resume_session.clone(),
                is_live: resolved.is_live,
                is_input_ready: resolved.is_input_ready,
            };
            return self
                .run_agent_binding_prompt(
                    node,
                    agent_ref,
                    agent_ref,
                    &resolved.session_id,
                    &agent,
                    wardian_core::models::AgentConversationMode::Current,
                    wardian_core::workflow::assignment::default_busy_policy_for(
                        InvocationKind::Manual,
                    ),
                    prompt,
                )
                .await;
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

    async fn run_assigned_prompt(
        &self,
        node: &str,
        agent_ref: &str,
        role: &str,
        assignment: &WorkflowRoleAssignment,
        prompt: String,
    ) -> Result<String, StepError> {
        match assignment {
            WorkflowRoleAssignment::TemporaryProvider {
                provider,
                workspace,
            } => {
                let cwd = workspace
                    .as_ref()
                    .map(PathBuf::from)
                    .unwrap_or_else(|| self.workspace.clone());
                self.runner
                    .run(AgentRunSpec {
                        node: node.to_string(),
                        provider: provider.clone(),
                        cwd,
                        prompt,
                        session_id: String::new(),
                        resume_session: None,
                    })
                    .await
                    .map_err(StepError::new)
            }
            WorkflowRoleAssignment::Agent {
                agent_id,
                conversation,
                busy_policy,
            } => {
                let agent = self.agent_catalog.get(agent_id).ok_or_else(|| {
                    StepError::new(format!(
                        "workflow role {role} is assigned to missing agent {agent_id}"
                    ))
                })?;
                self.run_agent_binding_prompt(
                    node,
                    agent_ref,
                    role,
                    agent_id,
                    agent,
                    conversation.clone(),
                    *busy_policy,
                    prompt,
                )
                .await
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn run_agent_binding_prompt(
        &self,
        node: &str,
        agent_ref: &str,
        role: &str,
        agent_id: &str,
        agent: &AgentBinding,
        conversation: wardian_core::models::AgentConversationMode,
        busy_policy: wardian_core::models::BusyPolicy,
        prompt: String,
    ) -> Result<String, StepError> {
        let route = resolve::choose_agent_route(AgentRouteInput {
            agent_id: agent_id.to_string(),
            conversation,
            busy_policy,
            is_live: agent.is_live,
            is_input_ready: agent.is_input_ready,
            has_resume_session: agent.resume_session.is_some(),
        });

        match route {
            PlannedAgentRoute::OpenSession => {
                let live_runner = self.live_runner.as_ref().ok_or_else(|| {
                    StepError::new(format!(
                        "workflow role {role} resolved to live agent {agent_id}, but live routing is unavailable"
                    ))
                })?;
                crate::utils::logging::log_debug(&format!(
                    "[workflow] node {node}: routing '{agent_ref}' to live agent {agent_id}"
                ));
                live_runner
                    .run_live(LiveAgentRunSpec {
                        node: node.to_string(),
                        session_id: agent.session_id.clone(),
                        prompt,
                        timeout: std::time::Duration::from_secs(600),
                    })
                    .await
                    .map_err(StepError::new)
            }
            PlannedAgentRoute::BackgroundResume => {
                crate::utils::logging::log_debug(&format!(
                    "[workflow] node {node}: background-resuming assigned agent {agent_id}"
                ));
                self.run_background_resume(node, agent, prompt).await
            }
            PlannedAgentRoute::BackgroundFresh => {
                crate::utils::logging::log_debug(&format!(
                    "[workflow] node {node}: running assigned agent {agent_id} as a fresh background conversation"
                ));
                self.runner
                    .run(AgentRunSpec {
                        node: node.to_string(),
                        provider: agent.provider.clone(),
                        cwd: agent.cwd.clone(),
                        prompt,
                        session_id: agent.session_id.clone(),
                        resume_session: None,
                    })
                    .await
                    .map_err(StepError::new)
            }
            PlannedAgentRoute::WaitForAgent => Err(StepError::new(format!(
                "workflow role {role} is assigned to busy agent {agent_id}; wait policy is not implemented yet"
            ))),
            PlannedAgentRoute::QueueForAgent => Err(StepError::new(format!(
                "workflow role {role} is assigned to busy agent {agent_id}; queue policy is not implemented yet"
            ))),
            PlannedAgentRoute::SkippedBusy => Err(StepError::skipped(format!(
                "workflow role {role} skipped because agent {agent_id} is busy"
            ))),
            PlannedAgentRoute::FailedBusy => Err(StepError::new(format!(
                "workflow role {role} failed because agent {agent_id} is busy"
            ))),
        }
    }

    async fn run_background_resume(
        &self,
        node: &str,
        agent: &AgentBinding,
        prompt: String,
    ) -> Result<String, StepError> {
        let resume_session = agent
            .resume_session
            .clone()
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| {
                StepError::new(format!(
                    "agent {} has no saved provider conversation for background resume",
                    agent.session_id
                ))
            })?;
        let lease = acquire_background_resume_lease(
            agent,
            &resume_session,
            &format!("{}/{}", self.owner_id, node),
            node,
        )
        .map_err(StepError::new)?;

        let result = self
            .runner
            .run(AgentRunSpec {
                node: node.to_string(),
                provider: agent.provider.clone(),
                cwd: agent.cwd.clone(),
                prompt,
                session_id: agent.session_id.clone(),
                resume_session: Some(resume_session),
            })
            .await;

        if let Err(error) = release_background_resume_lease(&lease.owner_kind, &lease.owner_id) {
            crate::utils::logging::log_debug(&format!(
                "[workflow] failed to release background resume lease: {error}"
            ));
        }

        result.map_err(StepError::new)
    }
}

fn assignment_role_name(agent_ref: &str) -> String {
    agent_ref
        .strip_prefix("role:")
        .or_else(|| agent_ref.strip_prefix("class:"))
        .unwrap_or(agent_ref)
        .to_string()
}

fn prompt_for_agent_task(prompt: String, output_schema: Option<&str>) -> String {
    let Some(schema) = output_schema
        .map(str::trim)
        .filter(|schema| !schema.is_empty())
    else {
        return prompt;
    };
    format!(
        "{prompt}\n\nWorkflow output contract:\nRespond with valid JSON that satisfies this output_schema. Return only the JSON object, or put the final JSON object in a trailing fenced ```json block.\noutput_schema:\n{schema}"
    )
}

fn acquire_background_resume_lease(
    agent: &AgentBinding,
    resume_session: &str,
    owner_id: &str,
    node: &str,
) -> Result<wardian_core::conversation_lease::ConversationLease, String> {
    let now = chrono::Utc::now();
    let now_rfc3339 = now.to_rfc3339();
    let expires_at = (now + chrono::Duration::minutes(20)).to_rfc3339();
    let lease = wardian_core::conversation_lease::ConversationLease {
        agent_id: agent.session_id.clone(),
        provider: agent.provider.clone(),
        resume_session: resume_session.to_string(),
        owner_kind: "workflow_run".to_string(),
        owner_id: owner_id.to_string(),
        owner_node_id: Some(node.to_string()),
        mode: "background_resume".to_string(),
        started_at: now_rfc3339.clone(),
        heartbeat_at: now_rfc3339,
        expires_at,
    };
    wardian_core::conversation_lease::acquire_lease(lease.clone(), &lease.started_at)?;
    Ok(lease)
}

fn release_background_resume_lease(owner_kind: &str, owner_id: &str) -> Result<(), String> {
    wardian_core::conversation_lease::release_owner_persisted(owner_kind, owner_id)
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
            let prompt = prompt_for_agent_task(req.prompt, req.output_schema.as_deref());
            let response = self.run_prompt(&req.node, &req.agent, prompt).await?;
            output::extract_structured_output(&response, req.output_schema.as_deref())
                .map(StepOutput)
                .map_err(StepError::new)
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
    use crate::workflow::runner::{FakeAgentRunner, FakeLiveAgentRunner};
    use std::sync::Arc;
    use std::sync::Mutex;
    use wardian_core::engine::executor::{AgentTaskRequest, DecisionRequest, StepExecutor};
    use wardian_core::models::{
        AgentConversationMode, BusyPolicy, WorkflowAssignments, WorkflowRoleAssignment,
    };

    fn exec_with(runner: FakeAgentRunner) -> LiveStepExecutor {
        LiveStepExecutor::new(
            Arc::new(runner),
            std::path::PathBuf::from("."),
            "mock".into(),
            HashMap::new(),
            HashMap::new(),
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
    async fn agent_task_with_schema_fails_when_response_is_not_structured() {
        let exec = exec_with(FakeAgentRunner::new().with_response("plan", "I am still thinking"));
        let err = exec
            .run_agent_task(AgentTaskRequest {
                node: "plan".into(),
                agent: "role:Coder".into(),
                prompt: "return json".into(),
                output_schema: Some(r#"{"decision":"string","reason":"string"}"#.into()),
            })
            .await
            .expect_err("schema-bound agent task should reject prose output");

        assert!(err.0.contains("valid JSON"));
    }

    #[tokio::test]
    async fn agent_task_with_schema_fails_when_required_field_is_missing() {
        let exec = exec_with(FakeAgentRunner::new().with_response("plan", r#"{"decision":"ok"}"#));
        let err = exec
            .run_agent_task(AgentTaskRequest {
                node: "plan".into(),
                agent: "role:Coder".into(),
                prompt: "return json".into(),
                output_schema: Some(r#"{"decision":"string","reason":"string"}"#.into()),
            })
            .await
            .expect_err("schema-bound agent task should require declared fields");

        assert!(err.0.contains("reason"));
    }

    #[tokio::test]
    async fn agent_task_with_schema_instructs_background_agents_to_return_json() {
        struct PromptCapturingRunner {
            prompt: Mutex<Option<String>>,
        }

        impl AgentRunner for PromptCapturingRunner {
            fn run(
                &self,
                spec: AgentRunSpec,
            ) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + '_>> {
                *self.prompt.lock().expect("prompt lock") = Some(spec.prompt);
                Box::pin(async { Ok(r#"{"decision":"buy","reason":"breakout"}"#.to_string()) })
            }
        }

        let runner = Arc::new(PromptCapturingRunner {
            prompt: Mutex::new(None),
        });
        let exec = LiveStepExecutor::new(
            runner.clone(),
            std::path::PathBuf::from("."),
            "mock".into(),
            HashMap::new(),
            HashMap::new(),
        );

        exec.run_agent_task(AgentTaskRequest {
            node: "plan".into(),
            agent: "role:Coder".into(),
            prompt: "analyze".into(),
            output_schema: Some(r#"{"decision":"string","reason":"string"}"#.into()),
        })
        .await
        .unwrap();

        let prompt = runner.prompt.lock().expect("prompt lock").clone().unwrap();
        assert!(prompt.contains("Respond with valid JSON"));
        assert!(prompt.contains(r#"{"decision":"string","reason":"string"}"#));
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

    #[tokio::test]
    async fn bound_active_agent_uses_live_runner_not_headless_runner() {
        let headless = Arc::new(FakeAgentRunner::new().with_response("plan", "{\"ok\":false}"));
        let live = Arc::new(FakeLiveAgentRunner::new().with_response("agent-123", "{\"ok\":true}"));

        let mut bindings = HashMap::new();
        bindings.insert("Coder".to_string(), "agent-123".to_string());

        let mut agent_catalog = HashMap::new();
        agent_catalog.insert(
            "agent-123".to_string(),
            AgentBinding {
                session_id: "agent-123".to_string(),
                provider: "gemini".to_string(),
                cwd: PathBuf::from("/agent-workspace"),
                resume_session: Some("provider-session".to_string()),
                is_live: true,
                is_input_ready: true,
            },
        );

        let exec = LiveStepExecutor::new_with_live_runner(
            headless.clone(),
            Some(live.clone()),
            PathBuf::from("/run-workspace"),
            "codex".into(),
            bindings,
            agent_catalog,
        );

        let out = exec
            .run_agent_task(AgentTaskRequest {
                node: "plan".into(),
                agent: "role:Coder".into(),
                prompt: "return json".into(),
                output_schema: None,
            })
            .await
            .unwrap();

        assert_eq!(out.0["ok"], true);
        assert_eq!(headless.calls(), Vec::<String>::new());
        assert_eq!(live.calls(), vec!["agent-123".to_string()]);
    }

    #[tokio::test]
    async fn bound_offline_agent_uses_headless_profile_runner() {
        let _guard = crate::utils::wardian_test_env_lock();
        let temp = tempfile::tempdir().expect("temp wardian home");
        let previous_home = std::env::var_os("WARDIAN_HOME");
        std::env::set_var("WARDIAN_HOME", temp.path());

        let headless = Arc::new(FakeAgentRunner::new().with_response("plan", "{\"ok\":true}"));
        let live =
            Arc::new(FakeLiveAgentRunner::new().with_response("agent-123", "{\"ok\":false}"));

        let mut bindings = HashMap::new();
        bindings.insert("Coder".to_string(), "agent-123".to_string());

        let mut agent_catalog = HashMap::new();
        agent_catalog.insert(
            "agent-123".to_string(),
            AgentBinding {
                session_id: "agent-123".to_string(),
                provider: "gemini".to_string(),
                cwd: PathBuf::from("/agent-workspace"),
                resume_session: Some("provider-session".to_string()),
                is_live: false,
                is_input_ready: false,
            },
        );

        let exec = LiveStepExecutor::new_with_live_runner(
            headless.clone(),
            Some(live.clone()),
            PathBuf::from("/run-workspace"),
            "codex".into(),
            bindings,
            agent_catalog,
        );

        let out = exec
            .run_agent_task(AgentTaskRequest {
                node: "plan".into(),
                agent: "role:Coder".into(),
                prompt: "return json".into(),
                output_schema: None,
            })
            .await
            .unwrap();

        assert_eq!(out.0["ok"], true);
        assert_eq!(headless.calls(), vec!["plan".to_string()]);
        assert_eq!(live.calls(), Vec::<String>::new());

        match previous_home {
            Some(value) => std::env::set_var("WARDIAN_HOME", value),
            None => std::env::remove_var("WARDIAN_HOME"),
        }
    }

    #[tokio::test]
    async fn background_resume_has_active_lease_during_headless_call() {
        struct LeaseCheckingRunner;

        impl AgentRunner for LeaseCheckingRunner {
            fn run(
                &self,
                spec: AgentRunSpec,
            ) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + '_>> {
                Box::pin(async move {
                    let leases = wardian_core::conversation_lease::load_leases();
                    let conflict = wardian_core::conversation_lease::find_active_conflict(
                        &leases,
                        &spec.session_id,
                        spec.resume_session.as_deref().unwrap_or_default(),
                        &chrono::Utc::now().to_rfc3339(),
                    );
                    assert!(conflict.is_some(), "background resume did not hold a lease");
                    Ok("{\"ok\":true}".to_string())
                })
            }
        }

        let _guard = crate::utils::wardian_test_env_lock();
        let temp = tempfile::tempdir().expect("temp wardian home");
        let previous_home = std::env::var_os("WARDIAN_HOME");
        std::env::set_var("WARDIAN_HOME", temp.path());

        let mut assignments = WorkflowAssignments::new();
        assignments.insert(
            "Coder".to_string(),
            WorkflowRoleAssignment::Agent {
                agent_id: "agent-123".to_string(),
                conversation: AgentConversationMode::Current,
                busy_policy: BusyPolicy::Wait,
            },
        );

        let mut agent_catalog = HashMap::new();
        agent_catalog.insert(
            "agent-123".to_string(),
            AgentBinding {
                session_id: "agent-123".to_string(),
                provider: "gemini".to_string(),
                cwd: PathBuf::from("/agent-workspace"),
                resume_session: Some("provider-session".to_string()),
                is_live: false,
                is_input_ready: false,
            },
        );

        let exec = LiveStepExecutor::new_with_assignments_and_live_runner(
            Arc::new(LeaseCheckingRunner),
            None,
            PathBuf::from("/run-workspace"),
            "codex".into(),
            HashMap::new(),
            assignments,
            agent_catalog,
        );

        let out = exec
            .run_agent_task(AgentTaskRequest {
                node: "plan".into(),
                agent: "role:Coder".into(),
                prompt: "return json".into(),
                output_schema: None,
            })
            .await
            .unwrap();

        assert_eq!(out.0["ok"], true);
        assert!(wardian_core::conversation_lease::load_leases().is_empty());

        match previous_home {
            Some(value) => std::env::set_var("WARDIAN_HOME", value),
            None => std::env::remove_var("WARDIAN_HOME"),
        }
    }

    #[tokio::test]
    async fn background_resume_requires_saved_provider_conversation() {
        let _guard = crate::utils::wardian_test_env_lock();
        let temp = tempfile::tempdir().expect("temp wardian home");
        let previous_home = std::env::var_os("WARDIAN_HOME");
        std::env::set_var("WARDIAN_HOME", temp.path());

        let mut assignments = WorkflowAssignments::new();
        assignments.insert(
            "Coder".to_string(),
            WorkflowRoleAssignment::Agent {
                agent_id: "agent-123".to_string(),
                conversation: AgentConversationMode::Current,
                busy_policy: BusyPolicy::Wait,
            },
        );

        let mut agent_catalog = HashMap::new();
        agent_catalog.insert(
            "agent-123".to_string(),
            AgentBinding {
                session_id: "agent-123".to_string(),
                provider: "gemini".to_string(),
                cwd: PathBuf::from("/agent-workspace"),
                resume_session: None,
                is_live: false,
                is_input_ready: false,
            },
        );

        let exec = LiveStepExecutor::new_with_assignments_and_live_runner(
            Arc::new(FakeAgentRunner::new().with_response("plan", "{\"ok\":true}")),
            None,
            PathBuf::from("/run-workspace"),
            "codex".into(),
            HashMap::new(),
            assignments,
            agent_catalog,
        );

        let err = exec
            .run_agent_task(AgentTaskRequest {
                node: "plan".into(),
                agent: "role:Coder".into(),
                prompt: "return json".into(),
                output_schema: None,
            })
            .await
            .expect_err("offline current conversation without resume_session should fail");

        assert!(err.to_string().contains("saved provider conversation"));

        match previous_home {
            Some(value) => std::env::set_var("WARDIAN_HOME", value),
            None => std::env::remove_var("WARDIAN_HOME"),
        }
    }

    #[tokio::test]
    async fn legacy_agent_binding_uses_assignment_route_not_unleased_headless_fallback() {
        let _guard = crate::utils::wardian_test_env_lock();
        let temp = tempfile::tempdir().expect("temp wardian home");
        let previous_home = std::env::var_os("WARDIAN_HOME");
        std::env::set_var("WARDIAN_HOME", temp.path());

        let mut bindings = HashMap::new();
        bindings.insert("Coder".to_string(), "agent-123".to_string());

        let mut agent_catalog = HashMap::new();
        agent_catalog.insert(
            "agent-123".to_string(),
            AgentBinding {
                session_id: "agent-123".to_string(),
                provider: "gemini".to_string(),
                cwd: PathBuf::from("/agent-workspace"),
                resume_session: None,
                is_live: false,
                is_input_ready: false,
            },
        );

        let exec = LiveStepExecutor::new_with_live_runner(
            Arc::new(FakeAgentRunner::new().with_response("plan", "{\"ok\":true}")),
            None,
            PathBuf::from("/run-workspace"),
            "codex".into(),
            bindings,
            agent_catalog,
        );

        let err = exec
            .run_agent_task(AgentTaskRequest {
                node: "plan".into(),
                agent: "role:Coder".into(),
                prompt: "return json".into(),
                output_schema: None,
            })
            .await
            .expect_err("legacy binding should use current-conversation route semantics");

        assert!(err.to_string().contains("saved provider conversation"));

        match previous_home {
            Some(value) => std::env::set_var("WARDIAN_HOME", value),
            None => std::env::remove_var("WARDIAN_HOME"),
        }
    }
}
