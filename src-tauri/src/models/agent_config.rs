use serde::{Deserialize, Serialize};

use super::AgentSessionPersistenceOverride;

fn default_provider() -> String {
    "claude".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AgentConfig {
    #[serde(default)]
    pub session_id: String,
    #[serde(default)]
    pub session_name: String,
    #[serde(default)]
    pub agent_class: String,
    #[serde(default)]
    pub folder: String,
    pub resume_session: Option<String>,
    #[serde(default)]
    pub is_off: bool,
    #[serde(default = "default_provider")]
    pub provider: String,
    #[serde(default)]
    pub debug: Option<bool>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub sandbox: Option<bool>,
    #[serde(default)]
    pub yolo: Option<bool>,
    #[serde(default)]
    pub approval_mode: Option<String>,
    #[serde(default)]
    pub policy: Option<Vec<String>>,
    #[serde(default)]
    pub experimental_acp: Option<bool>,
    #[serde(default)]
    pub allowed_mcp_server_names: Option<Vec<String>>,
    #[serde(default)]
    pub extensions: Option<Vec<String>>,
    #[serde(default)]
    pub include_directories: Option<Vec<String>>,
    #[serde(default)]
    pub system_include_directories: Option<Vec<String>>,
    #[serde(default)]
    pub screen_reader: Option<bool>,
    #[serde(default)]
    pub output_format: Option<String>,
    #[serde(default)]
    pub custom_args: Option<String>,
    #[serde(default)]
    pub session_persistence: AgentSessionPersistenceOverride,
    #[serde(default, skip)]
    pub fresh_provider_session_id: Option<String>,

    // Claude-specific fields
    #[serde(default)]
    pub permission_mode: Option<String>,
    #[serde(default)]
    pub max_turns: Option<u32>,
    #[serde(default)]
    pub allowed_tools: Option<Vec<String>>,
    #[serde(default)]
    pub disallowed_tools: Option<Vec<String>>,
    #[serde(default)]
    pub append_system_prompt: Option<String>,
    #[serde(default)]
    pub mcp_config: Option<String>,

    // Codex-specific fields
    #[serde(default)]
    pub codex_sandbox_mode: Option<String>,
    #[serde(default)]
    pub codex_approval_policy: Option<String>,
    #[serde(default)]
    pub codex_profile: Option<String>,
    #[serde(default)]
    pub codex_full_auto: Option<bool>,
    #[serde(default)]
    pub codex_search: Option<bool>,
    #[serde(default)]
    pub codex_skip_git_repo_check: Option<bool>,
    #[serde(default)]
    pub codex_ephemeral: Option<bool>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub codex_cleared_provider_sessions: Vec<String>,

    // OpenCode-specific fields
    #[serde(default)]
    pub opencode_agent: Option<String>,
    #[serde(default)]
    pub opencode_port: Option<u16>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentClassDefinition {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub is_default: bool,
    #[serde(skip_serializing_if = "Option::is_none", alias = "gemini_md")]
    pub instruction_content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub assigned_skills: Option<Vec<String>>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::AgentSessionPersistenceOverride;

    #[test]
    fn agent_config_serde_roundtrip() {
        let config = AgentConfig {
            session_id: "abc-123".into(),
            session_name: "TestAgent".into(),
            agent_class: "Coder".into(),
            folder: "C:/project".into(),
            resume_session: Some("def-456".into()),
            codex_cleared_provider_sessions: vec!["old-codex-session".into()],
            is_off: true,
            ..Default::default()
        };
        let json = serde_json::to_string(&config).unwrap();
        let deserialized: AgentConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(config.session_id, deserialized.session_id);
        assert_eq!(config.session_name, deserialized.session_name);
        assert_eq!(config.agent_class, deserialized.agent_class);
        assert_eq!(config.folder, deserialized.folder);
        assert_eq!(config.resume_session, deserialized.resume_session);
        assert_eq!(
            config.codex_cleared_provider_sessions,
            deserialized.codex_cleared_provider_sessions
        );
        assert_eq!(config.is_off, deserialized.is_off);
    }

    #[test]
    fn agent_config_defaults_regular_session_persistence_to_global_default() {
        let config: AgentConfig = serde_json::from_str(r#"{"session_id":"abc-123"}"#).unwrap();

        assert_eq!(
            config.session_persistence,
            AgentSessionPersistenceOverride::Default
        );
    }

    #[test]
    fn agent_class_definition_serde_roundtrip() {
        let cls = AgentClassDefinition {
            name: "DevOps".into(),
            description: "Manages CI/CD".into(),
            is_default: false,
            instruction_content: Some("some content".into()),
            assigned_skills: Some(vec!["skill1".into(), "skill2".into()]),
        };
        let json = serde_json::to_string(&cls).unwrap();
        let deserialized: AgentClassDefinition = serde_json::from_str(&json).unwrap();
        assert_eq!(cls.name, deserialized.name);
        assert_eq!(cls.description, deserialized.description);
        assert_eq!(cls.is_default, deserialized.is_default);
        assert_eq!(cls.instruction_content, deserialized.instruction_content);
        assert_eq!(cls.assigned_skills, deserialized.assigned_skills);
    }
}
