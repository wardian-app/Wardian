use crate::utils::OnboardingHintsState;
use crate::utils::{ShellOption, ShellSettings};
use serde::Serialize;
use wardian_core::models::AgentSessionPersistence;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UpdateEligibility {
    pub enabled: bool,
    pub channel: Option<String>,
    pub reason: Option<String>,
}

pub fn resolve_update_eligibility(
    debug_build: bool,
    update_channel: Option<&str>,
) -> UpdateEligibility {
    let normalized_channel = update_channel
        .map(str::trim)
        .filter(|channel| !channel.is_empty())
        .map(str::to_string);

    if debug_build {
        return UpdateEligibility {
            enabled: false,
            channel: normalized_channel,
            reason: Some("Updates are unavailable in development builds.".to_string()),
        };
    }

    if normalized_channel.as_deref() != Some("stable") {
        return UpdateEligibility {
            enabled: false,
            channel: normalized_channel,
            reason: Some(
                "Updates are only available in official installed release builds.".to_string(),
            ),
        };
    }

    UpdateEligibility {
        enabled: true,
        channel: normalized_channel,
        reason: None,
    }
}

#[cfg(test)]
mod update_eligibility_tests {
    use super::resolve_update_eligibility;

    #[test]
    fn update_eligibility_disables_debug_builds() {
        let eligibility = resolve_update_eligibility(true, Some("stable"));

        assert!(!eligibility.enabled);
        assert_eq!(eligibility.channel.as_deref(), Some("stable"));
        assert_eq!(
            eligibility.reason.as_deref(),
            Some("Updates are unavailable in development builds.")
        );
    }

    #[test]
    fn update_eligibility_disables_unmarked_release_builds() {
        let eligibility = resolve_update_eligibility(false, None);

        assert!(!eligibility.enabled);
        assert_eq!(eligibility.channel, None);
        assert_eq!(
            eligibility.reason.as_deref(),
            Some("Updates are only available in official installed release builds.")
        );
    }

    #[test]
    fn update_eligibility_enables_stable_release_builds() {
        let eligibility = resolve_update_eligibility(false, Some("stable"));

        assert!(eligibility.enabled);
        assert_eq!(eligibility.channel.as_deref(), Some("stable"));
        assert_eq!(eligibility.reason, None);
    }
}

#[tauri::command]
pub fn list_available_shells() -> Result<Vec<ShellOption>, String> {
    Ok(crate::utils::list_available_shells())
}

#[tauri::command]
pub fn get_update_eligibility() -> UpdateEligibility {
    resolve_update_eligibility(cfg!(debug_assertions), option_env!("WARDIAN_UPDATE_CHANNEL"))
}

pub fn update_plugins_enabled_for_current_build() -> bool {
    get_update_eligibility().enabled
}

#[tauri::command]
pub fn load_shell_settings() -> Result<ShellSettings, String> {
    crate::utils::load_shell_settings()
}

#[tauri::command]
pub fn save_shell_settings(settings: ShellSettings) -> Result<ShellSettings, String> {
    crate::utils::save_shell_settings(&settings)
}

#[tauri::command]
pub fn save_agent_session_persistence(
    persistence: AgentSessionPersistence,
) -> Result<ShellSettings, String> {
    crate::utils::save_agent_session_persistence(persistence)
}

#[tauri::command]
pub fn sync_provider_theme_settings(theme: String) -> Result<(), String> {
    crate::utils::sync_provider_theme_settings(&theme)
}

#[tauri::command]
pub fn load_onboarding_hints() -> Result<OnboardingHintsState, String> {
    crate::utils::load_onboarding_hints()
}

#[tauri::command]
pub fn dismiss_onboarding_hint(hint_id: String) -> Result<OnboardingHintsState, String> {
    crate::utils::dismiss_onboarding_hint(hint_id)
}
