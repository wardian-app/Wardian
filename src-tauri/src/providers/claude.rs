use crate::models::provider::{AgentEvent, AgentProvider};
use crate::models::AgentConfig;

/// The concrete `AgentProvider` implementation for Claude Code CLI.
pub struct ClaudeProvider;

impl Default for ClaudeProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl ClaudeProvider {
    pub fn new() -> Self {
        ClaudeProvider
    }

    fn is_real_user_query(parsed: &serde_json::Value) -> bool {
        let Some(message) = parsed.get("message") else {
            return true;
        };
        let Some(content) = message.get("content") else {
            return true;
        };

        let Some(items) = content.as_array() else {
            return true;
        };

        !items
            .iter()
            .any(|item| item.get("type").and_then(|v| v.as_str()) == Some("tool_result"))
    }

    fn assistant_event(parsed: &serde_json::Value) -> AgentEvent {
        let stop_reason = parsed
            .get("message")
            .and_then(|v| v.get("stop_reason"))
            .and_then(|v| v.as_str())
            .unwrap_or("");

        if stop_reason == "end_turn" || stop_reason == "stop_sequence" {
            return AgentEvent::ModelResponse;
        }

        AgentEvent::Generating
    }
}

impl AgentProvider for ClaudeProvider {
    fn name(&self) -> &str {
        "Claude"
    }

    fn get_executable(&self) -> (String, Vec<String>) {
        #[cfg(target_os = "windows")]
        {
            if let Some(paths) = std::env::var_os("PATH") {
                let path_exts = std::env::var("PATHEXT")
                    .ok()
                    .map(|value| {
                        value
                            .split(';')
                            .filter_map(|segment| {
                                let trimmed = segment.trim();
                                if trimmed.is_empty() {
                                    None
                                } else {
                                    Some(trimmed.to_ascii_lowercase())
                                }
                            })
                            .collect::<Vec<_>>()
                    })
                    .filter(|exts| !exts.is_empty())
                    .unwrap_or_else(|| {
                        vec![".exe".to_string(), ".cmd".to_string(), ".bat".to_string()]
                    });

                for path in std::env::split_paths(&paths) {
                    let direct = path.join("claude");
                    if direct.exists() {
                        return (direct.to_string_lossy().to_string(), vec![]);
                    }
                    for ext in &path_exts {
                        let candidate = path.join(format!("claude{ext}"));
                        if candidate.exists() {
                            return (candidate.to_string_lossy().to_string(), vec![]);
                        }
                    }
                }
            }

            if let Some(appdata) = dirs::data_dir() {
                let npm_claude = appdata.join("npm").join("claude.cmd");
                if npm_claude.exists() {
                    return (npm_claude.to_string_lossy().to_string(), vec![]);
                }
            }

            ("claude".to_string(), vec![])
        }

        #[cfg(not(target_os = "windows"))]
        {
            if let Some(paths) = std::env::var_os("PATH") {
                for path in std::env::split_paths(&paths) {
                    let full_path = path.join("claude");
                    if full_path.exists() {
                        ("claude".to_string(), vec![])
                    }
                }
            }

            let home = dirs::home_dir().unwrap_or_default();
            let fallbacks = vec![
                home.join(".npm-global/bin/claude"),
                std::path::PathBuf::from("/usr/local/bin/claude"),
                std::path::PathBuf::from("/opt/homebrew/bin/claude"),
            ];
            for path in fallbacks {
                if path.exists() {
                    return (path.to_string_lossy().to_string(), vec![]);
                }
            }

            ("claude".to_string(), vec![])
        }
    }

