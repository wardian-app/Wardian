use wardian_core::models::{AgentConfig, AgentExecutionPolicy, WorkflowAgentMode};

#[derive(Debug, Clone)]
pub struct AgentExecutionContext {
    pub config: AgentConfig,
    pub mode: WorkflowAgentMode,
    pub source_agent_id: Option<String>,
    pub execution_session_id: String,
    pub resume_session: Option<String>,
    pub use_source_runtime: bool,
}

fn workflow_execution_session_id(workflow_run_id: &str, node_id: &str) -> String {
    format!("workflow-{}-{}", workflow_run_id, node_id)
}

pub fn resolve_agent_execution_context(
    node_id: &str,
    node_config: &serde_json::Map<String, serde_json::Value>,
    target_agent: Option<&AgentConfig>,
    workflow_run_id: &str,
) -> Result<AgentExecutionContext, String> {
    let legacy_session_type = node_config.get("session_type").and_then(|v| v.as_str());
    let explicit_mode = node_config.get("mode").and_then(|v| v.as_str());
    let mut mode =
        AgentExecutionPolicy::from_legacy_session_type(legacy_session_type, explicit_mode).mode;
    if explicit_mode.is_none()
        && legacy_session_type.is_none()
        && mode == WorkflowAgentMode::Ephemeral
        && target_agent.is_some()
    {
        mode = WorkflowAgentMode::InheritFresh;
    }
    let execution_session_id = workflow_execution_session_id(workflow_run_id, node_id);

    let mut config = match mode {
        WorkflowAgentMode::Ephemeral => {
            let agent_class = node_config
                .get("agent_class")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let folder = node_config
                .get("folder")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            AgentConfig {
                session_id: execution_session_id.clone(),
                session_name: format!("Workflow {}", node_id),
                agent_class,
                folder,
                resume_session: None,
                is_off: true,
                ..Default::default()
            }
        }
        WorkflowAgentMode::InheritFresh | WorkflowAgentMode::InheritResume => {
            target_agent.cloned().ok_or_else(|| {
                "inherit_fresh and inherit_resume require an agent_id or role mapping".to_string()
            })?
        }
    };

    if let Some(output_format) = node_config.get("output_format").and_then(|v| v.as_str()) {
        config.output_format = Some(output_format.to_string());
    }
    if let Some(folder) = node_config.get("folder").and_then(|v| v.as_str()) {
        if !folder.trim().is_empty() {
            config.folder = folder.to_string();
        }
    }
    if let Some(provider) = node_config.get("provider").and_then(|v| v.as_str()) {
        if !provider.trim().is_empty() {
            config.provider = provider.to_string();
        }
    }

    let source_agent_id = target_agent.map(|agent| agent.session_id.clone());
    let resume_session = match mode {
        WorkflowAgentMode::InheritResume => config
            .resume_session
            .clone()
            .or_else(|| (!config.session_id.trim().is_empty()).then(|| config.session_id.clone())),
        WorkflowAgentMode::Ephemeral | WorkflowAgentMode::InheritFresh => {
            config.session_id = execution_session_id.clone();
            config.resume_session = None;
            None
        }
    };

    Ok(AgentExecutionContext {
        config,
        mode,
        source_agent_id,
        execution_session_id,
        resume_session,
        use_source_runtime: mode == WorkflowAgentMode::InheritResume,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn source_agent() -> AgentConfig {
        AgentConfig {
            session_id: "agent-123".into(),
            session_name: "Source Agent".into(),
            agent_class: "Coder".into(),
            folder: "D:/Development/Wardian".into(),
            provider: "opencode".into(),
            resume_session: Some("ses_source".into()),
            system_include_directories: Some(vec!["source-scope".into()]),
            include_directories: Some(vec!["user-scope".into()]),
            ..Default::default()
        }
    }

    fn object(value: serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
        value.as_object().expect("object").clone()
    }

    #[test]
    fn ephemeral_never_uses_resume_session() {
        let config = object(json!({
            "mode": "ephemeral",
            "agent_class": "Coder",
            "folder": "D:/Development/Wardian"
        }));

        let ctx =
            resolve_agent_execution_context("node-1", &config, None, "run-1").expect("context");

        assert_eq!(ctx.mode, WorkflowAgentMode::Ephemeral);
        assert_eq!(ctx.config.session_id, "workflow-run-1-node-1");
        assert_eq!(ctx.config.resume_session, None);
        assert_eq!(ctx.resume_session, None);
        assert!(!ctx.use_source_runtime);
    }

    #[test]
    fn inherit_fresh_clones_provider_settings_but_clears_resume_session() {
        let config = object(json!({ "mode": "inherit_fresh" }));

        let ctx =
            resolve_agent_execution_context("node-1", &config, Some(&source_agent()), "run-1")
                .expect("context");

        assert_eq!(ctx.mode, WorkflowAgentMode::InheritFresh);
        assert_eq!(ctx.config.provider, "opencode");
        assert_eq!(ctx.config.agent_class, "Coder");
        assert_eq!(ctx.config.session_id, "workflow-run-1-node-1");
        assert_eq!(ctx.config.resume_session, None);
        assert_eq!(ctx.resume_session, None);
        assert_eq!(ctx.source_agent_id.as_deref(), Some("agent-123"));
        assert_eq!(
            ctx.config.system_include_directories.as_deref(),
            Some(&["source-scope".to_string()][..])
        );
        assert!(!ctx.use_source_runtime);
    }

    #[test]
    fn inherit_resume_keeps_valid_resume_session() {
        let config = object(json!({ "mode": "inherit_resume" }));

        let ctx =
            resolve_agent_execution_context("node-1", &config, Some(&source_agent()), "run-1")
                .expect("context");

        assert_eq!(ctx.mode, WorkflowAgentMode::InheritResume);
        assert_eq!(ctx.config.session_id, "agent-123");
        assert_eq!(ctx.resume_session.as_deref(), Some("ses_source"));
        assert!(ctx.use_source_runtime);
    }

    #[test]
    fn missing_mode_for_workflow_resolves_to_ephemeral() {
        let config = object(json!({ "agent_class": "Coder" }));

        let ctx =
            resolve_agent_execution_context("node-1", &config, None, "run-1").expect("context");

        assert_eq!(ctx.mode, WorkflowAgentMode::Ephemeral);
    }

    #[test]
    fn legacy_direct_agent_without_mode_resolves_to_inherit_fresh() {
        let config = object(json!({ "agent_id": "agent-123" }));

        let ctx =
            resolve_agent_execution_context("node-1", &config, Some(&source_agent()), "run-1")
                .expect("context");

        assert_eq!(ctx.mode, WorkflowAgentMode::InheritFresh);
        assert_eq!(ctx.config.session_id, "workflow-run-1-node-1");
        assert_eq!(ctx.resume_session, None);
    }
}
