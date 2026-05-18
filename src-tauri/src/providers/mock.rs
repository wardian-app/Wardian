use wardian_core::models::provider::{AgentEvent, AgentProvider};
use wardian_core::models::AgentConfig;

/// A deterministic mock provider for automated testing.
///
/// Spawns `scripts/mock-agent.cjs` via Node.js, which emits scripted JSON
/// events matching the Gemini event format. Behavior is controlled by
/// environment variables (`WARDIAN_MOCK_SCENARIO`, `WARDIAN_MOCK_DELAY_MS`).
pub struct MockProvider;

impl Default for MockProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl MockProvider {
    pub fn new() -> Self {
        MockProvider
    }

    fn existing_script_path(path: std::path::PathBuf) -> Option<String> {
        if !path.exists() {
            return None;
        }

        Some(Self::node_compatible_script_path(
            std::fs::canonicalize(&path).unwrap_or(path),
        ))
    }

    fn node_compatible_script_path(path: std::path::PathBuf) -> String {
        let text = path.to_string_lossy();
        #[cfg(windows)]
        {
            if let Some(stripped) = text.strip_prefix("\\\\?\\UNC\\") {
                return format!("\\\\{stripped}");
            }
            if let Some(stripped) = text.strip_prefix("\\\\?\\") {
                return stripped.to_string();
            }
        }
        text.to_string()
    }

    /// Resolves the path to `scripts/mock-agent.cjs`.
    ///
    /// Priority:
    /// 1. `WARDIAN_MOCK_SCRIPT` env var (for test overrides)
    /// 2. Relative to the current executable (bundled resources)
    /// 3. Fallback to repo-relative path (development mode)
    fn resolve_mock_script_path() -> String {
        if let Ok(path) = std::env::var("WARDIAN_MOCK_SCRIPT") {
            if !path.is_empty() {
                if let Some(path) = Self::existing_script_path(std::path::PathBuf::from(path)) {
                    return path;
                }
            }
        }

        // In bundled mode, scripts are next to the executable
        if let Ok(exe) = std::env::current_exe() {
            if let Some(exe_dir) = exe.parent() {
                // Tauri bundles resources alongside the binary
                if let Some(path) =
                    Self::existing_script_path(exe_dir.join("scripts").join("mock-agent.cjs"))
                {
                    return path;
                }

                if let Some(path) = Self::existing_script_path(
                    exe_dir
                        .join("..")
                        .join("..")
                        .join("scripts")
                        .join("mock-agent.cjs"),
                ) {
                    return path;
                }
            }
        }

        // Development fallback: repo-relative paths from the Tauri app cwd or src-tauri/.
        for dev_path in [
            std::path::PathBuf::from("scripts").join("mock-agent.cjs"),
            std::path::PathBuf::from("..")
                .join("scripts")
                .join("mock-agent.cjs"),
        ] {
            if let Some(path) = Self::existing_script_path(dev_path) {
                return path;
            }
        }

        // Ultimate fallback
        "scripts/mock-agent.cjs".to_string()
    }
}

impl AgentProvider for MockProvider {
    fn name(&self) -> &str {
        "Mock"
    }

    fn get_executable(&self) -> (String, Vec<String>) {
        let node = if cfg!(target_os = "windows") {
            "node.exe"
        } else {
            "node"
        };
        let script = Self::resolve_mock_script_path();
        (node.to_string(), vec![script])
    }

