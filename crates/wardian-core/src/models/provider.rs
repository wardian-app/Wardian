use crate::models::AgentConfig;

/// Events emitted by an agent provider during PTY output parsing.
/// Each variant maps a provider-specific JSON event to a universal Wardian event.
#[derive(Debug, Clone, PartialEq)]
pub enum AgentEvent {
    /// The agent session has initialized.
    Init {
        session_id: String,
        timestamp: Option<String>,
    },
    /// A user query was submitted.
    UserQuery,
    /// The provider accepted a turn and assigned it an identity. This is a
    /// lifecycle signal, not another user query.
    TurnStarted { turn_id: String },
    /// The model has finished responding (agent is now idle).
    ModelResponse,
    /// The model is actively generating a response (agent is processing).
    Generating,
    /// The agent's full turn is complete (response + all tool calls finished).
    /// This is the definitive signal the automation engine waits for.
    TurnCompleted,
    /// The provider stopped the current turn without completing it.
    TurnInterrupted,
    /// The agent requires user intervention.
    ActionRequired { message: String },
    /// An unrecognized event type.
    Unknown,
}

/// Qualification for resuming the exact conversation after a workspace change.
///
/// This is independent of ordinary Resume or messaging transport capabilities.
/// Qualification requires observed identity, effective destination cwd and
/// retained history. Runtime-tree shutdown authority is checked separately for
/// the actual running instance before any handover mutation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceHandoverQualification {
    ExactConversationAtDestination,
    Unqualified { reason: &'static str },
}

/// Durable identity that must resume with history rather than same-ID fresh recovery.
/// The roster carries this requirement into delayed provider dispatch and restore.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ExactResumeIdentity {
    pub provider: String,
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<ExactResumeSourceBinding>,
}

/// Inspectable continuation evidence. Adapters own interpretation and verification;
/// the lifecycle only carries the typed receipt across shutdown and publication.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", content = "binding", rename_all = "snake_case")]
pub enum ExactResumeSourceBinding {
    AppendOnlyTranscript(AppendOnlyTranscriptBinding),
    StableProjection(StableProjectionBinding),
    MaterializedStore(MaterializedStoreBinding),
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AppendOnlyTranscriptBinding {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root_selector: Option<std::path::PathBuf>,
    pub config_root: std::path::PathBuf,
    pub canonical_path: std::path::PathBuf,
    pub session_id: String,
    pub native_identity: (String, String),
    pub confirmed_bytes: u64,
    pub prefix_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StableProjectionBinding {
    pub source_path: std::path::PathBuf,
    pub projection_path: std::path::PathBuf,
    pub session_id: String,
    pub native_identity: (String, String),
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ConfirmedResumeRecord {
    pub key: String,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MaterializedStoreBinding {
    pub source_path: std::path::PathBuf,
    pub session_id: String,
    pub native_identity: (String, String),
    pub confirmed_records: Vec<ConfirmedResumeRecord>,
}

/// Read-only launch context selected by the shared owner, never by custom args
/// or a receipt itself. Projection ownership must be checked by the caller.
#[derive(Debug, Clone)]
pub struct ResumeWorkspaceContext {
    pub launch_cwd: std::path::PathBuf,
    pub owned_projection: Option<std::path::PathBuf>,
    pub workspace_changed: bool,
}

/// Abstraction over a specific agent CLI harness (Gemini, Claude, Codex, etc.).
///
/// Implementations encapsulate the provider-specific binary resolution,
/// CLI argument construction, output parsing, and instruction file conventions.
pub trait AgentProvider: Send + Sync {
    /// Returns the human-readable display name (e.g., `"Gemini"`).
    fn name(&self) -> &str;

    /// Fail closed until this adapter's exact-conversation workspace handover
    /// has been qualified. Implementations must not infer this from accepting
    /// a resume argument or advertising a persistent messaging session.
    fn workspace_handover_qualification(&self) -> WorkspaceHandoverQualification {
        WorkspaceHandoverQualification::Unqualified {
            reason: "exact conversation identity, destination cwd and retained history have not been qualified",
        }
    }

    /// Read-only validation before an exact continuation can stop its old writer.
    /// Adapters override this to prove supported options and materialized history.
    /// Launch must repeat validation immediately before crossing the provider boundary.
    fn validate_exact_resume(&self, config: &AgentConfig) -> Result<(), String> {
        config.validate_exact_resume_identity()?;
        match self.workspace_handover_qualification() {
            WorkspaceHandoverQualification::ExactConversationAtDestination => Ok(()),
            WorkspaceHandoverQualification::Unqualified { reason } => Err(format!(
                "Exact continuation is not qualified for provider {}: {reason}",
                config.provider
            )),
        }
    }

    /// Prepare or revalidate adapter-owned source evidence before stopping a writer.
    /// The staged config carries this receipt through drain, restore and dispatch.
    fn prepare_exact_resume(
        &self,
        config: &mut AgentConfig,
        _context: &ResumeWorkspaceContext,
    ) -> Result<(), String> {
        self.validate_exact_resume(config)
    }

    /// Verify the retained receipt after drain and immediately before child launch.
    /// Implementations must not replace a prior receipt with newly discovered authority.
    fn revalidate_exact_resume(
        &self,
        config: &mut AgentConfig,
        _context: &ResumeWorkspaceContext,
    ) -> Result<(), String> {
        self.validate_exact_resume(config)
    }

    /// Validate exact continuation through the native broker's own transport.
    /// Interactive workspace qualification does not imply transport parity.
    fn validate_native_exact_resume(&self, config: &AgentConfig) -> Result<(), String> {
        self.validate_exact_resume(config)
    }

    /// Validate a staged ordinary Resume before disposal. Providers that route
    /// materialized resumes through a stricter launcher must reject here too.
    /// Fresh and pending initialization retain their provider-owned route.
    fn validate_resume_launch(&self, config: &AgentConfig) -> Result<(), String> {
        if config.exact_resume.is_some() {
            self.validate_exact_resume(config)?;
        }
        Ok(())
    }

    /// Resolves the executable binary and any OS-specific base arguments.
    /// Returns `(binary_name, base_args)`.
    fn get_executable(&self) -> (String, Vec<String>);

    /// Builds the full CLI argument list from an `AgentConfig`.
    fn get_spawn_args(&self, config: &AgentConfig, is_resume: bool) -> Vec<String>;

    /// Attempts to parse a raw line of PTY output into a provider-specific `AgentEvent`.
    /// Returns `None` if the line does not contain a recognizable event.
    fn parse_output(&self, line: &str) -> Option<AgentEvent>;

    /// Returns the instruction filename that this provider reads from class
    /// directories (for example, `"AGENTS.md"`).
    fn get_instruction_filename(&self) -> &str;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_event_debug_and_clone() {
        let event = AgentEvent::Init {
            session_id: "abc-123".into(),
            timestamp: Some("2026-01-01T00:00:00Z".into()),
        };
        let cloned = event.clone();
        assert_eq!(event, cloned);
        // Debug formatting should not panic
        let _ = format!("{:?}", event);
    }

    #[test]
    fn agent_event_variants_are_distinguishable() {
        let init = AgentEvent::Init {
            session_id: "s1".into(),
            timestamp: None,
        };
        let query = AgentEvent::UserQuery;
        let response = AgentEvent::ModelResponse;
        let action = AgentEvent::ActionRequired {
            message: "approve".into(),
        };
        let unknown = AgentEvent::Unknown;

        assert_ne!(init, query);
        assert_ne!(query, response);
        assert_ne!(response, action);
        assert_ne!(action, unknown);
    }
}
