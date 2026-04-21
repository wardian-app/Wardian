use crate::models::FileNode;
use std::fs;
use std::path::Path;

#[tauri::command]
pub fn resolve_system_include_directories(class_name: String, session_id: String) -> Vec<String> {
    crate::utils::fs::resolve_system_include_directories(&class_name, &session_id)
}

#[tauri::command]
pub fn validate_directory_path(path: String) -> bool {
    crate::utils::fs::validate_directory_path(&path)
}

#[tauri::command]
pub async fn get_explorer_root(
    session_id: Option<String>,
    state: tauri::State<'_, crate::state::AppState>,
) -> Result<String, String> {
    if let Some(id) = session_id {
        let agents = state.agents.lock().await;
        agents
            .get(&id)
            .map(|agent| agent.config.lock().unwrap().folder.clone())
            .ok_or_else(|| "Agent not found".to_string())
    } else {
        let app_dir = crate::manager::get_wardian_home().ok_or("No home dir")?;
        Ok(app_dir.to_string_lossy().into_owned())
    }
}

#[tauri::command]
pub async fn get_directory_tree(path: String) -> Result<Vec<FileNode>, String> {
    let mut nodes = Vec::new();
    let dir_path = Path::new(&path);

    if !dir_path.exists() || !dir_path.is_dir() {
        return Err(format!(
            "Path does not exist or is not a directory: {}",
            path
        ));
    }

    let entries = fs::read_dir(dir_path).map_err(|e| e.to_string())?;
    for entry in entries {
        let entry = entry.map_err(|e| e.to_string())?;
        let metadata = entry.metadata().map_err(|e| e.to_string())?;
        let is_dir = metadata.is_dir();
        let name = entry.file_name().to_string_lossy().into_owned();
        let extension = entry
            .path()
            .extension()
            .map(|s| s.to_string_lossy().into_owned());

        nodes.push(FileNode {
            name,
            path: entry.path().to_string_lossy().into_owned(),
            is_dir,
            extension,
        });
    }

    // Sort directories first, then alphabetically
    nodes.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then(a.name.cmp(&b.name)));

    Ok(nodes)
}

#[tauri::command]
pub async fn delete_file(path: String) -> Result<(), String> {
    let path = Path::new(&path);
    if path.is_dir() {
        fs::remove_dir_all(path).map_err(|e| e.to_string())?;
    } else {
        fs::remove_file(path).map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
pub async fn reveal_in_explorer(path: String) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("explorer")
            .args(["/select,", &path])
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .args(["-R", &path])
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open")
            .arg(&path)
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
pub async fn read_file_preview(path: String) -> Result<String, String> {
    fs::read_to_string(path).map_err(|e| e.to_string())
}
