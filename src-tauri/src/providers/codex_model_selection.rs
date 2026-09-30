use crate::providers::models::ProviderModelCatalog;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexLiveModelSelection {
    pub model: String,
    pub effort: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodexLiveSelectionError {
    DefaultUnavailable(String),
    Invalid(String),
}

impl std::fmt::Display for CodexLiveSelectionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DefaultUnavailable(message) | Self::Invalid(message) => f.write_str(message),
        }
    }
}

/// Resolve the complete concrete pair while retaining whether an unavailable
/// provider default should defer application after persistence.
pub fn resolve_live_selection_for_settings(
    catalog: &ProviderModelCatalog,
    model: Option<&str>,
    effort: Option<&str>,
) -> Result<CodexLiveModelSelection, CodexLiveSelectionError> {
    let selected_model = match model.map(str::trim).filter(|value| !value.is_empty()) {
        Some(model) => catalog
            .models
            .iter()
            .find(|option| option.id == model)
            .ok_or_else(|| {
                CodexLiveSelectionError::Invalid(format!(
                    "Codex model {model} is not present in the live catalog"
                ))
            })?,
        None => catalog
            .models
            .iter()
            .find(|option| option.is_default)
            .ok_or_else(|| {
                CodexLiveSelectionError::DefaultUnavailable(
                    catalog.refresh_error.clone().unwrap_or_else(|| {
                        "Codex catalog has no authoritative default model".to_string()
                    }),
                )
            })?,
    };

    let selected_effort = effort
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| selected_model.default_effort.clone())
        .ok_or_else(|| {
            CodexLiveSelectionError::DefaultUnavailable(format!(
                "Codex model {} did not report a default reasoning effort",
                selected_model.id
            ))
        })?;

    if !selected_model.effort_options.contains(&selected_effort) {
        return Err(CodexLiveSelectionError::Invalid(format!(
            "Codex model {} does not support reasoning effort {}",
            selected_model.id, selected_effort
        )));
    }

    Ok(CodexLiveModelSelection {
        model: selected_model.id.clone(),
        effort: selected_effort,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::models::{ProviderModelCatalog, ProviderModelOption};

    fn catalog() -> ProviderModelCatalog {
        ProviderModelCatalog {
            provider: "codex".to_string(),
            version: Some("codex-cli 0.149.0".to_string()),
            source: "live_catalog".to_string(),
            models: vec![
                ProviderModelOption {
                    id: "gpt-default".to_string(),
                    display_name: "GPT Default".to_string(),
                    effort_options: vec!["low".to_string(), "high".to_string()],
                    default_effort: Some("low".to_string()),
                    is_default: true,
                },
                ProviderModelOption {
                    id: "gpt-target".to_string(),
                    display_name: "GPT Target".to_string(),
                    effort_options: vec![
                        "low".to_string(),
                        "medium".to_string(),
                        "high".to_string(),
                        "xhigh".to_string(),
                        "max".to_string(),
                        "ultra".to_string(),
                    ],
                    default_effort: Some("medium".to_string()),
                    is_default: false,
                },
            ],
            refresh_error: None,
        }
    }

    #[test]
    fn provider_defaults_resolve_to_concrete_picker_choices() {
        assert_eq!(
            resolve_live_selection_for_settings(&catalog(), None, None).expect("resolve defaults"),
            CodexLiveModelSelection {
                model: "gpt-default".to_string(),
                effort: "low".to_string(),
            }
        );
        assert_eq!(
            resolve_live_selection_for_settings(&catalog(), Some("gpt-target"), None)
                .expect("resolve model default"),
            CodexLiveModelSelection {
                model: "gpt-target".to_string(),
                effort: "medium".to_string(),
            }
        );
    }

    #[test]
    fn missing_authoritative_default_is_deferred_by_settings_resolution() {
        let mut catalog = catalog();
        catalog.models[0].is_default = false;
        let error = resolve_live_selection_for_settings(&catalog, None, None)
            .expect_err("missing provider default must remain unresolved");
        assert!(matches!(
            error,
            CodexLiveSelectionError::DefaultUnavailable(_)
        ));
    }
}
