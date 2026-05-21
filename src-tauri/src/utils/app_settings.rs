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

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct AppSettingsOverrides {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub theme: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_patch_gemini: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_font_size: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_font_family: Option<Option<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AppSettingsDocument {
    pub schema_version: u8,
    pub settings: AppSettings,
    pub overrides: AppSettingsOverrides,
    #[serde(default)]
    pub persisted: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PersistedAppSettings {
    schema_version: u8,
    #[serde(default)]
    overrides: AppSettingsOverrides,
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
    #[cfg(target_os = "macos")]
    {
        return 12;
    }
    14
}

pub fn load_app_settings() -> Result<AppSettings, String> {
    let path = app_settings_path()?;
    load_app_settings_from_path(&path)
}

pub fn load_app_settings_document() -> Result<AppSettingsDocument, String> {
    let path = app_settings_path()?;
    load_app_settings_document_from_path(&path)
}

pub fn save_app_settings(settings: &AppSettings) -> Result<AppSettings, String> {
    let path = app_settings_path()?;
    save_app_settings_to_path(&path, settings)
}

pub fn save_app_settings_document(
    document: &AppSettingsDocument,
) -> Result<AppSettingsDocument, String> {
    let path = app_settings_path()?;
    save_app_settings_document_to_path(&path, document)
}

fn app_settings_path() -> Result<PathBuf, String> {
    let wardian_home = get_wardian_home().ok_or("Could not find Wardian home")?;
    Ok(wardian_home.join(APP_SETTINGS_FILE))
}

fn load_app_settings_from_path(path: &Path) -> Result<AppSettings, String> {
    load_app_settings_document_from_path(path).map(|document| document.settings)
}

fn load_app_settings_document_from_path(path: &Path) -> Result<AppSettingsDocument, String> {
    if !path.exists() {
        return Ok(app_settings_document_from_overrides(
            AppSettingsOverrides::default(),
            false,
        ));
    }

    let content = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let value = serde_json::from_str::<serde_json::Value>(&content).map_err(|e| e.to_string())?;
    if value
        .get("schema_version")
        .and_then(|version| version.as_u64())
        == Some(2)
    {
        let persisted =
            serde_json::from_value::<PersistedAppSettings>(value).map_err(|e| e.to_string())?;
        return Ok(app_settings_document_from_overrides(
            persisted.overrides,
            true,
        ));
    }

    let settings = serde_json::from_value::<AppSettings>(value).map_err(|e| e.to_string())?;
    let normalized = normalize_app_settings(settings);
    Ok(AppSettingsDocument {
        schema_version: 2,
        overrides: app_overrides_from_settings(&normalized, &AppSettings::default()),
        settings: normalized,
        persisted: true,
    })
}

fn save_app_settings_to_path(path: &Path, settings: &AppSettings) -> Result<AppSettings, String> {
    let normalized = normalize_app_settings(settings.clone());
    let document = AppSettingsDocument {
        schema_version: 2,
        overrides: app_overrides_from_settings(&normalized, &AppSettings::default()),
        settings: normalized,
        persisted: true,
    };
    save_app_settings_document_to_path(path, &document).map(|document| document.settings)
}

fn save_app_settings_document_to_path(
    path: &Path,
    document: &AppSettingsDocument,
) -> Result<AppSettingsDocument, String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }

    let normalized_overrides = normalize_app_overrides(document.overrides.clone());
    let normalized = app_settings_from_overrides(&normalized_overrides);
    let persisted = PersistedAppSettings {
        schema_version: 2,
        overrides: normalized_overrides,
    };
    let content = serde_json::to_string_pretty(&persisted).map_err(|e| e.to_string())?;
    std::fs::write(path, content).map_err(|e| e.to_string())?;
    Ok(AppSettingsDocument {
        schema_version: 2,
        settings: normalized,
        overrides: persisted.overrides,
        persisted: true,
    })
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

fn app_settings_document_from_overrides(
    overrides: AppSettingsOverrides,
    persisted: bool,
) -> AppSettingsDocument {
    let overrides = normalize_app_overrides(overrides);
    AppSettingsDocument {
        schema_version: 2,
        settings: app_settings_from_overrides(&overrides),
        overrides,
        persisted,
    }
}

fn app_settings_from_overrides(overrides: &AppSettingsOverrides) -> AppSettings {
    let defaults = AppSettings::default();
    normalize_app_settings(AppSettings {
        theme: overrides.theme.clone().unwrap_or(defaults.theme),
        auto_patch_gemini: overrides
            .auto_patch_gemini
            .unwrap_or(defaults.auto_patch_gemini),
        terminal_font_size: overrides
            .terminal_font_size
            .unwrap_or(defaults.terminal_font_size),
        terminal_font_family: overrides
            .terminal_font_family
            .clone()
            .unwrap_or(defaults.terminal_font_family),
    })
}

fn normalize_app_overrides(mut overrides: AppSettingsOverrides) -> AppSettingsOverrides {
    overrides.theme = overrides.theme.map(|theme| normalize_theme(&theme));
    overrides.terminal_font_size = overrides
        .terminal_font_size
        .map(|size| size.clamp(MIN_TERMINAL_FONT_SIZE, MAX_TERMINAL_FONT_SIZE));
    overrides.terminal_font_family = overrides
        .terminal_font_family
        .map(|family| family.and_then(|value| trim_to_option(&value)));
    overrides
}

fn app_overrides_from_settings(
    settings: &AppSettings,
    defaults: &AppSettings,
) -> AppSettingsOverrides {
    AppSettingsOverrides {
        theme: (settings.theme != defaults.theme).then(|| settings.theme.clone()),
        auto_patch_gemini: (settings.auto_patch_gemini != defaults.auto_patch_gemini)
            .then_some(settings.auto_patch_gemini),
        terminal_font_size: (settings.terminal_font_size != defaults.terminal_font_size)
            .then_some(settings.terminal_font_size),
        terminal_font_family: (settings.terminal_font_family != defaults.terminal_font_family)
            .then(|| settings.terminal_font_family.clone()),
    }
}

fn normalize_theme(value: &str) -> String {
    match value.trim() {
        "dark" => "dark".to_string(),
        "light" => "light".to_string(),
        _ => "system".to_string(),
    }
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
        let document =
            load_app_settings_document_from_path(&temp_dir.path().join("settings/app.json"))
                .expect("load default document");
        let settings = load_app_settings_from_path(&temp_dir.path().join("settings/app.json"))
            .expect("load defaults");

        assert!(!document.persisted);
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
        let document = load_app_settings_document_from_path(&path).expect("load document");
        let loaded = load_app_settings_from_path(&path).expect("load settings");

        assert!(document.persisted);
        assert_eq!(saved, settings);
        assert_eq!(loaded, settings);
    }

    #[test]
    fn app_settings_writes_sparse_override_document() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let path = temp_dir.path().join("settings/app.json");
        let settings = AppSettings {
            theme: "dark".to_string(),
            auto_patch_gemini: false,
            terminal_font_size: default_terminal_font_size(),
            terminal_font_family: None,
        };

        save_app_settings_to_path(&path, &settings).expect("save settings");
        let raw = fs::read_to_string(&path).expect("read settings file");
        let json: serde_json::Value = serde_json::from_str(&raw).expect("parse settings file");

        assert_eq!(json["schema_version"], 2);
        assert_eq!(json["overrides"]["theme"], "dark");
        assert!(json["overrides"].get("auto_patch_gemini").is_none());
        assert!(json["overrides"].get("terminal_font_size").is_none());
    }

    #[test]
    fn app_settings_loads_sparse_override_document_against_current_defaults() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let path = temp_dir.path().join("settings/app.json");
        fs::create_dir_all(path.parent().expect("parent")).expect("create parent");
        fs::write(
            &path,
            r#"{
              "schema_version": 2,
              "overrides": {
                "terminal_font_family": "JetBrains Mono, monospace"
              }
            }"#,
        )
        .expect("write settings");

        let loaded = load_app_settings_from_path(&path).expect("load settings");

        assert_eq!(loaded.theme, "system");
        assert!(!loaded.auto_patch_gemini);
        assert_eq!(loaded.terminal_font_size, default_terminal_font_size());
        assert_eq!(
            loaded.terminal_font_family,
            Some("JetBrains Mono, monospace".to_string())
        );
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
