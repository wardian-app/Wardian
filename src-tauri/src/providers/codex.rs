use crate::models::provider::{AgentEvent, AgentProvider};
use crate::models::AgentConfig;

/// Provider adapter for the OpenAI Codex CLI.
pub struct CodexProvider;

impl Default for CodexProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl CodexProvider {
    pub fn new() -> Self {
        CodexProvider
    }

    fn parse_action_required_from_arguments(arguments: &str) -> Option<String> {
        let parsed: serde_json::Value = serde_json::from_str(arguments).ok()?;
        let sandbox_permissions = parsed.get("sandbox_permissions")?.as_str()?;
        if sandbox_permissions != "require_escalated" {
            return None;
        }

        let justification = parsed
            .get("justification")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty());
        if let Some(justification) = justification {
            return Some(justification.to_string());
        }

        let command = parsed
            .get("command")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty());
        if let Some(command) = command {
            return Some(command.to_string());
        }

        Some("Approval required".to_string())
    }

    fn append_common_args(&self, args: &mut Vec<String>, config: &AgentConfig, is_exec_mode: bool) {
        if let Some(ref model) = config.model {
            args.push("--model".into());
            args.push(model.clone());
        }

        if let Some(ref profile) = config.codex_profile {
            if !profile.trim().is_empty() {
                args.push("--profile".into());
                args.push(profile.clone());
            }
        }

        let sandbox_mode = config
            .codex_sandbox_mode
            .as_ref()
            .filter(|mode| !mode.trim().is_empty())
            .cloned()
            .unwrap_or_else(|| "workspace-write".to_string());
        args.push("--sandbox".into());
        args.push(sandbox_mode);

        if let Some(ref approval_policy) = config.codex_approval_policy {
            if !approval_policy.trim().is_empty() {
                args.push("--ask-for-approval".into());
                args.push(approval_policy.clone());
            }
        }

        if config.codex_full_auto.unwrap_or(false) {
            args.push("--full-auto".into());
        }

        if config.codex_search.unwrap_or(false) {
            args.push("--search".into());
        }

        if is_exec_mode {
            if config.codex_skip_git_repo_check.unwrap_or(true) {
                args.push("--skip-git-repo-check".into());
            }

            if config.codex_ephemeral.unwrap_or(false) {
                args.push("--ephemeral".into());
            }
        }

        let mut final_includes = config
            .system_include_directories
            .clone()
            .unwrap_or_default();
        if let Some(ref user_dirs) = config.include_directories {
            for dir in user_dirs {
                if !final_includes.contains(dir) {
                    final_includes.push(dir.clone());
                }
            }
        }
        for dir in final_includes {
            args.push("--add-dir".into());
            args.push(dir);
        }
    }
}

impl AgentProvider for CodexProvider {
    fn name(&self) -> &str {
        "Codex"
    }

    fn get_executable(&self) -> (String, Vec<String>) {
        if cfg!(target_os = "windows") {
            ("codex.cmd".to_string(), vec![])
        } else {
            ("codex".to_string(), vec![])
        }
    }

    fn get_spawn_args(&self, config: &AgentConfig, is_resume: bool) -> Vec<String> {
        let mut args: Vec<String> = Vec::new();

        if is_resume {
            args.push("resume".into());
            if let Some(resume_id) = config.resume_session.as_ref().filter(|s| !s.trim().is_empty()) {
                args.push(resume_id.clone());
            }
        }

        self.append_common_args(&mut args, config, false);

        if let Some(ref custom) = config.custom_args {
            if let Some(parsed) = shlex::split(custom) {
                args.extend(parsed);
            }
        }

        args
    }

