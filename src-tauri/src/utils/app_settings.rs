use crate::utils::get_wardian_home;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const APP_SETTINGS_FILE: &str = "settings/app.json";
const MIN_TERMINAL_FONT_SIZE: u8 = 10;
const MAX_TERMINAL_FONT_SIZE: u8 = 20;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AppSettings {
    pub theme: String,
    #[serde(default)]
    pub auto_patch_gemini: bool,
    #[serde(default = "default_terminal_font_size")]
    pub terminal_font_size: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_font_family: Option<String>,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            theme: "system".to_string(),
            auto_patch_gemini: false,
            terminal_font_size: default_terminal_font_size(),
            terminal_font_family: None,
        }
    }
}

fn default_terminal_font_size() -> u8 {
    14
}

pub fn load_app_settings() -> Result<AppSettings, String> {
    let path = app_settings_path()?;
    load_app_settings_from_path(&path)
}

pub fn save_app_settings(settings: &AppSettings) -> Result<AppSettings, String> {
    let path = app_settings_path()?;
    save_app_settings_to_path(&path, settings)
}

fn app_settings_path() -> Result<PathBuf, String> {
    let wardian_home = get_wardian_home().ok_or("Could not find Wardian home")?;
    Ok(wardian_home.join(APP_SETTINGS_FILE))
}

fn load_app_settings_from_path(path: &Path) -> Result<AppSettings, String> {
    if !path.exists() {
        return Ok(AppSettings::default());
    }

    let content = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let settings = serde_json::from_str::<AppSettings>(&content).map_err(|e| e.to_string())?;
    Ok(normalize_app_settings(settings))
}

fn save_app_settings_to_path(path: &Path, settings: &AppSettings) -> Result<AppSettings, String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }

    let normalized = normalize_app_settings(settings.clone());
    let content = serde_json::to_string_pretty(&normalized).map_err(|e| e.to_string())?;
    std::fs::write(path, content).map_err(|e| e.to_string())?;
    Ok(normalized)
}

fn normalize_app_settings(mut settings: AppSettings) -> AppSettings {
    settings.theme = match settings.theme.trim() {
        "dark" => "dark".to_string(),
        "light" => "light".to_string(),
        _ => "system".to_string(),
    };
    settings.terminal_font_size = settings
        .terminal_font_size
        .clamp(MIN_TERMINAL_FONT_SIZE, MAX_TERMINAL_FONT_SIZE);
    settings.terminal_font_family = settings
        .terminal_font_family
        .and_then(|value| trim_to_option(&value));
    settings
}

fn trim_to_option(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn app_settings_defaults_when_file_missing() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let settings = load_app_settings_from_path(&temp_dir.path().join("settings/app.json"))
            .expect("load defaults");

        assert_eq!(settings.theme, "system");
        assert!(!settings.auto_patch_gemini);
        assert_eq!(settings.terminal_font_size, 14);
        assert_eq!(settings.terminal_font_family, None);
    }

    #[test]
    fn app_settings_round_trip_through_json_file() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let path = temp_dir.path().join("settings/app.json");
        let settings = AppSettings {
            theme: "dark".to_string(),
            auto_patch_gemini: true,
            terminal_font_size: 16,
            terminal_font_family: Some("JetBrains Mono, monospace".to_string()),
        };

        let saved = save_app_settings_to_path(&path, &settings).expect("save settings");
        let loaded = load_app_settings_from_path(&path).expect("load settings");

        assert_eq!(saved, settings);
        assert_eq!(loaded, settings);
    }

    #[test]
    fn app_settings_normalizes_invalid_values() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let path = temp_dir.path().join("settings/app.json");
        fs::create_dir_all(path.parent().expect("parent")).expect("create parent");
        fs::write(
            &path,
            r#"{
              "theme": "blue",
              "auto_patch_gemini": true,
              "terminal_font_size": 4,
              "terminal_font_family": "   "
            }"#,
        )
        .expect("write settings");

        let loaded = load_app_settings_from_path(&path).expect("load settings");

        assert_eq!(loaded.theme, "system");
        assert!(loaded.auto_patch_gemini);
        assert_eq!(loaded.terminal_font_size, 10);
        assert_eq!(loaded.terminal_font_family, None);
    }
}
