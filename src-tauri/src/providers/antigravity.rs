use std::path::{Path, PathBuf};

use wardian_core::models::provider::{AgentEvent, AgentProvider};
use wardian_core::models::AgentConfig;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AntigravityTranscriptSummary {
    pub conversation_id: Option<String>,
    pub last_text: Option<String>,
    pub last_step_index: Option<u64>,
}

pub struct AntigravityProvider;

impl Default for AntigravityProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl AntigravityProvider {
    pub fn new() -> Self {
        AntigravityProvider
    }

    pub fn antigravity_home() -> Option<PathBuf> {
        dirs::home_dir().map(|home| home.join(".gemini").join("antigravity-cli"))
    }

    pub fn transcript_path(home: &Path, conversation_id: &str) -> PathBuf {
        home.join("brain")
            .join(conversation_id)
            .join(".system_generated")
            .join("logs")
            .join("transcript.jsonl")
    }

    pub fn conversation_for_workspace(home: &Path, workspace: &Path) -> Option<String> {
        let cache = home.join("cache").join("last_conversations.json");
        let content = std::fs::read_to_string(cache).ok()?;
        let parsed: serde_json::Value = serde_json::from_str(&content).ok()?;
        let object = parsed.as_object()?;
        let workspace_key = normalize_path_key(workspace);
        object.iter().find_map(|(key, value)| {
            (normalize_path_text(key) == workspace_key)
                .then(|| value.as_str().map(str::to_string))
                .flatten()
        })
    }

    pub fn latest_conversation_id(home: &Path) -> Option<String> {
        let brain = home.join("brain");
        let entries = std::fs::read_dir(brain).ok()?;
        entries
            .flatten()
            .filter(|entry| entry.path().is_dir())
            .filter_map(|entry| {
                let modified = entry.metadata().ok()?.modified().ok()?;
                let name = entry.file_name().to_string_lossy().to_string();
                Some((modified, name))
            })
            .max_by_key(|(modified, _)| *modified)
            .map(|(_, name)| name)
    }

    pub fn summarize_conversation(
        home: &Path,
        conversation_id: &str,
    ) -> Result<AntigravityTranscriptSummary, String> {
        let path = Self::transcript_path(home, conversation_id);
        let content = std::fs::read_to_string(&path).map_err(|err| {
            format!(
                "Failed to read Antigravity transcript {}: {}",
                path.display(),
                err
            )
        })?;
        let mut summary = Self::summarize_transcript_content(&content);
        summary.conversation_id = Some(conversation_id.to_string());
        Ok(summary)
    }

    pub fn summarize_transcript_content(content: &str) -> AntigravityTranscriptSummary {
        let mut summary = AntigravityTranscriptSummary::default();
        for line in content.lines() {
            let Ok(parsed) = serde_json::from_str::<serde_json::Value>(line) else {
                continue;
            };
            if is_antigravity_model_response(&parsed) {
                if let Some(text) = parsed.get("content").and_then(|value| value.as_str()) {
                    let trimmed = text.trim();
                    if !trimmed.is_empty() {
                        summary.last_text = Some(trimmed.to_string());
                    }
                }
                summary.last_step_index = parsed.get("step_index").and_then(|value| value.as_u64());
            }
        }
        summary
    }
}

impl AgentProvider for AntigravityProvider {
    fn name(&self) -> &str {
        "antigravity"
    }

