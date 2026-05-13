use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::AgentSessionPersistenceOverride;

fn default_provider() -> String {
    "claude".to_string()
}

fn provider_key(provider: &str) -> &str {
    match provider.trim().to_ascii_lowercase().as_str() {
        "claude" => "claude",
        "gemini" => "gemini",
        "codex" => "codex",
        "opencode" => "opencode",
        "mock" => "mock",
        _ => "",
    }
}

fn provider_type_name(provider: &str) -> String {
    match provider_key(provider) {
        "claude" | "gemini" | "codex" | "opencode" | "mock" => provider_key(provider).to_string(),
        _ => {
            let provider = provider.trim().to_ascii_lowercase();
            if provider.is_empty() {
                "claude".to_string()
            } else {
                provider
            }
        }
    }
}

fn is_known_provider_type(provider: &str) -> bool {
    matches!(
        provider,
        "claude" | "gemini" | "codex" | "opencode" | "mock"
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ProviderConfigEncoding {
    #[default]
    Nested,
    LegacyFlat,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderConfig {
    Claude(ClaudeProviderConfig),
    Gemini(GeminiProviderConfig),
    Codex(CodexProviderConfig),
    OpenCode(OpenCodeProviderConfig),
    Mock(MockProviderConfig),
    Unknown(serde_json::Value),
}

impl Default for ProviderConfig {
    fn default() -> Self {
        Self::Claude(ClaudeProviderConfig::default())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ClaudeProviderConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub permission_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_turns: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allowed_tools: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disallowed_tools: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub append_system_prompt: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mcp_config: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct GeminiProviderConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sandbox: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub yolo: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approval_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub policy: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub experimental_acp: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allowed_mcp_server_names: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub screen_reader: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_format: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct CodexProviderConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sandbox_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approval_policy: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub full_auto: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub search: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skip_git_repo_check: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ephemeral: Option<bool>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cleared_provider_sessions: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct OpenCodeProviderConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct MockProviderConfig {}

impl ProviderConfig {
    pub fn type_name(&self) -> &str {
        match self {
            Self::Claude(_) => "claude",
            Self::Gemini(_) => "gemini",
            Self::Codex(_) => "codex",
            Self::OpenCode(_) => "opencode",
            Self::Mock(_) => "mock",
            Self::Unknown(value) => value
                .get("type")
                .and_then(|kind| kind.as_str())
                .unwrap_or("unknown"),
        }
    }

    fn default_for_provider(provider: &str) -> Self {
        match provider_key(provider) {
            "gemini" => Self::Gemini(GeminiProviderConfig::default()),
            "codex" => Self::Codex(CodexProviderConfig::default()),
            "opencode" => Self::OpenCode(OpenCodeProviderConfig::default()),
            "mock" => Self::Mock(MockProviderConfig::default()),
            "claude" => Self::Claude(ClaudeProviderConfig::default()),
            "" => {
                let provider_type = provider_type_name(provider);
                if provider_type == "claude" {
                    Self::Claude(ClaudeProviderConfig::default())
                } else {
                    Self::Unknown(serde_json::json!({ "type": provider_type }))
                }
            }
            _ => unreachable!("provider_key only returns known provider keys or an empty marker"),
        }
    }

    fn matches_provider(&self, provider: &str) -> bool {
        let provider_type = provider_type_name(provider);
        self.type_name() == provider_type
            || (matches!(self, Self::Unknown(_)) && !is_known_provider_type(&provider_type))
    }

    fn to_value_with_type<T: Serialize>(type_name: &str, config: &T) -> serde_json::Value {
        let mut value = serde_json::to_value(config).unwrap_or_else(|_| serde_json::json!({}));
        if let serde_json::Value::Object(ref mut object) = value {
            object.insert(
                "type".to_string(),
                serde_json::Value::String(type_name.into()),
            );
        }
        value
    }

    fn from_value(value: serde_json::Value) -> Result<Self, serde_json::Error> {
        let provider_type = value
            .get("type")
            .and_then(|kind| kind.as_str())
            .unwrap_or("")
            .to_ascii_lowercase();

        match provider_type.as_str() {
            "claude" => serde_json::from_value::<ClaudeProviderConfig>(value).map(Self::Claude),
            "gemini" => serde_json::from_value::<GeminiProviderConfig>(value).map(Self::Gemini),
            "codex" => serde_json::from_value::<CodexProviderConfig>(value).map(Self::Codex),
            "opencode" => {
                serde_json::from_value::<OpenCodeProviderConfig>(value).map(Self::OpenCode)
            }
            "mock" => serde_json::from_value::<MockProviderConfig>(value).map(Self::Mock),
            _ => Ok(Self::Unknown(value)),
        }
    }
}

impl Serialize for ProviderConfig {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let value = match self {
            Self::Claude(config) => Self::to_value_with_type("claude", config),
            Self::Gemini(config) => Self::to_value_with_type("gemini", config),
            Self::Codex(config) => Self::to_value_with_type("codex", config),
            Self::OpenCode(config) => Self::to_value_with_type("opencode", config),
            Self::Mock(config) => Self::to_value_with_type("mock", config),
            Self::Unknown(value) => value.clone(),
        };
        value.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for ProviderConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;
        Self::from_value(value).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone)]
pub struct AgentConfig {
    pub session_id: String,
    pub session_name: String,
    pub agent_class: String,
    pub folder: String,
    pub git_worktree: Option<bool>,
    pub git_worktree_source: Option<String>,
    pub git_worktree_folder: Option<String>,
    pub resume_session: Option<String>,
    pub is_off: bool,
    pub provider: String,
    pub debug: Option<bool>,
    pub model: Option<String>,
    pub include_directories: Option<Vec<String>>,
    pub system_include_directories: Option<Vec<String>>,
    pub custom_args: Option<String>,
    pub session_persistence: AgentSessionPersistenceOverride,
    pub fresh_provider_session_id: Option<String>,
    pub provider_config: ProviderConfig,
    pub provider_config_encoding: ProviderConfigEncoding,

    // Legacy flat provider fields retained during the transition. New nested
    // configs do not serialize these top-level fields.
    pub sandbox: Option<bool>,
    pub yolo: Option<bool>,
    pub approval_mode: Option<String>,
    pub policy: Option<Vec<String>>,
    pub experimental_acp: Option<bool>,
    pub allowed_mcp_server_names: Option<Vec<String>>,
    pub extensions: Option<Vec<String>>,
    pub screen_reader: Option<bool>,
    pub output_format: Option<String>,
    pub permission_mode: Option<String>,
    pub max_turns: Option<u32>,
    pub allowed_tools: Option<Vec<String>>,
    pub disallowed_tools: Option<Vec<String>>,
    pub append_system_prompt: Option<String>,
    pub mcp_config: Option<String>,
    pub codex_sandbox_mode: Option<String>,
    pub codex_approval_policy: Option<String>,
    pub codex_profile: Option<String>,
    pub codex_full_auto: Option<bool>,
    pub codex_search: Option<bool>,
    pub codex_skip_git_repo_check: Option<bool>,
    pub codex_ephemeral: Option<bool>,
    pub codex_cleared_provider_sessions: Vec<String>,
    pub opencode_agent: Option<String>,
    pub opencode_port: Option<u16>,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            session_id: String::new(),
            session_name: String::new(),
            agent_class: String::new(),
            folder: String::new(),
            git_worktree: None,
            git_worktree_source: None,
            git_worktree_folder: None,
            resume_session: None,
            is_off: false,
            provider: default_provider(),
            debug: None,
            model: None,
            include_directories: None,
            system_include_directories: None,
            custom_args: None,
            session_persistence: AgentSessionPersistenceOverride::Default,
            fresh_provider_session_id: None,
            provider_config: ProviderConfig::Claude(ClaudeProviderConfig::default()),
            provider_config_encoding: ProviderConfigEncoding::Nested,
            sandbox: None,
            yolo: None,
            approval_mode: None,
            policy: None,
            experimental_acp: None,
            allowed_mcp_server_names: None,
            extensions: None,
            screen_reader: None,
            output_format: None,
            permission_mode: None,
            max_turns: None,
            allowed_tools: None,
            disallowed_tools: None,
            append_system_prompt: None,
            mcp_config: None,
            codex_sandbox_mode: None,
            codex_approval_policy: None,
            codex_profile: None,
            codex_full_auto: None,
            codex_search: None,
            codex_skip_git_repo_check: None,
            codex_ephemeral: None,
            codex_cleared_provider_sessions: Vec::new(),
            opencode_agent: None,
            opencode_port: None,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default)]
struct AgentConfigCompat {
    pub session_id: String,
    pub session_name: String,
    pub agent_class: String,
    pub folder: String,
    pub git_worktree: Option<bool>,
    pub git_worktree_source: Option<String>,
    pub git_worktree_folder: Option<String>,
    pub resume_session: Option<String>,
    pub is_off: bool,
    #[serde(default = "default_provider")]
    pub provider: String,
    pub debug: Option<bool>,
    pub model: Option<String>,
    pub include_directories: Option<Vec<String>>,
    pub system_include_directories: Option<Vec<String>>,
    pub custom_args: Option<String>,
    pub session_persistence: AgentSessionPersistenceOverride,
    pub provider_config: Option<serde_json::Value>,
    pub sandbox: Option<bool>,
    pub yolo: Option<bool>,
    pub approval_mode: Option<String>,
    pub policy: Option<Vec<String>>,
    pub experimental_acp: Option<bool>,
    pub allowed_mcp_server_names: Option<Vec<String>>,
    pub extensions: Option<Vec<String>>,
    pub screen_reader: Option<bool>,
    pub output_format: Option<String>,
    pub permission_mode: Option<String>,
    pub max_turns: Option<u32>,
    pub allowed_tools: Option<Vec<String>>,
    pub disallowed_tools: Option<Vec<String>>,
    pub append_system_prompt: Option<String>,
    pub mcp_config: Option<String>,
    pub codex_sandbox_mode: Option<String>,
    pub codex_approval_policy: Option<String>,
    pub codex_profile: Option<String>,
    pub codex_full_auto: Option<bool>,
    pub codex_search: Option<bool>,
    pub codex_skip_git_repo_check: Option<bool>,
    pub codex_ephemeral: Option<bool>,
    pub codex_cleared_provider_sessions: Vec<String>,
    pub opencode_agent: Option<String>,
    pub opencode_port: Option<u16>,
}

impl Default for AgentConfigCompat {
    fn default() -> Self {
        let default = AgentConfig::default();
        Self {
            session_id: default.session_id,
            session_name: default.session_name,
            agent_class: default.agent_class,
            folder: default.folder,
            git_worktree: default.git_worktree,
            git_worktree_source: default.git_worktree_source,
            git_worktree_folder: default.git_worktree_folder,
            resume_session: default.resume_session,
            is_off: default.is_off,
            provider: default.provider,
            debug: default.debug,
            model: default.model,
            include_directories: default.include_directories,
            system_include_directories: default.system_include_directories,
            custom_args: default.custom_args,
            session_persistence: default.session_persistence,
            provider_config: None,
            sandbox: None,
            yolo: None,
            approval_mode: None,
            policy: None,
            experimental_acp: None,
            allowed_mcp_server_names: None,
            extensions: None,
            screen_reader: None,
            output_format: None,
            permission_mode: None,
            max_turns: None,
            allowed_tools: None,
            disallowed_tools: None,
            append_system_prompt: None,
            mcp_config: None,
            codex_sandbox_mode: None,
            codex_approval_policy: None,
            codex_profile: None,
            codex_full_auto: None,
            codex_search: None,
            codex_skip_git_repo_check: None,
            codex_ephemeral: None,
            codex_cleared_provider_sessions: Vec::new(),
            opencode_agent: None,
            opencode_port: None,
        }
    }
}

impl AgentConfigCompat {
    fn legacy_provider_config(&self) -> ProviderConfig {
        match provider_key(&self.provider) {
            "gemini" => ProviderConfig::Gemini(GeminiProviderConfig {
                sandbox: self.sandbox,
                yolo: self.yolo,
                approval_mode: self.approval_mode.clone(),
                policy: self.policy.clone(),
                experimental_acp: self.experimental_acp,
                allowed_mcp_server_names: self.allowed_mcp_server_names.clone(),
                extensions: self.extensions.clone(),
                screen_reader: self.screen_reader,
                output_format: self.output_format.clone(),
            }),
            "codex" => ProviderConfig::Codex(CodexProviderConfig {
                sandbox_mode: self.codex_sandbox_mode.clone(),
                approval_policy: self.codex_approval_policy.clone(),
                profile: self.codex_profile.clone(),
                full_auto: self.codex_full_auto,
                search: self.codex_search,
                skip_git_repo_check: self.codex_skip_git_repo_check,
                ephemeral: self.codex_ephemeral,
                cleared_provider_sessions: self.codex_cleared_provider_sessions.clone(),
            }),
            "opencode" => ProviderConfig::OpenCode(OpenCodeProviderConfig {
                agent: self.opencode_agent.clone(),
                port: self.opencode_port,
            }),
            "mock" => ProviderConfig::Mock(MockProviderConfig::default()),
            "claude" => ProviderConfig::Claude(ClaudeProviderConfig {
                permission_mode: self.permission_mode.clone(),
                max_turns: self.max_turns,
                allowed_tools: self.allowed_tools.clone(),
                disallowed_tools: self.disallowed_tools.clone(),
                append_system_prompt: self.append_system_prompt.clone(),
                mcp_config: self.mcp_config.clone(),
            }),
            "" if self.provider.trim().is_empty() => ProviderConfig::Claude(ClaudeProviderConfig {
                permission_mode: self.permission_mode.clone(),
                max_turns: self.max_turns,
                allowed_tools: self.allowed_tools.clone(),
                disallowed_tools: self.disallowed_tools.clone(),
                append_system_prompt: self.append_system_prompt.clone(),
                mcp_config: self.mcp_config.clone(),
            }),
            "" => ProviderConfig::default_for_provider(&self.provider),
            _ => unreachable!("provider_key only returns known provider keys or an empty marker"),
        }
    }
}

impl From<AgentConfigCompat> for AgentConfig {
    fn from(compat: AgentConfigCompat) -> Self {
        let legacy_provider_config = compat.legacy_provider_config();
        let default_provider_config = ProviderConfig::default_for_provider(&compat.provider);
        let had_nested_provider_config = compat.provider_config.is_some();
        let parsed_provider_config = compat
            .provider_config
            .clone()
            .and_then(|value| ProviderConfig::from_value(value).ok());
        let provider_config = match parsed_provider_config {
            Some(provider_config) if provider_config.matches_provider(&compat.provider) => {
                provider_config
            }
            Some(_) if legacy_provider_config != default_provider_config => legacy_provider_config,
            Some(_) => default_provider_config,
            None => legacy_provider_config,
        };

        let mut config = Self {
            session_id: compat.session_id,
            session_name: compat.session_name,
            agent_class: compat.agent_class,
            folder: compat.folder,
            git_worktree: compat.git_worktree,
            git_worktree_source: compat.git_worktree_source,
            git_worktree_folder: compat.git_worktree_folder,
            resume_session: compat.resume_session,
            is_off: compat.is_off,
            provider: compat.provider,
            debug: compat.debug,
            model: compat.model,
            include_directories: compat.include_directories,
            system_include_directories: compat.system_include_directories,
            custom_args: compat.custom_args,
            session_persistence: compat.session_persistence,
            fresh_provider_session_id: None,
            provider_config,
            provider_config_encoding: if had_nested_provider_config {
                ProviderConfigEncoding::Nested
            } else {
                ProviderConfigEncoding::LegacyFlat
            },
            sandbox: compat.sandbox,
            yolo: compat.yolo,
            approval_mode: compat.approval_mode,
            policy: compat.policy,
            experimental_acp: compat.experimental_acp,
            allowed_mcp_server_names: compat.allowed_mcp_server_names,
            extensions: compat.extensions,
            screen_reader: compat.screen_reader,
            output_format: compat.output_format,
            permission_mode: compat.permission_mode,
            max_turns: compat.max_turns,
            allowed_tools: compat.allowed_tools,
            disallowed_tools: compat.disallowed_tools,
            append_system_prompt: compat.append_system_prompt,
            mcp_config: compat.mcp_config,
            codex_sandbox_mode: compat.codex_sandbox_mode,
            codex_approval_policy: compat.codex_approval_policy,
            codex_profile: compat.codex_profile,
            codex_full_auto: compat.codex_full_auto,
            codex_search: compat.codex_search,
            codex_skip_git_repo_check: compat.codex_skip_git_repo_check,
            codex_ephemeral: compat.codex_ephemeral,
            codex_cleared_provider_sessions: compat.codex_cleared_provider_sessions,
            opencode_agent: compat.opencode_agent,
            opencode_port: compat.opencode_port,
        };
        config.sync_legacy_fields_from_provider_config();
        config
    }
}

impl<'de> Deserialize<'de> for AgentConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        AgentConfigCompat::deserialize(deserializer).map(Self::from)
    }
}

impl AgentConfig {
    pub fn claude_config(&self) -> ClaudeProviderConfig {
        match &self.provider_config {
            ProviderConfig::Claude(config) if provider_key(&self.provider) == "claude" => {
                config.clone()
            }
            _ => ClaudeProviderConfig::default(),
        }
    }

    pub fn gemini_config(&self) -> GeminiProviderConfig {
        match &self.provider_config {
            ProviderConfig::Gemini(config) if provider_key(&self.provider) == "gemini" => {
                config.clone()
            }
            _ => GeminiProviderConfig::default(),
        }
    }

    pub fn codex_config(&self) -> CodexProviderConfig {
        match &self.provider_config {
            ProviderConfig::Codex(config) if provider_key(&self.provider) == "codex" => {
                config.clone()
            }
            _ => CodexProviderConfig::default(),
        }
    }

    pub fn codex_config_mut(&mut self) -> &mut CodexProviderConfig {
        self.codex_config_mut_preserve_encoding()
    }

    pub fn codex_config_mut_preserve_encoding(&mut self) -> &mut CodexProviderConfig {
        let encoding = self.provider_config_encoding;
        if !matches!(self.provider_config, ProviderConfig::Codex(_)) {
            self.provider_config = ProviderConfig::Codex(CodexProviderConfig::default());
        }
        self.provider_config_encoding = encoding;
        match &mut self.provider_config {
            ProviderConfig::Codex(config) => config,
            _ => unreachable!("provider_config was normalized to codex"),
        }
    }

    pub fn opencode_config(&self) -> OpenCodeProviderConfig {
        match &self.provider_config {
            ProviderConfig::OpenCode(config) if provider_key(&self.provider) == "opencode" => {
                config.clone()
            }
            _ => OpenCodeProviderConfig::default(),
        }
    }

    pub fn reset_provider_config_for_provider(&mut self) {
        let encoding = self.provider_config_encoding;
        self.provider_config = ProviderConfig::default_for_provider(&self.provider);
        self.provider_config_encoding = encoding;
        self.sync_legacy_fields_from_provider_config();
    }

    pub fn normalize_provider_config_for_provider(&mut self) {
        if !self.provider_config.matches_provider(&self.provider) {
            self.reset_provider_config_for_provider();
        }
    }

    pub fn mark_provider_config_nested_for_save(&mut self) {
        self.provider_config_encoding = ProviderConfigEncoding::Nested;
        self.sync_legacy_fields_from_provider_config();
    }

    pub fn validate_provider_config_matches_provider(&self) -> Result<(), String> {
        if self.provider_config.matches_provider(&self.provider) {
            Ok(())
        } else {
            Err(format!(
                "provider_config type '{}' does not match provider '{}'",
                self.provider_config.type_name(),
                self.provider
            ))
        }
    }

    fn sync_legacy_fields_from_provider_config(&mut self) {
        self.sandbox = None;
        self.yolo = None;
        self.approval_mode = None;
        self.policy = None;
        self.experimental_acp = None;
        self.allowed_mcp_server_names = None;
        self.extensions = None;
        self.screen_reader = None;
        self.output_format = None;
        self.permission_mode = None;
        self.max_turns = None;
        self.allowed_tools = None;
        self.disallowed_tools = None;
        self.append_system_prompt = None;
        self.mcp_config = None;
        self.codex_sandbox_mode = None;
        self.codex_approval_policy = None;
        self.codex_profile = None;
        self.codex_full_auto = None;
        self.codex_search = None;
        self.codex_skip_git_repo_check = None;
        self.codex_ephemeral = None;
        self.codex_cleared_provider_sessions.clear();
        self.opencode_agent = None;
        self.opencode_port = None;

        match &self.provider_config {
            ProviderConfig::Claude(config) => {
                self.permission_mode = config.permission_mode.clone();
                self.max_turns = config.max_turns;
                self.allowed_tools = config.allowed_tools.clone();
                self.disallowed_tools = config.disallowed_tools.clone();
                self.append_system_prompt = config.append_system_prompt.clone();
                self.mcp_config = config.mcp_config.clone();
            }
            ProviderConfig::Gemini(config) => {
                self.sandbox = config.sandbox;
                self.yolo = config.yolo;
                self.approval_mode = config.approval_mode.clone();
                self.policy = config.policy.clone();
                self.experimental_acp = config.experimental_acp;
                self.allowed_mcp_server_names = config.allowed_mcp_server_names.clone();
                self.extensions = config.extensions.clone();
                self.screen_reader = config.screen_reader;
                self.output_format = config.output_format.clone();
            }
            ProviderConfig::Codex(config) => {
                self.codex_sandbox_mode = config.sandbox_mode.clone();
                self.codex_approval_policy = config.approval_policy.clone();
                self.codex_profile = config.profile.clone();
                self.codex_full_auto = config.full_auto;
                self.codex_search = config.search;
                self.codex_skip_git_repo_check = config.skip_git_repo_check;
                self.codex_ephemeral = config.ephemeral;
                self.codex_cleared_provider_sessions = config.cleared_provider_sessions.clone();
            }
            ProviderConfig::OpenCode(config) => {
                self.opencode_agent = config.agent.clone();
                self.opencode_port = config.port;
            }
            ProviderConfig::Mock(_) | ProviderConfig::Unknown(_) => {}
        }
    }

    fn serialize_shared<S>(&self, map: &mut S) -> Result<(), S::Error>
    where
        S: SerializeMap,
    {
        map.serialize_entry("session_id", &self.session_id)?;
        map.serialize_entry("session_name", &self.session_name)?;
        map.serialize_entry("agent_class", &self.agent_class)?;
        map.serialize_entry("folder", &self.folder)?;
        map.serialize_entry("git_worktree", &self.git_worktree)?;
        map.serialize_entry("git_worktree_source", &self.git_worktree_source)?;
        map.serialize_entry("git_worktree_folder", &self.git_worktree_folder)?;
        map.serialize_entry("resume_session", &self.resume_session)?;
        map.serialize_entry("is_off", &self.is_off)?;
        map.serialize_entry("provider", &self.provider)?;
        map.serialize_entry("debug", &self.debug)?;
        map.serialize_entry("model", &self.model)?;
        map.serialize_entry("include_directories", &self.include_directories)?;
        map.serialize_entry(
            "system_include_directories",
            &self.system_include_directories,
        )?;
        map.serialize_entry("custom_args", &self.custom_args)?;
        map.serialize_entry("session_persistence", &self.session_persistence)?;
        Ok(())
    }

    fn serialize_legacy_provider_fields<S>(&self, map: &mut S) -> Result<(), S::Error>
    where
        S: SerializeMap,
    {
        match provider_key(&self.provider) {
            "gemini" => {
                let config = self.gemini_config();
                map.serialize_entry("sandbox", &config.sandbox)?;
                map.serialize_entry("yolo", &config.yolo)?;
                map.serialize_entry("approval_mode", &config.approval_mode)?;
                map.serialize_entry("policy", &config.policy)?;
                map.serialize_entry("experimental_acp", &config.experimental_acp)?;
                map.serialize_entry("allowed_mcp_server_names", &config.allowed_mcp_server_names)?;
                map.serialize_entry("extensions", &config.extensions)?;
                map.serialize_entry("screen_reader", &config.screen_reader)?;
                map.serialize_entry("output_format", &config.output_format)?;
            }
            "codex" => {
                let config = self.codex_config();
                map.serialize_entry("codex_sandbox_mode", &config.sandbox_mode)?;
                map.serialize_entry("codex_approval_policy", &config.approval_policy)?;
                map.serialize_entry("codex_profile", &config.profile)?;
                map.serialize_entry("codex_full_auto", &config.full_auto)?;
                map.serialize_entry("codex_search", &config.search)?;
                map.serialize_entry("codex_skip_git_repo_check", &config.skip_git_repo_check)?;
                map.serialize_entry("codex_ephemeral", &config.ephemeral)?;
                if !config.cleared_provider_sessions.is_empty() {
                    map.serialize_entry(
                        "codex_cleared_provider_sessions",
                        &config.cleared_provider_sessions,
                    )?;
                }
            }
            "opencode" => {
                let config = self.opencode_config();
                map.serialize_entry("opencode_agent", &config.agent)?;
                map.serialize_entry("opencode_port", &config.port)?;
            }
            _ => {
                let config = self.claude_config();
                map.serialize_entry("permission_mode", &config.permission_mode)?;
                map.serialize_entry("max_turns", &config.max_turns)?;
                map.serialize_entry("allowed_tools", &config.allowed_tools)?;
                map.serialize_entry("disallowed_tools", &config.disallowed_tools)?;
                map.serialize_entry("append_system_prompt", &config.append_system_prompt)?;
                map.serialize_entry("mcp_config", &config.mcp_config)?;
            }
        }
        Ok(())
    }
}

impl Serialize for AgentConfig {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut map = serializer.serialize_map(None)?;
        self.serialize_shared(&mut map)?;
        if self.provider_config_encoding == ProviderConfigEncoding::LegacyFlat {
            self.serialize_legacy_provider_fields(&mut map)?;
        } else {
            map.serialize_entry("provider_config", &self.provider_config)?;
        }
        map.end()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentClassDefinition {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub is_default: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
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
            git_worktree: Some(true),
            git_worktree_source: Some("C:/source-project".into()),
            git_worktree_folder: Some("C:/source-project-worktree".into()),
            resume_session: Some("def-456".into()),
            provider: "codex".into(),
            provider_config: ProviderConfig::Codex(CodexProviderConfig {
                cleared_provider_sessions: vec!["old-codex-session".into()],
                ..Default::default()
            }),
            is_off: true,
            ..Default::default()
        };
        let json = serde_json::to_string(&config).unwrap();
        let deserialized: AgentConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(config.session_id, deserialized.session_id);
        assert_eq!(config.session_name, deserialized.session_name);
        assert_eq!(config.agent_class, deserialized.agent_class);
        assert_eq!(config.folder, deserialized.folder);
        assert_eq!(config.git_worktree, deserialized.git_worktree);
        assert_eq!(config.git_worktree_source, deserialized.git_worktree_source);
        assert_eq!(config.git_worktree_folder, deserialized.git_worktree_folder);
        assert_eq!(config.resume_session, deserialized.resume_session);
        assert_eq!(
            config.codex_config().cleared_provider_sessions,
            deserialized.codex_config().cleared_provider_sessions
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
    fn agent_config_defaults_missing_git_worktree_source_to_none() {
        let config: AgentConfig =
            serde_json::from_str(r#"{"session_id":"abc-123","git_worktree":true}"#).unwrap();

        assert_eq!(config.git_worktree, Some(true));
        assert_eq!(config.git_worktree_source, None);
    }

    #[test]
    fn legacy_flat_codex_fields_deserialize_into_nested_provider_config() {
        let config: AgentConfig = serde_json::from_str(
            r#"{
              "session_id":"agent-1",
              "provider":"codex",
              "codex_sandbox_mode":"workspace-write",
              "codex_approval_policy":"never",
              "codex_profile":"wardian",
              "codex_full_auto":true,
              "codex_search":true,
              "codex_skip_git_repo_check":false,
              "codex_ephemeral":true,
              "codex_cleared_provider_sessions":["old-thread"]
            }"#,
        )
        .unwrap();

        let codex = config.codex_config();
        assert_eq!(codex.sandbox_mode.as_deref(), Some("workspace-write"));
        assert_eq!(codex.approval_policy.as_deref(), Some("never"));
        assert_eq!(codex.profile.as_deref(), Some("wardian"));
        assert_eq!(codex.full_auto, Some(true));
        assert_eq!(codex.search, Some(true));
        assert_eq!(codex.skip_git_repo_check, Some(false));
        assert_eq!(codex.ephemeral, Some(true));
        assert_eq!(codex.cleared_provider_sessions, vec!["old-thread"]);
        assert_eq!(
            config.provider_config_encoding,
            ProviderConfigEncoding::LegacyFlat
        );
    }

    #[test]
    fn legacy_flat_claude_fields_deserialize_into_nested_provider_config() {
        let config: AgentConfig = serde_json::from_str(
            r#"{
              "provider":"claude",
              "permission_mode":"plan",
              "max_turns":4,
              "allowed_tools":["Read"],
              "disallowed_tools":["Bash"],
              "append_system_prompt":"extra",
              "mcp_config":"mcp.json"
            }"#,
        )
        .unwrap();

        let claude = config.claude_config();
        assert_eq!(claude.permission_mode.as_deref(), Some("plan"));
        assert_eq!(claude.max_turns, Some(4));
        assert_eq!(claude.allowed_tools, Some(vec!["Read".into()]));
        assert_eq!(claude.disallowed_tools, Some(vec!["Bash".into()]));
        assert_eq!(claude.append_system_prompt.as_deref(), Some("extra"));
        assert_eq!(claude.mcp_config.as_deref(), Some("mcp.json"));
    }

    #[test]
    fn legacy_flat_gemini_fields_deserialize_into_nested_provider_config() {
        let config: AgentConfig = serde_json::from_str(
            r#"{
              "provider":"gemini",
              "sandbox":true,
              "yolo":true,
              "approval_mode":"plan",
              "policy":["read_only"],
              "experimental_acp":true,
              "allowed_mcp_server_names":["sqlite"],
              "extensions":["github"],
              "screen_reader":true,
              "output_format":"json"
            }"#,
        )
        .unwrap();

        let gemini = config.gemini_config();
        assert_eq!(gemini.sandbox, Some(true));
        assert_eq!(gemini.yolo, Some(true));
        assert_eq!(gemini.approval_mode.as_deref(), Some("plan"));
        assert_eq!(gemini.policy, Some(vec!["read_only".into()]));
        assert_eq!(gemini.allowed_mcp_server_names, Some(vec!["sqlite".into()]));
        assert_eq!(gemini.extensions, Some(vec!["github".into()]));
        assert_eq!(gemini.experimental_acp, Some(true));
        assert_eq!(gemini.screen_reader, Some(true));
        assert_eq!(gemini.output_format.as_deref(), Some("json"));
    }

    #[test]
    fn legacy_flat_opencode_fields_deserialize_into_nested_provider_config() {
        let config: AgentConfig = serde_json::from_str(
            r#"{
              "provider":"opencode",
              "opencode_agent":"build",
              "opencode_port":4096
            }"#,
        )
        .unwrap();

        let opencode = config.opencode_config();
        assert_eq!(opencode.agent.as_deref(), Some("build"));
        assert_eq!(opencode.port, Some(4096));
    }

    #[test]
    fn new_nested_provider_config_serializes_without_legacy_provider_fields() {
        let config = AgentConfig {
            provider: "codex".into(),
            provider_config: ProviderConfig::Codex(CodexProviderConfig {
                sandbox_mode: Some("workspace-write".into()),
                approval_policy: Some("never".into()),
                profile: Some("wardian".into()),
                full_auto: Some(true),
                search: Some(true),
                skip_git_repo_check: Some(false),
                ephemeral: Some(true),
                cleared_provider_sessions: vec!["old-thread".into()],
            }),
            ..Default::default()
        };

        let value = serde_json::to_value(config).unwrap();
        assert!(value.get("provider_config").is_some());
        assert!(value.get("codex_sandbox_mode").is_none());
        assert!(value.get("codex_approval_policy").is_none());
        assert!(value.get("codex_cleared_provider_sessions").is_none());
    }

    #[test]
    fn nested_codex_provider_config_uses_internal_type_tag_contract() {
        let config = AgentConfig {
            provider: "codex".into(),
            provider_config: ProviderConfig::Codex(CodexProviderConfig {
                sandbox_mode: Some("workspace-write".into()),
                approval_policy: Some("never".into()),
                ..Default::default()
            }),
            ..Default::default()
        };

        let value = serde_json::to_value(config).unwrap();
        assert_eq!(value["provider_config"]["type"], "codex");
        assert_eq!(value["provider_config"]["sandbox_mode"], "workspace-write");
        assert_eq!(value["provider_config"]["approval_policy"], "never");
        assert!(value["provider_config"].get("codex").is_none());
    }

    #[test]
    fn nested_provider_config_wins_over_legacy_flat_fields() {
        let config: AgentConfig = serde_json::from_str(
            r#"{
              "provider":"codex",
              "codex_sandbox_mode":"read-only",
              "provider_config":{"type":"codex","sandbox_mode":"workspace-write"}
            }"#,
        )
        .unwrap();

        assert_eq!(
            config.codex_config().sandbox_mode.as_deref(),
            Some("workspace-write")
        );
    }

    #[test]
    fn malformed_known_provider_config_falls_back_to_legacy_flat_fields() {
        let config: AgentConfig = serde_json::from_str(
            r#"{
              "provider":"codex",
              "codex_sandbox_mode":"workspace-write",
              "provider_config":{"sandbox_mode":"read-only"}
            }"#,
        )
        .unwrap();

        assert_eq!(
            config.codex_config().sandbox_mode.as_deref(),
            Some("workspace-write")
        );
    }

    #[test]
    fn malformed_typed_provider_config_falls_back_to_legacy_flat_fields() {
        let config: AgentConfig = serde_json::from_str(
            r#"{
              "provider":"codex",
              "codex_sandbox_mode":"workspace-write",
              "provider_config":{"type":"codex","full_auto":"yes"}
            }"#,
        )
        .unwrap();

        assert_eq!(
            config.codex_config().sandbox_mode.as_deref(),
            Some("workspace-write")
        );
    }

    #[test]
    fn mismatched_provider_config_falls_back_to_legacy_flat_fields() {
        let config: AgentConfig = serde_json::from_str(
            r#"{
              "provider":"codex",
              "codex_sandbox_mode":"workspace-write",
              "provider_config":{"type":"gemini"}
            }"#,
        )
        .unwrap();

        assert_eq!(
            config.codex_config().sandbox_mode.as_deref(),
            Some("workspace-write")
        );
    }

    #[test]
    fn legacy_flat_config_preserves_flat_serialization_until_marked_nested() {
        let mut config: AgentConfig =
            serde_json::from_str(r#"{"provider":"codex","codex_sandbox_mode":"workspace-write"}"#)
                .unwrap();

        let legacy_value = serde_json::to_value(&config).unwrap();
        assert!(legacy_value.get("codex_sandbox_mode").is_some());
        assert!(legacy_value.get("provider_config").is_none());

        config.mark_provider_config_nested_for_save();
        let nested_value = serde_json::to_value(&config).unwrap();
        assert!(nested_value.get("codex_sandbox_mode").is_none());
        assert!(nested_value.get("provider_config").is_some());
    }

    #[test]
    fn legacy_flat_runtime_codex_mutation_preserves_flat_serialization() {
        let mut config: AgentConfig = serde_json::from_str(
            r#"{"provider":"codex","codex_cleared_provider_sessions":["old-thread"]}"#,
        )
        .unwrap();

        config
            .codex_config_mut_preserve_encoding()
            .cleared_provider_sessions
            .push("new-thread".to_string());

        let value = serde_json::to_value(&config).unwrap();
        assert!(value.get("provider_config").is_none());
        assert_eq!(
            value["codex_cleared_provider_sessions"],
            serde_json::json!(["old-thread", "new-thread"])
        );
    }

    #[test]
    fn mock_provider_config_roundtrips() {
        let config = AgentConfig {
            provider: "mock".into(),
            provider_config: ProviderConfig::Mock(MockProviderConfig::default()),
            ..Default::default()
        };

        let serialized = serde_json::to_string(&config).unwrap();
        let deserialized: AgentConfig = serde_json::from_str(&serialized).unwrap();

        assert!(matches!(
            deserialized.provider_config,
            ProviderConfig::Mock(_)
        ));
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
