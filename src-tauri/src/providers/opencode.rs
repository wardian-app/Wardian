use crate::models::provider::{AgentEvent, AgentProvider};
use crate::models::AgentConfig;

/// Provider adapter for the OpenCode CLI.
pub struct OpenCodeProvider;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OpenCodeRunSummary {
    pub session_id: Option<String>,
    pub last_text: Option<String>,
}

impl Default for OpenCodeProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl OpenCodeProvider {
    pub fn new() -> Self {
        OpenCodeProvider
    }

    #[cfg(target_os = "windows")]
    fn packaged_windows_binary_from_shim(base_dir: &std::path::Path) -> Option<String> {
        [
            base_dir
                .join("node_modules")
                .join("opencode-ai")
                .join("node_modules")
                .join("opencode-windows-x64")
                .join("bin")
                .join("opencode.exe"),
            base_dir
                .join("node_modules")
                .join("opencode-ai")
                .join("node_modules")
                .join("opencode-windows-x64-baseline")
                .join("bin")
                .join("opencode.exe"),
            base_dir
                .join("node_modules")
                .join("opencode-ai")
                .join("node_modules")
                .join("opencode-windows-arm64")
                .join("bin")
                .join("opencode.exe"),
        ]
        .into_iter()
        .find(|candidate| candidate.exists())
        .map(|candidate| candidate.to_string_lossy().to_string())
    }

    #[cfg(target_os = "windows")]
    fn find_windows_opencode_in_paths<I>(paths: I, path_exts: &[String]) -> Option<String>
    where
        I: IntoIterator<Item = std::path::PathBuf>,
    {
        for path in paths {
            let direct_exe = path.join("opencode.exe");
            if direct_exe.exists() {
                return Some(direct_exe.to_string_lossy().to_string());
            }

            let bare = path.join("opencode");
            if bare.exists() {
                if let Some(executable) = Self::packaged_windows_binary_from_shim(&path) {
                    return Some(executable);
                }
                return Some("opencode".to_string());
            }

            let powershell = path.join("opencode.ps1");
            if powershell.exists() {
                if let Some(executable) = Self::packaged_windows_binary_from_shim(&path) {
                    return Some(executable);
                }
                return Some("opencode".to_string());
            }

            for ext in path_exts {
                let candidate = path.join(format!("opencode{ext}"));
                if candidate.exists() {
                    if candidate
                        .extension()
                        .and_then(|value| value.to_str())
                        .is_some_and(|value| value.eq_ignore_ascii_case("exe"))
                    {
                        return Some(candidate.to_string_lossy().to_string());
                    }

                    if let Some(executable) = Self::packaged_windows_binary_from_shim(&path) {
                        return Some(executable);
                    }
                    return Some("opencode".to_string());
                }
            }
        }

        None
    }

    #[cfg(not(target_os = "windows"))]
    fn find_unix_opencode_in_paths<I>(paths: I) -> Option<String>
    where
        I: IntoIterator<Item = std::path::PathBuf>,
    {
        for path in paths {
            let full_path = path.join("opencode");
            if full_path.exists() {
                return Some(full_path.to_string_lossy().to_string());
            }
        }

        None
    }

    pub fn summarize_run_output(output: &str) -> OpenCodeRunSummary {
        let mut summary = OpenCodeRunSummary::default();

        for line in output.lines() {
            if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(line) {
                if summary.session_id.is_none() {
                    summary.session_id = parsed
                        .get("sessionID")
                        .and_then(|value| value.as_str())
                        .map(|value| value.to_string());
                }

                if parsed.get("type").and_then(|value| value.as_str()) == Some("text") {
                    if let Some(text) = parsed
                        .get("part")
                        .and_then(|value| value.get("text"))
                        .and_then(|value| value.as_str())
                    {
                        summary.last_text = Some(text.trim().to_string());
                    }
                }
            }
        }

        summary
    }
}

impl AgentProvider for OpenCodeProvider {
    fn name(&self) -> &str {
        "OpenCode"
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

                if let Some(executable) =
                    Self::find_windows_opencode_in_paths(std::env::split_paths(&paths), &path_exts)
                {
                    return (executable, vec![]);
                }
            }

