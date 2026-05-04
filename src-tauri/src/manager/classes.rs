use crate::utils::fs::*;
use std::fs;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager};
use wardian_core::models::AgentClassDefinition;

const BUNDLED_COMMON_SKILLS: &[&str] = &["wardian-skills/wardian-cli"];

pub fn get_all_agent_classes(_app: &AppHandle) -> Vec<AgentClassDefinition> {
    if let Some(app_dir) = get_wardian_home() {
        let classes_path = app_dir.join("classes.json");
        if let Ok(data) = std::fs::read_to_string(&classes_path) {
            return serde_json::from_str::<Vec<AgentClassDefinition>>(&data).unwrap_or_default();
        }
    }
    Vec::new()
}

pub fn save_classes(_app: &AppHandle, classes: &[AgentClassDefinition]) -> Result<(), String> {
    let app_dir = get_wardian_home().ok_or("No home dir")?;
    let json = serde_json::to_string_pretty(classes).map_err(|e| e.to_string())?;
    std::fs::write(app_dir.join("classes.json"), json).map_err(|e| e.to_string())?;
    Ok(())
}

pub fn init_agent_classes(app: &AppHandle) {
    if let Some(app_dir) = get_wardian_home() {
        let classes_dir = app_dir.join("classes");
        let _ = std::fs::create_dir_all(&classes_dir);
        let _ = std::fs::create_dir_all(app_dir.join("common/desk"));
        let _ = std::fs::create_dir_all(app_dir.join("common/lineages"));

        // Ensure Claude can discover skills from the canonical .agents/skills/ location
        ensure_claude_skills_link(&app_dir.join("common"));
        init_bundled_common_skills(app, &app_dir);

        let classes_path = app_dir.join("classes.json");

        // Migration and Initialization
        if !classes_path.exists() {
            let mut defaults: Vec<AgentClassDefinition> =
                serde_json::from_str(include_str!("../default_classes.json")).unwrap_or_default();
            for d in defaults.iter_mut() {
                d.is_default = true;
            }

            let custom_path = app_dir.join("custom_classes.json");
            if custom_path.exists() {
                if let Ok(data) = std::fs::read_to_string(&custom_path) {
                    let mut custom = serde_json::from_str::<Vec<AgentClassDefinition>>(&data)
                        .unwrap_or_default();
                    for c in custom.iter_mut() {
                        c.is_default = false;
                    }
                    defaults.extend(custom);
                }
                // We've successfully merged. We could delete custom_classes.json here.
                let _ = std::fs::remove_file(&custom_path);
            }

            let _ = save_classes(app, &defaults);
        }

        let classes = get_all_agent_classes(app);
        for cls in &classes {
            let role_dir = classes_dir.join(&cls.name);
            let _ = std::fs::create_dir_all(&role_dir);

            // 1. Create AGENTS.md master file
            let agents_md_path = role_dir.join("AGENTS.md");
            if !agents_md_path.exists() {
                let content = if cls.is_default {
                    app.path()
                        .resolve(
                            format!("agent_prompts/{}.md", cls.name),
                            tauri::path::BaseDirectory::Resource,
                        )
                        .ok()
                        .and_then(|p| std::fs::read_to_string(p).ok())
                        .unwrap_or_default()
                } else {
                    format!("# {} Agent\n\n{}\n", cls.name, cls.description)
                };
                let _ = std::fs::write(agents_md_path, content);
            }

            // 2. Symlink .claude/skills/ → .agents/skills/ for Claude discovery
            ensure_claude_skills_link(&role_dir);

            // 3. Create provider stub files for providers that do not read AGENTS.md directly
            for stub_name in &["GEMINI.md", "CLAUDE.md"] {
                let stub_path = role_dir.join(stub_name);
                if !stub_path.exists() {
                    let _ = std::fs::write(stub_path, "@AGENTS.md\n");
                }
            }
        }
    }
}

fn copy_dir_all(src: &Path, dst: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let dst_entry = dst.join(entry.file_name());
        if ty.is_dir() {
            copy_dir_all(&entry.path(), &dst_entry)?;
        } else {
            std::fs::copy(entry.path(), dst_entry)?;
        }
    }
    Ok(())
}

fn remove_existing_path(path: &Path) -> std::io::Result<()> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };

    if is_directory_link(&metadata) {
        return fs::remove_dir(path).or_else(|_| fs::remove_file(path));
    }

    if metadata.is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    }
}