    fn get_executable(&self) -> (String, Vec<String>) {
        #[cfg(target_os = "windows")]
        {
            if let Some(paths) = std::env::var_os("PATH") {
                for path in std::env::split_paths(&paths) {
                    for name in ["agy.exe", "agy.cmd", "agy.bat", "agy"] {
                        let candidate = path.join(name);
                        if candidate.is_file() {
                            return (candidate.to_string_lossy().to_string(), vec![]);
                        }
                    }
                }
            }

            if let Some(local) = dirs::data_local_dir() {
                let candidate = local.join("agy").join("bin").join("agy.exe");
                if candidate.is_file() {
                    return (candidate.to_string_lossy().to_string(), vec![]);
                }
            }

            ("agy".to_string(), vec![])
        }

        #[cfg(not(target_os = "windows"))]
        {
            if let Some(paths) = std::env::var_os("PATH") {
                for path in std::env::split_paths(&paths) {
                    let candidate = path.join("agy");
                    if candidate.is_file() {
                        return (candidate.to_string_lossy().to_string(), vec![]);
                    }
                }
            }

            let home = dirs::home_dir().unwrap_or_default();
            for path in [
                home.join(".local/bin/agy"),
                PathBuf::from("/usr/local/bin/agy"),
                PathBuf::from("/opt/homebrew/bin/agy"),
            ] {
                if path.is_file() {
                    return (path.to_string_lossy().to_string(), vec![]);
                }
            }

            ("agy".to_string(), vec![])
        }
    }