    fn get_spawn_args(&self, config: &AgentConfig, is_resume: bool) -> Vec<String> {
        let mut args = Vec::new();
        if is_resume {
            if let Some(ref sid) = config.resume_session {
                if !sid.is_empty() {
                    args.push("--resume".into());
                    args.push(sid.clone());
                }
            }
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
            "model" | "info" => Some(AgentEvent::ModelResponse),
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
            "result" => Some(AgentEvent::TurnCompleted),
            "action_required" => {
                let msg = parsed
                    .get("message")
                    .and_then(|v| v.as_str())
                    .unwrap_or("Action required")
                    .to_string();
                Some(AgentEvent::ActionRequired { message: msg })
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

    fn make_provider() -> MockProvider {
        MockProvider::new()
    }

    #[test]
    fn name_returns_mock() {
        assert_eq!(make_provider().name(), "Mock");
    }

    #[test]
    fn instruction_filename_is_agents_md() {
        assert_eq!(make_provider().get_instruction_filename(), "AGENTS.md");
    }

    #[test]
    fn get_executable_returns_node() {
        let (bin, args) = make_provider().get_executable();
        if cfg!(target_os = "windows") {
            assert_eq!(bin, "node.exe");
        } else {
            assert_eq!(bin, "node");
        }
        assert!(!args.is_empty(), "Should include script path");
        assert!(
            args[0].contains("mock-agent"),
            "First arg should be mock-agent script path, got: {}",
            args[0]
        );
    }

    #[test]
    fn get_executable_returns_absolute_script_path() {
        let (_bin, args) = make_provider().get_executable();

        assert!(
            std::path::Path::new(&args[0]).is_absolute(),
            "mock script path should survive provider cwd changes, got: {}",
            args[0]
        );
    }

    #[test]
    fn existing_script_path_returns_canonical_existing_path() {
        let temp = tempfile::tempdir().expect("temp dir");
        let script = temp.path().join("mock-agent.cjs");
        std::fs::write(&script, "console.log('mock');").expect("write script");

        let resolved = MockProvider::existing_script_path(script.clone()).unwrap();
        let expected =
            MockProvider::node_compatible_script_path(std::fs::canonicalize(script).unwrap());

        assert_eq!(resolved, expected);
        #[cfg(windows)]
        assert!(
            !resolved.starts_with(r"\\?\"),
            "Node rejects verbatim Windows script paths: {resolved}"
        );
    }

    #[test]
    fn existing_script_path_rejects_missing_path() {
        let temp = tempfile::tempdir().expect("temp dir");

        assert!(MockProvider::existing_script_path(temp.path().join("missing.cjs")).is_none());
    }

    #[test]
    fn resolve_mock_script_path_prefers_existing_env_override() {
        let _guard = crate::utils::wardian_test_env_lock();
        let temp = tempfile::tempdir().expect("temp dir");
        let script = temp.path().join("custom-mock-agent.cjs");
        std::fs::write(&script, "console.log('custom mock');").expect("write script");
        std::env::set_var("WARDIAN_MOCK_SCRIPT", &script);

        let resolved = MockProvider::resolve_mock_script_path();
        let expected =
            MockProvider::node_compatible_script_path(std::fs::canonicalize(&script).unwrap());

        assert_eq!(resolved, expected);
        std::env::remove_var("WARDIAN_MOCK_SCRIPT");
    }

    #[test]
    fn spawn_args_empty_for_fresh_session() {
        let config = AgentConfig::default();
        let args = make_provider().get_spawn_args(&config, false);
        assert!(args.is_empty());
    }

    #[test]
    fn spawn_args_resume_flag() {
        let config = AgentConfig {
            resume_session: Some("session-abc".into()),
            ..Default::default()
        };
        let args = make_provider().get_spawn_args(&config, true);
        assert!(args.contains(&"--resume".to_string()));
        assert!(args.contains(&"session-abc".to_string()));
    }

    #[test]
    fn spawn_args_no_resume_when_not_resuming() {
        let config = AgentConfig {
            resume_session: Some("session-abc".into()),
            ..Default::default()
        };
        let args = make_provider().get_spawn_args(&config, false);
        assert!(!args.contains(&"--resume".to_string()));
    }

    #[test]
    fn parse_output_init_event() {
        let line = r#"{"type":"init","session_id":"mock-001","timestamp":"2026-01-01T00:00:00Z"}"#;
        let event = make_provider().parse_output(line).unwrap();
        assert_eq!(
            event,
            AgentEvent::Init {
                session_id: "mock-001".into(),
                timestamp: Some("2026-01-01T00:00:00Z".into()),
            }
        );
    }

    #[test]
    fn parse_output_user_event() {
        let line = r#"{"type":"user","content":"hello"}"#;
        assert_eq!(
            make_provider().parse_output(line).unwrap(),
            AgentEvent::UserQuery
        );
    }

    #[test]
    fn parse_output_generating_event() {
        let line = r#"{"type":"message","role":"assistant","content":"thinking..."}"#;
        assert_eq!(
            make_provider().parse_output(line).unwrap(),
            AgentEvent::Generating
        );
    }

    #[test]
    fn parse_output_model_response_event() {
        let line = r#"{"type":"model","content":"done"}"#;
        assert_eq!(
            make_provider().parse_output(line).unwrap(),
            AgentEvent::ModelResponse
        );
    }

    #[test]
    fn parse_output_turn_completed_event() {
        let line = r#"{"type":"result","status":"success"}"#;
        assert_eq!(
            make_provider().parse_output(line).unwrap(),
            AgentEvent::TurnCompleted
        );
    }

    #[test]
    fn parse_output_action_required_event() {
        let line = r#"{"type":"action_required","message":"Approve file write?"}"#;
        let event = make_provider().parse_output(line).unwrap();
        assert_eq!(
            event,
            AgentEvent::ActionRequired {
                message: "Approve file write?".into()
            }
        );
    }

    #[test]
    fn parse_output_unknown_type() {
        let line = r#"{"type":"something_new","data":42}"#;
        assert_eq!(
            make_provider().parse_output(line).unwrap(),
            AgentEvent::Unknown
        );
    }

    #[test]
    fn parse_output_invalid_json() {
        assert!(make_provider().parse_output("not json").is_none());
    }

    #[test]
    fn parse_output_missing_type() {
        let line = r#"{"content":"no type field"}"#;
        assert!(make_provider().parse_output(line).is_none());
    }
}
