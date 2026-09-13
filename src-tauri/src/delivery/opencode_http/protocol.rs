use std::fmt;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

const LOOPBACK_HOST: &str = "127.0.0.1";
const OPENCODE_USERNAME: &str = "wardian";

/// Credentials and endpoint arguments that the interactive OpenCode spawn
/// path must apply to one runtime generation.
///
/// The password intentionally has no public accessor. The integration layer
/// may obtain environment entries through [`Self::environment`] but must not
/// include them in logs, receipts, or persisted configuration.
pub struct OpenCodeHttpLaunchPlan {
    generation: u64,
    runtime_generation: u64,
    endpoint: reqwest::Url,
    username: String,
    password: String,
}

impl fmt::Debug for OpenCodeHttpLaunchPlan {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OpenCodeHttpLaunchPlan")
            .field("generation", &self.generation)
            .field("runtime_generation", &self.runtime_generation)
            .field("endpoint", &self.endpoint)
            .field("username", &self.username)
            .field("password", &"<redacted>")
            .finish()
    }
}

impl OpenCodeHttpLaunchPlan {
    /// Create the process-local network overlay for the ordinary OpenCode TUI.
    ///
    /// The port must already have been reserved by the caller. This function
    /// does not bind, probe, or contact a server.
    pub fn new(generation: u64, runtime_generation: u64, port: u16) -> Result<Self, String> {
        if generation == 0 || runtime_generation == 0 {
            return Err("OpenCode HTTP launch generations must be non-zero".to_string());
        }
        if port == 0 {
            return Err("OpenCode HTTP launch requires a reserved non-zero port".to_string());
        }

        let endpoint = reqwest::Url::parse(&format!("http://{LOOPBACK_HOST}:{port}/"))
            .map_err(|_| "OpenCode HTTP loopback endpoint could not be formed".to_string())?;
        Ok(Self {
            generation,
            runtime_generation,
            endpoint,
            username: OPENCODE_USERNAME.to_string(),
            password: uuid::Uuid::new_v4().simple().to_string(),
        })
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn runtime_generation(&self) -> u64 {
        self.runtime_generation
    }

    pub fn endpoint(&self) -> &reqwest::Url {
        &self.endpoint
    }

    /// Arguments appended to the normal OpenCode TUI command.
    ///
    /// The manager must reject the overlay when the user's custom arguments
    /// already select a conflicting network/subcommand mode. It must preserve
    /// the rest of the original argument vector byte-for-byte in order and
    /// meaning.
    pub fn network_args(&self) -> Vec<String> {
        vec![
            "--hostname".to_string(),
            LOOPBACK_HOST.to_string(),
            "--port".to_string(),
            self.endpoint.port().unwrap_or_default().to_string(),
        ]
    }

    /// Child-only environment entries for the normal TUI process.
    pub fn environment(&self) -> [(&str, String); 2] {
        [
            ("OPENCODE_SERVER_USERNAME", self.username.clone()),
            ("OPENCODE_SERVER_PASSWORD", self.password.clone()),
        ]
    }

    pub(crate) fn bind(
        self,
        proof: OpenCodeHttpLaunchProof,
    ) -> Result<OpenCodeHttpBinding, String> {
        OpenCodeHttpBinding::from_launch(self, proof)
    }
}

/// Proof supplied by the interactive spawn owner after it has observed the
/// TUI's listener and exact provider session. The HTTP owner stores this proof
/// and rechecks its generation before every submission.
#[derive(Clone)]
pub struct OpenCodeHttpLaunchProof {
    pub agent_id: String,
    pub provider_session_id: String,
    pub process_identity: String,
    pub listener_identity: String,
    pub workspace: PathBuf,
    pub config_fingerprint: String,
    pub server_version: Option<String>,
}

impl fmt::Debug for OpenCodeHttpLaunchProof {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OpenCodeHttpLaunchProof")
            .field("agent_id", &self.agent_id)
            .field("provider_session_id", &self.provider_session_id)
            .field("process_identity", &self.process_identity)
            .field("listener_identity", &self.listener_identity)
            .field("workspace", &self.workspace)
            .field("config_fingerprint", &self.config_fingerprint)
            .field("server_version", &self.server_version)
            .finish()
    }
}

