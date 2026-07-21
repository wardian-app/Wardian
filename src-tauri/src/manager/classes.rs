use crate::utils::fs::*;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager};
use wardian_core::models::AgentClassDefinition;

const BUNDLED_COMMON_SKILLS: &[&str] = &["wardian-skills/wardian-cli"];

pub fn get_all_agent_classes(_app: &AppHandle) -> Vec<AgentClassDefinition> {
    get_wardian_home()
        .and_then(|app_dir| wardian_core::classes::load_class_definitions(&app_dir).ok())
        .unwrap_or_default()
}

pub fn save_classes(_app: &AppHandle, classes: &[AgentClassDefinition]) -> Result<(), String> {
    let app_dir = get_wardian_home().ok_or("No home dir")?;
    wardian_core::classes::save_class_definitions(&app_dir, classes)
}

pub fn init_agent_classes(app: &AppHandle) {
    if let Some(app_dir) = get_wardian_home() {
        let classes_dir = app_dir.join("classes");
        let _ = std::fs::create_dir_all(&classes_dir);
        let _ = std::fs::create_dir_all(app_dir.join("common/desk"));
        let _ = std::fs::create_dir_all(app_dir.join("common/lineages"));

        // Keep `.agents/skills` canonical while exposing provider-specific discovery shims.
        ensure_claude_skills_link(&app_dir.join("common"));
        init_bundled_common_skills(app, &app_dir);

        let classes_path = app_dir.join("classes.json");

        // Migration and Initialization
        if !classes_path.exists() {
            let mut defaults = wardian_core::classes::default_class_definitions();

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

        if let Ok(classes) = wardian_core::classes::initialize_classes(&app_dir) {
            for cls in &classes {
                let role_dir = classes_dir.join(&cls.name);

                // Expose canonical skills through provider-specific discovery shims.
                ensure_claude_skills_link(&role_dir);
            }
        }
    }
}

fn remove_existing_path(path: &Path) -> std::io::Result<()> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };

    if is_directory_link(&metadata) {
        return std::fs::remove_dir(path).or_else(|_| std::fs::remove_file(path));
    }

    if metadata.is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    }
}

#[cfg(windows)]
fn is_directory_link(metadata: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;

    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    metadata.is_dir() && metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn is_directory_link(metadata: &std::fs::Metadata) -> bool {
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
    if !source_rel_path.starts_with("wardian-skills/") {
        return Err(format!(
            "Bundled common skills must live under wardian-skills/: {source_rel_path}"
        ));
    }

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

pub fn get_agent_class_default_instruction(_app: &AppHandle, class_name: &str) -> Option<String> {
    wardian_core::classes::default_class_instruction(class_name).map(ToOwned::to_owned)
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
    fn bundled_common_skill_copies_reference_material() {
        let temp = tempfile::tempdir().expect("temp dir");
        let source_root = temp.path().join("resources").join("library").join("skills");
        let source = source_root.join("wardian-skills").join("wardian-cli");
        let reference = source.join("references").join("messaging.md");
        fs::create_dir_all(reference.parent().expect("reference parent")).expect("source dir");
        fs::write(source.join("SKILL.md"), "bundled").expect("source skill");
        fs::write(&reference, "messaging instructions").expect("reference material");

        let app_dir = temp.path().join("home");
        seed_bundled_common_skill(&source_root, &app_dir, "wardian-skills/wardian-cli")
            .expect("seed bundled skill");

        for deployed_skill in [
            app_dir
                .join("library")
                .join("skills")
                .join("wardian-skills")
                .join("wardian-cli"),
            app_dir
                .join("common")
                .join(".agents")
                .join("skills")
                .join("wardian-cli"),
        ] {
            assert_eq!(
                fs::read_to_string(deployed_skill.join("references").join("messaging.md"))
                    .expect("reference material"),
                "messaging instructions"
            );
        }
    }

    #[test]
    fn bundled_wardian_cli_skill_routes_to_packaged_references() {
        const ROOT: &str =
            include_str!("../../resources/library/skills/wardian-skills/wardian-cli/SKILL.md");

        for (link, reference) in [
            (
                "references/agents.md",
                include_str!(
                    "../../resources/library/skills/wardian-skills/wardian-cli/references/agents.md"
                ),
            ),
            (
                "references/orchestration.md",
                include_str!(
                    "../../resources/library/skills/wardian-skills/wardian-cli/references/orchestration.md"
                ),
            ),
            (
                "references/topology.md",
                include_str!(
                    "../../resources/library/skills/wardian-skills/wardian-cli/references/topology.md"
                ),
            ),
            (
                "references/messaging.md",
                include_str!(
                    "../../resources/library/skills/wardian-skills/wardian-cli/references/messaging.md"
                ),
            ),
            (
                "references/assets.md",
                include_str!(
                    "../../resources/library/skills/wardian-skills/wardian-cli/references/assets.md"
                ),
            ),
            (
                "references/workflows.md",
                include_str!(
                    "../../resources/library/skills/wardian-skills/wardian-cli/references/workflows.md"
                ),
            ),
            (
                "references/coordination-groups.md",
                include_str!(
                    "../../resources/library/skills/wardian-skills/wardian-cli/references/coordination-groups.md"
                ),
            ),
            (
                "references/runtime-debugging.md",
                include_str!(
                    "../../resources/library/skills/wardian-skills/wardian-cli/references/runtime-debugging.md"
                ),
            ),
        ] {
            assert!(ROOT.contains(link), "root skill must route to {link}");
            assert!(
                !reference.trim().is_empty(),
                "routed reference must contain instructions: {link}"
            );
        }

        for command in [
            "`wardian agent`",
            "`wardian agent wait`",
            "`wardian agent watch`",
            "`wardian graph`",
            "`wardian send`",
            "`wardian ask`",
            "`wardian reply`",
            "`wardian conversation`",
            "`wardian library`",
            "`wardian artifact`",
            "`wardian workflow`",
            "`wardian team`",
            "`wardian watchlist`",
        ] {
            assert!(
                ROOT.contains(command),
                "root skill must route top-level CLI command: {command}"
            );
        }
    }

    #[test]
    fn bundled_wardian_cli_skill_preserves_messaging_safety_contract() {
        const ROOT: &str =
            include_str!("../../resources/library/skills/wardian-skills/wardian-cli/SKILL.md");
        const MESSAGING: &str = include_str!(
            "../../resources/library/skills/wardian-skills/wardian-cli/references/messaging.md"
        );

        for required_root_instruction in [
            "Keep broadcasts and class sends neighbor-scoped",
            "`ask` accepts one named peer or UUID, never a broadcast",
            "Use `send --as-command` only for one explicit agent or UUID",
        ] {
            assert!(
                ROOT.contains(required_root_instruction),
                "root skill must retain messaging safety instruction: {required_root_instruction}"
            );
        }

        for required_messaging_instruction in [
            "Use `wardian send` for one-way inter-agent communication",
            "`all` and class targets resolve among neighbors",
            "`--queue-policy queue-if-busy` is the default",
            "Use an approval action only to answer an outstanding provider approval",
            "Use `ask` when the task needs a named peer's accountable result",
            "wardian ask reviewer-a1 --file review-request.md --timeout 10m",
            "wardian reply ask_0123456789abcdef --status blocked --file findings.md",
            "broadcasts, class selectors, and `--thread` are unsupported",
        ] {
            assert!(
                MESSAGING.contains(required_messaging_instruction),
                "messaging reference must retain safety instruction: {required_messaging_instruction}"
            );
        }
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
