use std::collections::HashMap;
use std::path::{Path, PathBuf};
use wardian_core::models::workflow::{AgentConversationMode, BusyPolicy};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentBinding {
    pub session_id: String,
    pub provider: String,
    pub cwd: PathBuf,
    pub resume_session: Option<String>,
    pub is_live: bool,
    pub is_input_ready: bool,
}

/// A resolved agent reference ready to run headlessly.
#[derive(Debug, Clone)]
pub struct ResolvedAgent {
    pub provider: String,
    pub cwd: PathBuf,
    pub session_id: String,
    pub resume_session: Option<String>,
    /// True for role:/class:/ephemeral workers; false for explicit live agent
    /// names whose live routing is deferred beyond 5a.
    pub is_ephemeral: bool,
    /// True only when the bound profile has a currently writable live PTY.
    pub is_live: bool,
    /// True only when the live PTY is currently input-ready.
    pub is_input_ready: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlannedAgentRoute {
    OpenSession,
    BackgroundResume,
    BackgroundFresh,
    WaitForAgent,
    QueueForAgent,
    SkippedBusy,
    FailedBusy,
}

#[derive(Debug, Clone)]
pub struct AgentRouteInput {
    pub agent_id: String,
    pub conversation: AgentConversationMode,
    pub busy_policy: BusyPolicy,
    pub is_live: bool,
    pub is_input_ready: bool,
    pub has_resume_session: bool,
}

pub fn choose_agent_route(input: AgentRouteInput) -> PlannedAgentRoute {
    match input.conversation {
        AgentConversationMode::FreshBackground => PlannedAgentRoute::BackgroundFresh,
        AgentConversationMode::Current if input.is_live && input.is_input_ready => {
            PlannedAgentRoute::OpenSession
        }
        AgentConversationMode::Current if !input.is_live => PlannedAgentRoute::BackgroundResume,
        AgentConversationMode::Current => match input.busy_policy {
            BusyPolicy::Wait => PlannedAgentRoute::WaitForAgent,
            BusyPolicy::Queue => PlannedAgentRoute::QueueForAgent,
            BusyPolicy::Skip => PlannedAgentRoute::SkippedBusy,
            BusyPolicy::Fail => PlannedAgentRoute::FailedBusy,
        },
    }
}

/// Resolve a blueprint `agent` reference. `role:`/`class:`/`ephemeral` map to a
/// fresh headless worker on `default_provider`; explicit names are marked as
/// non-ephemeral so the executor can log the deferred live-routing behavior.
/// A binding keyed by the role/class NAME overrides the provider this ref runs as.
pub fn resolve_agent(
    agent_ref: &str,
    workspace: &Path,
    default_provider: &str,
    bindings: &HashMap<String, String>,
) -> ResolvedAgent {
    resolve_agent_with_catalog(
        agent_ref,
        workspace,
        default_provider,
        bindings,
        &HashMap::new(),
    )
}

pub fn resolve_agent_with_catalog(
    agent_ref: &str,
    workspace: &Path,
    default_provider: &str,
    bindings: &HashMap<String, String>,
    agents: &HashMap<String, AgentBinding>,
) -> ResolvedAgent {
    let is_ephemeral = agent_ref.starts_with("role:")
        || agent_ref.starts_with("class:")
        || agent_ref == "ephemeral"
        || agent_ref.is_empty();

    let name = agent_ref
        .strip_prefix("role:")
        .or_else(|| agent_ref.strip_prefix("class:"))
        .unwrap_or(agent_ref);

    if let Some(binding) = bindings.get(name) {
        if is_known_provider(binding) {
            return ResolvedAgent {
                provider: binding.clone(),
                cwd: workspace.to_path_buf(),
                session_id: String::new(),
                resume_session: None,
                is_ephemeral,
                is_live: false,
                is_input_ready: false,
            };
        }

        if let Some(agent) = agents.get(binding) {
            return ResolvedAgent {
                provider: agent.provider.clone(),
                cwd: agent.cwd.clone(),
                session_id: agent.session_id.clone(),
                resume_session: agent.resume_session.clone(),
                is_ephemeral: false,
                is_live: agent.is_live,
                is_input_ready: agent.is_input_ready,
            };
        }
    }

    if let Some(agent) = agents.get(agent_ref) {
        return ResolvedAgent {
            provider: agent.provider.clone(),
            cwd: agent.cwd.clone(),
            session_id: agent.session_id.clone(),
            resume_session: agent.resume_session.clone(),
            is_ephemeral: false,
            is_live: agent.is_live,
            is_input_ready: agent.is_input_ready,
        };
    }

    ResolvedAgent {
        provider: default_provider.to_string(),
        cwd: workspace.to_path_buf(),
        session_id: String::new(),
        resume_session: None,
        is_ephemeral,
        is_live: false,
        is_input_ready: false,
    }
}

fn is_known_provider(value: &str) -> bool {
    matches!(
        value,
        "claude" | "codex" | "gemini" | "antigravity" | "opencode" | "mock"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn role_ref_resolves_to_ephemeral_headless_with_default_provider() {
        let r = resolve_agent("role:Coder", Path::new("/ws"), "codex", &HashMap::new());
        assert_eq!(r.provider, "codex");
        assert!(r.is_ephemeral);
        assert!(r.session_id.is_empty());
    }

    #[test]
    fn class_ref_is_ephemeral() {
        assert!(
            resolve_agent("class:Reviewer", Path::new("/ws"), "codex", &HashMap::new())
                .is_ephemeral
        );
    }

    #[test]
    fn explicit_name_is_not_ephemeral() {
        let r = resolve_agent("Wardian-Codex", Path::new("/ws"), "codex", &HashMap::new());
        assert!(!r.is_ephemeral);
    }

    #[test]
    fn binding_overrides_provider_for_a_role() {
        use std::collections::HashMap;
        let mut b = HashMap::new();
        b.insert("reasoning_gate".to_string(), "claude".to_string());
        let r = resolve_agent("role:reasoning_gate", Path::new("/ws"), "codex", &b);
        assert_eq!(r.provider, "claude"); // bound
        let r2 = resolve_agent("role:other", Path::new("/ws"), "codex", &b);
        assert_eq!(r2.provider, "codex"); // unbound -> default
    }

    #[test]
    fn binding_to_agent_session_uses_that_agent_runtime() {
        let mut bindings = HashMap::new();
        bindings.insert("reasoning_gate".to_string(), "agent-123".to_string());

        let mut agents = HashMap::new();
        agents.insert(
            "agent-123".to_string(),
            AgentBinding {
                session_id: "agent-123".to_string(),
                provider: "gemini".to_string(),
                cwd: PathBuf::from("/agent-workspace"),
                resume_session: Some("provider-session-456".to_string()),
                is_live: true,
                is_input_ready: true,
            },
        );

        let r = resolve_agent_with_catalog(
            "role:reasoning_gate",
            Path::new("/run-workspace"),
            "codex",
            &bindings,
            &agents,
        );

        assert_eq!(r.provider, "gemini");
        assert_eq!(r.cwd, PathBuf::from("/agent-workspace"));
        assert_eq!(r.session_id, "agent-123");
        assert_eq!(r.resume_session.as_deref(), Some("provider-session-456"));
        assert!(!r.is_ephemeral);
        assert!(r.is_live);
    }

    #[test]
    fn binding_to_offline_agent_session_uses_profile_without_live_routing() {
        let mut bindings = HashMap::new();
        bindings.insert("reasoning_gate".to_string(), "agent-123".to_string());

        let mut agents = HashMap::new();
        agents.insert(
            "agent-123".to_string(),
            AgentBinding {
                session_id: "agent-123".to_string(),
                provider: "gemini".to_string(),
                cwd: PathBuf::from("/agent-workspace"),
                resume_session: Some("provider-session-456".to_string()),
                is_live: false,
                is_input_ready: false,
            },
        );

        let r = resolve_agent_with_catalog(
            "role:reasoning_gate",
            Path::new("/run-workspace"),
            "codex",
            &bindings,
            &agents,
        );

        assert_eq!(r.provider, "gemini");
        assert_eq!(r.cwd, PathBuf::from("/agent-workspace"));
        assert_eq!(r.session_id, "agent-123");
        assert_eq!(r.resume_session.as_deref(), Some("provider-session-456"));
        assert!(!r.is_ephemeral);
        assert!(!r.is_live);
    }

    #[test]
    fn unbound_role_stays_headless_on_default_provider() {
        let r = resolve_agent_with_catalog(
            "role:reasoning_gate",
            Path::new("/run-workspace"),
            "codex",
            &HashMap::new(),
            &HashMap::new(),
        );

        assert_eq!(r.provider, "codex");
        assert_eq!(r.cwd, PathBuf::from("/run-workspace"));
        assert!(r.session_id.is_empty());
        assert!(r.resume_session.is_none());
        assert!(r.is_ephemeral);
    }

    #[test]
    fn busy_live_current_conversation_does_not_use_background_resume() {
        let route = choose_agent_route(AgentRouteInput {
            agent_id: "agent-1".into(),
            conversation: AgentConversationMode::Current,
            busy_policy: BusyPolicy::Skip,
            is_live: true,
            is_input_ready: false,
            has_resume_session: true,
        });
        assert_eq!(route, PlannedAgentRoute::SkippedBusy);
    }

    #[test]
    fn offline_current_conversation_uses_background_resume() {
        let route = choose_agent_route(AgentRouteInput {
            agent_id: "agent-1".into(),
            conversation: AgentConversationMode::Current,
            busy_policy: BusyPolicy::Wait,
            is_live: false,
            is_input_ready: false,
            has_resume_session: true,
        });
        assert_eq!(route, PlannedAgentRoute::BackgroundResume);
    }

    #[test]
    fn fresh_background_uses_profile_without_resume() {
        let route = choose_agent_route(AgentRouteInput {
            agent_id: "agent-1".into(),
            conversation: AgentConversationMode::FreshBackground,
            busy_policy: BusyPolicy::Wait,
            is_live: true,
            is_input_ready: false,
            has_resume_session: true,
        });
        assert_eq!(route, PlannedAgentRoute::BackgroundFresh);
    }
}