/// Generation-bound identity for one authenticated OpenCode TUI backend.
pub struct OpenCodeHttpBinding {
    pub(crate) generation: u64,
    pub(crate) runtime_generation: u64,
    pub(crate) endpoint: reqwest::Url,
    pub(crate) username: String,
    pub(crate) password: String,
    pub(crate) agent_id: String,
    pub(crate) provider_session_id: String,
    pub(crate) process_identity: String,
    pub(crate) listener_identity: String,
    pub(crate) workspace: PathBuf,
    pub(crate) config_fingerprint: String,
    pub(crate) expected_server_version: Option<String>,
}

impl fmt::Debug for OpenCodeHttpBinding {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OpenCodeHttpBinding")
            .field("generation", &self.generation)
            .field("runtime_generation", &self.runtime_generation)
            .field("endpoint", &self.endpoint)
            .field("username", &self.username)
            .field("password", &"<redacted>")
            .field("agent_id", &self.agent_id)
            .field("provider_session_id", &self.provider_session_id)
            .field("process_identity", &self.process_identity)
            .field("listener_identity", &self.listener_identity)
            .field("workspace", &self.workspace)
            .field("config_fingerprint", &self.config_fingerprint)
            .field("expected_server_version", &self.expected_server_version)
            .finish()
    }
}

impl OpenCodeHttpBinding {
    fn from_launch(
        plan: OpenCodeHttpLaunchPlan,
        proof: OpenCodeHttpLaunchProof,
    ) -> Result<Self, String> {
        validate_session_id(&proof.provider_session_id)?;
        if proof.agent_id.trim().is_empty()
            || proof.process_identity.trim().is_empty()
            || proof.listener_identity.trim().is_empty()
            || proof.config_fingerprint.trim().is_empty()
        {
            return Err("OpenCode HTTP launch proof is incomplete".to_string());
        }
        if !proof.workspace.is_absolute() {
            return Err("OpenCode HTTP launch proof requires an absolute workspace".to_string());
        }

        Ok(Self {
            generation: plan.generation,
            runtime_generation: plan.runtime_generation,
            endpoint: plan.endpoint,
            username: plan.username,
            password: plan.password,
            agent_id: proof.agent_id,
            provider_session_id: proof.provider_session_id,
            process_identity: proof.process_identity,
            listener_identity: proof.listener_identity,
            workspace: proof.workspace,
            config_fingerprint: proof.config_fingerprint,
            expected_server_version: proof.server_version,
        })
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn runtime_generation(&self) -> u64 {
        self.runtime_generation
    }

    pub fn endpoint(&self) -> &reqwest::Url {
        &self.endpoint
    }

    pub fn agent_id(&self) -> &str {
        &self.agent_id
    }

    pub fn provider_session_id(&self) -> &str {
        &self.provider_session_id
    }

    pub fn process_identity(&self) -> &str {
        &self.process_identity
    }

    pub fn listener_identity(&self) -> &str {
        &self.listener_identity
    }

    pub fn workspace(&self) -> &std::path::Path {
        &self.workspace
    }

    pub fn config_fingerprint(&self) -> &str {
        &self.config_fingerprint
    }
}

pub(crate) fn validate_session_id(session_id: &str) -> Result<(), String> {
    if !session_id.starts_with("ses_")
        || session_id.len() > 256
        || !session_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return Err("OpenCode HTTP binding requires one exact provider session id".to_string());
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize)]
pub struct OpenCodeModel {
    #[serde(rename = "providerID")]
    pub provider_id: String,
    #[serde(rename = "modelID")]
    pub model_id: String,
}

#[derive(Debug, Clone, Default)]
pub struct OpenCodePromptOptions {
    pub model: Option<OpenCodeModel>,
    pub agent: Option<String>,
    pub no_reply: Option<bool>,
    pub system: Option<String>,
    pub tools: Option<std::collections::BTreeMap<String, bool>>,
}

