use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::utils::get_wardian_home;

const ONBOARDING_HINTS_FILE: &str = "settings/onboarding.json";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OnboardingHintsState {
    #[serde(default)]
    pub dismissed_hint_ids: Vec<String>,
    #[serde(default = "default_contextual_tips_enabled")]
    pub contextual_tips_enabled: bool,
}

impl Default for OnboardingHintsState {
    fn default() -> Self {
        Self {
            dismissed_hint_ids: Vec::new(),
            contextual_tips_enabled: true,
        }
    }
}

fn default_contextual_tips_enabled() -> bool {
    true
}

pub fn load_onboarding_hints() -> Result<OnboardingHintsState, String> {
    let path = onboarding_hints_path()?;
    load_onboarding_hints_from_path(&path)
}

pub fn dismiss_onboarding_hint(hint_id: String) -> Result<OnboardingHintsState, String> {
    let path = onboarding_hints_path()?;
    dismiss_onboarding_hint_at_path(&path, &hint_id)
}

pub fn set_contextual_tips_enabled(enabled: bool) -> Result<OnboardingHintsState, String> {
    let path = onboarding_hints_path()?;
    let mut state = load_onboarding_hints_from_path(&path).unwrap_or_default();
    state.contextual_tips_enabled = enabled;
    save_onboarding_hints_to_path(&path, &state)
}

pub fn reset_onboarding_hints() -> Result<OnboardingHintsState, String> {
    let path = onboarding_hints_path()?;
    let mut state = load_onboarding_hints_from_path(&path).unwrap_or_default();
    state.dismissed_hint_ids.clear();
    save_onboarding_hints_to_path(&path, &state)
}

fn onboarding_hints_path() -> Result<PathBuf, String> {
    let wardian_home = get_wardian_home().ok_or("Could not find Wardian home")?;
    Ok(wardian_home.join(ONBOARDING_HINTS_FILE))
}

fn load_onboarding_hints_from_path(path: &Path) -> Result<OnboardingHintsState, String> {
    if !path.exists() {
        return Ok(OnboardingHintsState::default());
    }

    let content = std::fs::read_to_string(path).map_err(|error| error.to_string())?;
    let state = serde_json::from_str::<OnboardingHintsState>(&content)
        .map_err(|error| error.to_string())?;
    Ok(normalize_onboarding_state(state))
}

fn dismiss_onboarding_hint_at_path(
    path: &Path,
    hint_id: &str,
) -> Result<OnboardingHintsState, String> {
    let trimmed_hint_id = hint_id.trim();
    if trimmed_hint_id.is_empty() {
        return Err("Missing onboarding hint id".to_string());
    }

    let mut state = load_onboarding_hints_from_path(path).unwrap_or_default();
    state.dismissed_hint_ids.push(trimmed_hint_id.to_string());
    save_onboarding_hints_to_path(path, &state)
}

fn save_onboarding_hints_to_path(
    path: &Path,
    state: &OnboardingHintsState,
) -> Result<OnboardingHintsState, String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }

    let normalized = normalize_onboarding_state(state.clone());
    let content = serde_json::to_string_pretty(&normalized).map_err(|error| error.to_string())?;
    std::fs::write(path, content).map_err(|error| error.to_string())?;
    Ok(normalized)
}

fn normalize_onboarding_state(mut state: OnboardingHintsState) -> OnboardingHintsState {
    state.dismissed_hint_ids = state
        .dismissed_hint_ids
        .into_iter()
        .filter_map(|hint_id| {
            let trimmed = hint_id.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        })
        .collect();
    state.dismissed_hint_ids.sort();
    state.dismissed_hint_ids.dedup();
    state
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dismissed_hints_persist_under_wardian_home() {
        let _guard = crate::utils::wardian_test_env_lock();
        let home = tempfile::tempdir().expect("temp dir");
        unsafe { std::env::set_var("WARDIAN_HOME", home.path()) };

        let saved =
            dismiss_onboarding_hint("spawn-agent-first-run:v1".to_string()).expect("dismiss hint");
        assert_eq!(
            saved.dismissed_hint_ids,
            vec!["spawn-agent-first-run:v1".to_string()]
        );
        assert!(saved.contextual_tips_enabled);

        let loaded = load_onboarding_hints().expect("load hints");
        assert_eq!(loaded, saved);
        assert!(home.path().join("settings/onboarding.json").exists());
    }

    #[test]
    fn dismissed_hints_are_isolated_by_wardian_home() {
        let _guard = crate::utils::wardian_test_env_lock();
        let first_home = tempfile::tempdir().expect("first temp dir");
        let second_home = tempfile::tempdir().expect("second temp dir");

        unsafe { std::env::set_var("WARDIAN_HOME", first_home.path()) };
        dismiss_onboarding_hint("spawn-agent-first-run:v1".to_string()).expect("dismiss hint");

        unsafe { std::env::set_var("WARDIAN_HOME", second_home.path()) };
        let loaded = load_onboarding_hints().expect("load second home");

        assert!(loaded.dismissed_hint_ids.is_empty());
    }

    #[test]
    fn dismissed_hints_are_normalized() {
        let temp = tempfile::tempdir().expect("temp dir");
        let path = temp.path().join("onboarding.json");

        dismiss_onboarding_hint_at_path(&path, " spawn-agent-first-run:v1 ").expect("dismiss hint");
        let saved = save_onboarding_hints_to_path(
            &path,
            &OnboardingHintsState {
                dismissed_hint_ids: vec![
                    "spawn-agent-first-run:v1".to_string(),
                    " ".to_string(),
                    "another-hint:v1".to_string(),
                ],
                contextual_tips_enabled: false,
            },
        )
        .expect("save hints");

        assert_eq!(
            saved.dismissed_hint_ids,
            vec![
                "another-hint:v1".to_string(),
                "spawn-agent-first-run:v1".to_string(),
            ]
        );
        assert_eq!(
            load_onboarding_hints_from_path(&path).expect("load hints"),
            saved
        );
    }

    #[test]
    fn legacy_dismissal_only_state_keeps_contextual_tips_enabled() {
        let temp = tempfile::tempdir().expect("temp dir");
        let path = temp.path().join("onboarding.json");
        std::fs::write(&path, r#"{"dismissed_hint_ids":["hint:v1"]}"#).expect("write legacy state");

        let loaded = load_onboarding_hints_from_path(&path).expect("load legacy state");

        assert_eq!(loaded.dismissed_hint_ids, vec!["hint:v1"]);
        assert!(loaded.contextual_tips_enabled);
    }

    #[test]
    fn disabling_tips_and_reset_preserve_each_other() {
        let _guard = crate::utils::wardian_test_env_lock();
        let home = tempfile::tempdir().expect("temp dir");
        unsafe { std::env::set_var("WARDIAN_HOME", home.path()) };
        dismiss_onboarding_hint("hint:v1".to_string()).expect("dismiss hint");

        let disabled = set_contextual_tips_enabled(false).expect("disable tips");
        assert_eq!(disabled.dismissed_hint_ids, vec!["hint:v1"]);
        assert!(!disabled.contextual_tips_enabled);

        let reset = reset_onboarding_hints().expect("reset hints");

        assert!(reset.dismissed_hint_ids.is_empty());
        assert!(!reset.contextual_tips_enabled);
    }
}
