use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowAgentMode {
    Ephemeral,
    InheritFresh,
    InheritResume,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentSessionPersistence {
    Fresh,
    #[default]
    Resume,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentSessionPersistenceOverride {
    #[default]
    Default,
    Fresh,
    Resume,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgentExecutionPolicy {
    pub mode: WorkflowAgentMode,
}

impl AgentExecutionPolicy {
    pub fn from_legacy_session_type(
        legacy_session_type: Option<&str>,
        explicit_mode: Option<&str>,
    ) -> Self {
        if let Some(mode) = explicit_mode.and_then(parse_workflow_agent_mode) {
            return Self { mode };
        }

        let mode = match legacy_session_type {
            Some("temporary") => WorkflowAgentMode::Ephemeral,
            Some("persistent") => WorkflowAgentMode::InheritFresh,
            _ => WorkflowAgentMode::Ephemeral,
        };

        Self { mode }
    }
}

pub fn parse_workflow_agent_mode(value: &str) -> Option<WorkflowAgentMode> {
    match value {
        "ephemeral" => Some(WorkflowAgentMode::Ephemeral),
        "inherit_fresh" => Some(WorkflowAgentMode::InheritFresh),
        "inherit_resume" => Some(WorkflowAgentMode::InheritResume),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_temporary_maps_to_ephemeral() {
        let resolved = AgentExecutionPolicy::from_legacy_session_type(Some("temporary"), None);

        assert_eq!(resolved.mode, WorkflowAgentMode::Ephemeral);
    }

    #[test]
    fn legacy_persistent_maps_to_inherit_fresh() {
        let resolved = AgentExecutionPolicy::from_legacy_session_type(Some("persistent"), None);

        assert_eq!(resolved.mode, WorkflowAgentMode::InheritFresh);
    }

    #[test]
    fn explicit_mode_wins_over_legacy_session_type() {
        let resolved = AgentExecutionPolicy::from_legacy_session_type(
            Some("persistent"),
            Some("inherit_resume"),
        );

        assert_eq!(resolved.mode, WorkflowAgentMode::InheritResume);
    }
}
