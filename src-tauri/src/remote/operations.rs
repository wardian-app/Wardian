use crate::remote::models::{
    RemoteAgentActionRequest, RemoteAgentSummary, RemoteTerminalSnapshot, RemoteWatchlistResponse,
};
use crate::state::AppState;
use std::collections::HashMap;
use tauri::{AppHandle, Manager};
use wardian_core::control::MessageInputMode;
use wardian_core::models::chat::AgentChatEvent;

pub async fn remote_agent_roster(state: &AppState) -> Vec<RemoteAgentSummary> {
    let agents = state.agents.lock().await;
    let order = state.agent_order.lock().await;
    let mut summaries_by_id = agents
        .values()
        .filter_map(|agent| {
            let config = agent.config.lock().ok()?.clone();
            let status = agent.current_status.lock().ok()?.clone();

            Some((
                config.session_id.clone(),
                RemoteAgentSummary {
                    session_id: config.session_id,
                    session_name: config.session_name,
                    agent_class: config.agent_class,
                    provider: config.provider,
                    workspace: config.folder,
                    status,
                    latest_text: None,
                },
            ))
        })
        .collect::<HashMap<_, _>>();

    let mut ordered = Vec::with_capacity(summaries_by_id.len());
    for session_id in order.iter() {
        if let Some(summary) = summaries_by_id.remove(session_id) {
            ordered.push(summary);
        }
    }

    let mut remaining = summaries_by_id.into_values().collect::<Vec<_>>();
    remaining.sort_by(|left, right| {
        left.session_name
            .cmp(&right.session_name)
            .then_with(|| left.session_id.cmp(&right.session_id))
    });
    ordered.extend(remaining);
    ordered
}

pub fn remote_watchlist_state() -> Result<RemoteWatchlistResponse, String> {
    let Some(home) = crate::utils::fs::get_wardian_home() else {
        return Ok(RemoteWatchlistResponse {
            watchlists: serde_json::json!([]),
            teams: serde_json::json!([]),
            prefs: None,
        });
    };

    let persisted_state = std::fs::read_to_string(home.join("watchlists").join("index.json"))
        .ok()
        .and_then(|data| serde_json::from_str::<serde_json::Value>(&data).ok())
        .unwrap_or_else(|| serde_json::json!([]));
    let (watchlists, teams) = if let Some(state) = persisted_state.as_object() {
        (
            state
                .get("watchlists")
                .filter(|value| value.is_array())
                .cloned()
                .unwrap_or_else(|| serde_json::json!([])),
            state
                .get("teams")
                .filter(|value| value.is_array())
                .cloned()
                .unwrap_or_else(|| serde_json::json!([])),
        )
    } else if persisted_state.is_array() {
        (persisted_state, serde_json::json!([]))
    } else {
        (serde_json::json!([]), serde_json::json!([]))
    };
    let prefs = std::fs::read_to_string(home.join("watchlists").join("prefs.json"))
        .ok()
        .and_then(|data| serde_json::from_str::<serde_json::Value>(&data).ok());

    Ok(RemoteWatchlistResponse {
        watchlists,
        teams,
        prefs,
    })
}

pub async fn remote_agent_chat_transcript(
    state: &AppState,
    session_id: &str,
) -> Result<Vec<AgentChatEvent>, String> {
    crate::commands::chat::load_agent_chat_transcript_for_state(state, session_id.to_string()).await
}

pub async fn remote_agent_terminal_snapshot(
    state: &AppState,
    session_id: &str,
    since: Option<&str>,
    tail_bytes: Option<usize>,
) -> Result<RemoteTerminalSnapshot, String> {
    let watch_state = {
        let agents = state.agents.lock().await;
        agents
            .get(session_id)
            .map(|agent| agent.watch_state.clone())
            .ok_or_else(|| "agent_not_found".to_string())?
    };
    let snapshot = watch_state
        .lock()
        .map_err(|_| "watch_state_unavailable".to_string())?
        .snapshot_since(since, tail_bytes)
        .map_err(|error| error.code().to_string())?;

    Ok(RemoteTerminalSnapshot {
        cursor: snapshot.output.cursor,
        text: snapshot.output.text,
        truncated: snapshot.output.truncated,
        omitted_bytes: snapshot.output.omitted_bytes,
    })
}

