use crate::utils::get_wardian_home;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const APP_SETTINGS_FILE: &str = "settings/app.json";
const MIN_TERMINAL_FONT_SIZE: u8 = 10;
const MAX_TERMINAL_FONT_SIZE: u8 = 20;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileOpenActions {
    pub text: String,
    pub image: String,
    pub pdf: String,
}

impl Default for FileOpenActions {
    fn default() -> Self {
        Self {
            text: "wardian".to_string(),
            image: "wardian".to_string(),
            pdf: "wardian".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AppSettings {
    pub theme: String,
    #[serde(default)]
    pub auto_patch_gemini: bool,
    #[serde(default = "default_terminal_font_size")]
    pub terminal_font_size: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_font_family: Option<String>,
    #[serde(default = "default_grid_card_display_mode")]
    pub grid_card_display_mode: String,
    #[serde(default = "default_watchlist_new_agent_position")]
    pub watchlist_new_agent_position: String,
    #[serde(default = "default_titlebar_telemetry_visible")]
    pub titlebar_telemetry_visible: bool,
    #[serde(default = "default_external_editor")]
    pub external_editor: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_editor_custom_executable: Option<String>,
    #[serde(default)]
    pub file_open_actions: FileOpenActions,
    #[serde(default = "default_workbench_new_tab_action")]
    pub workbench_new_tab_action: String,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grid_card_display_mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub watchlist_new_agent_position: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub titlebar_telemetry_visible: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_editor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_editor_custom_executable: Option<Option<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_open_actions: Option<FileOpenActions>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workbench_new_tab_action: Option<String>,
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
            grid_card_display_mode: default_grid_card_display_mode(),
            watchlist_new_agent_position: default_watchlist_new_agent_position(),
            titlebar_telemetry_visible: default_titlebar_telemetry_visible(),
            external_editor: default_external_editor(),
            external_editor_custom_executable: None,
            file_open_actions: FileOpenActions::default(),
            workbench_new_tab_action: default_workbench_new_tab_action(),
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

fn default_grid_card_display_mode() -> String {
    "terminal".to_string()
}

fn default_watchlist_new_agent_position() -> String {
    "top".to_string()
}

fn default_titlebar_telemetry_visible() -> bool {
    false
}

fn default_external_editor() -> String {
    "system".to_string()
}

fn default_workbench_new_tab_action() -> String {
    "home".to_string()
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
        let legacy_file_click_action = value
            .get("overrides")
            .and_then(|overrides| overrides.get("explorer_file_click_action"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
        let mut persisted =
            serde_json::from_value::<PersistedAppSettings>(value).map_err(|e| e.to_string())?;
        let legacy_present = legacy_file_click_action.is_some();
        if persisted.overrides.file_open_actions.is_none() {
            persisted.overrides.file_open_actions = legacy_file_click_action
                .as_deref()
                .map(|action| file_open_actions_from_legacy(Some(action)))
                .filter(|actions| *actions != FileOpenActions::default());
        }
        let document = app_settings_document_from_overrides(persisted.overrides, true);
        return if legacy_present {
            save_app_settings_document_to_path(path, &document)
        } else {
            Ok(document)
        };
    }

    let legacy_file_click_action = value
        .get("explorer_file_click_action")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    let has_file_open_actions = value.get("file_open_actions").is_some();
    let mut settings = serde_json::from_value::<AppSettings>(value).map_err(|e| e.to_string())?;
    if !has_file_open_actions {
        settings.file_open_actions =
            file_open_actions_from_legacy(legacy_file_click_action.as_deref());
    }
    let normalized = normalize_app_settings(settings);
    let document = AppSettingsDocument {
        schema_version: 2,
        overrides: app_overrides_from_settings(&normalized, &AppSettings::default()),
        settings: normalized,
        persisted: true,
    };
    save_app_settings_document_to_path(path, &document)
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
    wardian_core::conversations::write_json_atomic(path, &persisted).map_err(|e| e.to_string())?;
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
    settings.grid_card_display_mode =
        normalize_grid_card_display_mode(&settings.grid_card_display_mode);
    settings.watchlist_new_agent_position =
        normalize_watchlist_new_agent_position(&settings.watchlist_new_agent_position);
    settings.external_editor = normalize_external_editor(&settings.external_editor);
    settings.external_editor_custom_executable = settings
        .external_editor_custom_executable
        .and_then(|value| trim_to_option(&value));
    settings.file_open_actions = normalize_file_open_actions(settings.file_open_actions);
    settings.workbench_new_tab_action =
        normalize_workbench_new_tab_action(&settings.workbench_new_tab_action);
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
        grid_card_display_mode: overrides
            .grid_card_display_mode
            .clone()
            .unwrap_or(defaults.grid_card_display_mode),
        watchlist_new_agent_position: overrides
            .watchlist_new_agent_position
            .clone()
            .unwrap_or(defaults.watchlist_new_agent_position),
        titlebar_telemetry_visible: overrides
            .titlebar_telemetry_visible
            .unwrap_or(defaults.titlebar_telemetry_visible),
        external_editor: overrides
            .external_editor
            .clone()
            .unwrap_or(defaults.external_editor),
        external_editor_custom_executable: overrides
            .external_editor_custom_executable
            .clone()
            .unwrap_or(defaults.external_editor_custom_executable),
        file_open_actions: overrides
            .file_open_actions
            .clone()
            .unwrap_or(defaults.file_open_actions),
        workbench_new_tab_action: overrides
            .workbench_new_tab_action
            .clone()
            .unwrap_or(defaults.workbench_new_tab_action),
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
    overrides.grid_card_display_mode = overrides
        .grid_card_display_mode
        .map(|mode| normalize_grid_card_display_mode(&mode));
    overrides.watchlist_new_agent_position = overrides
        .watchlist_new_agent_position
        .map(|position| normalize_watchlist_new_agent_position(&position));
    overrides.external_editor = overrides
        .external_editor
        .map(|editor| normalize_external_editor(&editor));
    overrides.external_editor_custom_executable = overrides
        .external_editor_custom_executable
        .map(|executable| executable.and_then(|value| trim_to_option(&value)));
    overrides.file_open_actions = overrides.file_open_actions.map(normalize_file_open_actions);
    overrides.workbench_new_tab_action = overrides.workbench_new_tab_action.and_then(|action| {
        let normalized = normalize_workbench_new_tab_action(&action);
        (normalized != default_workbench_new_tab_action()).then_some(normalized)
    });
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
        grid_card_display_mode: (settings.grid_card_display_mode
            != defaults.grid_card_display_mode)
            .then(|| settings.grid_card_display_mode.clone()),
        watchlist_new_agent_position: (settings.watchlist_new_agent_position
            != defaults.watchlist_new_agent_position)
            .then(|| settings.watchlist_new_agent_position.clone()),
        titlebar_telemetry_visible: (settings.titlebar_telemetry_visible
            != defaults.titlebar_telemetry_visible)
            .then_some(settings.titlebar_telemetry_visible),
        external_editor: (settings.external_editor != defaults.external_editor)
            .then(|| settings.external_editor.clone()),
        external_editor_custom_executable: (settings.external_editor_custom_executable
            != defaults.external_editor_custom_executable)
            .then(|| settings.external_editor_custom_executable.clone()),
        file_open_actions: (settings.file_open_actions != defaults.file_open_actions)
            .then(|| settings.file_open_actions.clone()),
        workbench_new_tab_action: (settings.workbench_new_tab_action
            != defaults.workbench_new_tab_action)
            .then(|| settings.workbench_new_tab_action.clone()),
    }
}

fn normalize_theme(value: &str) -> String {
    match value.trim() {
        "dark" => "dark".to_string(),
        "light" => "light".to_string(),
        _ => "system".to_string(),
    }
}

fn normalize_grid_card_display_mode(value: &str) -> String {
    match value.trim() {
        "chat" => "chat".to_string(),
        _ => "terminal".to_string(),
    }
}

fn normalize_watchlist_new_agent_position(value: &str) -> String {
    match value.trim() {
        "bottom" => "bottom".to_string(),
        _ => "top".to_string(),
    }
}

fn normalize_external_editor(value: &str) -> String {
    match value.trim() {
        "vscode" => "vscode".to_string(),
        "custom" => "custom".to_string(),
        _ => "system".to_string(),
    }
}

fn normalize_file_open_action(value: &str) -> String {
    match value.trim() {
        "external" => "external".to_string(),
        _ => "wardian".to_string(),
    }
}

fn normalize_file_open_actions(actions: FileOpenActions) -> FileOpenActions {
    FileOpenActions {
        text: normalize_file_open_action(&actions.text),
        image: normalize_file_open_action(&actions.image),
        pdf: normalize_file_open_action(&actions.pdf),
    }
}

fn file_open_actions_from_legacy(action: Option<&str>) -> FileOpenActions {
    let target = if action.map(str::trim) == Some("external") {
        "external"
    } else {
        "wardian"
    };
    FileOpenActions {
        text: target.to_string(),
        image: target.to_string(),
        pdf: target.to_string(),
    }
}

fn normalize_workbench_new_tab_action(value: &str) -> String {
    match value.trim() {
        "palette" => "palette".to_string(),
        _ => "home".to_string(),
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
        assert_eq!(settings.grid_card_display_mode, "terminal");
        assert_eq!(settings.watchlist_new_agent_position, "top");
        assert!(!settings.titlebar_telemetry_visible);
        assert_eq!(settings.external_editor, "system");
        assert_eq!(settings.external_editor_custom_executable, None);
        assert_eq!(settings.file_open_actions, FileOpenActions::default());
        assert_eq!(
            serde_json::to_value(&settings).expect("serialize defaults")
                ["workbench_new_tab_action"],
            "home"
        );
    }

    #[test]
    fn app_settings_persists_palette_workbench_new_tab_action_override() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let path = temp_dir.path().join("settings/app.json");
        fs::create_dir_all(path.parent().expect("parent")).expect("create parent");
        fs::write(
            &path,
            r#"{
              "schema_version": 2,
              "overrides": {
                "workbench_new_tab_action": "palette"
              }
            }"#,
        )
        .expect("write settings");

        let document = load_app_settings_document_from_path(&path).expect("load settings");
        let serialized = serde_json::to_value(&document).expect("serialize document");

        assert_eq!(
            serialized["settings"]["workbench_new_tab_action"],
            "palette"
        );
        assert_eq!(
            serialized["overrides"]["workbench_new_tab_action"],
            "palette"
        );
    }

    #[test]
    fn app_settings_normalizes_invalid_workbench_new_tab_action_to_home_without_override() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let path = temp_dir.path().join("settings/app.json");
        fs::create_dir_all(path.parent().expect("parent")).expect("create parent");
        fs::write(
            &path,
            r#"{
              "schema_version": 2,
              "overrides": {
                "workbench_new_tab_action": "cards"
              }
            }"#,
        )
        .expect("write settings");

        let document = load_app_settings_document_from_path(&path).expect("load settings");
        let serialized = serde_json::to_value(&document).expect("serialize document");

        assert_eq!(serialized["settings"]["workbench_new_tab_action"], "home");
        assert!(serialized["overrides"]
            .get("workbench_new_tab_action")
            .is_none());
    }

