use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use tauri::{AppHandle, Emitter, Manager};

use crate::state::{AppState, LibraryWatchRegistration};
use crate::utils::fs::get_wardian_home;
use wardian_core::library::{self, LibrarySectionId};
use wardian_core::models::{
    AgentConfig, DeployedSkillRef, LibraryIndex, LibraryItemMetadata, SkillDeployment,
};

const LIBRARY_WATCH_DEBOUNCE_MS: u64 = 200;
static LIBRARY_WATCH_GENERATION: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, serde::Serialize)]
struct LibraryChangedPayload {
    library_type: String,
}

fn parse_section(section: &str) -> Result<LibrarySectionId, String> {
    LibrarySectionId::parse(section).ok_or_else(|| format!("Unknown library section: {section}"))
}

// --- Watching -----------------------------------------------------------
//
// The library watcher only supports a single logical type, `"library"`,
// which covers everything under `library/` (skills, prompts, workflows)
// plus `classes/` (class definitions can also be edited from the library
// UI). The `library_type` parameter is kept on the commands for interface
// stability, but any value other than `"library"` is rejected.

fn is_supported_watch_library_type(library_type: &str) -> bool {
    library_type == "library"
}

fn library_dir_for_type(home: &Path, library_type: &str) -> Result<PathBuf, String> {
    match library_type {
        "library" => Ok(home.join("library")),
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

/// Walk `library/skills` for directories that contain a `SKILL.md` whose
/// canonical path lies outside the skills root itself (i.e. the skill
/// directory is a live link to an external target). Notify's recursive
/// watch does not reliably follow reparse points/symlinks on every
/// platform, so those external targets need to be watched explicitly.
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

/// Build the full set of paths the `"library"` watcher should observe: the
/// `library/` root and `classes/` root (both recursive), plus any
/// externally-linked skill targets discovered under `library/skills`. The
/// external-link discovery is deliberately scoped to library skills only —
/// deployed (target-side) skill junctions under `classes/*/.agents/skills`
/// are not separately watched.
fn discover_library_watch_targets(home: &Path, library_root: &Path) -> Result<Vec<PathBuf>, String> {
    let mut targets = vec![library_root
        .canonicalize()
        .unwrap_or_else(|_| library_root.to_path_buf())];

    let classes_root = home.join("classes");
    fs::create_dir_all(&classes_root).map_err(|e| e.to_string())?;
    let canonical_classes = classes_root
        .canonicalize()
        .unwrap_or_else(|_| classes_root.clone());
    if !targets.iter().any(|t| t == &canonical_classes) {
        targets.push(canonical_classes);
    }

    let skills_dir = library_root.join("skills");
    fs::create_dir_all(&skills_dir).map_err(|e| e.to_string())?;
    for extra in discover_skill_watch_targets(&skills_dir)? {
        if !targets.iter().any(|t| t == &extra) {
            targets.push(extra);
        }
    }

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

// --- Index / mutations ---------------------------------------------------
//
// Every command below resolves the Wardian home, parses its section (where
// applicable), and delegates the actual work to `wardian_core::library`.
// No business logic lives here; this layer only adapts Tauri's calling
// convention (AppHandle, async commands, String errors) onto the core
// engine, and layers the Antigravity live-projection refresh on top where
// a deployment changed.

#[tauri::command]
pub async fn get_library_index(_app: AppHandle) -> Result<LibraryIndex, String> {
    let home = get_wardian_home().ok_or("Could not find Wardian home")?;
    library::build_library_index(&home)
}

#[tauri::command]
pub async fn read_library_item(
    _app: AppHandle,
    section: String,
    path: String,
) -> Result<String, String> {
    let home = get_wardian_home().ok_or("Could not find Wardian home")?;
    let section = parse_section(&section)?;
    library::read_item(&home, section, &path)
}

#[tauri::command]
pub async fn save_library_item(
    _app: AppHandle,
    section: String,
    path: String,
    content: String,
) -> Result<(), String> {
    let home = get_wardian_home().ok_or("Could not find Wardian home")?;
    let section = parse_section(&section)?;
    library::save_item(&home, section, &path, &content)
}

#[tauri::command]
pub async fn update_library_metadata(
    _app: AppHandle,
    entry_ref: String,
    metadata: LibraryItemMetadata,
) -> Result<(), String> {
    let home = get_wardian_home().ok_or("Could not find Wardian home")?;
    library::update_metadata(&home, &entry_ref, metadata)
}

#[tauri::command]
pub async fn create_library_folder(
    _app: AppHandle,
    section: String,
    path: String,
) -> Result<(), String> {
    let home = get_wardian_home().ok_or("Could not find Wardian home")?;
    let section = parse_section(&section)?;
    library::create_folder(&home, section, &path)
}

#[tauri::command]
pub async fn rename_library_entry(
    _app: AppHandle,
    section: String,
    from_path: String,
    to_path: String,
) -> Result<(), String> {
    let home = get_wardian_home().ok_or("Could not find Wardian home")?;
    let section = parse_section(&section)?;
    library::rename_entry(&home, section, &from_path, &to_path)
}

#[tauri::command]
pub async fn delete_library_entry(
    _app: AppHandle,
    section: String,
    path: String,
) -> Result<(), String> {
    let home = get_wardian_home().ok_or("Could not find Wardian home")?;
    let section = parse_section(&section)?;
    library::delete_entry(&home, section, &path)
}

#[tauri::command]
pub async fn open_library_folder(
    _app: AppHandle,
    section: String,
    path: Option<String>,
) -> Result<(), String> {
    let home = get_wardian_home().ok_or("Could not find Wardian home")?;
    let section_id = parse_section(&section)?;

    let mut target_dir = section_id.root_for_home(&home);
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

// --- Deployments -----------------------------------------------------------

/// The final path component of a section-relative skill path, used as the
/// deployed skill directory's name (e.g. `dev/planner` -> `planner`).
fn skill_name_from_rel(rel: &str) -> String {
    Path::new(rel)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default()
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

/// Refresh every live Antigravity skill projection. Snapshots the current
/// agent configs first (no filesystem work), then exits early unless at
/// least one config is an Antigravity provider — this keeps the common
/// case (deploy/rename/delete with no Antigravity agents running) from
/// touching the filesystem at all.
async fn refresh_live_antigravity_skill_projections(app: &AppHandle) {
    let state = app.state::<AppState>();
    let configs = {
        let agents = state.agents.lock().await;
        agents
            .values()
            .filter_map(|agent| agent.config.lock().ok().map(|config| config.clone()))
            .collect::<Vec<_>>()
    };

    if !configs.iter().any(|c| c.provider == "antigravity") {
        return;
    }

    refresh_antigravity_skill_projections_for_configs(configs);
}

/// Deploy a single library skill to a single target. Used internally by
/// agent-cloning flows; keeps its pre-refactor signature but now delegates
/// to the core engine.
pub(crate) fn deploy_skill_from_library(
    source_path: &str,
    target_type: &str,
    target_id: &str,
) -> Result<(), String> {
    let home = get_wardian_home().ok_or("Could not find Wardian home")?;
    library::deploy_skill(&home, source_path, target_type, target_id)?;
    Ok(())
}

/// Resolve a deployed skill directory back to a library source using only
/// the `.wardian-skill-source` marker or an exact canonical-path match —
/// no same-name inference. This is deliberately more conservative than
/// `list_deployed_skill_refs_for_target`: it backs the agent-clone preview
/// flow, where guessing a source for an unmarked, unlinked directory could
/// cause the clone to deploy the wrong skill content.
fn resolve_deployed_skill_source_strict(
    deployed_path: &Path,
    sources: &[library::SkillSource],
) -> Option<String> {
    if let Ok(marker) = fs::read_to_string(deployed_path.join(library::DEPLOYED_SKILL_SOURCE_FILE)) {
        let marker = marker.trim().replace('\\', "/");
        if !marker.is_empty() && sources.iter().any(|s| s.rel_path == marker) {
            return Some(marker);
        }
    }

    if let Ok(canonical) = deployed_path.canonicalize() {
        if let Some(source) = sources.iter().find(|s| s.canonical == canonical) {
            return Some(source.rel_path.clone());
        }
    }

    None
}

pub(crate) fn list_deployed_skill_refs_for_target_strict(
    target_type: &str,
    target_id: &str,
) -> Result<Vec<DeployedSkillRef>, String> {
    let home = get_wardian_home().ok_or("Could not find Wardian home")?;
    let sources = library::collect_skill_sources(&home);
    let target_skills_dir = library::get_target_skills_dir(&home, target_type, target_id)?;

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
        let source_path = resolve_deployed_skill_source_strict(&path, &sources);
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
    let home = get_wardian_home().ok_or("Could not find Wardian home")?;
    let sources = library::collect_skill_sources(&home);
    let scan = library::scan_deployments(&home, &sources);

    let mut deployed = Vec::new();
    for (rel_path, targets) in &scan.deployments {
        if targets
            .iter()
            .any(|t| t.target_type == target_type && t.target_id == target_id)
        {
            deployed.push(DeployedSkillRef {
                name: skill_name_from_rel(rel_path),
                source_path: Some(rel_path.clone()),
            });
        }
    }
    for orphan in &scan.orphans {
        if orphan.target_type == target_type && orphan.target_id == target_id {
            deployed.push(DeployedSkillRef {
                name: orphan.skill_name.clone(),
                source_path: None,
            });
        }
    }

    deployed.sort_by(|a, b| {
        a.source_path
            .as_deref()
            .unwrap_or(&a.name)
            .cmp(b.source_path.as_deref().unwrap_or(&b.name))
    });

    Ok(deployed)
}

#[tauri::command]
pub async fn deploy_skill(
    app: AppHandle,
    source_path: String,
    target_type: String,
    target_id: String,
) -> Result<(), String> {
    let home = get_wardian_home().ok_or("Could not find Wardian home")?;
    library::deploy_skill(&home, &source_path, &target_type, &target_id)?;
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
    let home = get_wardian_home().ok_or("Could not find Wardian home")?;
    library::remove_deployed_skill(&home, &target_type, &target_id, &skill_name)?;
    refresh_live_antigravity_skill_projections(&app).await;
    Ok(())
}

#[tauri::command]
pub async fn list_deployed_skills(
    _app: AppHandle,
    target_type: String,
    target_id: String,
) -> Result<Vec<String>, String> {
    let home = get_wardian_home().ok_or("Could not find Wardian home")?;
    let target_skills_dir = library::get_target_skills_dir(&home, &target_type, &target_id)?;
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
    let home = get_wardian_home().ok_or("Could not find Wardian home")?;
    let sources = library::collect_skill_sources(&home);
    let scan = library::scan_deployments(&home, &sources);

    let mut deployments = Vec::new();
    match source_path.as_deref() {
        Some(source_path) => {
            for target in scan.deployments.get(source_path).into_iter().flatten() {
                deployments.push(SkillDeployment {
                    target_type: target.target_type.clone(),
                    target_id: target.target_id.clone(),
                });
            }
        }
        None => {
            for (rel_path, targets) in &scan.deployments {
                if skill_name_from_rel(rel_path) != skill_name {
                    continue;
                }
                for target in targets {
                    deployments.push(SkillDeployment {
                        target_type: target.target_type.clone(),
                        target_id: target.target_id.clone(),
                    });
                }
            }
            for orphan in &scan.orphans {
                if orphan.skill_name == skill_name {
                    deployments.push(SkillDeployment {
                        target_type: orphan.target_type.clone(),
                        target_id: orphan.target_id.clone(),
                    });
                }
            }
        }
    }

    Ok(deployments)
}

#[tauri::command]
pub async fn set_skill_deployments(
    app: AppHandle,
    source_path: String,
    targets: Vec<SkillDeployment>,
) -> Result<(), String> {
    let home = get_wardian_home().ok_or("Could not find Wardian home")?;
    // The core engine's per-target add/remove outcome counts are useful for
    // logging/telemetry but not part of this command's contract; discard
    // them here. The Antigravity refresh only needs to run once, after the
    // whole desired set has been reconciled (this used to run once per
    // deploy/undeploy call from the frontend, which was O(n) refreshes for
    // one save).
    let _outcome = library::set_skill_deployments(&home, &source_path, &targets)?;
    refresh_live_antigravity_skill_projections(&app).await;
    Ok(())
}

#[tauri::command]
pub async fn remove_orphan_deployment(
    _app: AppHandle,
    target_type: String,
    target_id: String,
    skill_name: String,
) -> Result<(), String> {
    let home = get_wardian_home().ok_or("Could not find Wardian home")?;
    library::remove_orphan_deployment(&home, &target_type, &target_id, &skill_name)
}

// --- Watching (commands) ---------------------------------------------------

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
    let library_root = ensure_library_watch_dir(&home, &library_type)?;
    let watch_targets = discover_library_watch_targets(&home, &library_root)?;
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
                if let Ok(library_root) = library_dir_for_type(&home, &task_library_type) {
                    if let Ok(next_targets) = discover_library_watch_targets(&home, &library_root) {
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

            refresh_live_antigravity_skill_projections(&app_handle).await;

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
    fn library_watch_helpers_resolve_and_create_library_dir() {
        let temp = tempfile::tempdir().expect("temp dir");
        let library_dir = temp.path().join("library");

        assert_eq!(
            library_dir_for_type(temp.path(), "library").expect("library dir"),
            library_dir
        );
        assert!(library_dir_for_type(temp.path(), "skills").is_err());

        let ensured = ensure_library_watch_dir(temp.path(), "library").expect("ensure library dir");
        assert_eq!(ensured, library_dir);
        assert!(library_dir.is_dir());
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
    fn discover_library_watch_targets_includes_library_and_classes_roots() {
        let temp = tempfile::tempdir().expect("temp dir");
        let home = temp.path();
        let library_root = home.join("library");
        let external_skill = home.join("external").join("planner");
        let linked_skill = library_root.join("skills").join("planner");

        fs::create_dir_all(&library_root).expect("library root");
        fs::create_dir_all(home.join("classes")).expect("classes root");
        fs::create_dir_all(&external_skill).expect("external skill dir");
        fs::write(external_skill.join("SKILL.md"), "linked").expect("external skill file");
        fs::create_dir_all(library_root.join("skills")).expect("skills dir");
        create_directory_link(&external_skill, &linked_skill).expect("linked skill");

        let targets = discover_library_watch_targets(home, &library_root).expect("watch targets");

        let canonical_library = library_root.canonicalize().expect("canonical library");
        let canonical_classes = home
            .join("classes")
            .canonicalize()
            .expect("canonical classes");
        let canonical_external = external_skill.canonicalize().expect("canonical external");

        assert!(targets.contains(&canonical_library));
        assert!(targets.contains(&canonical_classes));
        assert!(targets.contains(&canonical_external));
    }

    #[test]
    fn library_watch_generation_is_not_current_after_registration_removed() {
        let state = AppState::new();
        assert!(!is_current_library_watch_generation(&state, "library", 1));
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