    fn parse_output(&self, line: &str) -> Option<AgentEvent> {
        let parsed: serde_json::Value = serde_json::from_str(line).ok()?;
        let msg_type = parsed.get("type")?.as_str()?;

        match msg_type {
            "thread.started" => {
                let session_id = parsed
                    .get("thread_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                Some(AgentEvent::Init {
                    session_id,
                    timestamp: None,
                })
            }
            "turn.started" => Some(AgentEvent::UserQuery),
            "turn.completed" => Some(AgentEvent::ModelResponse),
            "item.completed" => {
                let item_type = parsed
                    .get("item")
                    .and_then(|v| v.get("type"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                match item_type {
                    "agent_message" => Some(AgentEvent::Generating),
                    _ => Some(AgentEvent::Unknown),
                }
            }
            "response_item" => {
                let payload = parsed.get("payload")?;
                let payload_type = payload.get("type").and_then(|v| v.as_str()).unwrap_or("");
                match payload_type {
                    "function_call" => {
                        let arguments = payload.get("arguments").and_then(|v| v.as_str()).unwrap_or("");
                        Self::parse_action_required_from_arguments(arguments)
                            .map(|message| AgentEvent::ActionRequired { message })
                            .or(Some(AgentEvent::Unknown))
                    }
                    "function_call_output" => Some(AgentEvent::Generating),
                    "message" => {
                        let role = payload.get("role").and_then(|v| v.as_str()).unwrap_or("");
                        match role {
                            "assistant" => Some(AgentEvent::Generating),
                            "user" => Some(AgentEvent::UserQuery),
                            _ => Some(AgentEvent::Unknown),
                        }
                    }
                    _ => Some(AgentEvent::Unknown),
                }
            }
            "event_msg" => {
                let inner_type = parsed
                    .get("payload")
                    .and_then(|v| v.get("type"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                match inner_type {
                    "task_started" => Some(AgentEvent::Generating),
                    "user_message" => Some(AgentEvent::UserQuery),
                    "agent_message" => Some(AgentEvent::Generating),
                    "task_complete" => Some(AgentEvent::ModelResponse),
                    "exec_approval_request" => {
                        let message = parsed
                            .get("payload")
                            .and_then(|v| v.get("command"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("command")
                            .to_string();
                        Some(AgentEvent::ActionRequired { message })
                    }
                    _ => Some(AgentEvent::Unknown),
                }
            }
            _ => Some(AgentEvent::Unknown),
        }
    }

    fn get_instruction_filename(&self) -> &str {
        "AGENTS.md"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_provider() -> CodexProvider {
        CodexProvider::new()
    }

    #[test]
    fn name_returns_codex() {
        let p = make_provider();
        assert_eq!(p.name(), "Codex");
    }

    #[test]
    fn instruction_filename_is_agents_md() {
        let p = make_provider();
        assert_eq!(p.get_instruction_filename(), "AGENTS.md");
    }

    #[test]
    fn spawn_args_resume_and_model() {
        let p = make_provider();
        let config = AgentConfig {
            resume_session: Some("session-abc".into()),
            model: Some("gpt-5.4".into()),
            codex_profile: Some("wardian".into()),
            codex_sandbox_mode: Some("workspace-write".into()),
            codex_approval_policy: Some("on-request".into()),
            codex_search: Some(true),
            ..Default::default()
        };
        let args = p.get_spawn_args(&config, true);
        assert_eq!(args[0], "resume");
        assert!(args.contains(&"session-abc".to_string()));
        assert!(args.contains(&"--model".to_string()));
        assert!(args.contains(&"gpt-5.4".to_string()));
        assert!(args.contains(&"--profile".to_string()));
        assert!(args.contains(&"wardian".to_string()));
        assert!(args.contains(&"--sandbox".to_string()));
        assert!(args.contains(&"workspace-write".to_string()));
        assert!(args.contains(&"--ask-for-approval".to_string()));
        assert!(args.contains(&"on-request".to_string()));
        assert!(args.contains(&"--search".to_string()));
    }

    #[test]
    fn spawn_args_include_directories() {
        let p = make_provider();
        let config = AgentConfig {
            system_include_directories: Some(vec!["/sys/dir".into()]),
            include_directories: Some(vec!["/user/dir".into()]),
            ..Default::default()
        };
        let args = p.get_spawn_args(&config, false);
        let count = args.iter().filter(|a| *a == "--add-dir").count();
        assert_eq!(count, 2);
    }

    #[test]
    fn parse_output_thread_started_event() {
        let p = make_provider();
        let line = r#"{"type":"thread.started","thread_id":"abc-123"}"#;
        let event = p.parse_output(line).unwrap();
        assert_eq!(
            event,
            AgentEvent::Init {
                session_id: "abc-123".into(),
                timestamp: None,
            }
        );
    }

    #[test]
    fn parse_output_turn_started_event() {
        let p = make_provider();
        let line = r#"{"type":"turn.started"}"#;
        assert_eq!(p.parse_output(line).unwrap(), AgentEvent::UserQuery);
    }

    #[test]
    fn parse_output_turn_completed_event() {
        let p = make_provider();
        let line = r#"{"type":"turn.completed","usage":{"input_tokens":1}}"#;
        assert_eq!(p.parse_output(line).unwrap(), AgentEvent::ModelResponse);
    }

    #[test]
    fn parse_output_agent_message_event() {
        let p = make_provider();
        let line = r#"{"type":"item.completed","item":{"type":"agent_message","text":"hello"}}"#;
        assert_eq!(p.parse_output(line).unwrap(), AgentEvent::Generating);
    }

    #[test]
    fn parse_output_task_started_event() {
        let p = make_provider();
        let line = r#"{"type":"event_msg","payload":{"type":"task_started","turn_id":"abc"}}"#;
        assert_eq!(p.parse_output(line).unwrap(), AgentEvent::Generating);
    }

    #[test]
    fn parse_output_task_complete_event() {
        let p = make_provider();
        let line = r#"{"type":"event_msg","payload":{"type":"task_complete","turn_id":"abc"}}"#;
        assert_eq!(p.parse_output(line).unwrap(), AgentEvent::ModelResponse);
    }

    #[test]
    fn parse_output_response_item_function_call_requires_approval() {
        let p = make_provider();
        let line = r#"{"type":"response_item","payload":{"type":"function_call","arguments":"{\"command\":\"Get-Content foo\",\"sandbox_permissions\":\"require_escalated\",\"justification\":\"Allow reading foo?\"}"}}"#;
        assert_eq!(
            p.parse_output(line).unwrap(),
            AgentEvent::ActionRequired {
                message: "Allow reading foo?".into(),
            }
        );
    }

    #[test]
    fn parse_output_response_item_function_call_without_approval_is_unknown() {
        let p = make_provider();
        let line = r#"{"type":"response_item","payload":{"type":"function_call","arguments":"{\"command\":\"Get-Content foo\"}"}}"#;
        assert_eq!(p.parse_output(line).unwrap(), AgentEvent::Unknown);
    }
}