    #[test]
    fn app_settings_persists_titlebar_telemetry_override() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let path = temp_dir.path().join("settings/app.json");
        let document = AppSettingsDocument {
            schema_version: 2,
            settings: AppSettings::default(),
            overrides: AppSettingsOverrides {
                titlebar_telemetry_visible: Some(false),
                ..AppSettingsOverrides::default()
            },
            persisted: false,
        };

        let saved = save_app_settings_document_to_path(&path, &document).expect("save settings");
        let loaded = load_app_settings_document_from_path(&path).expect("load settings");

        assert!(!saved.settings.titlebar_telemetry_visible);
        assert_eq!(loaded.overrides.titlebar_telemetry_visible, Some(false));
        assert!(!loaded.settings.titlebar_telemetry_visible);
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
            grid_card_display_mode: "chat".to_string(),
            watchlist_new_agent_position: "top".to_string(),
            titlebar_telemetry_visible: true,
            external_editor: "custom".to_string(),
            external_editor_custom_executable: Some("/opt/editor/bin/editor".to_string()),
            file_open_actions: FileOpenActions {
                text: "external".to_string(),
                image: "external".to_string(),
                pdf: "external".to_string(),
            },
            workbench_new_tab_action: "palette".to_string(),
        };

        let saved = save_app_settings_to_path(&path, &settings).expect("save settings");
        let document = load_app_settings_document_from_path(&path).expect("load document");
        let loaded = load_app_settings_from_path(&path).expect("load settings");

        assert!(document.persisted);
        assert_eq!(saved.file_open_actions, settings.file_open_actions);
        assert_eq!(loaded, saved);
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
            grid_card_display_mode: "terminal".to_string(),
            watchlist_new_agent_position: "top".to_string(),
            titlebar_telemetry_visible: true,
            external_editor: "system".to_string(),
            external_editor_custom_executable: None,
            file_open_actions: FileOpenActions::default(),
            workbench_new_tab_action: "home".to_string(),
        };

        save_app_settings_to_path(&path, &settings).expect("save settings");
        let raw = fs::read_to_string(&path).expect("read settings file");
        let json: serde_json::Value = serde_json::from_str(&raw).expect("parse settings file");

        assert_eq!(json["schema_version"], 2);
        assert_eq!(json["overrides"]["theme"], "dark");
        assert!(json["overrides"].get("auto_patch_gemini").is_none());
        assert!(json["overrides"].get("terminal_font_size").is_none());
        assert!(json["overrides"].get("grid_card_display_mode").is_none());
        assert!(json["overrides"]
            .get("explorer_file_click_action")
            .is_none());
    }

    #[test]
    fn app_settings_discards_legacy_explorer_file_click_override() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let path = temp_dir.path().join("settings/app.json");
        fs::create_dir_all(path.parent().expect("parent")).expect("create parent");
        fs::write(
            &path,
            r#"{
              "schema_version": 2,
              "overrides": {
                "explorer_file_click_action": "preview"
              }
            }"#,
        )
        .expect("write settings");

        load_app_settings_document_from_path(&path).expect("load settings");
        let raw = fs::read_to_string(&path).expect("read settings file");
        let json: serde_json::Value = serde_json::from_str(&raw).expect("parse settings file");

        assert!(json["overrides"]
            .get("explorer_file_click_action")
            .is_none());
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
                "terminal_font_family": "JetBrains Mono, monospace",
                "watchlist_new_agent_position": "bottom",
                "external_editor": "vscode",
                "explorer_file_click_action": "external"
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
        assert_eq!(loaded.grid_card_display_mode, "terminal");
        assert_eq!(loaded.watchlist_new_agent_position, "bottom");
        assert_eq!(loaded.external_editor, "vscode");
        assert_eq!(
            loaded.file_open_actions,
            FileOpenActions {
                text: "external".to_string(),
                image: "external".to_string(),
                pdf: "external".to_string(),
            }
        );
        let raw = fs::read_to_string(&path).expect("read migrated settings file");
        let json: serde_json::Value =
            serde_json::from_str(&raw).expect("parse migrated settings file");
        assert!(json["overrides"]
            .get("explorer_file_click_action")
            .is_none());
        assert_eq!(json["overrides"]["file_open_actions"]["text"], "external");
    }

    #[test]
    fn app_settings_migrates_legacy_flat_file_click_action_to_family_preferences() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let path = temp_dir.path().join("settings/app.json");
        fs::create_dir_all(path.parent().expect("parent")).expect("create parent");
        fs::write(
            &path,
            r#"{
              "theme": "system",
              "explorer_file_click_action": "external"
            }"#,
        )
        .expect("write settings");

        let loaded = load_app_settings_from_path(&path).expect("load settings");

        assert_eq!(
            loaded.file_open_actions,
            FileOpenActions {
                text: "external".to_string(),
                image: "external".to_string(),
                pdf: "external".to_string(),
            }
        );
        let raw = fs::read_to_string(&path).expect("read migrated settings file");
        let json: serde_json::Value =
            serde_json::from_str(&raw).expect("parse migrated settings file");
        assert_eq!(json["schema_version"], 2);
        assert!(json["overrides"]
            .get("explorer_file_click_action")
            .is_none());
        assert_eq!(json["overrides"]["file_open_actions"]["text"], "external");
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
              "terminal_font_family": "   ",
              "grid_card_display_mode": "cards",
              "watchlist_new_agent_position": "middle",
              "external_editor": "emacs",
              "external_editor_custom_executable": "   ",
              "explorer_file_click_action": "open"
            }"#,
        )
        .expect("write settings");

        let loaded = load_app_settings_from_path(&path).expect("load settings");

        assert_eq!(loaded.theme, "system");
        assert!(loaded.auto_patch_gemini);
        assert_eq!(loaded.terminal_font_size, 10);
        assert_eq!(loaded.terminal_font_family, None);
        assert_eq!(loaded.grid_card_display_mode, "terminal");
        assert_eq!(loaded.watchlist_new_agent_position, "top");
        assert_eq!(loaded.external_editor, "system");
        assert_eq!(loaded.external_editor_custom_executable, None);
    }
}
