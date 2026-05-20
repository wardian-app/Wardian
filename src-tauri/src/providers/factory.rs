use crate::providers::antigravity::AntigravityProvider;
use crate::providers::claude::ClaudeProvider;
use crate::providers::codex::CodexProvider;
use crate::providers::gemini::GeminiProvider;
use crate::providers::mock::MockProvider;
use crate::providers::opencode::OpenCodeProvider;
use std::sync::Arc;
use wardian_core::models::provider::AgentProvider;

/// Resolves the correct `AgentProvider` implementation based on the provider name
/// stored in `AgentConfig`.
pub struct ProviderFactory;

impl ProviderFactory {
    /// Returns an `Arc<dyn AgentProvider>` for the given provider name.
    ///
    /// Currently supported: `"gemini"`, `"claude"`, `"codex"`, `"antigravity"`, `"opencode"`, and `"mock"`.
    /// Returns `Err` for unknown provider names.
    pub fn resolve(provider_name: &str) -> Result<Arc<dyn AgentProvider>, String> {
        let lower = provider_name.to_lowercase();
        match lower.as_str() {
            "gemini" => Ok(Arc::new(GeminiProvider::new())),
            "claude" => Ok(Arc::new(ClaudeProvider::new())),
            "codex" => Ok(Arc::new(CodexProvider::new())),
            "antigravity" => Ok(Arc::new(AntigravityProvider::new())),
            "opencode" => Ok(Arc::new(OpenCodeProvider::new())),
            "mock" => Ok(Arc::new(MockProvider::new())),
            other => Err(format!(
                "Unknown provider '{}'. Supported providers: gemini, claude, codex, antigravity, opencode, mock",
                other
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_gemini_succeeds() {
        let provider = ProviderFactory::resolve("gemini");
        assert!(provider.is_ok());
        assert_eq!(provider.unwrap().name(), "Gemini");
    }

    #[test]
    fn resolve_gemini_case_insensitive() {
        assert!(ProviderFactory::resolve("Gemini").is_ok());
        assert!(ProviderFactory::resolve("GEMINI").is_ok());
        assert!(ProviderFactory::resolve("gEmInI").is_ok());
    }

    #[test]
    fn resolve_claude_succeeds() {
        let provider = ProviderFactory::resolve("claude");
        assert!(provider.is_ok());
        assert_eq!(provider.unwrap().name(), "Claude");
    }

    #[test]
    fn resolve_codex_succeeds() {
        let provider = ProviderFactory::resolve("codex");
        assert!(provider.is_ok());
        assert_eq!(provider.unwrap().name(), "Codex");
    }

    #[test]
    fn resolve_opencode_succeeds() {
        let provider = ProviderFactory::resolve("opencode");
        assert!(provider.is_ok());
        assert_eq!(provider.unwrap().name(), "OpenCode");
    }

    #[test]
    fn resolve_antigravity_succeeds() {
        let provider = ProviderFactory::resolve("antigravity");
        assert!(provider.is_ok());
        assert_eq!(provider.unwrap().name(), "antigravity");
    }

    #[test]
    fn resolve_antigravity_case_insensitive() {
        assert!(ProviderFactory::resolve("Antigravity").is_ok());
        assert!(ProviderFactory::resolve("ANTIGRAVITY").is_ok());
    }

    #[test]
    fn resolve_unknown_returns_error() {
        let result = ProviderFactory::resolve("invalid_provider_name");
        assert!(result.is_err());
        match result {
            Err(err) => {
                assert!(err.contains("Unknown provider"));
                assert!(err.contains("invalid_provider_name"));
            }
            Ok(_) => panic!("Expected error for unknown provider"),
        }
    }

    #[test]
    fn resolve_empty_string_returns_error() {
        let result = ProviderFactory::resolve("");
        assert!(result.is_err());
    }

    #[test]
    fn resolved_provider_implements_trait_correctly() {
        let provider = ProviderFactory::resolve("gemini").unwrap();
        // Verify the returned provider actually works
        assert_eq!(provider.get_instruction_filename(), "GEMINI.md");
        let (bin, _) = provider.get_executable();
        assert!(!bin.is_empty());
    }

    #[test]
    fn resolved_codex_provider_uses_agents_md() {
        let provider = ProviderFactory::resolve("codex").unwrap();
        assert_eq!(provider.get_instruction_filename(), "AGENTS.md");
    }

    #[test]
    fn resolved_opencode_provider_uses_agents_md() {
        let provider = ProviderFactory::resolve("opencode").unwrap();
        assert_eq!(provider.get_instruction_filename(), "AGENTS.md");
    }

    #[test]
    fn resolved_antigravity_provider_uses_agents_md() {
        let provider = ProviderFactory::resolve("antigravity").unwrap();
        assert_eq!(provider.get_instruction_filename(), "AGENTS.md");
    }
}
