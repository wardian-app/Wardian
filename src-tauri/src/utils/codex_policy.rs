use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Official selector for Codex's Windows Computer Use plugin.
pub const COMPUTER_USE_PLUGIN: &str = "computer-use@openai-bundled";

/// A plugin Wardian permits for a Codex agent class.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AllowedCodexPlugin {
    pub selector: String,
    pub requires_apps: bool,
}

impl AllowedCodexPlugin {
    pub fn computer_use() -> Self {
        Self {
            selector: COMPUTER_USE_PLUGIN.to_string(),
            requires_apps: true,
        }
    }
}

/// The effective Codex plugin surface for one Wardian agent class.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct CodexPluginPolicy {
    pub allowed_plugins: Vec<AllowedCodexPlugin>,
}

impl CodexPluginPolicy {
    pub fn requires_apps(&self) -> bool {
        self.allowed_plugins
            .iter()
            .any(|plugin| plugin.requires_apps)
    }

    /// Codex feature disables that retain Wardian's default-deny surface.
    pub fn launch_feature_disables(&self) -> Vec<&'static str> {
        if self.allowed_plugins.is_empty() {
            vec!["plugins", "apps"]
        } else if self.requires_apps() {
            Vec::new()
        } else {
            vec!["apps"]
        }
    }

    /// Stable, non-secret identifier for detecting a policy change at restart.
    pub fn fingerprint(&self) -> String {
        let mut normalized = self.allowed_plugins.clone();
        normalized.sort_by(|left, right| left.selector.cmp(&right.selector));
        let serialized = serde_json::to_vec(&normalized).unwrap_or_default();
        format!("{:x}", Sha256::digest(serialized))
    }
}

/// Resolve the built-in default-deny plugin policy for a Codex agent class.
pub fn resolve_codex_plugin_policy(
    class_name: &str,
    _runtime_policy: &crate::utils::CodexRuntimePolicy,
) -> CodexPluginPolicy {
    let allowed_plugins = match class_name.trim() {
        "Electrical Engineer" | "Mechanical Engineer" => vec![AllowedCodexPlugin::computer_use()],
        _ => Vec::new(),
    };

    CodexPluginPolicy { allowed_plugins }
}

#[cfg(test)]
mod tests {
    use super::{resolve_codex_plugin_policy, AllowedCodexPlugin};
    use crate::utils::CodexRuntimePolicy;

    #[test]
    fn electrical_engineer_allows_computer_use_without_feature_disables() {
        let policy =
            resolve_codex_plugin_policy("Electrical Engineer", &CodexRuntimePolicy::default());

        assert_eq!(
            policy.allowed_plugins,
            vec![AllowedCodexPlugin::computer_use()]
        );
        assert!(policy.launch_feature_disables().is_empty());
    }

    #[test]
    fn coder_is_default_deny() {
        let policy = resolve_codex_plugin_policy("Coder", &CodexRuntimePolicy::default());

        assert!(policy.allowed_plugins.is_empty());
        assert_eq!(policy.launch_feature_disables(), vec!["plugins", "apps"]);
    }
}
