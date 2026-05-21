use crate::remote::models::{RemoteAgentActionRequest, RemoteAgentSummary, RemoteWorkflowSummary};
use crate::state::AppState;
use tauri::{AppHandle, Manager};

pub const REMOTE_AGENT_LATEST_TEXT_BYTES: usize = 4096;

pub async fn remote_agent_roster(state: &AppState) -> Vec<RemoteAgentSummary> {
    let agents = state.agents.lock().await;
    agents
        .values()
        .filter_map(|agent| {
            let config = agent.config.lock().ok()?.clone();
            let status = agent.current_status.lock().ok()?.clone();
            let latest_text = agent
                .watch_state
                .lock()
                .ok()
                .and_then(|watch| {
                    watch
                        .snapshot_since(None, Some(REMOTE_AGENT_LATEST_TEXT_BYTES))
                        .ok()
                })
                .and_then(|snapshot| {
                    bounded_clean_latest_text(&snapshot.transcript.latest_text)
                        .or_else(|| bounded_clean_latest_text(&snapshot.output.text))
                });

            Some(RemoteAgentSummary {
                session_id: config.session_id,
                session_name: config.session_name,
                agent_class: config.agent_class,
                provider: config.provider,
                workspace: config.folder,
                status,
                latest_text,
            })
        })
        .collect()
}

pub fn validate_remote_agent_action(request: &RemoteAgentActionRequest) -> Result<(), String> {
    match request.action.as_str() {
        "send_prompt"
            if request
                .prompt
                .as_ref()
                .is_none_or(|prompt| prompt.trim().is_empty()) =>
        {
            Err("prompt_required".to_string())
        }
        "send_prompt" | "pause" | "resume" | "clear" | "kill" => Ok(()),
        _ => Err("unsupported_remote_agent_action".to_string()),
    }
}

pub async fn run_remote_agent_action(
    app: &AppHandle,
    request: RemoteAgentActionRequest,
) -> Result<(), String> {
    validate_remote_agent_action(&request)?;
    match request.action.as_str() {
        "send_prompt" => {
            let state = app.state::<AppState>();
            crate::commands::terminal::submit_prompt_to_agent(
                request.target,
                request.prompt.unwrap_or_default(),
                state,
                app.clone(),
            )
            .await
        }
        "pause" => {
            let state = app.state::<AppState>();
            crate::commands::agent::pause_agent(request.target, state, app.clone()).await
        }
        "resume" => {
            let state = app.state::<AppState>();
            crate::commands::agent::resume_agent(request.target, state, app.clone()).await
        }
        "clear" => {
            let state = app.state::<AppState>();
            crate::commands::agent::clear_agent_session(request.target, state, app.clone()).await
        }
        "kill" => {
            let state = app.state::<AppState>();
            crate::commands::agent::kill_agent(request.target, state, app.clone()).await
        }
        _ => Err("unsupported_remote_agent_action".to_string()),
    }
}

pub fn remote_workflow_summaries() -> Result<Vec<RemoteWorkflowSummary>, String> {
    Ok(crate::workflow_engine::list_workflows()?
        .into_iter()
        .map(|workflow| RemoteWorkflowSummary {
            id: workflow.id,
            name: workflow.name,
            node_count: workflow.nodes.len(),
        })
        .collect())
}

fn bounded_clean_latest_text(value: &str) -> Option<String> {
    let clean = crate::state::terminal_text::strip_terminal_controls(value);
    let trimmed = clean.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(tail_string_by_bytes(
        trimmed,
        REMOTE_AGENT_LATEST_TEXT_BYTES,
    ))
}

