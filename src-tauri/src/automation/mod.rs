pub mod listener;
pub mod ops;
pub mod output;
pub mod resolve;
pub mod runner;
pub mod runs;
pub mod schedule;
pub mod session_close;

use resolve::{AgentBinding, AgentRouteInput, PlannedAgentRoute};
use runner::{AgentRunSpec, AgentRunner, LiveAgentRunSpec, LiveAgentRunner};
use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use wardian_core::engine::{
    AgentTaskRequest, ChosenPort, DecisionRequest, MemoryCommitRequest, NotifyRequest,
    ScriptRequest, ShellRequest, StepError, StepExecutor, StepOutput,
};
use wardian_core::models::{AutomationAssignments, AutomationRoleAssignment, InvocationKind};

/// The real StepExecutor: drives headless agents and local side effects for one
/// automation run.
pub struct LiveStepExecutor {
    runner: Arc<dyn AgentRunner>,
    live_runner: Option<Arc<dyn LiveAgentRunner>>,
    workspace: PathBuf,
    default_provider: String,
    bindings: HashMap<String, String>,
    assignments: AutomationAssignments,
    agent_catalog: HashMap<String, AgentBinding>,
    owner_id: String,
    memory_principal: Option<String>,
    notification_app: Option<tauri::AppHandle>,
}

impl LiveStepExecutor {
    pub fn new(
        runner: Arc<dyn AgentRunner>,
        workspace: PathBuf,
        default_provider: String,
        bindings: HashMap<String, String>,
        agent_catalog: HashMap<String, AgentBinding>,
    ) -> Self {
        let assignments = wardian_core::automation::assignment::normalize_assignments(
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
        let assignments = wardian_core::automation::assignment::normalize_assignments(
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
        assignments: AutomationAssignments,
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
            owner_id: "automation/manual".to_string(),
            memory_principal: None,
            notification_app: None,
        }
    }

    pub fn with_owner_id(mut self, owner_id: String) -> Self {
        self.owner_id = owner_id;
        self
    }

    /// Authorize memory commits for one invocation-owned agent. The value is
    /// supplied by a trusted launch boundary, never interpolated model/input
    /// data.
    pub fn with_memory_principal(mut self, agent_id: String) -> Self {
        self.memory_principal = Some(agent_id);
        self
    }

    pub fn with_notification_app(mut self, app: tauri::AppHandle) -> Self {
        self.notification_app = Some(app);
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
                config: resolved.config.clone(),
            };
            return self
                .run_agent_binding_prompt(
                    node,
                    agent_ref,
                    agent_ref,
                    &resolved.session_id,
                    &agent,
                    wardian_core::models::AgentConversationMode::Current,
                    wardian_core::automation::assignment::default_busy_policy_for(
                        InvocationKind::Manual,
                    ),
                    prompt,
                )
                .await;
        }

        let session_id = if resolved.session_id.trim().is_empty() {
            temporary_provider_session_id(&self.owner_id, node)
        } else {
            resolved.session_id
        };

