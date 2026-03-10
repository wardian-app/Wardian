pub fn get_wardian_home() -> Option<std::path::PathBuf> {
    dirs::home_dir().map(|h| h.join(".wardian"))
}

pub fn resolve_system_include_directories(class_name: &str) -> Vec<String> {
    let mut dirs = Vec::new();
    if let Some(app_dir) = get_wardian_home() {
        let class_path = app_dir.join("classes").join(class_name);
        let common_path = app_dir.join("common");

        if common_path.exists() {
            dirs.push(common_path.to_string_lossy().to_string());
        }
        if class_path.exists() {
            dirs.push(class_path.to_string_lossy().to_string());
        }
    }
    dirs
}

pub fn validate_directory_path(path: &str) -> bool {
    let p = std::path::Path::new(path);
    p.exists() && p.is_dir()
}
