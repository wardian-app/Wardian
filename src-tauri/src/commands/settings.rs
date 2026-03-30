use crate::utils::{ShellOption, ShellSettings};

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
