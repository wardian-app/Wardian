use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use tauri::AppHandle;

use crate::models::{LibraryFolder, LibraryItemMetadata, LibraryNode, LibraryPrompt};
use crate::utils::fs::get_wardian_home;
use crate::manager::log_debug;

const LIBRARY_PROMPTS_DIR: &str = "library/prompts";
const LIBRARY_METADATA_FILE: &str = "library/library.json";

#[tauri::command]
pub async fn get_library_tree(_app: AppHandle) -> Result<LibraryFolder, String> {
    let wardian_home = get_wardian_home().ok_or("Could not find Wardian home")?;
    let prompts_dir = wardian_home.join(LIBRARY_PROMPTS_DIR);
    let metadata_path = wardian_home.join(LIBRARY_METADATA_FILE);

    if !prompts_dir.exists() {
        if let Err(e) = fs::create_dir_all(&prompts_dir) {
            log_debug(&format!("[Wardian] Failed to create library prompts dir: {}", e));
        }
    }

    let mut metadata_map: HashMap<String, LibraryItemMetadata> = HashMap::new();
    if metadata_path.exists() {
        if let Ok(data) = fs::read_to_string(&metadata_path) {
            if let Ok(map) = serde_json::from_str(&data) {
                metadata_map = map;
            }
        }
    }

    fn build_tree(dir: &Path, base_dir: &Path, metadata_map: &HashMap<String, LibraryItemMetadata>) -> LibraryFolder {
        let mut children = Vec::new();
        let rel_path = dir.strip_prefix(base_dir).unwrap_or(dir).to_string_lossy().replace('\\', "/");
        let name = dir.file_name().unwrap_or_default().to_string_lossy().to_string();

        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    let folder = build_tree(&path, base_dir, metadata_map);
                    children.push(LibraryNode::Folder(folder));
                } else if path.extension().map_or(false, |e| e == "md") {
                    let file_rel_path = path.strip_prefix(base_dir).unwrap_or(&path).to_string_lossy().replace('\\', "/");
                    let file_name = path.file_stem().unwrap_or_default().to_string_lossy().to_string();
                    let content = fs::read_to_string(&path).unwrap_or_default();
                    
                    let metadata = metadata_map.get(&file_rel_path).cloned().unwrap_or_else(|| LibraryItemMetadata {
                        id: uuid::Uuid::new_v4().to_string(),
                        tags: vec![],
                        is_starred: false,
                        last_used: None,
                    });

                    children.push(LibraryNode::Prompt(LibraryPrompt {
                        path: file_rel_path,
                        name: file_name,
                        content,
                        metadata,
                    }));
                }
            }
        }

        LibraryFolder {
            path: rel_path,
            name: if name.is_empty() { "Root".to_string() } else { name },
            children,
        }
    }

    Ok(build_tree(&prompts_dir, &prompts_dir, &metadata_map))
}

#[tauri::command]
pub async fn save_prompt(_app: AppHandle, path: String, content: String, metadata: LibraryItemMetadata) -> Result<(), String> {
    let wardian_home = get_wardian_home().ok_or("Could not find Wardian home")?;
    let prompts_dir = wardian_home.join(LIBRARY_PROMPTS_DIR);
    let file_path = prompts_dir.join(&path);

    if let Some(parent) = file_path.parent() {
        let _ = fs::create_dir_all(parent);
    }

    fs::write(&file_path, content).map_err(|e| e.to_string())?;

    // Update metadata
    let metadata_path = wardian_home.join(LIBRARY_METADATA_FILE);
    let mut metadata_map: HashMap<String, LibraryItemMetadata> = HashMap::new();
    if metadata_path.exists() {
        if let Ok(data) = fs::read_to_string(&metadata_path) {
            if let Ok(map) = serde_json::from_str(&data) {
                metadata_map = map;
            }
        }
    } else if let Some(parent) = metadata_path.parent() {
        let _ = fs::create_dir_all(parent);
    }

    metadata_map.insert(path, metadata);
    let json = serde_json::to_string_pretty(&metadata_map).map_err(|e| e.to_string())?;
    fs::write(&metadata_path, json).map_err(|e| e.to_string())?;

    Ok(())
}

#[tauri::command]
pub async fn update_prompt_metadata(_app: AppHandle, path: String, metadata: LibraryItemMetadata) -> Result<(), String> {
    let wardian_home = get_wardian_home().ok_or("Could not find Wardian home")?;
    let metadata_path = wardian_home.join(LIBRARY_METADATA_FILE);
    
    let mut metadata_map: HashMap<String, LibraryItemMetadata> = HashMap::new();
    if metadata_path.exists() {
        if let Ok(data) = fs::read_to_string(&metadata_path) {
            if let Ok(map) = serde_json::from_str(&data) {
                metadata_map = map;
            }
        }
    } else if let Some(parent) = metadata_path.parent() {
        let _ = fs::create_dir_all(parent);
    }

    metadata_map.insert(path, metadata);
    let json = serde_json::to_string_pretty(&metadata_map).map_err(|e| e.to_string())?;
    fs::write(&metadata_path, json).map_err(|e| e.to_string())?;

    Ok(())
}

#[tauri::command]
pub async fn open_library_folder(_app: AppHandle, path: Option<String>) -> Result<(), String> {
    let wardian_home = get_wardian_home().ok_or("Could not find Wardian home")?;
    let mut target_dir = wardian_home.join(LIBRARY_PROMPTS_DIR);
    if let Some(p) = path {
        if !p.is_empty() {
            target_dir = target_dir.join(p);
        }
    }

    if !target_dir.exists() {
        let _ = fs::create_dir_all(&target_dir);
    }

    #[cfg(target_os = "windows")]
    {
        // On Windows, use Canonicalize to get absolute path for explorer, otherwise it might open Documents
        let abs_path = target_dir.canonicalize().unwrap_or(target_dir);
        std::process::Command::new("explorer")
            .arg(abs_path)
            .spawn()
            .map_err(|e| e.to_string())?;
    }

    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(target_dir)
            .spawn()
            .map_err(|e| e.to_string())?;
    }

    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open")
            .arg(target_dir)
            .spawn()
            .map_err(|e| e.to_string())?;
    }

    Ok(())
}