    fn get_spawn_args(&self, config: &AgentConfig, is_resume: bool) -> Vec<String> {
        let mut args = Vec::new();
        let antigravity = config.antigravity_config();

        let mut directories = config
            .system_include_directories
            .clone()
            .unwrap_or_default();
        if let Some(user_dirs) = config.include_directories.as_ref() {
            for dir in user_dirs {
                if !directories.contains(dir) {
                    directories.push(dir.clone());
                }
            }
        }
        let directories = crate::utils::fs::project_antigravity_include_directories(
            &config.session_id,
            directories,
        );
        for dir in directories {
            args.push("--add-dir".to_string());
            args.push(dir);
        }

        if antigravity.sandbox.unwrap_or(false) {
            args.push("--sandbox".to_string());
        }
        if antigravity.dangerously_skip_permissions.unwrap_or(false) {
            args.push("--dangerously-skip-permissions".to_string());
        }

        if is_resume {
            if let Some(session_id) = config
                .resume_session
                .as_ref()
                .filter(|value| !value.trim().is_empty())
            {
                args.push("--conversation".to_string());
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
        match parsed.get("type").and_then(|value| value.as_str()) {
            Some("USER_INPUT") => Some(AgentEvent::UserQuery),
            Some("PLANNER_RESPONSE") if is_antigravity_model_response(&parsed) => {
                Some(AgentEvent::TurnCompleted)
            }
            Some("PLANNER_RESPONSE") => Some(AgentEvent::Generating),
            Some("SYSTEM_MESSAGE") | Some("CONVERSATION_HISTORY") => Some(AgentEvent::Unknown),
            _ => Some(AgentEvent::Unknown),
        }
    }

    fn get_instruction_filename(&self) -> &str {
        "AGENTS.md"
    }
}

fn is_antigravity_model_response(value: &serde_json::Value) -> bool {
    value.get("source").and_then(|value| value.as_str()) == Some("MODEL")
        && value.get("type").and_then(|value| value.as_str()) == Some("PLANNER_RESPONSE")
        && value.get("status").and_then(|value| value.as_str()) == Some("DONE")
}

fn normalize_path_key(path: &Path) -> String {
    normalize_path_text(&path.to_string_lossy())
}

fn normalize_path_text(path: &str) -> String {
    path.replace('\\', "/")
        .trim_end_matches('/')
        .to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use wardian_core::models::{AntigravityProviderConfig, ProviderConfig};

    fn make_provider() -> AntigravityProvider {
        AntigravityProvider::new()
    }

    fn make_antigravity_config(antigravity: AntigravityProviderConfig) -> AgentConfig {
        AgentConfig {
            provider: "antigravity".into(),
            provider_config: ProviderConfig::Antigravity(antigravity),
            ..Default::default()
        }
    }

    #[test]
    fn name_returns_lowercase_antigravity() {
        assert_eq!(make_provider().name(), "antigravity");
    }

    #[test]
    fn instruction_filename_is_agents_md() {
        assert_eq!(make_provider().get_instruction_filename(), "AGENTS.md");
    }

    #[test]
    fn spawn_args_include_context_dirs_sandbox_permissions_and_resume() {
        let provider = make_provider();
        let config = AgentConfig {
            system_include_directories: Some(vec!["common".into(), "class".into()]),
            include_directories: Some(vec!["class".into(), "user".into()]),
            resume_session: Some("conversation-123".into()),
            ..make_antigravity_config(AntigravityProviderConfig {
                sandbox: Some(true),
                dangerously_skip_permissions: Some(true),
                ..Default::default()
            })
        };

        let args = provider.get_spawn_args(&config, true);

        assert_eq!(
            args,
            vec![
                "--add-dir",
                "common",
                "--add-dir",
                "class",
                "--add-dir",
                "user",
                "--sandbox",
                "--dangerously-skip-permissions",
                "--conversation",
                "conversation-123",
            ]
        );
    }

    #[test]
    fn spawn_args_project_hidden_wardian_include_roots_before_add_dir() {
        let provider = make_provider();
        let temp = tempfile::tempdir().expect("temp dir");
        let hidden = temp.path().join(".wardian").join("common");
        std::fs::create_dir_all(hidden.join(".agents").join("skills").join("role-skill"))
            .expect("create skill");
        std::fs::write(hidden.join("AGENTS.md"), "instructions").expect("write agents");
        let config = AgentConfig {
            session_id: "antigravity-session".to_string(),
            system_include_directories: Some(vec![hidden.to_string_lossy().to_string()]),
            ..make_antigravity_config(AntigravityProviderConfig::default())
        };

        let args = provider.get_spawn_args(&config, false);

        assert_eq!(args[0], "--add-dir");
        assert_ne!(args[1], hidden.to_string_lossy());
        let projected = PathBuf::from(&args[1]);
        assert!(projected.join("AGENTS.md").exists());
        assert!(projected
            .join(".agents")
            .join("skills")
            .join("role-skill")
            .exists());
    }

    #[test]
    fn parse_output_maps_transcript_lifecycle() {
        let provider = make_provider();

        assert_eq!(
            provider
                .parse_output(r#"{"type":"USER_INPUT","source":"USER_EXPLICIT"}"#)
                .unwrap(),
            AgentEvent::UserQuery
        );
        assert_eq!(
            provider
                .parse_output(
                    r#"{"source":"MODEL","type":"PLANNER_RESPONSE","status":"DONE","content":"ok"}"#
                )
                .unwrap(),
            AgentEvent::TurnCompleted
        );
    }

    #[test]
    fn summarize_transcript_content_returns_last_model_response() {
        let content = concat!(
            "{\"step_index\":0,\"source\":\"USER_EXPLICIT\",\"type\":\"USER_INPUT\",\"status\":\"DONE\",\"content\":\"hello\"}\n",
            "{\"step_index\":2,\"source\":\"MODEL\",\"type\":\"PLANNER_RESPONSE\",\"status\":\"DONE\",\"content\":\"first\"}\n",
            "{\"step_index\":6,\"source\":\"MODEL\",\"type\":\"PLANNER_RESPONSE\",\"status\":\"DONE\",\"content\":\"second\"}\n",
        );

        let summary = AntigravityProvider::summarize_transcript_content(content);

        assert_eq!(summary.last_text.as_deref(), Some("second"));
        assert_eq!(summary.last_step_index, Some(6));
    }

    #[test]
    fn conversation_for_workspace_reads_cache_with_path_normalization() {
        let temp = tempfile::tempdir().expect("temp dir");
        let home = temp.path();
        std::fs::create_dir_all(home.join("cache")).expect("cache dir");
        std::fs::write(
            home.join("cache").join("last_conversations.json"),
            r#"{"C:\\Project\\Wardian":"conversation-123"}"#,
        )
        .expect("cache");

        let conversation =
            AntigravityProvider::conversation_for_workspace(home, Path::new("C:/Project/Wardian"));

        assert_eq!(conversation.as_deref(), Some("conversation-123"));
    }
}