#[cfg(windows)]
fn is_directory_link(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;

    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    metadata.is_dir() && metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn is_directory_link(metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}

fn bundled_library_skills_root(app: &AppHandle) -> Option<PathBuf> {
    app.path()
        .resolve(
            "resources/library/skills",
            tauri::path::BaseDirectory::Resource,
        )
        .ok()
        .filter(|path| path.exists())
}

fn seed_bundled_common_skill(
    source_root: &Path,
    app_dir: &Path,
    source_rel_path: &str,
) -> Result<(), String> {
    let source = source_root.join(source_rel_path);
    if !source.join("SKILL.md").is_file() {
        return Err(format!(
            "Bundled skill source is missing SKILL.md: {}",
            source.display()
        ));
    }

    let library_skill = app_dir.join("library").join("skills").join(source_rel_path);
    remove_existing_path(&library_skill).map_err(|e| e.to_string())?;
    copy_dir_all(&source, &library_skill).map_err(|e| e.to_string())?;

    let skill_name = Path::new(source_rel_path)
        .file_name()
        .ok_or_else(|| format!("Bundled skill path has no final component: {source_rel_path}"))?;
    let common_skill = app_dir
        .join("common")
        .join(".agents")
        .join("skills")
        .join(skill_name);
    remove_existing_path(&common_skill).map_err(|e| e.to_string())?;
    create_directory_link(&library_skill, &common_skill).or_else(|link_error| {
        crate::manager::log_debug(&format!(
            "[Wardian] Failed to link bundled skill {:?} to {:?}; falling back to copy: {}",
            library_skill, common_skill, link_error
        ));
        copy_dir_all(&library_skill, &common_skill).map_err(|copy_error| copy_error.to_string())
    })?;

    Ok(())
}

fn init_bundled_common_skills(app: &AppHandle, app_dir: &Path) {
    let Some(source_root) = bundled_library_skills_root(app) else {
        return;
    };

    for source_rel_path in BUNDLED_COMMON_SKILLS {
        if let Err(error) = seed_bundled_common_skill(&source_root, app_dir, source_rel_path) {
            crate::manager::log_debug(&format!(
                "[Wardian] Failed to seed bundled skill {source_rel_path}: {error}"
            ));
        }
    }
}

pub fn get_agent_class_default_instruction(app: &AppHandle, class_name: &str) -> Option<String> {
    app.path()
        .resolve(
            format!("agent_prompts/{}.md", class_name),
            tauri::path::BaseDirectory::Resource,
        )
        .ok()
        .and_then(|p| std::fs::read_to_string(p).ok())
}

#[cfg(test)]
mod tests {
    use super::seed_bundled_common_skill;
    use std::fs;

    #[test]
    fn bundled_common_skill_is_copied_to_library_and_deployed_to_common() {
        let temp = tempfile::tempdir().expect("temp dir");
        let source_root = temp.path().join("resources").join("library").join("skills");
        let source = source_root.join("wardian-skills").join("wardian-cli");
        fs::create_dir_all(&source).expect("source dir");
        fs::write(source.join("SKILL.md"), "bundled").expect("source skill");

        let app_dir = temp.path().join("home");
        seed_bundled_common_skill(&source_root, &app_dir, "wardian-skills/wardian-cli")
            .expect("seed bundled skill");

        assert_eq!(
            fs::read_to_string(
                app_dir
                    .join("library")
                    .join("skills")
                    .join("wardian-skills")
                    .join("wardian-cli")
                    .join("SKILL.md")
            )
            .expect("library skill"),
            "bundled"
        );
        assert_eq!(
            fs::read_to_string(
                app_dir
                    .join("common")
                    .join(".agents")
                    .join("skills")
                    .join("wardian-cli")
                    .join("SKILL.md")
            )
            .expect("common skill"),
            "bundled"
        );
    }

    #[test]
    fn bundled_wardian_skill_overwrites_existing_library_copy() {
        let temp = tempfile::tempdir().expect("temp dir");
        let source_root = temp.path().join("resources").join("library").join("skills");
        let source = source_root.join("wardian-skills").join("wardian-cli");
        fs::create_dir_all(&source).expect("source dir");
        fs::write(source.join("SKILL.md"), "bundled").expect("source skill");

        let app_dir = temp.path().join("home");
        let existing = app_dir
            .join("library")
            .join("skills")
            .join("wardian-skills")
            .join("wardian-cli");
        fs::create_dir_all(&existing).expect("existing dir");
        fs::write(existing.join("SKILL.md"), "custom").expect("existing skill");

        seed_bundled_common_skill(&source_root, &app_dir, "wardian-skills/wardian-cli")
            .expect("seed bundled skill");

        assert_eq!(
            fs::read_to_string(existing.join("SKILL.md")).expect("overwritten skill"),
            "bundled"
        );
        assert_eq!(
            fs::read_to_string(
                app_dir
                    .join("common")
                    .join(".agents")
                    .join("skills")
                    .join("wardian-cli")
                    .join("SKILL.md")
            )
            .expect("common skill"),
            "bundled"
        );
    }

    #[test]
    fn bundled_wardian_skill_overwrites_existing_common_copy() {
        let temp = tempfile::tempdir().expect("temp dir");
        let source_root = temp.path().join("resources").join("library").join("skills");
        let source = source_root.join("wardian-skills").join("wardian-cli");
        fs::create_dir_all(&source).expect("source dir");
        fs::write(source.join("SKILL.md"), "bundled").expect("source skill");

        let app_dir = temp.path().join("home");
        let common = app_dir
            .join("common")
            .join(".agents")
            .join("skills")
            .join("wardian-cli");
        fs::create_dir_all(&common).expect("common dir");
        fs::write(common.join("SKILL.md"), "stale").expect("common skill");

        seed_bundled_common_skill(&source_root, &app_dir, "wardian-skills/wardian-cli")
            .expect("seed bundled skill");

        assert_eq!(
            fs::read_to_string(common.join("SKILL.md")).expect("overwritten common skill"),
            "bundled"
        );
    }
}