        self.runner
            .run(AgentRunSpec {
                node: node.to_string(),
                provider: resolved.provider,
                cwd: resolved.cwd,
                prompt,
                session_id,
                agent_session_id: None,
                resume_session: resolved.resume_session,
                config_override: resolved.config,
                lease_owner: None,
            })
            .await
            .map_err(StepError::new)
    }

    async fn run_assigned_prompt(
        &self,
        node: &str,
        agent_ref: &str,
        role: &str,
        assignment: &AutomationRoleAssignment,
        prompt: String,
    ) -> Result<String, StepError> {
        match assignment {
            AutomationRoleAssignment::TemporaryProvider {
                provider,
                workspace,
                model,
                effort,
            } => {
                let cwd = workspace
                    .as_ref()
                    .map(PathBuf::from)
                    .unwrap_or_else(|| self.workspace.clone());
                let session_id = temporary_provider_session_id(&self.owner_id, node);
                let config_override = wardian_core::models::AgentConfig {
                    session_id: session_id.clone(),
                    provider: provider.clone(),
                    folder: cwd.to_string_lossy().to_string(),
                    model: model.clone(),
                    provider_config: match provider.as_str() {
                        "codex" => wardian_core::models::ProviderConfig::Codex(
                            wardian_core::models::CodexProviderConfig {
                                reasoning_effort: effort.clone(),
                                ..Default::default()
                            },
                        ),
                        "claude" => wardian_core::models::ProviderConfig::Claude(
                            wardian_core::models::ClaudeProviderConfig {
                                reasoning_effort: effort.clone(),
                                ..Default::default()
                            },
                        ),
                        "antigravity" => wardian_core::models::ProviderConfig::Antigravity(
                            wardian_core::models::AntigravityProviderConfig {
                                reasoning_effort: effort.clone(),
                                ..Default::default()
                            },
                        ),
                        "gemini" => {
                            wardian_core::models::ProviderConfig::Gemini(Default::default())
                        }
                        "opencode" => {
                            wardian_core::models::ProviderConfig::OpenCode(Default::default())
                        }
                        "mock" => wardian_core::models::ProviderConfig::Mock(Default::default()),
                        _ => wardian_core::models::ProviderConfig::Unknown(serde_json::json!({})),
                    },
                    ..Default::default()
                };
                self.runner
                    .run(AgentRunSpec {
                        node: node.to_string(),
                        provider: provider.clone(),
                        cwd,
                        prompt,
                        // Temporary providers are automation-owned sessions, not
                        // anonymous processes. The identity lets the provider
                        // habitat project its workspace link to this role's
                        // resolved project/folder while keeping it distinct
                        // from registered agents and their conversations.
                        session_id,
                        agent_session_id: None,
                        resume_session: None,
                        config_override: Some(config_override),
                        lease_owner: None,
                    })
                    .await
                    .map_err(StepError::new)
            }
            AutomationRoleAssignment::Agent {
                agent_id,
                conversation,
                busy_policy,
            } => {
                let agent = self.agent_catalog.get(agent_id).ok_or_else(|| {
                    StepError::new(format!(
                        "automation role {role} is assigned to missing agent {agent_id}"
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
                        "automation role {role} resolved to live agent {agent_id}, but live routing is unavailable"
                    ))
                })?;
                crate::utils::logging::log_debug(&format!(
                    "[automation] node {node}: routing '{agent_ref}' to live agent {agent_id}"
                ));
                live_runner
                    .run_live(LiveAgentRunSpec {
                        run_id: self.owner_id.clone(),
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
                    "[automation] node {node}: background-resuming assigned agent {agent_id}"
                ));
                self.run_background_resume(node, agent, prompt).await
            }
            PlannedAgentRoute::BackgroundFresh => {
                crate::utils::logging::log_debug(&format!(
                    "[automation] node {node}: running assigned agent {agent_id} as a fresh background conversation"
                ));
                self.run_background_fresh(node, agent, prompt).await
            }
            PlannedAgentRoute::WaitForAgent => Err(StepError::new(format!(
                "automation role {role} is assigned to busy agent {agent_id}; wait policy is not implemented yet"
            ))),
            PlannedAgentRoute::QueueForAgent => Err(StepError::new(format!(
                "automation role {role} is assigned to busy agent {agent_id}; queue policy is not implemented yet"
            ))),
            PlannedAgentRoute::SkippedBusy => Err(StepError::skipped(format!(
                "automation role {role} skipped because agent {agent_id} is busy"
            ))),
            PlannedAgentRoute::FailedBusy => Err(StepError::new(format!(
                "automation role {role} failed because agent {agent_id} is busy"
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
        let lease = acquire_background_agent_lease(
            agent,
            &resume_session,
            &format!("{}/{}", self.owner_id, node),
            node,
            "background_resume",
        )
        .map_err(StepError::new)?;
        let mut lease_guard =
            wardian_core::conversation_lease::PersistedConversationLeaseGuard::new(&lease);

        let result = self
            .runner
            .run(AgentRunSpec {
                node: node.to_string(),
                provider: agent.provider.clone(),
                cwd: agent.cwd.clone(),
                prompt,
                session_id: agent.session_id.clone(),
                agent_session_id: Some(agent.session_id.clone()),
                resume_session: Some(resume_session),
                config_override: agent.config.clone(),
                lease_owner: Some(lease_guard.owner().clone()),
            })
            .await;

        let release_result = lease_guard.release();
        match (result, release_result) {
            (Ok(response), Err(error)) => {
                crate::utils::logging::log_debug(&format!(
                    "[automation] background resume completed but lease cleanup remains pending until the guard retries it or the lease expires: {error}"
                ));
                Ok(response)
            }
            (Err(run_error), Err(release_error)) => Err(StepError::new(format!(
                "{run_error}; additionally failed to release conversation lease: {release_error}"
            ))),
            (result, Ok(())) => result.map_err(StepError::new),
        }
    }

    async fn run_background_fresh(
        &self,
        node: &str,
        agent: &AgentBinding,
        prompt: String,
    ) -> Result<String, StepError> {
        let lease = acquire_background_agent_lease(
            agent,
            "",
            &format!("{}/{}", self.owner_id, node),
            node,
            "background_fresh",
        )
        .map_err(StepError::new)?;
        let mut lease_guard =
            wardian_core::conversation_lease::PersistedConversationLeaseGuard::new(&lease);

        let result = self
            .runner
            .run(AgentRunSpec {
                node: node.to_string(),
                provider: agent.provider.clone(),
                cwd: agent.cwd.clone(),
                prompt,
                // Keep the provider conversation distinct from the registered
                // agent while the lease and lifecycle gate remain attached to
                // that agent's real session id.
                session_id: fresh_background_session_id(&self.owner_id, node),
                agent_session_id: Some(agent.session_id.clone()),
                resume_session: None,
                config_override: agent.config.clone(),
                lease_owner: Some(lease_guard.owner().clone()),
            })
            .await;

        let release_result = lease_guard.release();
        match (result, release_result) {
            (Ok(response), Err(error)) => {
                crate::utils::logging::log_debug(&format!(
                    "[automation] fresh background run completed but lease cleanup remains pending until the guard retries it or the lease expires: {error}"
                ));
                Ok(response)
            }
            (Err(run_error), Err(release_error)) => Err(StepError::new(format!(
                "{run_error}; additionally failed to release conversation lease: {release_error}"
            ))),
            (result, Ok(())) => result.map_err(StepError::new),
        }
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
        "{prompt}\n\nAutomation output contract:\nRespond with valid JSON that satisfies this output_schema. Return only the JSON object, or put the final JSON object in a trailing fenced ```json block.\noutput_schema:\n{schema}"
    )
}

fn fresh_background_session_id(owner_id: &str, node: &str) -> String {
    format!(
        "automation-bg-{}-{}",
        sanitize_session_component(owner_id),
        sanitize_session_component(node)
    )
}

fn temporary_provider_session_id(owner_id: &str, node: &str) -> String {
    format!(
        "automation-temp-{}-{}",
        sanitize_session_component(owner_id),
        sanitize_session_component(node)
    )
}

fn sanitize_session_component(value: &str) -> String {
    let sanitized: String = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '-'
            }
        })
        .collect();
    let trimmed = sanitized.trim_matches('-');
    if trimmed.is_empty() {
        "unknown".to_string()
    } else {
        trimmed.to_string()
    }
}

