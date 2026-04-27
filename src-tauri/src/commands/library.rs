use std::collections::HashMap;
use std::fs;
use std::path::Path;
use tauri::AppHandle;

use crate::manager::log_debug;
use crate::models::{LibraryFolder, LibraryItemMetadata, LibraryNode, LibraryPrompt};
use crate::utils::fs::get_wardian_home;

const LIBRARY_PROMPTS_DIR: &str = "library/prompts";
const LIBRARY_SKILLS_DIR: &str = "library/skills";
const LIBRARY_METADATA_FILE: &str = "library/library.json";

#[tauri::command]
pub async fn get_library_tree(
    _app: AppHandle,
    library_type: String,
) -> Result<LibraryFolder, String> {
    let wardian_home = get_wardian_home().ok_or("Could not find Wardian home")?;

    let target_dir = if library_type == "skills" {
        wardian_home.join(LIBRARY_SKILLS_DIR)
    } else {
        wardian_home.join(LIBRARY_PROMPTS_DIR)
    };

    let metadata_path = wardian_home.join(LIBRARY_METADATA_FILE);

    if !target_dir.exists() {
        if let Err(e) = fs::create_dir_all(&target_dir) {
            log_debug(&format!("[Wardian] Failed to create library dir: {}", e));
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

    fn build_tree(
        dir: &Path,
        base_dir: &Path,
        metadata_map: &HashMap<String, LibraryItemMetadata>,
        is_skills: bool,
    ) -> LibraryFolder {
        let mut children = Vec::new();
        let rel_path = dir
            .strip_prefix(base_dir)
            .unwrap_or(dir)
            .to_string_lossy()
            .replace('\\', "/");
        let name = dir
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();

        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();

                if path.is_dir() {
                    // For skills, check if it's an actual skill (e.g. contains SKILL.md)
                    let is_skill_node = is_skills && path.join("SKILL.md").exists();

                    if is_skill_node {
                        let file_rel_path = path
                            .strip_prefix(base_dir)
                            .unwrap_or(&path)
                            .to_string_lossy()
                            .replace('\\', "/");
                        let file_name = path
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .to_string();
                        let metadata =
                            metadata_map
                                .get(&file_rel_path)
                                .cloned()
                                .unwrap_or_else(|| LibraryItemMetadata {
                                    id: uuid::Uuid::new_v4().to_string(),
                                    tags: vec![],
                                    is_starred: false,
                                    last_used: None,
                                });

                        let content = fs::read_to_string(path.join("SKILL.md")).unwrap_or_default();
                        let description = content.lines().next().unwrap_or("").to_string();

                        children.push(LibraryNode::Skill(crate::models::LibrarySkill {
                            path: file_rel_path,
                            name: file_name,
                            description,
                            content,
                            metadata,
                        }));
                    } else {
                        let folder = build_tree(&path, base_dir, metadata_map, is_skills);
                        children.push(LibraryNode::Folder(folder));
                    }
                } else if !is_skills && path.extension().is_some_and(|e| e == "md") {
                    let file_rel_path = path
                        .strip_prefix(base_dir)
                        .unwrap_or(&path)
                        .to_string_lossy()
                        .replace('\\', "/");
                    let file_name = path
                        .file_stem()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_string();
                    let content = fs::read_to_string(&path).unwrap_or_default();

                    let metadata = metadata_map
                        .get(&file_rel_path)
                        .cloned()
                        .unwrap_or_else(|| LibraryItemMetadata {
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
            name: if name.is_empty() {
                "Root".to_string()
            } else {
                name
            },
            children,
        }
    }

    Ok(build_tree(
        &target_dir,
        &target_dir,
        &metadata_map,
        library_type == "skills",
    ))
}

#[tauri::command]
pub async fn save_library_item(
    _app: AppHandle,
    library_type: String,
    path: String,
    content: String,
    metadata: LibraryItemMetadata,
) -> Result<(), String> {
    let wardian_home = get_wardian_home().ok_or("Could not find Wardian home")?;

    let base_dir = if library_type == "skills" {
        wardian_home.join(LIBRARY_SKILLS_DIR)
    } else {
        wardian_home.join(LIBRARY_PROMPTS_DIR)
    };

    let mut file_path = base_dir.join(&path);
    if library_type == "skills" {
        file_path = file_path.join("SKILL.md");
    }

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
pub async fn update_library_metadata(
    _app: AppHandle,
    path: String,
    metadata: LibraryItemMetadata,
) -> Result<(), String> {
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
pub async fn open_library_folder(
    _app: AppHandle,
    library_type: String,
    path: Option<String>,
) -> Result<(), String> {
    let wardian_home = get_wardian_home().ok_or("Could not find Wardian home")?;

    let base_dir = if library_type == "skills" {
        LIBRARY_SKILLS_DIR
    } else {
        LIBRARY_PROMPTS_DIR
    };

    let mut target_dir = wardian_home.join(base_dir);
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

fn get_target_skills_dir(target_type: &str, target_id: &str) -> Result<std::path::PathBuf, String> {
    let home = get_wardian_home().ok_or("Could not find Wardian home")?;
    let base = match target_type {
        "agent" => home.join("agents").join(target_id),
        "class" => home.join("classes").join(target_id),
        "user" => home.join("common"),
        _ => return Err(format!("Unknown target type: {}", target_type)),
    };
    Ok(base.join(".agents").join("skills"))
}

fn copy_dir_all(src: impl AsRef<Path>, dst: impl AsRef<Path>) -> std::io::Result<()> {
    fs::create_dir_all(&dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        if ty.is_dir() {
            copy_dir_all(entry.path(), dst.as_ref().join(entry.file_name()))?;
        } else {
            fs::copy(entry.path(), dst.as_ref().join(entry.file_name()))?;
        }
    }
    Ok(())
}

fn remove_existing_deployment(path: &Path) -> std::io::Result<()> {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return Ok(());
    };

    if metadata.file_type().is_symlink() {
        #[cfg(target_os = "windows")]
        {
            return fs::remove_dir(path).or_else(|_| fs::remove_file(path));
        }

        #[cfg(not(target_os = "windows"))]
        {
            return fs::remove_file(path);
        }
    }

    if metadata.is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    }
}

#[cfg(target_os = "windows")]
fn link_skill_dir(src: &Path, dst: &Path) -> std::io::Result<()> {
    let src = src.canonicalize()?;
    let dst = match (dst.parent(), dst.file_name()) {
        (Some(parent), Some(file_name)) => parent.canonicalize()?.join(file_name),
        _ => dst.to_path_buf(),
    };

    let output = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(&dst)
        .arg(&src)
        .output()?;

    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        Err(std::io::Error::other(format!(
            "mklink /J failed: {}{}",
            stdout, stderr
        )))
    }
}

#[cfg(not(target_os = "windows"))]
fn link_skill_dir(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(src, dst)
}

fn deploy_skill_dir_with_linker<F>(src_dir: &Path, dst_dir: &Path, linker: F) -> std::io::Result<()>
where
    F: FnOnce(&Path, &Path) -> std::io::Result<()>,
{
    remove_existing_deployment(dst_dir)?;

    if let Some(parent) = dst_dir.parent() {
        fs::create_dir_all(parent)?;
    }

    match linker(src_dir, dst_dir) {
        Ok(()) => Ok(()),
        Err(link_error) => {
            log_debug(&format!(
                "[Wardian] Failed to link skill {:?} to {:?}; falling back to copy: {}",
                src_dir, dst_dir, link_error
            ));
            copy_dir_all(src_dir, dst_dir)
        }
    }
}

fn deploy_skill_from_library(
    source_path: &str,
    target_type: &str,
    target_id: &str,
) -> Result<(), String> {
    let home = get_wardian_home().ok_or("Could not find Wardian home")?;
    let src_dir = home.join(LIBRARY_SKILLS_DIR).join(source_path);

    if !src_dir.exists() || !src_dir.is_dir() {
        return Err(format!(
            "Skill source not found or is not a directory: {:?}",
            src_dir
        ));
    }

    let target_skills_dir = get_target_skills_dir(target_type, target_id)?;
    let skill_name = Path::new(source_path).file_name().unwrap_or_default();
    let dst_dir = target_skills_dir.join(skill_name);

    deploy_skill_dir_with_linker(&src_dir, &dst_dir, link_skill_dir).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn deploy_skill(
    _app: AppHandle,
    source_path: String,
    target_type: String,
    target_id: String,
) -> Result<(), String> {
    deploy_skill_from_library(&source_path, &target_type, &target_id)
}

#[tauri::command]
pub async fn remove_deployed_skill(
    _app: AppHandle,
    target_type: String,
    target_id: String,
    skill_name: String,
) -> Result<(), String> {
    let target_skills_dir = get_target_skills_dir(&target_type, &target_id)?;
    let dst_dir = target_skills_dir.join(&skill_name);

    if dst_dir.exists() {
        fs::remove_dir_all(&dst_dir).map_err(|e| e.to_string())?;
    }

    Ok(())
}

#[tauri::command]
pub async fn list_deployed_skills(
    _app: AppHandle,
    target_type: String,
    target_id: String,
) -> Result<Vec<String>, String> {
    let target_skills_dir = get_target_skills_dir(&target_type, &target_id)?;
    let mut skills = Vec::new();

    if target_skills_dir.exists() {
        if let Ok(entries) = fs::read_dir(target_skills_dir) {
            for entry in entries.flatten() {
                if let Ok(file_type) = entry.file_type() {
                    if file_type.is_dir() {
                        skills.push(entry.file_name().to_string_lossy().to_string());
                    }
                }
            }
        }
    }

    Ok(skills)
}

#[tauri::command]
pub async fn list_skill_deployments(
    _app: AppHandle,
    skill_name: String,
) -> Result<Vec<crate::models::SkillDeployment>, String> {
    let home = get_wardian_home().ok_or("Could not find Wardian home")?;
    let mut deployments = Vec::new();

    // Check user (global)
    let user_target = home
        .join("common")
        .join(".agents")
        .join("skills")
        .join(&skill_name);
    if user_target.exists() && user_target.is_dir() {
        deployments.push(crate::models::SkillDeployment {
            target_type: "user".to_string(),
            target_id: "global".to_string(),
        });
    }

    // Check classes
    let classes_dir = home.join("classes");
    if classes_dir.exists() {
        if let Ok(entries) = fs::read_dir(classes_dir) {
            for entry in entries.flatten() {
                if let Ok(ty) = entry.file_type() {
                    if ty.is_dir() {
                        let class_name = entry.file_name().to_string_lossy().to_string();
                        let class_target = entry
                            .path()
                            .join(".agents")
                            .join("skills")
                            .join(&skill_name);
                        if class_target.exists() && class_target.is_dir() {
                            deployments.push(crate::models::SkillDeployment {
                                target_type: "class".to_string(),
                                target_id: class_name,
                            });
                        }
                    }
                }
            }
        }
    }

    // Check agents
    let agents_dir = home.join("agents");
    if agents_dir.exists() {
        if let Ok(entries) = fs::read_dir(agents_dir) {
            for entry in entries.flatten() {
                if let Ok(ty) = entry.file_type() {
                    if ty.is_dir() {
                        let agent_id = entry.file_name().to_string_lossy().to_string();
                        let agent_target = entry
                            .path()
                            .join(".agents")
                            .join("skills")
                            .join(&skill_name);
                        if agent_target.exists() && agent_target.is_dir() {
                            deployments.push(crate::models::SkillDeployment {
                                target_type: "agent".to_string(),
                                target_id: agent_id,
                            });
                        }
                    }
                }
            }
        }
    }

    Ok(deployments)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    struct WardianHomeGuard;

    impl Drop for WardianHomeGuard {
        fn drop(&mut self) {
            unsafe { std::env::remove_var("WARDIAN_HOME") };
        }
    }

    #[test]
    fn deploy_skill_uses_live_link_for_agent_targets() {
        let _lock = crate::utils::wardian_test_env_lock();
        let temp = tempfile::tempdir().expect("temp dir");
        unsafe { std::env::set_var("WARDIAN_HOME", temp.path()) };
        let _env_guard = WardianHomeGuard;

        let source_dir = temp.path().join("library").join("skills").join("planner");
        fs::create_dir_all(&source_dir).expect("source skill dir");
        fs::write(source_dir.join("SKILL.md"), "original").expect("source skill");

        deploy_skill_from_library("planner", "agent", "agent-1").expect("deploy skill");

        fs::write(source_dir.join("SKILL.md"), "updated").expect("update source skill");
        let deployed_skill = temp
            .path()
            .join("agents")
            .join("agent-1")
            .join(".agents")
            .join("skills")
            .join("planner")
            .join("SKILL.md");

        let debug_log =
            fs::read_to_string(temp.path().join("wardian_debug.log")).unwrap_or_default();
        assert_eq!(
            fs::read_to_string(deployed_skill).expect("deployed skill content"),
            "updated",
            "expected live-linked skill content; debug log:\n{}",
            debug_log
        );
    }

    #[test]
    fn deploy_skill_copies_directory_when_link_creation_fails() {
        let temp = tempfile::tempdir().expect("temp dir");
        let source_dir = temp.path().join("source");
        let target_dir = temp.path().join("target");
        fs::create_dir_all(source_dir.join("nested")).expect("source dirs");
        fs::write(source_dir.join("SKILL.md"), "fallback").expect("source skill");
        fs::write(source_dir.join("nested").join("notes.md"), "notes").expect("nested file");

        deploy_skill_dir_with_linker(&source_dir, &target_dir, |_src, _dst| {
            Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "link denied",
            ))
        })
        .expect("fallback copy");

        fs::write(source_dir.join("SKILL.md"), "source changed").expect("update source skill");

        assert_eq!(
            fs::read_to_string(target_dir.join("SKILL.md")).expect("copied skill content"),
            "fallback"
        );
        assert_eq!(
            fs::read_to_string(target_dir.join("nested").join("notes.md")).expect("nested copy"),
            "notes"
        );
    }
}
