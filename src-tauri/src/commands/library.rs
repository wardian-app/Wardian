use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use tauri::{AppHandle, Emitter, Manager};

use crate::manager::log_debug;
use crate::state::{AppState, LibraryWatchRegistration};
use crate::utils::fs::{copy_dir_all, get_wardian_home};
use wardian_core::models::{
    AgentConfig, DeployedSkillRef, LibraryFolder, LibraryItemMetadata, LibraryNode, LibraryPrompt,
    SkillDeployment,
};

const LIBRARY_PROMPTS_DIR: &str = "library/prompts";
const LIBRARY_SKILLS_DIR: &str = "library/skills";
const LIBRARY_METADATA_FILE: &str = "library/library.json";
const DEPLOYED_SKILL_SOURCE_FILE: &str = ".wardian-skill-source";
const LIBRARY_WATCH_DEBOUNCE_MS: u64 = 200;
static LIBRARY_WATCH_GENERATION: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, serde::Serialize)]
struct LibraryChangedPayload {
    library_type: String,
}

fn is_supported_watch_library_type(library_type: &str) -> bool {
    library_type == "skills"
}

fn library_dir_for_type(home: &Path, library_type: &str) -> Result<PathBuf, String> {
    match library_type {
        "skills" => Ok(home.join(LIBRARY_SKILLS_DIR)),
        _ => Err(format!("Unsupported library watch type: {}", library_type)),
    }
}

fn ensure_library_watch_dir(home: &Path, library_type: &str) -> Result<PathBuf, String> {
    let dir = library_dir_for_type(home, library_type)?;
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}

fn path_is_within(child: &Path, parent: &Path) -> bool {
    child == parent || child.starts_with(parent)
}

fn discover_skill_watch_targets(skills_dir: &Path) -> Result<Vec<PathBuf>, String> {
    let canonical_skills = skills_dir
        .canonicalize()
        .unwrap_or_else(|_| skills_dir.to_path_buf());
    let mut targets = vec![canonical_skills.clone()];

    fn scan(dir: &Path, canonical_root: &Path, targets: &mut Vec<PathBuf>) -> Result<(), String> {
        let entries = match fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(_) => return Ok(()),
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }

            if path.join("SKILL.md").exists() {
                if let Ok(canonical) = path.canonicalize() {
                    if !path_is_within(&canonical, canonical_root)
                        && !targets.iter().any(|existing| existing == &canonical)
                    {
                        targets.push(canonical);
                    }
                }
                continue;
            }

            scan(&path, canonical_root, targets)?;
        }

        Ok(())
    }

    scan(skills_dir, &canonical_skills, &mut targets)?;
    Ok(targets)
}