fn acquire_background_agent_lease(
    agent: &AgentBinding,
    resume_session: &str,
    owner_id: &str,
    node: &str,
    mode: &str,
) -> Result<wardian_core::conversation_lease::ConversationLease, String> {
    let now = chrono::Utc::now();
    let now_rfc3339 = now.to_rfc3339();
    let expires_at = (now + chrono::Duration::minutes(20)).to_rfc3339();
    let lease = wardian_core::conversation_lease::ConversationLease {
        agent_id: agent.session_id.clone(),
        provider: agent.provider.clone(),
        resume_session: resume_session.to_string(),
        owner_kind: "automation_run".to_string(),
        owner_id: owner_id.to_string(),
        acquisition_id: uuid::Uuid::new_v4().to_string(),
        owner_node_id: Some(node.to_string()),
        mode: mode.to_string(),
        started_at: now_rfc3339.clone(),
        heartbeat_at: now_rfc3339,
        expires_at,
    };
    wardian_core::conversation_lease::acquire_lease(lease.clone(), &lease.started_at)?;
    Ok(lease)
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
        Box::pin(async move { ops::notify(self.notification_app.as_ref(), &req) })
    }

    fn memory_commit<'life0, 'async_trait>(
        &'life0 self,
        req: MemoryCommitRequest,
    ) -> Pin<Box<dyn Future<Output = Result<StepOutput, StepError>> + Send + 'async_trait>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
            // The principal is issued at the trusted provider/invocation launch boundary.
            // Keep that decision stable for the lifetime of this running invocation.
            let principal = self.memory_principal.as_deref().ok_or_else(|| {
                StepError::new(
                    "memory_commit requires an authenticated invocation memory principal",
                )
            })?;
            if req.agent_id != principal {
                return Err(StepError::new(format!(
                    "memory_commit requested agent {} but the invocation authorizes {}",
                    req.agent_id, principal
                )));
            }
            let batch: wardian_core::memory::MemoryCommitBatch =
                serde_json::from_value(req.payload).map_err(|error| {
                    StepError::new(format!("invalid memory_commit payload: {error}"))
                })?;
            if batch.agent_id != req.agent_id {
                return Err(StepError::new(format!(
                    "memory_commit payload agent_id {} does not match authorized agent {}",
                    batch.agent_id, req.agent_id
                )));
            }
            if let Some(expected_workspace) = req.workspace.as_deref() {
                if wardian_core::memory::normalize_workspace(batch.workspace.as_deref())
                    != wardian_core::memory::normalize_workspace(Some(expected_workspace))
                {
                    return Err(StepError::new(
                        "memory_commit payload workspace does not match the invocation workspace",
                    ));
                }
            }
            if let Some(expected_key) = req
                .idempotency_key
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                if batch.idempotency_key.trim() != expected_key {
                    return Err(StepError::new(
                        "memory_commit payload idempotency key does not match the invocation boundary",
                    ));
                }
            }
            match req.archive_available {
                Some(true) => {
                    let expected_conversation = req
                        .conversation_id
                        .as_deref()
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .ok_or_else(|| {
                            StepError::new(
                                "archive-backed memory_commit requires a trusted conversation ID",
                            )
                        })?;
                    let expected_sequence = req.source_sequence.ok_or_else(|| {
                        StepError::new(
                            "archive-backed memory_commit requires a trusted source sequence",
                        )
                    })?;
                    let cursor = batch.cursor.as_ref().ok_or_else(|| {
                        StepError::new("archive-backed memory_commit requires a cursor")
                    })?;
                    let payload_conversation = cursor
                        .conversation_id
                        .as_deref()
                        .map(str::trim)
                        .filter(|value| !value.is_empty());
                    if payload_conversation != Some(expected_conversation)
                        || cursor.sequence != expected_sequence
                    {
                        return Err(StepError::new(
                            "memory_commit cursor does not match the invocation boundary",
                        ));
                    }
                }
                Some(false) if batch.cursor.is_some() => {
                    return Err(StepError::new(
                        "archive-less memory_commit must omit the cursor",
                    ));
                }
                _ => {}
            }
            let actor = wardian_core::memory::MemoryActor::agent(&req.agent_id);
            let result = wardian_core::memory::MemoryStore::from_default_home()
                .and_then(|store| store.commit_batch(&actor, batch))
                .map_err(|error| StepError::new(error.to_string()))?;
            serde_json::to_value(result)
                .map(StepOutput)
                .map_err(|error| StepError::new(error.to_string()))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::automation::runner::{FakeAgentRunner, FakeLiveAgentRunner};
    use std::sync::Arc;
    use std::sync::Mutex;
    use wardian_core::engine::executor::{AgentTaskRequest, DecisionRequest, StepExecutor};
    use wardian_core::models::{
        AgentConfig, AgentConversationMode, AutomationAssignments, AutomationRoleAssignment,
        BusyPolicy,
    };

    struct TestWardianHome {
        _lock: tokio::sync::MutexGuard<'static, ()>,
        _home: tempfile::TempDir,
        previous_home: Option<std::ffi::OsString>,
    }

    impl TestWardianHome {
        async fn new_async() -> Self {
            let lock = crate::utils::wardian_test_env_lock_async().await;
            let home = tempfile::tempdir().expect("temp wardian home");
            let previous_home = std::env::var_os("WARDIAN_HOME");
            std::env::set_var("WARDIAN_HOME", home.path());
            std::fs::create_dir_all(home.path().join("settings")).expect("settings directory");
            std::fs::write(
                home.path().join("settings/app.json"),
                r#"{"schema_version":2,"overrides":{"memory_enabled":true}}"#,
            )
            .expect("enable memory for automation tests");
            Self {
                _lock: lock,
                _home: home,
                previous_home,
            }
        }
    }

    impl Drop for TestWardianHome {
        fn drop(&mut self) {
            match self.previous_home.take() {
                Some(value) => std::env::set_var("WARDIAN_HOME", value),
                None => std::env::remove_var("WARDIAN_HOME"),
            }
        }
    }

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
    async fn memory_commit_rejects_model_selected_agent_identity() {
        let _home = TestWardianHome::new_async().await;
        let exec = exec_with(FakeAgentRunner::new()).with_memory_principal("agent-a".into());
        let payload = serde_json::json!({
            "agent_id": "agent-b",
            "idempotency_key": "run:boundary:one",
            "operations": []
        });

        let error = exec
            .memory_commit(MemoryCommitRequest {
                node: "commit".into(),
                agent_id: "agent-a".into(),
                workspace: None,
                conversation_id: None,
                source_sequence: None,
                archive_available: None,
                idempotency_key: None,
                payload,
            })
            .await
            .expect_err("model-selected peer identity must be rejected");

        assert!(error
            .to_string()
            .contains("does not match authorized agent"));
        assert!(wardian_core::memory::MemoryStore::from_default_home()
            .unwrap()
            .list_events(&wardian_core::memory::MemoryActor::Operator, "agent-b")
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn memory_commit_keeps_launch_authority_after_setting_changes() {
        let home = TestWardianHome::new_async().await;
        let exec = exec_with(FakeAgentRunner::new()).with_memory_principal("agent-a".into());
        std::fs::write(
            home._home.path().join("settings/app.json"),
            r#"{"schema_version":2,"overrides":{}}"#,
        )
        .expect("disable memory after invocation launch");

        let output = exec
            .memory_commit(MemoryCommitRequest {
                node: "commit".into(),
                agent_id: "agent-a".into(),
                workspace: None,
                conversation_id: None,
                source_sequence: None,
                archive_available: None,
                idempotency_key: None,
                payload: serde_json::json!({
                    "agent_id": "agent-a",
                    "idempotency_key": "run:boundary:launch-authority",
                    "operations": []
                }),
            })
            .await
            .expect("a launch-authorized invocation remains coherent");

        assert_eq!(output.0["idempotency_key"], "run:boundary:launch-authority");
    }

    #[tokio::test]
    async fn memory_commit_rejects_model_selected_cursor_epoch() {
        let home = TestWardianHome::new_async().await;
        let exec = exec_with(FakeAgentRunner::new()).with_memory_principal("agent-a".into());
        let payload = serde_json::json!({
            "agent_id": "agent-a",
            "workspace": "workspace-a",
            "idempotency_key": "boundary-a",
            "operations": [{
                "op": "save",
                "kind": "current",
                "text": "Stale duplicate",
                "evidence_excerpt": "Old evidence",
                "sources": []
            }],
            "cursor": {
                "cursor_key": "model-bypass",
                "conversation_id": "conversation-b",
                "sequence": 1
            }
        });

        let error = exec
            .memory_commit(MemoryCommitRequest {
                node: "commit".into(),
                agent_id: "agent-a".into(),
                workspace: Some("workspace-a".into()),
                conversation_id: Some("conversation-a".into()),
                source_sequence: Some(100),
                archive_available: Some(true),
                idempotency_key: Some("boundary-a".into()),
                payload,
            })
            .await
            .expect_err("model-selected cursor epoch must be rejected");

        assert!(error.to_string().contains("cursor does not match"));
        let store = wardian_core::memory::MemoryStore::from_default_home().unwrap();
        assert!(store
            .list_active(
                &wardian_core::memory::MemoryActor::Operator,
                "agent-a",
                Some("workspace-a"),
            )
            .unwrap()
            .is_empty());
        let connection = rusqlite::Connection::open(home._home.path().join("memory.db")).unwrap();
        let cursors: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM memory_consolidation_cursors",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(cursors, 0);
    }

    #[tokio::test]
    async fn automation_cannot_commit_for_caller_selected_peer_principal() {
        let _home = TestWardianHome::new_async().await;
        let run_root = tempfile::tempdir().unwrap();
        let payload = serde_json::json!({
            "agent_id": "agent-b",
            "idempotency_key": "run:boundary:peer",
            "operations": []
        });
        let blueprint = wardian_core::automation::Blueprint {
            schema: 2,
            id: "memory-authority-test".into(),
            name: "Memory authority test".into(),
            nodes: vec![
                wardian_core::automation::Node {
                    id: "trigger".into(),
                    r#type: "manual_trigger".into(),
                    name: None,
                    parent: None,
                    fields: serde_json::Map::new(),
                    position: None,
                },
                wardian_core::automation::Node {
                    id: "extract".into(),
                    r#type: "task".into(),
                    name: None,
                    parent: None,
                    fields: serde_json::json!({"agent":"role:curator","prompt":"extract"})
                        .as_object()
                        .unwrap()
                        .clone(),
                    position: None,
                },
                wardian_core::automation::Node {
                    id: "commit".into(),
                    r#type: "memory_commit".into(),
                    name: None,
                    parent: None,
                    fields: serde_json::json!({
                        "source_node":"extract",
                        "agent_id":"{{trigger.output.agent_id}}"
                    })
                    .as_object()
                    .unwrap()
                    .clone(),
                    position: None,
                },
            ],
            edges: vec![
                wardian_core::automation::Edge {
                    from: "trigger".into(),
                    from_port: "out".into(),
                    to: "extract".into(),
                    to_port: "in".into(),
                },
                wardian_core::automation::Edge {
                    from: "extract".into(),
                    from_port: "out".into(),
                    to: "commit".into(),
                    to_port: "in".into(),
                },
            ],
            body: String::new(),
        };
        let response = payload.to_string();
        let exec = exec_with(FakeAgentRunner::new().with_response("extract", &response))
            .with_memory_principal("agent-a".into());

        let state = wardian_core::engine::Engine::start_with_id(
            &blueprint,
            "run-peer-spoof",
            serde_json::json!({"agent_id":"agent-b"}),
            run_root.path(),
            &exec,
        )
        .await
        .unwrap();

        assert_eq!(state.status, wardian_core::engine::RunStatus::Failed);
        assert!(state
            .failure
            .as_deref()
            .is_some_and(|failure| failure.contains("invocation authorizes agent-a")));
        assert!(wardian_core::memory::MemoryStore::from_default_home()
            .unwrap()
            .list_events(&wardian_core::memory::MemoryActor::Operator, "agent-b")
            .unwrap()
            .is_empty());
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
                config: None,
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
        let _home = TestWardianHome::new_async().await;

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
                config: None,
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
    }

    #[tokio::test]
    async fn temporary_provider_uses_a_automation_owned_session_for_its_workspace() {
        struct TemporaryProviderSpecRunner;

        impl AgentRunner for TemporaryProviderSpecRunner {
            fn run(
                &self,
                spec: AgentRunSpec,
            ) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + '_>> {
                Box::pin(async move {
                    assert_eq!(spec.provider, "codex");
                    assert_eq!(spec.cwd, PathBuf::from("/automation-project"));
                    assert!(
                        spec.session_id.starts_with("automation-temp-scheduled-42-review"),
                        "temporary provider needs its own automation session so the habitat maps workspace to the project folder"
                    );
                    assert!(spec.agent_session_id.is_none());
                    assert!(spec.resume_session.is_none());
                    let override_config =
                        spec.config_override.expect("temporary provider override");
                    assert_eq!(override_config.model.as_deref(), Some("gpt-5.6-luna"));
                    assert_eq!(
                        override_config.codex_config().reasoning_effort.as_deref(),
                        Some("low")
                    );
                    assert!(spec.lease_owner.is_none());
                    Ok("{\"ok\":true}".to_string())
                })
            }
        }

        let assignments = AutomationAssignments::from([(
            "Reviewer".to_string(),
            AutomationRoleAssignment::TemporaryProvider {
                provider: "codex".to_string(),
                workspace: Some("/automation-project".to_string()),
                model: Some("gpt-5.6-luna".to_string()),
                effort: Some("low".to_string()),
            },
        )]);
        let exec = LiveStepExecutor::new_with_assignments_and_live_runner(
            Arc::new(TemporaryProviderSpecRunner),
            None,
            PathBuf::from("/run-log"),
            "mock".into(),
            HashMap::new(),
            assignments,
            HashMap::new(),
        )
        .with_owner_id("scheduled-42".to_string());

        let output = exec
            .run_agent_task(AgentTaskRequest {
                node: "review".to_string(),
                agent: "role:Reviewer".to_string(),
                prompt: "review the project".to_string(),
                output_schema: None,
            })
            .await
            .expect("temporary provider automation task");

        assert_eq!(output.0["ok"], true);
    }

    #[tokio::test]
    async fn legacy_provider_binding_uses_a_automation_owned_session() {
        struct LegacyProviderSpecRunner;

        impl AgentRunner for LegacyProviderSpecRunner {
            fn run(
                &self,
                spec: AgentRunSpec,
            ) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + '_>> {
                Box::pin(async move {
                    assert_eq!(spec.provider, "codex");
                    assert_eq!(spec.cwd, PathBuf::from("/automation-project"));
                    assert!(spec.session_id.starts_with("automation-temp-manual-9-plan"));
                    Ok("{\"ok\":true}".to_string())
                })
            }
        }

        let bindings = HashMap::from([("Planner".to_string(), "codex".to_string())]);
        let exec = LiveStepExecutor::new(
            Arc::new(LegacyProviderSpecRunner),
            PathBuf::from("/automation-project"),
            "mock".into(),
            bindings,
            HashMap::new(),
        )
        .with_owner_id("manual-9".to_string());

        let output = exec
            .run_agent_task(AgentTaskRequest {
                node: "plan".to_string(),
                agent: "role:Planner".to_string(),
                prompt: "plan the project".to_string(),
                output_schema: None,
            })
            .await
            .expect("legacy provider automation task");

        assert_eq!(output.0["ok"], true);
    }

    #[tokio::test]
    async fn fresh_background_assigned_agent_does_not_reuse_live_session_identity() {
        struct IdentityCheckingRunner;

        impl AgentRunner for IdentityCheckingRunner {
            fn run(
                &self,
                spec: AgentRunSpec,
            ) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + '_>> {
                Box::pin(async move {
                    assert_ne!(
                        spec.session_id, "agent-123",
                        "fresh background runs must not reuse the visible agent identity"
                    );
                    assert!(
                        spec.session_id.contains("automation-bg"),
                        "fresh background runs should get an automation-scoped identity"
                    );
                    assert!(
                        spec.resume_session.is_none(),
                        "fresh background runs must not resume the visible provider conversation"
                    );
                    assert_eq!(
                        spec.agent_session_id.as_deref(),
                        Some("agent-123"),
                        "the real agent identity owns lifecycle exclusion even when the provider session is synthetic"
                    );
                    assert!(
                        spec.lease_owner.is_some(),
                        "fresh background runs must retain a durable lifecycle lease"
                    );
                    let leases = wardian_core::conversation_lease::load_leases();
                    assert!(
                        wardian_core::conversation_lease::find_active_conflict(
                            &leases,
                            "agent-123",
                            "",
                            &chrono::Utc::now().to_rfc3339(),
                        )
                        .is_some(),
                        "fresh background runs must serialize against the registered agent"
                    );
                    let config = spec
                        .config_override
                        .expect("fresh background should keep the assigned agent profile");
                    assert_eq!(config.session_id, "agent-123");
                    assert_eq!(config.provider, "gemini");
                    Ok("{\"ok\":true}".to_string())
                })
            }
        }

        let _home = TestWardianHome::new_async().await;

        let mut assignments = AutomationAssignments::new();
        assignments.insert(
            "Coder".to_string(),
            AutomationRoleAssignment::Agent {
                agent_id: "agent-123".to_string(),
                conversation: AgentConversationMode::FreshBackground,
                busy_policy: BusyPolicy::Fail,
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
                is_live: true,
                is_input_ready: true,
                config: Some(AgentConfig {
                    session_id: "agent-123".to_string(),
                    provider: "gemini".to_string(),
                    folder: "/agent-workspace".to_string(),
                    ..AgentConfig::default()
                }),
            },
        );

        let exec = LiveStepExecutor::new_with_assignments_and_live_runner(
            Arc::new(IdentityCheckingRunner),
            None,
            PathBuf::from("/run-workspace"),
            "codex".into(),
            HashMap::new(),
            assignments,
            agent_catalog,
        )
        .with_owner_id("run-456".to_string());

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

        let _home = TestWardianHome::new_async().await;

        let mut assignments = AutomationAssignments::new();
        assignments.insert(
            "Coder".to_string(),
            AutomationRoleAssignment::Agent {
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
                config: None,
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
    }

    #[tokio::test]
    async fn background_resume_requires_saved_provider_conversation() {
        let _home = TestWardianHome::new_async().await;

        let mut assignments = AutomationAssignments::new();
        assignments.insert(
            "Coder".to_string(),
            AutomationRoleAssignment::Agent {
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
                config: None,
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
    }

    #[tokio::test]
    async fn legacy_agent_binding_uses_assignment_route_not_unleased_headless_fallback() {
        let _home = TestWardianHome::new_async().await;

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
                config: None,
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
    }
}
