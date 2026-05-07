use wardian_core::models::provider::{AgentEvent, AgentProvider};
use wardian_core::models::AgentConfig;

/// The first concrete `AgentProvider` implementation, wrapping the Gemini CLI.
pub struct GeminiProvider;

impl Default for GeminiProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl GeminiProvider {
    pub fn new() -> Self {
        GeminiProvider
    }
}

impl AgentProvider for GeminiProvider {
    fn name(&self) -> &str {
        "Gemini"
    }

    fn get_executable(&self) -> (String, Vec<String>) {
        let exe_name = if cfg!(target_os = "windows") {
            "gemini.cmd"
        } else {
            "gemini"
        };

        // 1. Try bare command in PATH
        if let Some(paths) = std::env::var_os("PATH") {
            for path in std::env::split_paths(&paths) {
                let full_path = path.join(exe_name);
                if full_path.exists() {
                    return (exe_name.to_string(), vec![]);
                }
            }
        }

        // 2. Robust Fallback
        if cfg!(target_os = "windows") {
            if let Some(appdata) = dirs::data_dir() {
                let npm_gemini = appdata.join("npm").join("gemini.cmd");
                if npm_gemini.exists() {
                    return (npm_gemini.to_string_lossy().to_string(), vec![]);
                }

                // Legacy index.js lookup
                let index_js = appdata
                    .join("npm")
                    .join("node_modules")
                    .join("@google")
                    .join("gemini-cli")
                    .join("dist")
                    .join("index.js");
                if index_js.exists() {
                    return (
                        "node".to_string(),
                        vec![index_js.to_string_lossy().to_string()],
                    );
                }
            }
        } else {
            let home = dirs::home_dir().unwrap_or_default();
            let fallbacks = vec![
                home.join(".npm-global/bin/gemini"),
                std::path::PathBuf::from("/usr/local/bin/gemini"),
                std::path::PathBuf::from("/opt/homebrew/bin/gemini"),
            ];
            for path in fallbacks {
                if path.exists() {
                    return (path.to_string_lossy().to_string(), vec![]);
                }
            }
        }

        // 3. Ultimate Fallback
        (exe_name.to_string(), vec![])
    }

    fn get_spawn_args(&self, config: &AgentConfig, is_resume: bool) -> Vec<String> {
        let mut args: Vec<String> = Vec::new();

        // Include directories (system + user-specified)
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
            args.push("--include-directories".into());
            args.push(final_includes.join(","));
        }

        if config.debug.unwrap_or(false) {
            args.push("--debug".into());
        }
        if let Some(ref model) = config.model {
            args.push("--model".into());
            args.push(model.clone());
        }
        if config.sandbox.unwrap_or(false) {
            args.push("--sandbox".into());
        }
        if config.yolo.unwrap_or(false) {
            args.push("--yolo".into());
        }
        if let Some(ref approval) = config.approval_mode {
            if !approval.trim().is_empty() {
                args.push("--approval-mode".into());
                args.push(approval.clone());
            }
        }
        if let Some(ref policy) = config.policy {
            if !policy.is_empty() {
                args.push("--policy".into());
                args.push(policy.join(","));
            }
        }
        if config.experimental_acp.unwrap_or(false) {
            args.push("--experimental-acp".into());
        }
        if let Some(ref servers) = config.allowed_mcp_server_names {
            for s in servers {
                args.push("--allowed-mcp-server-names".into());
                args.push(s.clone());
            }
        }
        if let Some(ref extensions) = config.extensions {
            if !extensions.is_empty() {
                args.push("--extensions".into());
                args.push(extensions.join(","));
            }
        }
        if config.screen_reader.unwrap_or(false) {
            args.push("--screen-reader".into());
        }
        if let Some(ref format) = config.output_format {
            args.push("--output-format".into());
            args.push(format.clone());
        }

        // Custom args (shell-parsed)
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

        match msg_type {
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
            "user" => Some(AgentEvent::UserQuery),
            // These event types are streaming response chunks — emitted *during* generation,
            // not at completion. Mapping them to ModelResponse (Idle) would overwrite
            // "Processing..." mid-turn. Only "result" signals a completed turn.
            "gemini" | "model" => Some(AgentEvent::Generating),
            "info" => Some(AgentEvent::Unknown),
            "message" => {
                let role = parsed.get("role").and_then(|v| v.as_str()).unwrap_or("");
                if role == "user" {
                    Some(AgentEvent::UserQuery)
                } else if role == "assistant" || role == "model" {
                    Some(AgentEvent::Generating)
                } else {
                    Some(AgentEvent::Unknown)
                }
            }
            "tool_use" => {
                let tool_name = parsed
                    .get("tool_name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("tool")
                    .to_string();
                Some(AgentEvent::ActionRequired { message: tool_name })
            }
            "tool_result" => Some(AgentEvent::Generating),
            "result" => Some(AgentEvent::TurnCompleted),
            _ => Some(AgentEvent::Unknown),
        }
    }

