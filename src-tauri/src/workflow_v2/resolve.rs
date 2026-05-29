use std::path::{Path, PathBuf};

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
}

/// Resolve a blueprint `agent` reference. `role:`/`class:`/`ephemeral` map to a
/// fresh headless worker on `default_provider`; explicit names are marked as
/// non-ephemeral so the executor can log the deferred live-routing behavior.
pub fn resolve_agent(agent_ref: &str, workspace: &Path, default_provider: &str) -> ResolvedAgent {
    let is_ephemeral = agent_ref.starts_with("role:")
        || agent_ref.starts_with("class:")
        || agent_ref == "ephemeral"
        || agent_ref.is_empty();

    ResolvedAgent {
        provider: default_provider.to_string(),
        cwd: workspace.to_path_buf(),
        session_id: String::new(),
        resume_session: None,
        is_ephemeral,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn role_ref_resolves_to_ephemeral_headless_with_default_provider() {
        let r = resolve_agent("role:Coder", Path::new("/ws"), "codex");
        assert_eq!(r.provider, "codex");
        assert!(r.is_ephemeral);
        assert!(r.session_id.is_empty());
    }

    #[test]
    fn class_ref_is_ephemeral() {
        assert!(resolve_agent("class:Reviewer", Path::new("/ws"), "codex").is_ephemeral);
    }

    #[test]
    fn explicit_name_is_not_ephemeral() {
        let r = resolve_agent("Wardian-Codex", Path::new("/ws"), "codex");
        assert!(!r.is_ephemeral);
    }
}
