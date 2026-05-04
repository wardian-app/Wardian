use crate::utils::{ShellOption, ShellSettings};
use wardian_core::models::AgentSessionPersistence;

#[tauri::command]
pub fn list_available_shells() -> Result<Vec<ShellOption>, String> {
    Ok(crate::utils::list_available_shells())
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
