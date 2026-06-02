use serde::Deserialize;
use std::fs;
use std::path::Path;
use wardian_core::models::AgentConfig;
use wardian_core::models::FileNode;

#[derive(Debug, Clone, Deserialize)]
pub struct ExternalEditorLaunchSettings {
    pub external_editor: String,
    pub external_editor_custom_executable: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ExternalEditorLaunchSpec {
    program: String,
    args: Vec<String>,
}

fn resolve_agent_visible_workspace(config: &AgentConfig) -> String {
    if config.git_worktree == Some(true) {
        if let Some(path) = config
            .git_worktree_folder
            .as_deref()
            .map(str::trim)
            .filter(|path| !path.is_empty())
        {
            return path.to_string();
        }
    }
    config.folder.clone()
}

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
            .map(|agent| resolve_agent_visible_workspace(&agent.config.lock().unwrap()))
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
        let args = windows_explorer_args(Path::new(&path));
        std::process::Command::new("explorer")
            .args(args)
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
        let reveal_path = Path::new(&path);
        let open_path = if reveal_path.is_file() {
            reveal_path.parent().unwrap_or(reveal_path)
        } else {
            reveal_path
        };
        std::process::Command::new("xdg-open")
            .arg(open_path)
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
pub async fn open_in_external_editor(
    path: String,
    editor: ExternalEditorLaunchSettings,
) -> Result<(), String> {
    let launch = external_editor_launch(Path::new(&path), &editor)?;
    std::process::Command::new(&launch.program)
        .args(&launch.args)
        .spawn()
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn terminal_link_target_exists(path: String) -> bool {
    Path::new(&path).exists()
}

fn external_editor_launch(
    path: &Path,
    editor: &ExternalEditorLaunchSettings,
) -> Result<ExternalEditorLaunchSpec, String> {
    let path_arg = path.to_string_lossy().into_owned();
    match editor.external_editor.trim() {
        "vscode" => Ok(ExternalEditorLaunchSpec {
            program: vscode_command().to_string(),
            args: vec![path_arg],
        }),
        "custom" => {
            let program = editor
                .external_editor_custom_executable
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| "Custom editor executable is not configured.".to_string())?;
            Ok(ExternalEditorLaunchSpec {
                program: program.to_string(),
                args: vec![path_arg],
            })
        }
        _ => Ok(system_default_open_launch(path_arg)),
    }
}

#[cfg(target_os = "windows")]
fn vscode_command() -> &'static str {
    "code.cmd"
}

#[cfg(not(target_os = "windows"))]
fn vscode_command() -> &'static str {
    "code"
}

#[cfg(target_os = "windows")]
fn system_default_open_launch(path: String) -> ExternalEditorLaunchSpec {
    ExternalEditorLaunchSpec {
        program: "cmd".to_string(),
        args: vec!["/C".to_string(), "start".to_string(), "".to_string(), path],
    }
}

#[cfg(target_os = "macos")]
fn system_default_open_launch(path: String) -> ExternalEditorLaunchSpec {
    ExternalEditorLaunchSpec {
        program: "open".to_string(),
        args: vec![path],
    }
}

#[cfg(target_os = "linux")]
fn system_default_open_launch(path: String) -> ExternalEditorLaunchSpec {
    ExternalEditorLaunchSpec {
        program: "xdg-open".to_string(),
        args: vec![path],
    }
}

#[cfg(target_os = "windows")]
fn windows_explorer_args(path: &Path) -> Vec<String> {
    let normalized_path = path.to_string_lossy().replace('/', "\\");

    if path.is_dir() {
        vec![normalized_path]
    } else {
        vec!["/select,".to_string(), normalized_path]
    }
}