    fn get_spawn_args(&self, config: &AgentConfig, is_resume: bool) -> Vec<String> {
        let mut args: Vec<String> = vec![
            "--verbose".into(),
            "--input-format".into(),
            "stream-json".into(),
            "--output-format".into(),
            "stream-json".into(),
        ];

        if let Some(ref model) = config.model {
            args.push("--model".into());
            args.push(model.clone());
        }

        // Only set fresh-session identity on non-resume launches.
        if !is_resume && !config.session_id.trim().is_empty() {
            args.push("--session-id".into());
            args.push(config.session_id.clone());
        }
        if !is_resume && !config.session_name.trim().is_empty() {
            args.push("--name".into());
            args.push(config.session_name.clone());
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
        if !final_includes.is_empty() {
            for dir in final_includes {
                args.push("--add-dir".into());
                args.push(dir);
            }
        }

        if config.debug.unwrap_or(false) {
            args.push("--debug".into());
            args.push("api,hooks".into());
        }

        // Claude-specific parameters
        if let Some(ref mode) = config.permission_mode {
            if !mode.trim().is_empty() {
                args.push("--permission-mode".into());
                args.push(mode.clone());
            }
        }
        if let Some(turns) = config.max_turns {
            if turns > 0 {
                args.push("--max-turns".into());
                args.push(turns.to_string());
            }
        }
        if let Some(ref tools) = config.allowed_tools {
            for tool in tools {
                args.push("--allowedTools".into());
                args.push(tool.clone());
            }
        }
        if let Some(ref tools) = config.disallowed_tools {
            for tool in tools {
                args.push("--disallowedTools".into());
                args.push(tool.clone());
            }
        }
        if let Some(ref prompt) = config.append_system_prompt {
            if !prompt.trim().is_empty() {
                args.push("--append-system-prompt".into());
                args.push(prompt.clone());
            }
        }
        if let Some(ref path) = config.mcp_config {
            if !path.trim().is_empty() {
                args.push("--mcp-config".into());
                args.push(path.clone());
            }
        }

        // Custom args (shell-parsed) - users can supply additional flags here
        if let Some(ref custom) = config.custom_args {
            if let Some(parsed) = shlex::split(custom) {
                args.extend(parsed);
            }
        }

        // Resume flag
        let resume_id = config.resume_session.as_deref().unwrap_or("");
        if is_resume && !resume_id.is_empty() {
            args.push("--resume".into());
            args.push(resume_id.to_string());
        }

        args
    }

    fn parse_output(&self, line: &str) -> Option<AgentEvent> {
        let parsed: serde_json::Value = serde_json::from_str(line).ok()?;
        let msg_type = parsed.get("type")?.as_str()?;

        // Example Claude stream-json mapping:
        // Claude's exact JSON format is undocumented, so we will pass
        // most events directly mapping known keys, or fallback to returning Unknown
        // so that the frontend terminal logic can just render the raw JSON payload.
        match msg_type {
            "system" => {
                let subtype = parsed.get("subtype").and_then(|v| v.as_str()).unwrap_or("");
                match subtype {
                    "init" => {
                        let session_id = parsed
                            .get("session_id")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let timestamp = parsed
                            .get("timestamp")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string());
                        Some(AgentEvent::Init {
                            session_id,
                            timestamp,
                        })
                    }
                    // Claude Code emits this when a tool call needs explicit permission
                    "permission_request" => {
                        let message = parsed
                            .get("tool_name")
                            .and_then(|v| v.as_str())
                            .unwrap_or("tool")
                            .to_string();
                        Some(AgentEvent::ActionRequired { message })
                    }
                    "turn_duration" => Some(AgentEvent::ModelResponse),
                    _ => Some(AgentEvent::Unknown),
                }
            }
            // Only count real user prompts as queries. Tool results are part of the same turn.
            "user" => {
                if Self::is_real_user_query(&parsed) {
                    Some(AgentEvent::UserQuery)
                } else {
                    Some(AgentEvent::Generating)
                }
            }
            // Claude is actively streaming a response
            "assistant" => Some(Self::assistant_event(&parsed)),
            "message_stream" => Some(AgentEvent::Generating),
            "progress" => Some(AgentEvent::Generating),
            // Claude finished the full response turn
            "result" => Some(AgentEvent::TurnCompleted),
            _ => Some(AgentEvent::Unknown),
        }
    }

