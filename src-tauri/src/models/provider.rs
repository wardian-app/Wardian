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
    /// The model has finished responding (agent is now idle).
    ModelResponse,
    /// The model is actively generating a response (agent is processing).
    Generating,
    /// The agent requires user intervention.
    ActionRequired { message: String },
    /// An unrecognized event type.
    Unknown,
}

/// Abstraction over a specific agent CLI harness (Gemini, Claude, Codex, etc.).
///
/// Implementations encapsulate the provider-specific binary resolution,
/// CLI argument construction, output parsing, and instruction file conventions.
pub trait AgentProvider: Send + Sync {
    /// Returns the human-readable display name (e.g., `"Gemini"`).
    fn name(&self) -> &str;

    /// Resolves the executable binary and any OS-specific base arguments.
    /// Returns `(binary_name, base_args)`.
    fn get_executable(&self) -> (String, Vec<String>);

    /// Builds the full CLI argument list from an `AgentConfig`.
    fn get_spawn_args(&self, config: &AgentConfig, is_resume: bool) -> Vec<String>;

    /// Attempts to parse a raw line of PTY output into a provider-specific `AgentEvent`.
    /// Returns `None` if the line does not contain a recognizable event.
    fn parse_output(&self, line: &str) -> Option<AgentEvent>;

    /// Returns the instruction filename that this provider reads from class
    /// directories (e.g., `"GEMINI.md"`, `"CLAUDE.md"`, `"AGENTS.md"`).
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
