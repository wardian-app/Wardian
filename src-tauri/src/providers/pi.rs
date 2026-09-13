use std::io::BufRead;
use std::path::{Path, PathBuf};

use wardian_core::models::provider::{AgentEvent, AgentProvider};
use wardian_core::models::AgentConfig;

pub(super) mod provenance;

pub struct PiProvider;

impl Default for PiProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl PiProvider {
    pub fn new() -> Self {
        Self
    }

    /// Pi's Wardian-owned session directory. Authentication, packages, themes,
    /// and user settings remain in Pi's normal global agent directory.
    pub fn session_dir(session_id: &str) -> Option<PathBuf> {
        let session_id = session_id.trim();
        if session_id.is_empty() {
            return None;
        }
        crate::utils::fs::get_wardian_home().map(|home| {
            home.join("agents")
                .join(session_id)
                .join("pi")
                .join("sessions")
        })
    }

    /// Resolves only the JSONL whose header binds the requested Pi session ID.
    pub fn session_file(session_dir: &Path, provider_session_id: &str) -> Option<PathBuf> {
        let provider_session_id = provider_session_id.trim();
        if provider_session_id.is_empty() {
            return None;
        }

        let entries = std::fs::read_dir(session_dir).ok()?;
        entries.filter_map(Result::ok).find_map(|entry| {
            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) != Some("jsonl") {
                return None;
            }
            let file = std::fs::File::open(&path).ok()?;
            let mut header = String::new();
            std::io::BufReader::new(file).read_line(&mut header).ok()?;
            let parsed: serde_json::Value = serde_json::from_str(header.trim_end()).ok()?;
            (parsed.get("type").and_then(|value| value.as_str()) == Some("session")
                && parsed.get("id").and_then(|value| value.as_str()) == Some(provider_session_id))
            .then_some(path)
        })
    }

    fn append_context_args(args: &mut Vec<String>, config: &AgentConfig) {
        let mut roots = config
            .system_include_directories
            .clone()
            .unwrap_or_default();
        if let Some(user_roots) = config.include_directories.as_ref() {
            for root in user_roots {
                if !roots.contains(root) {
                    roots.push(root.clone());
                }
            }
        }

        for root in roots {
            let root = PathBuf::from(root);
            let instructions = root.join("AGENTS.md");
            if instructions.is_file() {
                args.push("--append-system-prompt".into());
                args.push(instructions.to_string_lossy().to_string());
            }
            let skills = root.join(".agents").join("skills");
            if skills.is_dir() {
                args.push("--skill".into());
                args.push(skills.to_string_lossy().to_string());
            }
        }
    }

    fn assistant_stop_reason(value: &serde_json::Value) -> Option<&str> {
        value
            .get("message")
            .unwrap_or(value)
            .get("stopReason")
            .and_then(|reason| reason.as_str())
    }
}

impl AgentProvider for PiProvider {
    fn name(&self) -> &str {
        "Pi"
    }

    fn get_executable(&self) -> (String, Vec<String>) {
        #[cfg(target_os = "windows")]
        {
            if let Some(paths) = std::env::var_os("PATH") {
                for path in std::env::split_paths(&paths) {
                    if let Some(launch) =
                        crate::providers::npm::node_launch_from_npm_cmd_shim(&path, "pi")
                    {
                        return launch;
                    }
                    for name in ["pi.exe", "pi.cmd", "pi.bat", "pi"] {
                        let candidate = path.join(name);
                        if candidate.is_file() {
                            return (candidate.to_string_lossy().to_string(), vec![]);
                        }
                    }
                }
            }

            if let Some(appdata) = dirs::data_dir() {
                let npm_dir = appdata.join("npm");
                if let Some(launch) =
                    crate::providers::npm::node_launch_from_npm_cmd_shim(&npm_dir, "pi")
                {
                    return launch;
                }
            }
            ("pi".into(), vec![])
        }

        #[cfg(not(target_os = "windows"))]
        {
            if let Some(paths) = std::env::var_os("PATH") {
                for path in std::env::split_paths(&paths) {
                    let candidate = path.join("pi");
                    if candidate.is_file() {
                        return (candidate.to_string_lossy().to_string(), vec![]);
                    }
                }
            }
            let home = dirs::home_dir().unwrap_or_default();
            for candidate in [
                home.join(".npm-global/bin/pi"),
                PathBuf::from("/usr/local/bin/pi"),
                PathBuf::from("/opt/homebrew/bin/pi"),
            ] {
                if candidate.is_file() {
                    return (candidate.to_string_lossy().to_string(), vec![]);
                }
            }
            ("pi".into(), vec![])
        }
    }