#[derive(Debug, Clone)]
pub struct OpenCodePrompt {
    pub message_id: String,
    pub text: String,
    pub options: OpenCodePromptOptions,
}

impl OpenCodePrompt {
    pub fn new(
        message_id: impl Into<String>,
        text: impl Into<String>,
        options: OpenCodePromptOptions,
    ) -> Result<Self, String> {
        let message_id = message_id.into();
        let text = text.into();
        if message_id.trim().is_empty() {
            return Err("OpenCode prompt requires a message id".to_string());
        }
        if text.is_empty() {
            return Err("OpenCode prompt requires non-empty canonical text".to_string());
        }
        if options
            .agent
            .as_deref()
            .is_some_and(|agent| agent.trim().is_empty())
        {
            return Err("OpenCode prompt agent cannot be blank".to_string());
        }
        Ok(Self {
            message_id,
            text,
            options,
        })
    }

    pub fn body_sha256(&self) -> Result<String, String> {
        let body = self.request_body()?;
        Ok(format!("{:x}", Sha256::digest(body)))
    }

    /// Exact OpenCode 1.18.x `/session/:id/prompt_async` request body.
    pub fn request_body(&self) -> Result<Vec<u8>, String> {
        #[derive(Serialize)]
        struct PromptPart<'a> {
            #[serde(rename = "type")]
            part_type: &'static str,
            text: &'a str,
        }

        #[derive(Serialize)]
        struct PromptAsyncBody<'a> {
            #[serde(rename = "messageID")]
            message_id: &'a str,
            #[serde(skip_serializing_if = "Option::is_none")]
            model: Option<&'a OpenCodeModel>,
            #[serde(skip_serializing_if = "Option::is_none")]
            agent: Option<&'a String>,
            #[serde(rename = "noReply", skip_serializing_if = "Option::is_none")]
            no_reply: Option<bool>,
            #[serde(skip_serializing_if = "Option::is_none")]
            system: Option<&'a String>,
            #[serde(skip_serializing_if = "Option::is_none")]
            tools: Option<&'a std::collections::BTreeMap<String, bool>>,
            parts: [PromptPart<'a>; 1],
        }

        serde_json::to_vec(&PromptAsyncBody {
            message_id: &self.message_id,
            model: self.options.model.as_ref(),
            agent: self.options.agent.as_ref(),
            no_reply: self.options.no_reply,
            system: self.options.system.as_ref(),
            tools: self.options.tools.as_ref(),
            parts: [PromptPart {
                part_type: "text",
                text: &self.text,
            }],
        })
        .map_err(|_| "OpenCode prompt body could not be encoded".to_string())
    }
}

/// One response from OpenCode's `GET /session/:id/message` endpoint.
#[derive(Debug, Clone, Deserialize)]
pub struct OpenCodeStoredMessage {
    pub info: Value,
    #[serde(default)]
    pub parts: Vec<Value>,
}

impl OpenCodeStoredMessage {
    pub fn is_exact_user_prompt(&self, session_id: &str, prompt: &OpenCodePrompt) -> bool {
        self.info.get("id").and_then(Value::as_str) == Some(prompt.message_id.as_str())
            && self.info.get("sessionID").and_then(Value::as_str) == Some(session_id)
            && self.info.get("role").and_then(Value::as_str) == Some("user")
            && self.parts.iter().any(|part| {
                part.get("type").and_then(Value::as_str) == Some("text")
                    && part.get("text").and_then(Value::as_str) == Some(prompt.text.as_str())
            })
    }

    pub fn is_assistant_child_of(&self, session_id: &str, user_message_id: &str) -> bool {
        self.info.get("sessionID").and_then(Value::as_str) == Some(session_id)
            && self.info.get("role").and_then(Value::as_str) == Some("assistant")
            && self.info.get("parentID").and_then(Value::as_str) == Some(user_message_id)
    }
}