            ("opencode".to_string(), vec![])
        }

        #[cfg(not(target_os = "windows"))]
        {
            if let Some(paths) = std::env::var_os("PATH") {
                if let Some(executable) =
                    Self::find_unix_opencode_in_paths(std::env::split_paths(&paths))
                {
                    return (executable, vec![]);
                }
            }

            let home = dirs::home_dir().unwrap_or_default();
            let fallbacks = vec![
                home.join(".npm-global/bin/opencode"),
                std::path::PathBuf::from("/usr/local/bin/opencode"),
                std::path::PathBuf::from("/opt/homebrew/bin/opencode"),
            ];
            for path in fallbacks {
                if path.exists() {
                    return (path.to_string_lossy().to_string(), vec![]);
                }
            }

            ("opencode".to_string(), vec![])
        }
    }

    fn get_spawn_args(&self, config: &AgentConfig, is_resume: bool) -> Vec<String> {
        let mut args = Vec::new();

        if config.debug.unwrap_or(false) {
            args.push("--print-logs".into());
        }

        if let Some(model) = config.model.as_ref().filter(|s| !s.trim().is_empty()) {
            args.push("--model".into());
            args.push(model.clone());
        }

        if let Some(agent) = config
            .opencode_agent
            .as_ref()
            .filter(|s| !s.trim().is_empty())
        {
            args.push("--agent".into());
            args.push(agent.clone());
        }

        if is_resume {
            if let Some(session_id) = config
                .resume_session
                .as_ref()
                .filter(|s| !s.trim().is_empty())
            {
                args.push("--session".into());
                args.push(session_id.clone());
            }
        }

        if let Some(custom) = config.custom_args.as_ref() {
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
            "step_start" => Some(AgentEvent::UserQuery),
            "text" => Some(AgentEvent::Generating),
            "tool_use" => Some(AgentEvent::Generating),
            "step_finish" => {
                let reason = parsed
                    .get("part")
                    .and_then(|v| v.get("reason"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                match reason {
                    "stop" => Some(AgentEvent::ModelResponse),
                    "tool-calls" => Some(AgentEvent::Generating),
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

    fn make_provider() -> OpenCodeProvider {
        OpenCodeProvider::new()
    }

    #[test]
    fn name_returns_opencode() {
        let provider = make_provider();
        assert_eq!(provider.name(), "OpenCode");
    }

    #[test]
    fn instruction_filename_is_agents_md() {
        let provider = make_provider();
        assert_eq!(provider.get_instruction_filename(), "AGENTS.md");
    }

    #[test]
    fn spawn_args_include_model_agent_and_resume_session() {
        let provider = make_provider();
        let config = AgentConfig {
            debug: Some(true),
            model: Some("openai/gpt-5".into()),
            opencode_agent: Some("build".into()),
            resume_session: Some("ses_123".into()),
            ..Default::default()
        };

        let args = provider.get_spawn_args(&config, true);

        assert!(args.contains(&"--print-logs".to_string()));
        assert!(args.contains(&"--model".to_string()));
        assert!(args.contains(&"openai/gpt-5".to_string()));
        assert!(args.contains(&"--agent".to_string()));
        assert!(args.contains(&"build".to_string()));
        assert!(args.contains(&"--session".to_string()));
        assert!(args.contains(&"ses_123".to_string()));
    }

    #[test]
    fn spawn_args_parse_custom_args() {
        let provider = make_provider();
        let config = AgentConfig {
            custom_args: Some("--prompt test --fork".into()),
            ..Default::default()
        };

        let args = provider.get_spawn_args(&config, false);

        assert!(args.contains(&"--prompt".to_string()));
        assert!(args.contains(&"test".to_string()));
        assert!(args.contains(&"--fork".to_string()));
    }

    #[test]
    fn parse_output_step_start_counts_as_user_query() {
        let provider = make_provider();
        let line = r#"{"type":"step_start","timestamp":1775197573899,"sessionID":"ses_test"}"#;

        let event = provider.parse_output(line).unwrap();

        assert_eq!(event, AgentEvent::UserQuery);
    }

    #[test]
    fn parse_output_text_is_generating() {
        let provider = make_provider();
        let line =
            r#"{"type":"text","sessionID":"ses_test","part":{"type":"text","text":"hello"}}"#;
        assert_eq!(provider.parse_output(line).unwrap(), AgentEvent::Generating);
    }

    #[test]
    fn parse_output_step_finish_stop_is_model_response() {
        let provider = make_provider();
        let line = r#"{"type":"step_finish","sessionID":"ses_test","part":{"reason":"stop"}}"#;
        assert_eq!(
            provider.parse_output(line).unwrap(),
            AgentEvent::ModelResponse
        );
    }

    #[test]
    fn parse_output_step_finish_tool_calls_stays_generating() {
        let provider = make_provider();
        let line =
            r#"{"type":"step_finish","sessionID":"ses_test","part":{"reason":"tool-calls"}}"#;
        assert_eq!(provider.parse_output(line).unwrap(), AgentEvent::Generating);
    }

    #[test]
    fn parse_output_invalid_json_is_none() {
        let provider = make_provider();
        assert!(provider.parse_output("not json").is_none());
    }

    #[test]
    fn summarize_run_output_extracts_session_and_last_text() {
        let output = concat!(
            "{\"type\":\"step_start\",\"sessionID\":\"ses_test\"}\n",
            "{\"type\":\"text\",\"part\":{\"text\":\"hello\"}}\n",
            "{\"type\":\"text\",\"part\":{\"text\":\"world\"}}\n"
        );

        let summary = OpenCodeProvider::summarize_run_output(output);

        assert_eq!(
            summary,
            OpenCodeRunSummary {
                session_id: Some("ses_test".into()),
                last_text: Some("world".into()),
            }
        );
    }

    #[test]
    fn summarize_run_output_ignores_invalid_lines() {
        let output = "not-json\n{\"type\":\"text\",\"part\":{\"text\":\"hello\"}}";

        let summary = OpenCodeProvider::summarize_run_output(output);

        assert_eq!(summary.last_text, Some("hello".into()));
        assert_eq!(summary.session_id, None);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_path_lookup_prefers_packaged_executable_from_shim_dir() {
        let temp = tempfile::tempdir().expect("temp dir");
        let cmd = temp.path().join("opencode.cmd");
        std::fs::write(&cmd, "@echo off\r\n").expect("cmd shim");
        let exe = temp
            .path()
            .join("node_modules")
            .join("opencode-ai")
            .join("node_modules")
            .join("opencode-windows-x64")
            .join("bin")
            .join("opencode.exe");
        std::fs::create_dir_all(exe.parent().expect("parent")).expect("create dirs");
        std::fs::write(&exe, "").expect("exe");

        let resolved = OpenCodeProvider::find_windows_opencode_in_paths(
            vec![temp.path().to_path_buf()],
            &[".exe".into(), ".cmd".into(), ".bat".into()],
        );

        assert_eq!(resolved, Some(exe.to_string_lossy().to_string()));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_path_lookup_uses_packaged_executable_for_powershell_script() {
        let temp = tempfile::tempdir().expect("temp dir");
        let ps1 = temp.path().join("opencode.ps1");
        std::fs::write(&ps1, "Write-Output hi\r\n").expect("ps1 shim");
        let exe = temp
            .path()
            .join("node_modules")
            .join("opencode-ai")
            .join("node_modules")
            .join("opencode-windows-x64")
            .join("bin")
            .join("opencode.exe");
        std::fs::create_dir_all(exe.parent().expect("parent")).expect("create dirs");
        std::fs::write(&exe, "").expect("exe");

        let resolved = OpenCodeProvider::find_windows_opencode_in_paths(
            vec![temp.path().to_path_buf()],
            &[".exe".into(), ".cmd".into(), ".bat".into()],
        );

        assert_eq!(resolved, Some(exe.to_string_lossy().to_string()));
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn unix_path_lookup_finds_opencode_binary() {
        let temp = tempfile::tempdir().expect("temp dir");
        let opencode = temp.path().join("opencode");
        std::fs::write(&opencode, "#!/bin/sh\n").expect("binary");

        let resolved =
            OpenCodeProvider::find_unix_opencode_in_paths(vec![temp.path().to_path_buf()]);

        assert_eq!(resolved, Some(opencode.to_string_lossy().to_string()));
    }
}
