#[tauri::command]
pub fn resolve_system_include_directories(class_name: String) -> Vec<String> {
    crate::utils::fs::resolve_system_include_directories(&class_name)
}

#[tauri::command]
pub fn validate_directory_path(path: String) -> bool {
    crate::utils::fs::validate_directory_path(&path)
}
