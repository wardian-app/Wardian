use std::collections::HashMap;

use crate::models::{
    AgentConversationMode, BusyPolicy, InvocationKind, WorkflowAssignments, WorkflowRoleAssignment,
};

pub fn is_known_provider(value: &str) -> bool {
    matches!(
        value,
        "claude" | "codex" | "gemini" | "antigravity" | "opencode" | "mock"
    )
}

pub fn default_busy_policy_for(invocation: InvocationKind) -> BusyPolicy {
    match invocation {
        InvocationKind::Manual => BusyPolicy::Fail,
        InvocationKind::Scheduled => BusyPolicy::Skip,
    }
}

pub fn normalize_assignments(
    assignments: Option<WorkflowAssignments>,
    legacy_bindings: &HashMap<String, String>,
    invocation: InvocationKind,
) -> WorkflowAssignments {
    let mut normalized = assignments.unwrap_or_default();
    for (role, target) in legacy_bindings {
        normalized.entry(role.clone()).or_insert_with(|| {
            if is_known_provider(target) {
                WorkflowRoleAssignment::TemporaryProvider {
                    provider: target.clone(),
                    workspace: None,
                }
            } else {
                WorkflowRoleAssignment::Agent {
                    agent_id: target.clone(),
                    conversation: AgentConversationMode::Current,
                    busy_policy: default_busy_policy_for(invocation),
                }
            }
        });
    }
    normalized
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn provider_legacy_binding_becomes_temporary_provider() {
        let mut bindings = HashMap::new();
        bindings.insert("summarizer".to_string(), "gemini".to_string());

        let assignments = normalize_assignments(None, &bindings, InvocationKind::Manual);

        assert_eq!(
            assignments.get("summarizer"),
            Some(&WorkflowRoleAssignment::TemporaryProvider {
                provider: "gemini".to_string(),
                workspace: None,
            })
        );
    }

    #[test]
    fn agent_legacy_binding_becomes_current_conversation_with_invocation_default() {
        let mut bindings = HashMap::new();
        bindings.insert("reasoning_gate".to_string(), "agent-123".to_string());

        let manual = normalize_assignments(None, &bindings, InvocationKind::Manual);
        let scheduled = normalize_assignments(None, &bindings, InvocationKind::Scheduled);

        assert_eq!(
            manual.get("reasoning_gate"),
            Some(&WorkflowRoleAssignment::Agent {
                agent_id: "agent-123".to_string(),
                conversation: AgentConversationMode::Current,
                busy_policy: BusyPolicy::Fail,
            })
        );
        assert_eq!(
            scheduled.get("reasoning_gate"),
            Some(&WorkflowRoleAssignment::Agent {
                agent_id: "agent-123".to_string(),
                conversation: AgentConversationMode::Current,
                busy_policy: BusyPolicy::Skip,
            })
        );
    }
}
