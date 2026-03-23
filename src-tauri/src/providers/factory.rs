use std::sync::Arc;
use crate::models::provider::AgentProvider;
use crate::providers::gemini::GeminiProvider;

/// Resolves the correct `AgentProvider` implementation based on the provider name
/// stored in `AgentConfig`.
pub struct ProviderFactory;

impl ProviderFactory {
    /// Returns an `Arc<dyn AgentProvider>` for the given provider name.
    ///
    /// Currently supported: `"gemini"`.
    /// Returns `Err` for unknown provider names.
    pub fn resolve(provider_name: &str) -> Result<Arc<dyn AgentProvider>, String> {
        match provider_name.to_lowercase().as_str() {
            "gemini" => Ok(Arc::new(GeminiProvider::new())),
            other => Err(format!(
                "Unknown provider '{}'. Supported providers: gemini",
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
    fn resolve_unknown_returns_error() {
        let result = ProviderFactory::resolve("claude");
        assert!(result.is_err());
        match result {
            Err(err) => {
                assert!(err.contains("Unknown provider"));
                assert!(err.contains("claude"));
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
}