pub async fn remote_agent_terminal_raw_output(
    state: &AppState,
    session_id: &str,
    tail_bytes: Option<usize>,
) -> Result<String, String> {
    let watch_state = {
        let agents = state.agents.lock().await;
        agents
            .get(session_id)
            .map(|agent| agent.watch_state.clone())
            .ok_or_else(|| "agent_not_found".to_string())?
    };
    let snapshot = watch_state
        .lock()
        .map_err(|_| "watch_state_unavailable".to_string())?
        .raw_snapshot_since(None, tail_bytes)
        .map_err(|error| error.code().to_string())?;

    Ok(snapshot.text)
}

pub fn validate_remote_agent_action(request: &RemoteAgentActionRequest) -> Result<(), String> {
    if request.action == "send_prompt" {
        if request
            .prompt
            .as_ref()
            .is_none_or(|prompt| prompt.trim().is_empty())
        {
            Err("prompt_required".to_string())
        } else if matches!(
            request.input_mode.unwrap_or_default(),
            MessageInputMode::Message | MessageInputMode::Command
        ) {
            Ok(())
        } else {
            Err("unsupported_remote_input_mode".to_string())
        }
    } else {
        match request.action.as_str() {
            "pause" | "resume" | "clear" | "kill" => Ok(()),
            _ => Err("unsupported_remote_agent_action".to_string()),
        }
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
            crate::delivery::submit_live_surface_prompt(
                Some(app),
                &state,
                crate::delivery::LiveSurfacePromptRequest {
                    session_id: request.target,
                    prompt: request.prompt.unwrap_or_default(),
                    interaction_id: None,
                    input_mode: request.input_mode.unwrap_or_default(),
                    queue_policy: wardian_core::control::QueuePolicy::LiveOnly,
                    approval_action: None,
                    origin: None,
                    runtime_state: "live_pty_available",
                    mark_prompt_started: true,
                    payload_sent_detail: None,
                    delivery_message_id: None,
                },
            )
            .await
            .map_err(|error| error.to_string())
            .map(|_| ())
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
            crate::commands::agent::clear_agent_session(request.target, None, state, app.clone())
                .await
        }
        "kill" => {
            let state = app.state::<AppState>();
            crate::commands::agent::kill_agent(request.target, state, app.clone()).await
        }
        _ => Err("unsupported_remote_agent_action".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{ActiveAgent, AgentWatchState, AppState};
    use std::sync::{Arc, Mutex};
    use wardian_core::control::WatchTranscriptMessage;
    use wardian_core::models::AgentConfig;

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
            runtime_generation: None,
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
        assert_eq!(roster[0].latest_text, None);
    }

    #[tokio::test]
    async fn remote_agent_roster_preserves_desktop_agent_order() {
        let state = AppState::new();
        insert_agent(&state, test_agent("agent-1", "Alpha", "Coder", "Idle")).await;
        insert_agent(
            &state,
            test_agent("agent-2", "Beta", "Reviewer", "Processing"),
        )
        .await;
        state
            .agent_order
            .lock()
            .await
            .extend(["agent-2".to_string(), "agent-1".to_string()]);

        let roster = remote_agent_roster(&state).await;

        assert_eq!(
            roster
                .iter()
                .map(|agent| agent.session_id.as_str())
                .collect::<Vec<_>>(),
            vec!["agent-2", "agent-1"]
        );
    }

    #[tokio::test]
    async fn remote_agent_roster_omits_latest_text_by_default() {
        let state = AppState::new();
        let agent = test_agent("agent-1", "CoderOne", "Coder", "Processing");
        {
            let mut watch = agent.watch_state.lock().expect("watch state");
            watch.push_output(format!("\u{1b}[31m{}\u{1b}[0m", "x".repeat(5000)).as_bytes());
        }
        insert_agent(&state, agent).await;

        let roster = remote_agent_roster(&state).await;

        assert_eq!(roster[0].latest_text, None);
    }

    #[tokio::test]
    async fn remote_watchlist_state_reads_persisted_state_and_prefs() {
        let _guard = crate::utils::wardian_test_env_lock();
        let temp = tempfile::tempdir().expect("temp home");
        let watchlists_dir = temp.path().join("watchlists");
        std::fs::create_dir_all(&watchlists_dir).expect("watchlists dir");
        std::fs::write(
            watchlists_dir.join("index.json"),
            serde_json::json!({
                "version": 2,
                "teams": [{ "id": "team-1", "name": "Core", "agentIds": ["agent-2", "agent-1"] }],
                "watchlists": [{ "id": "main", "name": "Main", "entries": [{ "type": "team", "teamId": "team-1" }] }]
            })
            .to_string(),
        )
        .expect("watchlist json");
        std::fs::write(
            watchlists_dir.join("prefs.json"),
            serde_json::json!({
                "columns": [],
                "sort": null,
                "preserve_team_grouping_when_sorted": false,
                "collapsed_team_ids": ["team-1"]
            })
            .to_string(),
        )
        .expect("prefs json");

        unsafe { std::env::set_var("WARDIAN_HOME", temp.path()) };
        let response = remote_watchlist_state().expect("watchlist response");
        unsafe { std::env::remove_var("WARDIAN_HOME") };

        assert_eq!(response.watchlists[0]["id"], "main");
        assert_eq!(response.teams[0]["agentIds"][0], "agent-2");
        assert_eq!(
            response.prefs.as_ref().expect("prefs")["collapsed_team_ids"][0],
            "team-1"
        );
    }

    #[tokio::test]
    async fn remote_watchlist_state_uses_empty_defaults_for_missing_or_bad_files() {
        let _guard = crate::utils::wardian_test_env_lock();
        let temp = tempfile::tempdir().expect("temp home");
        std::fs::create_dir_all(temp.path().join("watchlists")).expect("watchlists dir");
        std::fs::write(temp.path().join("watchlists/index.json"), "{").expect("bad index");
        std::fs::write(temp.path().join("watchlists/prefs.json"), "{").expect("bad prefs");

        unsafe { std::env::set_var("WARDIAN_HOME", temp.path()) };
        let response = remote_watchlist_state().expect("watchlist response");
        unsafe { std::env::remove_var("WARDIAN_HOME") };

        assert_eq!(response.watchlists, serde_json::json!([]));
        assert_eq!(response.teams, serde_json::json!([]));
        assert!(response.prefs.is_none());
    }

    #[tokio::test]
    async fn remote_agent_chat_transcript_returns_normalized_messages() {
        let state = AppState::new();
        let agent = test_agent("agent-1", "CoderOne", "Coder", "Idle");
        {
            let mut watch = agent.watch_state.lock().expect("watch state");
            watch.push_transcript(WatchTranscriptMessage {
                role: "assistant".to_string(),
                text: "Use the shared chat transcript model.".to_string(),
                provider: "mock".to_string(),
                turn_id: Some("turn-1".to_string()),
                source: Some("model".to_string()),
            });
        }
        insert_agent(&state, agent).await;

        let transcript = remote_agent_chat_transcript(&state, "agent-1")
            .await
            .expect("remote chat transcript");

        assert!(transcript.iter().any(|event| {
            event.kind == wardian_core::models::chat::AgentChatEventKind::Message
                && event.role == Some(wardian_core::models::chat::AgentChatRole::Assistant)
                && event.text.as_deref() == Some("Use the shared chat transcript model.")
        }));
    }

    #[tokio::test]
    async fn remote_agent_terminal_snapshot_returns_sanitized_output_without_draining() {
        let state = AppState::new();
        let agent = test_agent("agent-1", "CoderOne", "Coder", "Processing");
        {
            let mut watch = agent.watch_state.lock().expect("watch state");
            watch.push_output(b"\x1b[31mred terminal\x1b[0m\nsecond line");
        }
        insert_agent(&state, agent).await;

        let first = remote_agent_terminal_snapshot(&state, "agent-1", None, Some(4096))
            .await
            .expect("first terminal snapshot");
        let second = remote_agent_terminal_snapshot(&state, "agent-1", None, Some(4096))
            .await
            .expect("second terminal snapshot");

        assert_eq!(first.text, "red terminal\nsecond line");
        assert_eq!(second.text, first.text);
        assert_eq!(second.cursor, first.cursor);
        assert!(!first.truncated);
        assert_eq!(first.omitted_bytes, 0);
    }

    #[tokio::test]
    async fn remote_agent_terminal_snapshot_respects_tail_bytes() {
        let state = AppState::new();
        let agent = test_agent("agent-1", "CoderOne", "Coder", "Processing");
        {
            let mut watch = agent.watch_state.lock().expect("watch state");
            watch.push_output(b"alpha beta gamma");
        }
        insert_agent(&state, agent).await;

        let snapshot = remote_agent_terminal_snapshot(&state, "agent-1", None, Some(5))
            .await
            .expect("bounded terminal snapshot");

        assert_eq!(snapshot.text, "gamma");
        assert!(snapshot.truncated);
        assert!(snapshot.omitted_bytes > 0);
    }

    #[tokio::test]
    async fn remote_agent_terminal_raw_output_preserves_escape_sequences_without_draining() {
        let state = AppState::new();
        let agent = test_agent("agent-1", "CoderOne", "Coder", "Processing");
        {
            let mut watch = agent.watch_state.lock().expect("watch state");
            watch.push_output(b"\x1b[31mred terminal\x1b[0m\nsecond line");
        }
        insert_agent(&state, agent).await;

        let first = remote_agent_terminal_raw_output(&state, "agent-1", Some(4096))
            .await
            .expect("first raw terminal output");
        let second = remote_agent_terminal_raw_output(&state, "agent-1", Some(4096))
            .await
            .expect("second raw terminal output");

        assert_eq!(first, "\x1b[31mred terminal\x1b[0m\nsecond line");
        assert_eq!(second, first);
    }

    #[tokio::test]
    async fn remote_agent_terminal_snapshot_rejects_unknown_agent() {
        let state = AppState::new();

        assert_eq!(
            remote_agent_terminal_snapshot(&state, "missing-agent", None, Some(4096))
                .await
                .unwrap_err(),
            "agent_not_found"
        );
    }

    #[test]
    fn run_remote_agent_action_rejects_unknown_actions_before_dispatch() {
        let request = crate::remote::models::RemoteAgentActionRequest {
            action: "open_shell".to_string(),
            target: "agent-1".to_string(),
            prompt: None,
            input_mode: None,
        };

        assert_eq!(
            validate_remote_agent_action(&request).unwrap_err(),
            "unsupported_remote_agent_action"
        );
    }

    #[test]
    fn remote_send_prompt_accepts_command_mode() {
        let request = crate::remote::models::RemoteAgentActionRequest {
            action: "send_prompt".to_string(),
            target: "agent-1".to_string(),
            prompt: Some("/status".to_string()),
            input_mode: Some(wardian_core::control::MessageInputMode::Command),
        };

        validate_remote_agent_action(&request).expect("command mode should be accepted");
    }

    #[test]
    fn remote_send_prompt_rejects_approval_action_mode() {
        let request = crate::remote::models::RemoteAgentActionRequest {
            action: "send_prompt".to_string(),
            target: "agent-1".to_string(),
            prompt: Some("1".to_string()),
            input_mode: Some(wardian_core::control::MessageInputMode::ApprovalAction),
        };

        assert_eq!(
            validate_remote_agent_action(&request).unwrap_err(),
            "unsupported_remote_input_mode"
        );
    }

    #[test]
    fn remote_agent_action_rejects_clone() {
        let request = crate::remote::models::RemoteAgentActionRequest {
            action: "clone".to_string(),
            target: "agent-1".to_string(),
            prompt: None,
            input_mode: None,
        };

        assert_eq!(
            validate_remote_agent_action(&request).unwrap_err(),
            "unsupported_remote_agent_action"
        );
    }
}