    fn get_instruction_filename(&self) -> &str {
        "GEMINI.md"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_provider() -> GeminiProvider {
        GeminiProvider::new()
    }

    #[test]
    fn name_returns_gemini() {
        let p = make_provider();
        assert_eq!(p.name(), "Gemini");
    }

    #[test]
    fn instruction_filename_is_provider_specific_stub() {
        let p = make_provider();
        assert_eq!(p.get_instruction_filename(), "GEMINI.md");
    }

    #[test]
    fn get_executable_returns_valid_binary() {
        let p = make_provider();
        let (bin, _args) = p.get_executable();
        // On any platform, the binary name should be non-empty
        assert!(!bin.is_empty());
        if cfg!(target_os = "windows") {
            // Should be either "node" or "gemini.cmd"
            assert!(bin == "node" || bin == "gemini.cmd");
        } else {
            assert_eq!(bin, "gemini");
        }
    }

    #[test]
    fn spawn_args_minimal_config() {
        let p = make_provider();
        let config = AgentConfig::default();
        let args = p.get_spawn_args(&config, false);
        // No flags should be added for a default config
        assert!(
            args.is_empty(),
            "Default config should produce no args, got: {:?}",
            args
        );
    }

    #[test]
    fn spawn_args_debug_and_model() {
        let p = make_provider();
        let config = AgentConfig {
            debug: Some(true),
            model: Some("gemini-2.5-pro".into()),
            ..Default::default()
        };
        let args = p.get_spawn_args(&config, false);
        assert!(args.contains(&"--debug".to_string()));
        assert!(args.contains(&"--model".to_string()));
        assert!(args.contains(&"gemini-2.5-pro".to_string()));
    }

    #[test]
    fn spawn_args_sandbox_and_yolo() {
        let p = make_provider();
        let config = AgentConfig {
            sandbox: Some(true),
            yolo: Some(true),
            ..Default::default()
        };
        let args = p.get_spawn_args(&config, false);
        assert!(args.contains(&"--sandbox".to_string()));
        assert!(args.contains(&"--yolo".to_string()));
    }

    #[test]
    fn spawn_args_resume_flag() {
        let p = make_provider();
        let config = AgentConfig {
            resume_session: Some("session-abc".into()),
            ..Default::default()
        };

        // is_resume=false should NOT add --resume
        let args_no_resume = p.get_spawn_args(&config, false);
        assert!(!args_no_resume.contains(&"--resume".to_string()));

        // is_resume=true should add --resume
        let args_resume = p.get_spawn_args(&config, true);
        assert!(args_resume.contains(&"--resume".to_string()));
        assert!(args_resume.contains(&"session-abc".to_string()));
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
        assert!(args.contains(&"--include-directories".to_string()));
        // Should be comma-joined
        let idx = args
            .iter()
            .position(|a| a == "--include-directories")
            .unwrap();
        assert_eq!(args[idx + 1], "/sys/dir,/user/dir");
    }

    #[test]
    fn spawn_args_approval_mode() {
        let p = make_provider();
        let config = AgentConfig {
            approval_mode: Some("auto".into()),
            ..Default::default()
        };
        let args = p.get_spawn_args(&config, false);
        assert!(args.contains(&"--approval-mode".to_string()));
        assert!(args.contains(&"auto".to_string()));
    }

    #[test]
    fn spawn_args_empty_approval_mode_ignored() {
        let p = make_provider();
        let config = AgentConfig {
            approval_mode: Some("  ".into()),
            ..Default::default()
        };
        let args = p.get_spawn_args(&config, false);
        assert!(!args.contains(&"--approval-mode".to_string()));
    }

    #[test]
    fn spawn_args_policy() {
        let p = make_provider();
        let config = AgentConfig {
            policy: Some(vec!["policy1".into(), "policy2".into()]),
            ..Default::default()
        };
        let args = p.get_spawn_args(&config, false);
        assert!(args.contains(&"--policy".to_string()));
        let idx = args.iter().position(|a| a == "--policy").unwrap();
        assert_eq!(args[idx + 1], "policy1,policy2");
    }

    #[test]
    fn spawn_args_mcp_server_names() {
        let p = make_provider();
        let config = AgentConfig {
            allowed_mcp_server_names: Some(vec!["server1".into(), "server2".into()]),
            ..Default::default()
        };
        let args = p.get_spawn_args(&config, false);
        // Each server name gets its own --allowed-mcp-server-names flag
        let count = args
            .iter()
            .filter(|a| *a == "--allowed-mcp-server-names")
            .count();
        assert_eq!(count, 2);
    }

    #[test]
    fn spawn_args_extensions() {
        let p = make_provider();
        let config = AgentConfig {
            extensions: Some(vec!["ext1".into(), "ext2".into()]),
            ..Default::default()
        };
        let args = p.get_spawn_args(&config, false);
        assert!(args.contains(&"--extensions".to_string()));
        let idx = args.iter().position(|a| a == "--extensions").unwrap();
        assert_eq!(args[idx + 1], "ext1,ext2");
    }

    #[test]
    fn spawn_args_custom_args() {
        let p = make_provider();
        let config = AgentConfig {
            custom_args: Some("--foo bar --baz".into()),
            ..Default::default()
        };
        let args = p.get_spawn_args(&config, false);
        assert!(args.contains(&"--foo".to_string()));
        assert!(args.contains(&"bar".to_string()));
        assert!(args.contains(&"--baz".to_string()));
    }

    #[test]
    fn spawn_args_screen_reader_and_output_format() {
        let p = make_provider();
        let config = AgentConfig {
            screen_reader: Some(true),
            output_format: Some("json".into()),
            ..Default::default()
        };
        let args = p.get_spawn_args(&config, false);
        assert!(args.contains(&"--screen-reader".to_string()));
        assert!(args.contains(&"--output-format".to_string()));
        assert!(args.contains(&"json".to_string()));
    }

    #[test]
    fn parse_output_init_event() {
        let p = make_provider();
        let line = r#"{"type":"init","session_id":"abc-123","timestamp":"2026-01-01T00:00:00Z"}"#;
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
    fn parse_output_user_event() {
        let p = make_provider();
        let line = r#"{"type":"user","content":"hello"}"#;
        let event = p.parse_output(line).unwrap();
        assert_eq!(event, AgentEvent::UserQuery);

        let line_msg = r#"{"type":"message","role":"user","content":"hello"}"#;
        assert_eq!(p.parse_output(line_msg).unwrap(), AgentEvent::UserQuery);
    }

    #[test]
    fn parse_output_model_events() {
        let p = make_provider();

        // "gemini" and "model" are streaming response chunks emitted during generation;
        // they must set Processing..., not Idle.
        for msg_type in &["gemini", "model"] {
            let line = format!(r#"{{"type":"{}","content":"response"}}"#, msg_type);
            let event = p.parse_output(&line).unwrap();
            assert_eq!(
                event,
                AgentEvent::Generating,
                "Expected Generating for type: {}",
                msg_type
            );
        }

        // "info" is a neutral informational event — no status change.
        let line_info = r#"{"type":"info","content":"some info"}"#;
        assert_eq!(p.parse_output(line_info).unwrap(), AgentEvent::Unknown);

        // "result" is the true end-of-turn signal → Idle.
        let line_result = r#"{"type":"result","status":"success"}"#;
        assert_eq!(
            p.parse_output(line_result).unwrap(),
            AgentEvent::TurnCompleted
        );

        let line_gen = r#"{"type":"message","role":"assistant","content":"hello"}"#;
        assert_eq!(p.parse_output(line_gen).unwrap(), AgentEvent::Generating);

        let line_tool_use = r#"{"type":"tool_use","tool_name":"read_file"}"#;
        assert_eq!(
            p.parse_output(line_tool_use).unwrap(),
            AgentEvent::ActionRequired {
                message: "read_file".to_string()
            }
        );

        let line_tool_result = r#"{"type":"tool_result","tool_id":"123"}"#;
        assert_eq!(
            p.parse_output(line_tool_result).unwrap(),
            AgentEvent::Generating
        );
    }

    #[test]
    fn parse_output_unknown_type() {
        let p = make_provider();
        let line = r#"{"type":"something_new","data":42}"#;
        let event = p.parse_output(line).unwrap();
        assert_eq!(event, AgentEvent::Unknown);
    }

    #[test]
    fn parse_output_invalid_json() {
        let p = make_provider();
        let event = p.parse_output("not json at all");
        assert!(event.is_none());
    }

    #[test]
    fn parse_output_missing_type_field() {
        let p = make_provider();
        let line = r#"{"content":"no type field"}"#;
        let event = p.parse_output(line);
        assert!(event.is_none());
    }
}