    fn get_spawn_args(&self, config: &AgentConfig, is_resume: bool) -> Vec<String> {
        let pi = config.pi_config();
        let mut args = vec!["--tui-mode".into(), "regular".into()];

        if let Some(session_dir) = Self::session_dir(&config.session_id) {
            let _ = std::fs::create_dir_all(&session_dir);
            args.push("--session-dir".into());
            args.push(session_dir.to_string_lossy().to_string());
        }

        if is_resume {
            if let Some(session_id) = config
                .resume_session
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                args.push("--session".into());
                args.push(session_id.into());
            }
        } else if let Some(session_id) = config
            .fresh_provider_session_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            args.push("--session-id".into());
            args.push(session_id.into());
            if !config.session_name.trim().is_empty() {
                args.push("--name".into());
                args.push(config.session_name.clone());
            }
        }

        if let Some(model) = config
            .model
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            args.push("--model".into());
            args.push(model.into());
        }
        if let Some(thinking) = pi
            .reasoning_effort
            .as_deref()
            .map(str::trim)
            .filter(|value| {
                matches!(
                    *value,
                    "off" | "minimal" | "low" | "medium" | "high" | "xhigh" | "max"
                )
            })
        {
            args.push("--thinking".into());
            args.push(thinking.into());
        }
        match pi.project_trust.as_deref().map(str::trim) {
            Some("approve") => args.push("--approve".into()),
            Some("ignore") => args.push("--no-approve".into()),
            _ => {}
        }
        if pi.no_tools.unwrap_or(false) {
            args.push("--no-tools".into());
        }
        if let Some(tools) = pi.tools.filter(|values| !values.is_empty()) {
            args.push("--tools".into());
            args.push(tools.join(","));
        }
        if let Some(tools) = pi.exclude_tools.filter(|values| !values.is_empty()) {
            args.push("--exclude-tools".into());
            args.push(tools.join(","));
        }
        if pi.offline.unwrap_or(false) {
            args.push("--offline".into());
        }

        Self::append_context_args(&mut args, config);
        if let Some(custom) = config.custom_args.as_deref().and_then(shlex::split) {
            args.extend(custom);
        }
        args
    }

    fn parse_output(&self, line: &str) -> Option<AgentEvent> {
        let parsed: serde_json::Value = serde_json::from_str(line.trim()).ok()?;
        let kind = parsed.get("type").and_then(|value| value.as_str())?;
        match kind {
            "session" => Some(AgentEvent::Init {
                session_id: parsed.get("id")?.as_str()?.to_string(),
                timestamp: parsed
                    .get("timestamp")
                    .and_then(|value| value.as_str())
                    .map(str::to_string),
            }),
            "agent_start" | "turn_start" => Some(AgentEvent::UserQuery),
            "message_start"
            | "message_update"
            | "tool_execution_start"
            | "tool_execution_update"
            | "tool_execution_end" => Some(AgentEvent::Generating),
            "agent_end" => Some(AgentEvent::TurnCompleted),
            "message_end" => match Self::assistant_stop_reason(&parsed) {
                Some("stop" | "length") => Some(AgentEvent::ModelResponse),
                Some("toolUse") => Some(AgentEvent::Generating),
                Some("error" | "aborted") => Some(AgentEvent::TurnCompleted),
                _ => Some(AgentEvent::Unknown),
            },
            "message" => {
                let message = parsed.get("message")?;
                match message.get("role").and_then(|value| value.as_str()) {
                    Some("user") => Some(AgentEvent::UserQuery),
                    Some("assistant") => match Self::assistant_stop_reason(message) {
                        Some("stop" | "length" | "error" | "aborted") => {
                            Some(AgentEvent::TurnCompleted)
                        }
                        Some("toolUse") => Some(AgentEvent::Generating),
                        _ => Some(AgentEvent::Generating),
                    },
                    Some("toolResult" | "bashExecution") => Some(AgentEvent::Generating),
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
    use wardian_core::models::{PiProviderConfig, ProviderConfig};

    #[test]
    fn fresh_spawn_uses_exact_session_and_wardian_context() {
        let root = tempfile::tempdir().expect("context root");
        std::fs::write(root.path().join("AGENTS.md"), "instructions").expect("instructions");
        std::fs::create_dir_all(root.path().join(".agents").join("skills")).expect("skills");
        let config = AgentConfig {
            session_id: "wardian-agent".into(),
            session_name: "Pi Builder".into(),
            provider: "pi".into(),
            fresh_provider_session_id: Some("pi-session".into()),
            system_include_directories: Some(vec![root.path().to_string_lossy().to_string()]),
            model: Some("anthropic/claude-sonnet-4-5".into()),
            provider_config: ProviderConfig::Pi(PiProviderConfig {
                reasoning_effort: Some("high".into()),
                project_trust: Some("ignore".into()),
                tools: Some(vec!["read".into(), "bash".into()]),
                offline: Some(true),
                ..Default::default()
            }),
            ..Default::default()
        };

        let args = PiProvider::new().get_spawn_args(&config, false);
        assert!(args
            .windows(2)
            .any(|pair| pair == ["--session-id", "pi-session"]));
        assert!(args.windows(2).any(|pair| pair == ["--name", "Pi Builder"]));
        assert!(args
            .windows(2)
            .any(|pair| pair == ["--tui-mode", "regular"]));
        assert!(args.windows(2).any(|pair| pair == ["--thinking", "high"]));
        assert!(args.windows(2).any(|pair| pair == ["--tools", "read,bash"]));
        assert!(args.contains(&"--no-approve".into()));
        assert!(args.contains(&"--offline".into()));
        assert!(args
            .windows(2)
            .any(|pair| pair[0] == "--append-system-prompt"));
        assert!(args.windows(2).any(|pair| pair[0] == "--skill"));
    }

    #[test]
    fn parses_session_and_definitive_completion_events() {
        let provider = PiProvider::new();
        assert!(matches!(
            provider.parse_output(r#"{"type":"session","id":"pi-1","timestamp":"now"}"#),
            Some(AgentEvent::Init { session_id, .. }) if session_id == "pi-1"
        ));
        assert_eq!(
            provider.parse_output(r#"{"type":"agent_end","messages":[]}"#),
            Some(AgentEvent::TurnCompleted)
        );
        assert_eq!(
            provider.parse_output(
                r#"{"type":"message","message":{"role":"assistant","stopReason":"stop"}}"#
            ),
            Some(AgentEvent::TurnCompleted)
        );
    }

    #[test]
    fn resolves_only_session_file_with_matching_header() {
        let dir = tempfile::tempdir().expect("session dir");
        let wanted = dir.path().join("wanted.jsonl");
        std::fs::write(
            &wanted,
            r#"{"type":"session","id":"wanted"}
"#,
        )
        .expect("wanted session");
        std::fs::write(
            dir.path().join("newer.jsonl"),
            r#"{"type":"session","id":"other"}
"#,
        )
        .expect("other session");

        assert_eq!(PiProvider::session_file(dir.path(), "wanted"), Some(wanted));
        assert_eq!(PiProvider::session_file(dir.path(), "missing"), None);
    }

    #[test]
    fn resolves_session_without_reading_large_non_utf8_transcript_tail() {
        use std::io::Write;

        let dir = tempfile::tempdir().expect("session dir");
        let wanted = dir.path().join("wanted.jsonl");
        let mut file = std::fs::File::create(&wanted).expect("session file");
        file.write_all(b"{\"type\":\"session\",\"id\":\"wanted\"}\n")
            .expect("session header");
        file.write_all(&vec![0xff; 1024 * 1024])
            .expect("large transcript tail");

        assert_eq!(PiProvider::session_file(dir.path(), "wanted"), Some(wanted));
    }
}