#[cfg(test)]
fn is_current_library_watch_generation(
    state: &AppState,
    library_type: &str,
    generation: u64,
) -> bool {
    state
        .library_watchers
        .try_lock()
        .ok()
        .and_then(|watchers| watchers.get(library_type).map(|entry| entry.generation))
        == Some(generation)
}

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

                        children.push(LibraryNode::Skill(wardian_core::models::LibrarySkill {
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
        crate::utils::process::new_silent_std_command("explorer")
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

pub(crate) fn get_target_skills_dir(
    target_type: &str,
    target_id: &str,
) -> Result<std::path::PathBuf, String> {
    let home = get_wardian_home().ok_or("Could not find Wardian home")?;
    let base = match target_type {
        "agent" => home.join("agents").join(target_id),
        "class" => home.join("classes").join(target_id),
        "user" => home.join("common"),
        _ => return Err(format!("Unknown target type: {}", target_type)),
    };
    Ok(base.join(".agents").join("skills"))
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

    let output = crate::utils::process::new_silent_std_command("cmd")
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

fn deploy_skill_dir_with_linker<F>(
    src_dir: &Path,
    dst_dir: &Path,
    linker: F,
) -> std::io::Result<bool>
where
    F: FnOnce(&Path, &Path) -> std::io::Result<()>,
{
    remove_existing_deployment(dst_dir)?;

    if let Some(parent) = dst_dir.parent() {
        fs::create_dir_all(parent)?;
    }

    match linker(src_dir, dst_dir) {
        Ok(()) => Ok(false),
        Err(link_error) => {
            log_debug(&format!(
                "[Wardian] Failed to link skill {:?} to {:?}; falling back to copy: {}",
                src_dir, dst_dir, link_error
            ));
            copy_dir_all(src_dir, dst_dir)?;
            Ok(true)
        }
    }
}

pub(crate) fn deploy_skill_from_library(
    source_path: &str,
    target_type: &str,
    target_id: &str,
) -> Result<(), String> {
    deploy_skill_from_library_with_linker(source_path, target_type, target_id, link_skill_dir)
}

fn deploy_skill_from_library_with_linker<F>(
    source_path: &str,
    target_type: &str,
    target_id: &str,
    linker: F,
) -> Result<(), String>
where
    F: FnOnce(&Path, &Path) -> std::io::Result<()>,
{
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

    let copied =
        deploy_skill_dir_with_linker(&src_dir, &dst_dir, linker).map_err(|e| e.to_string())?;
    if copied {
        fs::write(dst_dir.join(DEPLOYED_SKILL_SOURCE_FILE), source_path)
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn refresh_antigravity_skill_projections_for_configs(
    configs: impl IntoIterator<Item = AgentConfig>,
) {
    for config in configs {
        if config.provider != "antigravity" {
            continue;
        }

        let mut directories = config
            .system_include_directories
            .clone()
            .unwrap_or_default();
        if let Some(user_dirs) = config.include_directories.as_ref() {
            for dir in user_dirs {
                if !directories.contains(dir) {
                    directories.push(dir.clone());
                }
            }
        }

        if directories.is_empty() {
            continue;
        }

        let _ = crate::utils::fs::project_antigravity_include_directories(
            &config.session_id,
            directories,
        );
    }
}

async fn refresh_live_antigravity_skill_projections(app: &AppHandle) {
    let state = app.state::<AppState>();
    let configs = {
        let agents = state.agents.lock().await;
        agents
            .values()
            .filter_map(|agent| agent.config.lock().ok().map(|config| config.clone()))
            .collect::<Vec<_>>()
    };
    refresh_antigravity_skill_projections_for_configs(configs);
}

fn list_deployed_skill_names(target_type: &str, target_id: &str) -> Result<Vec<String>, String> {
    let target_skills_dir = get_target_skills_dir(target_type, target_id)?;
    let mut skills = Vec::new();

    if !target_skills_dir.exists() {
        return Ok(skills);
    }

    let entries = fs::read_dir(target_skills_dir).map_err(|e| e.to_string())?;
    for entry in entries.flatten() {
        if entry.path().is_dir() {
            skills.push(entry.file_name().to_string_lossy().to_string());
        }
    }
    skills.sort();

    Ok(skills)
}

fn collect_library_skill_sources(
    dir: &Path,
    base_dir: &Path,
    sources: &mut Vec<(String, String, PathBuf)>,
) -> Result<(), String> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return Ok(()),
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        if path.join("SKILL.md").exists() {
            let rel_path = path
                .strip_prefix(base_dir)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            let name = path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            if let Ok(canonical) = path.canonicalize() {
                sources.push((rel_path, name, canonical));
            }
            continue;
        }

        collect_library_skill_sources(&path, base_dir, sources)?;
    }

    Ok(())
}

fn read_deployed_skill_source_marker(path: &Path) -> Option<String> {
    fs::read_to_string(path.join(DEPLOYED_SKILL_SOURCE_FILE))
        .ok()
        .map(|source| source.trim().replace('\\', "/"))
        .filter(|source| !source.is_empty())
}

fn source_path_for_deployed_skill(
    deployed_path: &Path,
    deployed_name: &str,
    library_sources: &[(String, String, PathBuf)],
    infer_same_name_source: bool,
) -> Option<String> {
    if let Some(marker_source) = read_deployed_skill_source_marker(deployed_path) {
        if library_sources
            .iter()
            .any(|(rel_path, _, _)| rel_path == &marker_source)
        {
            return Some(marker_source);
        }
    }

    if let Ok(canonical_path) = deployed_path.canonicalize() {
        if let Some((rel_path, _, _)) = library_sources
            .iter()
            .find(|(_, _, source_canonical)| source_canonical == &canonical_path)
        {
            return Some(rel_path.clone());
        }
    }

    if !infer_same_name_source {
        return None;
    }

    let mut same_name_sources = library_sources
        .iter()
        .filter(|(_, name, _)| name == deployed_name);
    let only_source = same_name_sources.next();
    if only_source.is_some() && same_name_sources.next().is_none() {
        return only_source.map(|(rel_path, _, _)| rel_path.clone());
    }

    None
}

fn list_deployed_skill_refs_for_target_with_options(
    target_type: &str,
    target_id: &str,
    infer_same_name_source: bool,
) -> Result<Vec<DeployedSkillRef>, String> {
    let home = get_wardian_home().ok_or("Could not find Wardian home")?;
    let target_skills_dir = get_target_skills_dir(target_type, target_id)?;
    let library_skills_dir = home.join(LIBRARY_SKILLS_DIR);
    let mut library_sources = Vec::new();
    collect_library_skill_sources(
        &library_skills_dir,
        &library_skills_dir,
        &mut library_sources,
    )?;

    let mut deployed = Vec::new();
    if !target_skills_dir.exists() {
        return Ok(deployed);
    }

    let entries = fs::read_dir(target_skills_dir).map_err(|e| e.to_string())?;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        let name = entry.file_name().to_string_lossy().to_string();
        let source_path =
            source_path_for_deployed_skill(&path, &name, &library_sources, infer_same_name_source);

        deployed.push(DeployedSkillRef { name, source_path });
    }

    deployed.sort_by(|a, b| {
        a.source_path
            .as_deref()
            .unwrap_or(&a.name)
            .cmp(b.source_path.as_deref().unwrap_or(&b.name))
    });

    Ok(deployed)
}

pub(crate) fn list_deployed_skill_refs_for_target(
    target_type: &str,
    target_id: &str,
) -> Result<Vec<DeployedSkillRef>, String> {
    list_deployed_skill_refs_for_target_with_options(target_type, target_id, true)
}

pub(crate) fn list_deployed_skill_refs_for_target_strict(
    target_type: &str,
    target_id: &str,
) -> Result<Vec<DeployedSkillRef>, String> {
    list_deployed_skill_refs_for_target_with_options(target_type, target_id, false)
}

fn deployment_matches_source(
    target: &Path,
    skill_name: &str,
    source_path: Option<&str>,
    library_sources: &[(String, String, PathBuf)],
    home: &Path,
) -> bool {
    if !target.exists() || !target.is_dir() {
        return false;
    }

    let Some(source_path) = source_path else {
        return true;
    };

    if read_deployed_skill_source_marker(target).as_deref() == Some(source_path) {
        return true;
    }

    let source = home.join(LIBRARY_SKILLS_DIR).join(source_path);
    if matches!(
        (target.canonicalize(), source.canonicalize()),
        (Ok(target), Ok(source)) if target == source
    ) {
        return true;
    }

    let mut same_name_sources = library_sources
        .iter()
        .filter(|(_, name, _)| name == skill_name);
    let only_source = same_name_sources.next();
    only_source
        .map(|(rel_path, _, _)| rel_path == source_path)
        .unwrap_or(false)
        && same_name_sources.next().is_none()
}

#[tauri::command]
pub async fn deploy_skill(
    app: AppHandle,
    source_path: String,
    target_type: String,
    target_id: String,
) -> Result<(), String> {
    deploy_skill_from_library(&source_path, &target_type, &target_id)?;
    refresh_live_antigravity_skill_projections(&app).await;
    Ok(())
}

#[tauri::command]
pub async fn remove_deployed_skill(
    app: AppHandle,
    target_type: String,
    target_id: String,
    skill_name: String,
) -> Result<(), String> {
    let target_skills_dir = get_target_skills_dir(&target_type, &target_id)?;
    let dst_dir = target_skills_dir.join(&skill_name);

    if dst_dir.exists() {
        fs::remove_dir_all(&dst_dir).map_err(|e| e.to_string())?;
    }

    refresh_live_antigravity_skill_projections(&app).await;
    Ok(())
}

#[tauri::command]
pub async fn list_deployed_skills(
    _app: AppHandle,
    target_type: String,
    target_id: String,
) -> Result<Vec<String>, String> {
    list_deployed_skill_names(&target_type, &target_id)
}

#[tauri::command]
pub async fn list_deployed_skill_refs(
    _app: AppHandle,
    target_type: String,
    target_id: String,
) -> Result<Vec<DeployedSkillRef>, String> {
    list_deployed_skill_refs_for_target(&target_type, &target_id)
}

#[tauri::command]
pub async fn list_skill_deployments(
    _app: AppHandle,
    skill_name: String,
    source_path: Option<String>,
) -> Result<Vec<SkillDeployment>, String> {
    list_skill_deployments_for_source(&skill_name, source_path.as_deref())
}

fn list_skill_deployments_for_source(
    skill_name: &str,
    source_path: Option<&str>,
) -> Result<Vec<SkillDeployment>, String> {
    let home = get_wardian_home().ok_or("Could not find Wardian home")?;
    let library_skills_dir = home.join(LIBRARY_SKILLS_DIR);
    let mut library_sources = Vec::new();
    collect_library_skill_sources(
        &library_skills_dir,
        &library_skills_dir,
        &mut library_sources,
    )?;
    let mut deployments = Vec::new();

    let user_target = home
        .join("common")
        .join(".agents")
        .join("skills")
        .join(skill_name);
    if deployment_matches_source(
        &user_target,
        skill_name,
        source_path,
        &library_sources,
        &home,
    ) {
        deployments.push(SkillDeployment {
            target_type: "user".to_string(),
            target_id: "global".to_string(),
        });
    }

    let classes_dir = home.join("classes");
    if classes_dir.exists() {
        if let Ok(entries) = fs::read_dir(classes_dir) {
            for entry in entries.flatten() {
                if let Ok(ty) = entry.file_type() {
                    if ty.is_dir() {
                        let class_name = entry.file_name().to_string_lossy().to_string();
                        let class_target =
                            entry.path().join(".agents").join("skills").join(skill_name);
                        if deployment_matches_source(
                            &class_target,
                            skill_name,
                            source_path,
                            &library_sources,
                            &home,
                        ) {
                            deployments.push(SkillDeployment {
                                target_type: "class".to_string(),
                                target_id: class_name,
                            });
                        }
                    }
                }
            }
        }
    }

    let agents_dir = home.join("agents");
    if agents_dir.exists() {
        if let Ok(entries) = fs::read_dir(agents_dir) {
            for entry in entries.flatten() {
                if let Ok(ty) = entry.file_type() {
                    if ty.is_dir() {
                        let agent_id = entry.file_name().to_string_lossy().to_string();
                        let agent_target =
                            entry.path().join(".agents").join("skills").join(skill_name);
                        if deployment_matches_source(
                            &agent_target,
                            skill_name,
                            source_path,
                            &library_sources,
                            &home,
                        ) {
                            deployments.push(SkillDeployment {
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

#[tauri::command]
pub async fn library_watch(
    app: AppHandle,
    state: tauri::State<'_, AppState>,
    library_type: String,
) -> Result<(), String> {
    if !is_supported_watch_library_type(&library_type) {
        return Err(format!("Unsupported library watch type: {}", library_type));
    }

    let mut registrations = state.library_watchers.lock().await;
    if let Some(registration) = registrations.get_mut(&library_type) {
        registration.ref_count += 1;
        return Ok(());
    }

    let home = get_wardian_home().ok_or("Could not find Wardian home")?;
    let skills_dir = ensure_library_watch_dir(&home, &library_type)?;
    let watch_targets = discover_skill_watch_targets(&skills_dir)?;
    let generation = LIBRARY_WATCH_GENERATION.fetch_add(1, Ordering::Relaxed);
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<()>();

    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if res.is_ok() {
            let _ = tx.send(());
        }
    })
    .map_err(|e| e.to_string())?;

    for target in &watch_targets {
        notify::Watcher::watch(&mut watcher, target, notify::RecursiveMode::Recursive)
            .map_err(|e| e.to_string())?;
    }

    registrations.insert(
        library_type.clone(),
        LibraryWatchRegistration {
            watcher,
            ref_count: 1,
            generation,
            watched_paths: watch_targets,
        },
    );
    drop(registrations);

    let app_handle = app.clone();
    let task_library_type = library_type.clone();
    tokio::spawn(async move {
        loop {
            if rx.recv().await.is_none() {
                break;
            }
            loop {
                match tokio::time::timeout(
                    std::time::Duration::from_millis(LIBRARY_WATCH_DEBOUNCE_MS),
                    rx.recv(),
                )
                .await
                {
                    Ok(Some(_)) => continue,
                    Ok(None) => return,
                    Err(_) => break,
                }
            }

            let state = app_handle.state::<AppState>();
            {
                let registrations = state.library_watchers.lock().await;
                if registrations
                    .get(&task_library_type)
                    .map(|entry| entry.generation)
                    != Some(generation)
                {
                    return;
                }
            }

            if let Some(home) = get_wardian_home() {
                if let Ok(skills_dir) = library_dir_for_type(&home, &task_library_type) {
                    if let Ok(next_targets) = discover_skill_watch_targets(&skills_dir) {
                        let mut registrations = state.library_watchers.lock().await;
                        let Some(registration) = registrations.get_mut(&task_library_type) else {
                            return;
                        };
                        if registration.generation != generation {
                            return;
                        }
                        for target in registration.watched_paths.clone() {
                            if next_targets.iter().any(|path| path == &target) {
                                continue;
                            }
                            let _ = notify::Watcher::unwatch(&mut registration.watcher, &target);
                        }
                        registration
                            .watched_paths
                            .retain(|target| next_targets.iter().any(|path| path == target));

                        for target in next_targets {
                            if registration
                                .watched_paths
                                .iter()
                                .any(|path| path == &target)
                            {
                                continue;
                            }
                            if notify::Watcher::watch(
                                &mut registration.watcher,
                                &target,
                                notify::RecursiveMode::Recursive,
                            )
                            .is_ok()
                            {
                                registration.watched_paths.push(target);
                            }
                        }
                    }
                }
            }

            {
                let registrations = state.library_watchers.lock().await;
                if registrations
                    .get(&task_library_type)
                    .map(|entry| entry.generation)
                    != Some(generation)
                {
                    return;
                }
            }

            if task_library_type == "skills" {
                refresh_live_antigravity_skill_projections(&app_handle).await;
            }

            let _ = app_handle.emit(
                "library-changed",
                LibraryChangedPayload {
                    library_type: task_library_type.clone(),
                },
            );
        }
    });

    Ok(())
}

#[tauri::command]
pub async fn library_unwatch(
    state: tauri::State<'_, AppState>,
    library_type: String,
) -> Result<(), String> {
    if !is_supported_watch_library_type(&library_type) {
        return Err(format!("Unsupported library watch type: {}", library_type));
    }

    let mut registrations = state.library_watchers.lock().await;
    let Some(registration) = registrations.get_mut(&library_type) else {
        return Ok(());
    };

    if registration.ref_count > 1 {
        registration.ref_count -= 1;
    } else {
        registrations.remove(&library_type);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::AppState;
    use crate::utils::fs::create_directory_link;
    use std::fs;
    use std::io::Write;
    use std::path::Path;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

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
    fn list_deployed_skills_includes_linked_skill_directories() {
        let _lock = crate::utils::wardian_test_env_lock();
        let temp = tempfile::tempdir().expect("temp dir");
        unsafe { std::env::set_var("WARDIAN_HOME", temp.path()) };
        let _env_guard = WardianHomeGuard;

        let source_dir = temp.path().join("library").join("skills").join("planner");
        fs::create_dir_all(&source_dir).expect("source skill dir");
        fs::write(source_dir.join("SKILL.md"), "linked").expect("source skill");

        deploy_skill_from_library("planner", "agent", "agent-1").expect("deploy skill");

        assert_eq!(
            list_deployed_skill_names("agent", "agent-1").expect("list skills"),
            vec!["planner".to_string()]
        );
    }

    #[test]
    fn refresh_antigravity_skill_projections_picks_up_live_skill_changes() {
        let _lock = crate::utils::wardian_test_env_lock();
        let temp = tempfile::tempdir().expect("temp dir");
        unsafe { std::env::set_var("WARDIAN_HOME", temp.path()) };
        let _env_guard = WardianHomeGuard;

        let hidden = temp.path().join(".wardian");
        let common = hidden.join("common");
        let skills = common.join(".agents").join("skills");
        let library = hidden.join("library").join("skills");
        let obsolete = skills.join("obsolete");
        let late_source = library.join("wardian-skills").join("late-skill");
        let late_deployed = skills.join("late-skill");
        fs::create_dir_all(&obsolete).expect("obsolete skill");
        fs::write(obsolete.join("SKILL.md"), "obsolete").expect("obsolete file");
        let session_id = format!(
            "antigravity-live-skills-{}",
            temp.path()
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
        );

        let config = AgentConfig {
            provider: "antigravity".to_string(),
            session_id,
            system_include_directories: Some(vec![common.to_string_lossy().to_string()]),
            ..Default::default()
        };

        refresh_antigravity_skill_projections_for_configs(vec![config.clone()]);
        let projected = crate::utils::fs::project_antigravity_include_directories(
            &config.session_id,
            vec![common.to_string_lossy().to_string()],
        );
        let projected_skills = PathBuf::from(&projected[0]).join(".agents").join("skills");
        assert!(projected_skills.join("obsolete").join("SKILL.md").is_file());

        fs::create_dir_all(&late_source).expect("late source skill");
        fs::write(late_source.join("SKILL.md"), "late").expect("late source file");
        create_directory_link(&late_source, &late_deployed).expect("deploy late skill link");
        fs::remove_dir_all(&obsolete).expect("remove obsolete skill");

        refresh_antigravity_skill_projections_for_configs(vec![config]);

        assert_eq!(
            fs::read_to_string(projected_skills.join("late-skill").join("SKILL.md"))
                .expect("late skill file"),
            "late"
        );
        assert!(
            !projected_skills.join("obsolete").exists(),
            "projection should mirror removed skills after refresh"
        );
    }

    #[test]
    fn list_deployed_skill_refs_distinguishes_duplicate_skill_names_by_source_path() {
        let _lock = crate::utils::wardian_test_env_lock();
        let temp = tempfile::tempdir().expect("temp dir");
        unsafe { std::env::set_var("WARDIAN_HOME", temp.path()) };
        let _env_guard = WardianHomeGuard;

        let first = temp
            .path()
            .join("library")
            .join("skills")
            .join("group-a")
            .join("planner");
        let second = temp
            .path()
            .join("library")
            .join("skills")
            .join("group-b")
            .join("planner");
        fs::create_dir_all(&first).expect("first skill");
        fs::create_dir_all(&second).expect("second skill");
        fs::write(first.join("SKILL.md"), "first").expect("first skill file");
        fs::write(second.join("SKILL.md"), "second").expect("second skill file");

        deploy_skill_from_library("group-b/planner", "agent", "agent-1").expect("deploy skill");

        assert_eq!(
            list_deployed_skill_refs_for_target("agent", "agent-1").expect("deployed refs"),
            vec![DeployedSkillRef {
                name: "planner".to_string(),
                source_path: Some("group-b/planner".to_string()),
            }]
        );
    }

    #[test]
    fn list_skill_deployments_filters_duplicate_skill_names_by_source_path() {
        let _lock = crate::utils::wardian_test_env_lock();
        let temp = tempfile::tempdir().expect("temp dir");
        unsafe { std::env::set_var("WARDIAN_HOME", temp.path()) };
        let _env_guard = WardianHomeGuard;

        for source in ["group-a/planner", "group-b/planner"] {
            let source_dir = temp.path().join("library").join("skills").join(source);
            fs::create_dir_all(&source_dir).expect("source skill");
            fs::write(source_dir.join("SKILL.md"), source).expect("source skill file");
        }

        deploy_skill_from_library("group-b/planner", "agent", "agent-1").expect("deploy skill");

        assert!(
            list_skill_deployments_for_source("planner", Some("group-a/planner"))
                .expect("group-a deployments")
                .is_empty()
        );
        assert_eq!(
            list_skill_deployments_for_source("planner", Some("group-b/planner"))
                .expect("group-b deployments")
                .len(),
            1
        );
    }

    #[test]
    fn copied_deployed_skill_refs_keep_source_path_marker() {
        let _lock = crate::utils::wardian_test_env_lock();
        let temp = tempfile::tempdir().expect("temp dir");
        unsafe { std::env::set_var("WARDIAN_HOME", temp.path()) };
        let _env_guard = WardianHomeGuard;

        let source_dir = temp
            .path()
            .join("library")
            .join("skills")
            .join("group-a")
            .join("planner");
        fs::create_dir_all(&source_dir).expect("source skill");
        fs::write(source_dir.join("SKILL.md"), "source").expect("source skill file");

        deploy_skill_from_library_with_linker(
            "group-a/planner",
            "agent",
            "agent-1",
            |_src, _dst| {
                Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "link denied",
                ))
            },
        )
        .expect("deploy copied skill");

        assert_eq!(
            list_deployed_skill_refs_for_target("agent", "agent-1").expect("deployed refs"),
            vec![DeployedSkillRef {
                name: "planner".to_string(),
                source_path: Some("group-a/planner".to_string()),
            }]
        );
        assert_eq!(
            list_skill_deployments_for_source("planner", Some("group-a/planner"))
                .expect("deployments")
                .len(),
            1
        );
    }

    #[test]
    fn unmarked_copied_deployment_infers_source_only_when_name_is_unique() {
        let _lock = crate::utils::wardian_test_env_lock();
        let temp = tempfile::tempdir().expect("temp dir");
        unsafe { std::env::set_var("WARDIAN_HOME", temp.path()) };
        let _env_guard = WardianHomeGuard;

        let source_dir = temp.path().join("library").join("skills").join("planner");
        let target_dir = temp
            .path()
            .join("agents")
            .join("agent-1")
            .join(".agents")
            .join("skills")
            .join("planner");
        fs::create_dir_all(&source_dir).expect("source skill");
        fs::write(source_dir.join("SKILL.md"), "source").expect("source skill file");
        copy_dir_all(&source_dir, &target_dir).expect("legacy copied deployment");

        assert_eq!(
            list_deployed_skill_refs_for_target("agent", "agent-1").expect("deployed refs"),
            vec![DeployedSkillRef {
                name: "planner".to_string(),
                source_path: Some("planner".to_string()),
            }]
        );
    }

    #[test]
    fn unmarked_copied_duplicate_deployment_stays_ambiguous() {
        let _lock = crate::utils::wardian_test_env_lock();
        let temp = tempfile::tempdir().expect("temp dir");
        unsafe { std::env::set_var("WARDIAN_HOME", temp.path()) };
        let _env_guard = WardianHomeGuard;

        for source in ["group-a/planner", "group-b/planner"] {
            let source_dir = temp.path().join("library").join("skills").join(source);
            fs::create_dir_all(&source_dir).expect("source skill");
            fs::write(source_dir.join("SKILL.md"), source).expect("source skill file");
        }
        let target_dir = temp
            .path()
            .join("agents")
            .join("agent-1")
            .join(".agents")
            .join("skills")
            .join("planner");
        copy_dir_all(
            temp.path()
                .join("library")
                .join("skills")
                .join("group-b")
                .join("planner"),
            &target_dir,
        )
        .expect("legacy copied deployment");

        assert_eq!(
            list_deployed_skill_refs_for_target("agent", "agent-1").expect("deployed refs"),
            vec![DeployedSkillRef {
                name: "planner".to_string(),
                source_path: None,
            }]
        );
        assert!(
            list_skill_deployments_for_source("planner", Some("group-b/planner"))
                .expect("group-b deployments")
                .is_empty()
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

        let copied = deploy_skill_dir_with_linker(&source_dir, &target_dir, |_src, _dst| {
            Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "link denied",
            ))
        })
        .expect("fallback copy");
        assert!(copied);

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

    #[test]
    fn library_watch_helpers_resolve_and_create_skills_dir() {
        let temp = tempfile::tempdir().expect("temp dir");
        let skills_dir = temp.path().join("library").join("skills");

        assert_eq!(
            library_dir_for_type(temp.path(), "skills").expect("skills dir"),
            skills_dir
        );
        assert!(library_dir_for_type(temp.path(), "prompts").is_err());

        let ensured = ensure_library_watch_dir(temp.path(), "skills").expect("ensure skills dir");
        assert_eq!(ensured, skills_dir);
        assert!(skills_dir.is_dir());
    }

    #[test]
    fn discover_skill_watch_targets_includes_nested_linked_skill_roots() {
        let temp = tempfile::tempdir().expect("temp dir");
        let skills_dir = temp.path().join("library").join("skills");
        let external_skill = temp.path().join("external").join("planner");
        let nested_parent = skills_dir.join("category");
        let linked_skill = nested_parent.join("planner");

        fs::create_dir_all(&external_skill).expect("external skill dir");
        fs::write(external_skill.join("SKILL.md"), "linked skill").expect("external skill file");
        fs::create_dir_all(&nested_parent).expect("nested parent");
        create_directory_link(&external_skill, &linked_skill).expect("linked skill");

        let targets = discover_skill_watch_targets(&skills_dir).expect("watch targets");
        let canonical_skills = skills_dir.canonicalize().expect("canonical skills");
        let canonical_external = external_skill.canonicalize().expect("canonical external");

        assert!(targets.contains(&canonical_skills));
        assert!(targets.contains(&canonical_external));
        assert_eq!(
            targets
                .iter()
                .filter(|target| **target == canonical_external)
                .count(),
            1
        );
    }

    #[test]
    fn library_watch_generation_is_not_current_after_registration_removed() {
        let state = AppState::new();
        assert!(!is_current_library_watch_generation(&state, "skills", 1));
    }

    fn fs_event_touches(event: &notify::Event, expected: &Path) -> bool {
        let expected_paths = comparable_fs_event_paths(expected);
        event.paths.iter().any(|path| {
            let event_paths = comparable_fs_event_paths(path);
            event_paths.iter().any(|event_path| {
                expected_paths.iter().any(|expected_path| {
                    comparable_fs_event_path_matches(event_path, expected_path)
                })
            })
        })
    }

    fn comparable_fs_event_path_matches(path: &str, expected: &str) -> bool {
        let separator = if cfg!(windows) { "\\" } else { "/" };
        path == expected
            || path.starts_with(&format!("{}{}", expected, separator))
            || expected.starts_with(&format!("{}{}", path, separator))
    }

    fn comparable_fs_event_paths(path: &Path) -> Vec<String> {
        let mut paths = vec![comparable_fs_event_path(path)];
        if let Ok(canonical) = path.canonicalize() {
            paths.push(comparable_fs_event_path(&canonical));
        }
        paths.sort();
        paths.dedup();
        paths
    }

    fn comparable_fs_event_path(path: &Path) -> String {
        #[cfg(windows)]
        {
            let mut path = path.to_string_lossy().replace('/', "\\");
            if let Some(stripped) = path.strip_prefix(r"\\?\UNC\") {
                path = format!(r"\\{}", stripped);
            } else if let Some(stripped) = path.strip_prefix(r"\\?\") {
                path = stripped.to_string();
            }
            path.trim_end_matches('\\').to_ascii_lowercase()
        }

        #[cfg(not(windows))]
        {
            path.to_string_lossy().trim_end_matches('/').to_string()
        }
    }

    fn wait_for_fs_event(rx: &mpsc::Receiver<notify::Event>, label: &str, expected: &Path) {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut observed = Vec::new();
        loop {
            let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                panic!(
                    "timed out waiting for filesystem event: {}; observed events: {:#?}",
                    label, observed
                );
            };
            let event = rx.recv_timeout(remaining).unwrap_or_else(|_| {
                panic!(
                    "timed out waiting for filesystem event: {}; observed events: {:#?}",
                    label, observed
                )
            });
            if fs_event_touches(&event, expected) {
                break;
            }
            observed.push((event.kind, event.paths));
        }
        drain_fs_events(rx);
    }

    fn drain_fs_events(rx: &mpsc::Receiver<notify::Event>) {
        while rx.try_recv().is_ok() {}
    }

    fn wait_for_fs_remove_event(rx: &mpsc::Receiver<notify::Event>, label: &str) {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut observed = Vec::new();
        loop {
            let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                panic!(
                    "timed out waiting for filesystem remove event: {}; observed events: {:#?}",
                    label, observed
                );
            };
            let event = rx.recv_timeout(remaining).unwrap_or_else(|_| {
                panic!(
                    "timed out waiting for filesystem remove event: {}; observed events: {:#?}",
                    label, observed
                )
            });
            if matches!(event.kind, notify::event::EventKind::Remove(_)) {
                break;
            }
            observed.push((event.kind, event.paths));
        }
        drain_fs_events(rx);
    }

    fn wait_for_test_watcher_ready(rx: &mpsc::Receiver<notify::Event>, watched_dir: &Path) {
        let sentinel = watched_dir.join(".wardian-watch-ready");
        fs::write(&sentinel, "ready").expect("write watch sentinel");
        wait_for_fs_event(rx, "watch sentinel create", &sentinel);
        drain_fs_events(rx);
    }

    #[test]
    fn recursive_skill_watcher_observes_create_modify_and_remove() {
        let temp = tempfile::tempdir().expect("temp dir");
        let skills_dir = temp.path().join("library").join("skills");
        fs::create_dir_all(&skills_dir).expect("skills dir");
        let existing_skill_dir = skills_dir.join("planner-existing");
        let existing_skill_file = existing_skill_dir.join("SKILL.md");
        fs::create_dir_all(&existing_skill_dir).expect("existing skill dir");
        fs::write(&existing_skill_file, "one").expect("existing skill file");

        let targets = discover_skill_watch_targets(&skills_dir).expect("watch targets");
        let (tx, rx) = mpsc::channel::<notify::Event>();
        let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            if let Ok(event) = res {
                let _ = tx.send(event);
            }
        })
        .expect("watcher");

        for target in targets {
            notify::Watcher::watch(&mut watcher, &target, notify::RecursiveMode::Recursive)
                .expect("watch target");
        }
        wait_for_test_watcher_ready(&rx, &skills_dir);

        let created_skill_dir = skills_dir.join("planner-created");
        fs::create_dir_all(&created_skill_dir).expect("create skill dir");
        wait_for_fs_event(&rx, "skill dir create", &created_skill_dir);

        let mut file = fs::OpenOptions::new()
            .append(true)
            .open(&existing_skill_file)
            .expect("open skill for modify");
        file.write_all(b"\ntwo").expect("modify skill");
        file.sync_all().expect("sync skill modify");
        wait_for_fs_event(&rx, "skill modify", &existing_skill_file);

        fs::remove_file(&existing_skill_file).expect("remove skill file");
        wait_for_fs_remove_event(&rx, "skill file remove");

        fs::remove_dir(&existing_skill_dir).expect("remove skill dir");
        wait_for_fs_remove_event(&rx, "skill dir remove");
    }

    #[test]
    fn linked_skill_watcher_observes_external_target_edits() {
        let temp = tempfile::tempdir().expect("temp dir");
        let skills_dir = temp.path().join("library").join("skills");
        let external_skill = temp.path().join("external").join("planner");
        let nested_parent = skills_dir.join("category");
        let linked_skill = nested_parent.join("planner");

        fs::create_dir_all(&external_skill).expect("external skill dir");
        fs::write(external_skill.join("SKILL.md"), "one").expect("external skill file");
        fs::create_dir_all(&nested_parent).expect("nested parent");
        create_directory_link(&external_skill, &linked_skill).expect("linked skill");

        let targets = discover_skill_watch_targets(&skills_dir).expect("watch targets");
        let (tx, rx) = mpsc::channel::<notify::Event>();
        let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            if let Ok(event) = res {
                let _ = tx.send(event);
            }
        })
        .expect("watcher");

        for target in targets {
            notify::Watcher::watch(&mut watcher, &target, notify::RecursiveMode::Recursive)
                .expect("watch target");
        }
        wait_for_test_watcher_ready(&rx, &external_skill);

        let external_skill_file = external_skill.join("SKILL.md");
        let mut file = fs::OpenOptions::new()
            .append(true)
            .open(&external_skill_file)
            .expect("open linked target for modify");
        file.write_all(b"\ntwo").expect("modify linked target");
        file.sync_all().expect("sync linked target modify");
        wait_for_fs_event(&rx, "linked target modify", &external_skill_file);
    }
}