#[tauri::command]
pub async fn read_file_preview(path: String) -> Result<String, String> {
    fs::read_to_string(path).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_agent_visible_workspace_uses_worktree_folder_for_tooling_only() {
        let config = AgentConfig {
            folder: "C:/repo".to_string(),
            git_worktree: Some(true),
            git_worktree_source: Some("C:/repo".to_string()),
            git_worktree_folder: Some("C:/wardian/agents/agent-1/worktree".to_string()),
            ..Default::default()
        };

        assert_eq!(
            resolve_agent_visible_workspace(&config),
            "C:/wardian/agents/agent-1/worktree"
        );
        assert_eq!(config.folder, "C:/repo");
    }

    #[test]
    fn resolve_agent_visible_workspace_falls_back_to_launch_folder_without_worktree_path() {
        let config = AgentConfig {
            folder: "C:/repo".to_string(),
            git_worktree: Some(true),
            ..Default::default()
        };

        assert_eq!(resolve_agent_visible_workspace(&config), "C:/repo");
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_reveal_selects_files_with_normalized_separators() {
        let temp = tempfile::tempdir().expect("temp dir");
        let file_path = temp.path().join("nested/file.txt");
        fs::create_dir_all(file_path.parent().expect("file parent")).expect("create parent");
        fs::write(&file_path, "test").expect("write file");

        let args = windows_explorer_args(Path::new("D:/Development/Wardian/file.txt"));
        assert_eq!(
            args,
            vec![
                "/select,".to_string(),
                "D:\\Development\\Wardian\\file.txt".to_string()
            ]
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_reveal_opens_directories_instead_of_selecting_parent() {
        let temp = tempfile::tempdir().expect("temp dir");
        let dir_path = temp.path().join("workspace");
        fs::create_dir_all(&dir_path).expect("create dir");

        let args = windows_explorer_args(&dir_path);
        assert_eq!(args.len(), 1);
        assert_eq!(args[0], dir_path.to_string_lossy().replace('/', "\\"));
    }

    #[test]
    fn external_editor_launch_uses_system_default_by_default() {
        let launch = external_editor_launch(
            Path::new("/tmp/project/notes.md"),
            &ExternalEditorLaunchSettings {
                external_editor: "system".to_string(),
                external_editor_custom_executable: None,
            },
        )
        .expect("launch spec");

        assert!(launch.args.contains(&"/tmp/project/notes.md".to_string()));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn external_editor_launch_uses_windows_start_for_system_default() {
        let launch = external_editor_launch(
            Path::new("C:/Users/Test Project/notes.md"),
            &ExternalEditorLaunchSettings {
                external_editor: "system".to_string(),
                external_editor_custom_executable: None,
            },
        )
        .expect("launch spec");

        assert_eq!(launch.program, "cmd");
        assert_eq!(
            launch.args,
            vec![
                "/C".to_string(),
                "start".to_string(),
                "".to_string(),
                "C:/Users/Test Project/notes.md".to_string()
            ]
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn external_editor_launch_uses_macos_open_for_system_default() {
        let launch = external_editor_launch(
            Path::new("/tmp/project/notes.md"),
            &ExternalEditorLaunchSettings {
                external_editor: "system".to_string(),
                external_editor_custom_executable: None,
            },
        )
        .expect("launch spec");

        assert_eq!(launch.program, "open");
        assert_eq!(launch.args, vec!["/tmp/project/notes.md".to_string()]);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn external_editor_launch_uses_xdg_open_for_system_default() {
        let launch = external_editor_launch(
            Path::new("/tmp/project/notes.md"),
            &ExternalEditorLaunchSettings {
                external_editor: "system".to_string(),
                external_editor_custom_executable: None,
            },
        )
        .expect("launch spec");

        assert_eq!(launch.program, "xdg-open");
        assert_eq!(launch.args, vec!["/tmp/project/notes.md".to_string()]);
    }

    #[test]
    fn external_editor_launch_uses_vscode_command_when_selected() {
        let launch = external_editor_launch(
            Path::new("/tmp/project/notes.md"),
            &ExternalEditorLaunchSettings {
                external_editor: "vscode".to_string(),
                external_editor_custom_executable: None,
            },
        )
        .expect("launch spec");

        assert!(launch.program == "code" || launch.program == "code.cmd");
        assert_eq!(launch.args, vec!["/tmp/project/notes.md".to_string()]);
    }

    #[test]
    fn external_editor_launch_uses_custom_executable_when_selected() {
        let launch = external_editor_launch(
            Path::new("/tmp/project/notes.md"),
            &ExternalEditorLaunchSettings {
                external_editor: "custom".to_string(),
                external_editor_custom_executable: Some("/opt/editor/bin/editor".to_string()),
            },
        )
        .expect("launch spec");

        assert_eq!(launch.program, "/opt/editor/bin/editor");
        assert_eq!(launch.args, vec!["/tmp/project/notes.md".to_string()]);
    }

    #[test]
    fn external_editor_launch_rejects_custom_without_executable() {
        let error = external_editor_launch(
            Path::new("/tmp/project/notes.md"),
            &ExternalEditorLaunchSettings {
                external_editor: "custom".to_string(),
                external_editor_custom_executable: Some("   ".to_string()),
            },
        )
        .expect_err("custom editor should require executable");

        assert_eq!(error, "Custom editor executable is not configured.");
    }

    #[test]
    fn terminal_link_target_exists_returns_true_for_files_and_directories() {
        let temp = tempfile::tempdir().expect("temp dir");
        let file_path = temp.path().join("notes");
        fs::write(&file_path, "test").expect("write file");

        assert!(terminal_link_target_exists(file_path.to_string_lossy().into_owned()));
        assert!(terminal_link_target_exists(
            temp.path().to_string_lossy().into_owned()
        ));
    }

    #[test]
    fn terminal_link_target_exists_returns_false_for_missing_paths() {
        let temp = tempfile::tempdir().expect("temp dir");
        let missing_path = temp.path().join("missing");

        assert!(!terminal_link_target_exists(
            missing_path.to_string_lossy().into_owned()
        ));
    }
}