    fn get_instruction_filename(&self) -> &str {
        "CLAUDE.md"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_provider() -> ClaudeProvider {
        ClaudeProvider::new()
    }

    #[test]
    fn name_returns_claude() {
        let p = make_provider();
        assert_eq!(p.name(), "Claude");
    }

    #[test]
    fn instruction_filename_is_claude_md() {
        let p = make_provider();
        assert_eq!(p.get_instruction_filename(), "CLAUDE.md");
    }

    #[test]
    fn spawn_args_minimal_config() {
        let p = make_provider();
        let config = AgentConfig::default();
        let args = p.get_spawn_args(&config, false);
        // Base persistent session arguments (no --print)
        assert_eq!(
            args[0..5],
            vec![
                "--verbose",
                "--input-format",
                "stream-json",
                "--output-format",
                "stream-json"
            ]
        );
    }

    #[test]
    fn parse_output_init_event() {
        let p = make_provider();
        let line = r#"{"type":"system","subtype":"init","session_id":"abc-123","timestamp":"2026-01-01T00:00:00Z"}"#;
        let event = p.parse_output(line).unwrap();
        assert_eq!(
            event,
            AgentEvent::Init {
                session_id: "abc-123".into(),
                timestamp: Some("2026-01-01T00:00:00Z".into()),
            }
        );
    }

    #[test]
    fn parse_output_permission_request() {
        let p = make_provider();
        let line = r#"{"type":"system","subtype":"permission_request","tool_name":"bash"}"#;
        let event = p.parse_output(line).unwrap();
        assert_eq!(
            event,
            AgentEvent::ActionRequired {
                message: "bash".into()
            }
        );
    }

    #[test]
    fn parse_output_assistant_is_generating() {
        let p = make_provider();
        let line = r#"{"type":"assistant","message":{"role":"assistant","content":[]}}"#;
        assert_eq!(p.parse_output(line).unwrap(), AgentEvent::Generating);
    }

    #[test]
    fn parse_output_assistant_end_turn_is_idle() {
        let p = make_provider();
        let line = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"done"}],"stop_reason":"end_turn"}}"#;
        assert_eq!(p.parse_output(line).unwrap(), AgentEvent::ModelResponse);
    }

    #[test]
    fn parse_output_assistant_tool_use_is_generating() {
        let p = make_provider();
        let line = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","name":"Bash","id":"tool-1","input":{"command":"git status"}}],"stop_reason":"tool_use"}}"#;
        assert_eq!(p.parse_output(line).unwrap(), AgentEvent::Generating);
    }

    #[test]
    fn parse_output_result_is_turn_completed() {
        let p = make_provider();
        let line = r#"{"type":"result","subtype":"success","result":"done"}"#;
        assert_eq!(p.parse_output(line).unwrap(), AgentEvent::TurnCompleted);
    }

    #[test]
    fn parse_output_user_is_query() {
        let p = make_provider();
        let line = r#"{"type":"user","message":{"role":"user","content":"hello"}}"#;
        assert_eq!(p.parse_output(line).unwrap(), AgentEvent::UserQuery);
    }

    #[test]
    fn parse_output_user_tool_result_is_generating() {
        let p = make_provider();
        let line = r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"tool-1","content":"ok"}]}}"#;
        assert_eq!(p.parse_output(line).unwrap(), AgentEvent::Generating);
    }

    #[test]
    fn parse_output_turn_duration_is_idle() {
        let p = make_provider();
        let line = r#"{"type":"system","subtype":"turn_duration","durationMs":1234}"#;
        assert_eq!(p.parse_output(line).unwrap(), AgentEvent::ModelResponse);
    }

    #[test]
    fn parse_output_unknown_type() {
        let p = make_provider();
        let line = r#"{"type":"something_new","data":42}"#;
        assert_eq!(p.parse_output(line).unwrap(), AgentEvent::Unknown);
    }

    #[test]
    fn parse_output_invalid_json() {
        let p = make_provider();
        assert!(p.parse_output("not json").is_none());
    }

    #[test]
    fn spawn_args_permission_mode() {
        let p = make_provider();
        let config = AgentConfig {
            permission_mode: Some("auto-accept".into()),
            ..Default::default()
        };
        let args = p.get_spawn_args(&config, false);
        assert!(args.contains(&"--permission-mode".to_string()));
        assert!(args.contains(&"auto-accept".to_string()));
    }

    #[test]
    fn spawn_args_max_turns() {
        let p = make_provider();
        let config = AgentConfig {
            max_turns: Some(10),
            ..Default::default()
        };
        let args = p.get_spawn_args(&config, false);
        assert!(args.contains(&"--max-turns".to_string()));
        assert!(args.contains(&"10".to_string()));
    }

    #[test]
    fn spawn_args_allowed_tools() {
        let p = make_provider();
        let config = AgentConfig {
            allowed_tools: Some(vec!["Read".into(), "Write".into()]),
            ..Default::default()
        };
        let args = p.get_spawn_args(&config, false);
        let count = args.iter().filter(|a| *a == "--allowedTools").count();
        assert_eq!(count, 2);
    }

    #[test]
    fn spawn_args_disallowed_tools() {
        let p = make_provider();
        let config = AgentConfig {
            disallowed_tools: Some(vec!["Bash".into()]),
            ..Default::default()
        };
        let args = p.get_spawn_args(&config, false);
        assert!(args.contains(&"--disallowedTools".to_string()));
        assert!(args.contains(&"Bash".to_string()));
    }

    #[test]
    fn spawn_args_append_system_prompt() {
        let p = make_provider();
        let config = AgentConfig {
            append_system_prompt: Some("Always respond in JSON".into()),
            ..Default::default()
        };
        let args = p.get_spawn_args(&config, false);
        assert!(args.contains(&"--append-system-prompt".to_string()));
        assert!(args.contains(&"Always respond in JSON".to_string()));
    }

    #[test]
    fn spawn_args_mcp_config() {
        let p = make_provider();
        let config = AgentConfig {
            mcp_config: Some("/path/to/mcp.json".into()),
            ..Default::default()
        };
        let args = p.get_spawn_args(&config, false);
        assert!(args.contains(&"--mcp-config".to_string()));
        assert!(args.contains(&"/path/to/mcp.json".to_string()));
    }

    #[test]
    fn spawn_args_name_skipped_on_resume() {
        let p = make_provider();
        let config = AgentConfig {
            session_name: "MyAgent".into(),
            resume_session: Some("session-abc".into()),
            ..Default::default()
        };
        // Fresh spawn includes --name
        let args_fresh = p.get_spawn_args(&config, false);
        assert!(args_fresh.contains(&"--name".to_string()));
        // Resume omits --name
        let args_resume = p.get_spawn_args(&config, true);
        assert!(!args_resume.contains(&"--name".to_string()));
        assert!(args_resume.contains(&"--resume".to_string()));
    }

    #[test]
    fn fresh_spawn_uses_explicit_session_id() {
        let p = make_provider();
        let config = AgentConfig {
            session_id: "019d331a-0500-7592-969f-8f437886f42b".into(),
            ..Default::default()
        };
        let args = p.get_spawn_args(&config, false);
        assert!(args.contains(&"--session-id".to_string()));
        assert!(args.contains(&"019d331a-0500-7592-969f-8f437886f42b".to_string()));
    }
}