fn tail_string_by_bytes(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_string();
    }
    let mut start = value.len().saturating_sub(max_bytes);
    while start < value.len() && !value.is_char_boundary(start) {
        start += 1;
    }
    value[start..].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{ActiveAgent, AgentWatchState, AppState};
    use std::sync::{Arc, Mutex};
    use wardian_core::control::WatchTranscriptMessage;
    use wardian_core::models::{AgentConfig, WorkflowDefinition, WorkflowNode, WorkflowSettings};

    struct WardianHomeGuard(Option<std::ffi::OsString>);

    impl WardianHomeGuard {
        fn set(path: &std::path::Path) -> Self {
            let previous = std::env::var_os("WARDIAN_HOME");
            unsafe { std::env::set_var("WARDIAN_HOME", path) };
            Self(previous)
        }
    }

    impl Drop for WardianHomeGuard {
        fn drop(&mut self) {
            match self.0.take() {
                Some(value) => unsafe { std::env::set_var("WARDIAN_HOME", value) },
                None => unsafe { std::env::remove_var("WARDIAN_HOME") },
            }
        }
    }

    fn test_agent(
        session_id: &str,
        session_name: &str,
        agent_class: &str,
        status: &str,
    ) -> ActiveAgent {
        ActiveAgent {
            config: Arc::new(Mutex::new(AgentConfig {
                session_id: session_id.to_string(),
                session_name: session_name.to_string(),
                agent_class: agent_class.to_string(),
                provider: "mock".to_string(),
                folder: "<absolute-workspace-path>".to_string(),
                ..Default::default()
            })),
            child_process: None,
            background_processes: Vec::new(),
            pty_master: None,
            stdin_tx: None,
            output_buffer: Arc::new(Mutex::new(String::new())),
            process_id: Some(1234),
            query_count: Arc::new(Mutex::new(0)),
            init_timestamp: Arc::new(Mutex::new(None)),
            current_status: Arc::new(Mutex::new(status.to_string())),
            last_status_at: Arc::new(Mutex::new(None)),
            watch_state: Arc::new(Mutex::new(AgentWatchState::new(
                session_id.to_string(),
                16,
                8192,
            ))),
            terminal_title: Arc::new(Mutex::new(String::new())),
            last_output_at: Arc::new(Mutex::new(None)),
            log_path: Arc::new(Mutex::new(None)),
            log_last_modified: Arc::new(Mutex::new(None)),
            #[cfg(windows)]
            job_object: None,
        }
    }

    async fn insert_agent(state: &AppState, agent: ActiveAgent) {
        let session_id = agent.config.lock().expect("config").session_id.clone();
        state.agents.lock().await.insert(session_id, agent);
    }

    #[tokio::test]
    async fn remote_agent_roster_maps_agent_summary_fields() {
        let state = AppState::new();
        let agent = test_agent("agent-1", "CoderOne", "Coder", "Idle");
        {
            let mut watch = agent.watch_state.lock().expect("watch state");
            watch.push_transcript(WatchTranscriptMessage {
                role: "assistant".to_string(),
                text: "ready from transcript".to_string(),
                provider: "mock".to_string(),
                turn_id: Some("turn-1".to_string()),
                source: Some("model".to_string()),
            });
        }
        insert_agent(&state, agent).await;

        let roster = remote_agent_roster(&state).await;

        assert_eq!(roster.len(), 1);
        assert_eq!(roster[0].session_id, "agent-1");
        assert_eq!(roster[0].session_name, "CoderOne");
        assert_eq!(roster[0].agent_class, "Coder");
        assert_eq!(roster[0].provider, "mock");
        assert_eq!(roster[0].workspace, "<absolute-workspace-path>");
        assert_eq!(roster[0].status, "Idle");
        assert_eq!(
            roster[0].latest_text.as_deref(),
            Some("ready from transcript")
        );
    }

    #[tokio::test]
    async fn remote_agent_roster_uses_sanitized_bounded_output_fallback() {
        let state = AppState::new();
        let agent = test_agent("agent-1", "CoderOne", "Coder", "Processing");
        {
            let mut watch = agent.watch_state.lock().expect("watch state");
            watch.push_output(format!("\u{1b}[31m{}\u{1b}[0m", "x".repeat(5000)).as_bytes());
        }
        insert_agent(&state, agent).await;

        let roster = remote_agent_roster(&state).await;
        let latest = roster[0].latest_text.as_ref().expect("latest text");

        assert!(!latest.contains('\u{1b}'));
        assert!(latest.len() <= REMOTE_AGENT_LATEST_TEXT_BYTES);
    }

    #[test]
    fn run_remote_agent_action_rejects_unknown_actions_before_dispatch() {
        let request = crate::remote::models::RemoteAgentActionRequest {
            action: "open_shell".to_string(),
            target: "agent-1".to_string(),
            prompt: None,
        };

        assert_eq!(
            validate_remote_agent_action(&request).unwrap_err(),
            "unsupported_remote_agent_action"
        );
    }

    #[test]
    fn remote_workflow_summaries_maps_existing_workflows() {
        let _guard = crate::utils::wardian_test_env_lock();
        let temp = tempfile::tempdir().expect("temp dir");
        let _home = WardianHomeGuard::set(temp.path());
        std::fs::create_dir_all(temp.path().join("workflows")).expect("workflows dir");
        std::fs::write(
            temp.path().join("workflows").join("wf-1.json"),
            serde_json::to_string(&sample_workflow()).expect("workflow json"),
        )
        .expect("write workflow");

        let summaries = remote_workflow_summaries().expect("workflow summaries");

        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].id, "wf-1");
        assert_eq!(summaries[0].name, "Remote Workflow");
        assert_eq!(summaries[0].node_count, 2);
    }

    fn sample_workflow() -> WorkflowDefinition {
        WorkflowDefinition {
            id: "wf-1".to_string(),
            name: "Remote Workflow".to_string(),
            settings: WorkflowSettings {
                max_iterations: 1,
                on_limit_reached: "stop".to_string(),
            },
            nodes: vec![
                WorkflowNode {
                    id: "node-1".to_string(),
                    r#type: "agent".to_string(),
                    name: Some("First".to_string()),
                    config: serde_json::json!({}),
                    parameter_schema: None,
                    dependencies: None,
                    position: None,
                },
                WorkflowNode {
                    id: "node-2".to_string(),
                    r#type: "agent".to_string(),
                    name: Some("Second".to_string()),
                    config: serde_json::json!({}),
                    parameter_schema: None,
                    dependencies: None,
                    position: None,
                },
            ],
            role_mappings: Default::default(),
        }
    }
}
