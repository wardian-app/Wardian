use crate::utils::fs::*;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager};
use wardian_core::models::AgentClassDefinition;

const BUNDLED_COMMON_SKILLS: &[&str] = &["wardian-skills/wardian-cli"];
const BUNDLED_AUTOMATION_SAMPLES: &[&str] = &[
    "code-change-review.md",
    "scheduled-brief.md",
    "research-brief.md",
    "incident-triage.md",
    "conversation-pattern-review.md",
    "memory-consolidation.md",
];

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
        init_bundled_automation_samples(app, &app_dir);

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

fn bundled_library_automations_root(app: &AppHandle) -> Option<PathBuf> {
    app.path()
        .resolve(
            "resources/library/automations",
            tauri::path::BaseDirectory::Resource,
        )
        .ok()
        .filter(|path| path.exists())
}

fn seed_bundled_automation_sample(
    source_root: &Path,
    app_dir: &Path,
    sample_name: &str,
) -> Result<(), String> {
    let source = source_root.join(sample_name);
    if !source.is_file() {
        return Err(format!(
            "Bundled automation sample is missing: {}",
            source.display()
        ));
    }

    let destination = app_dir
        .join("library")
        .join("automations")
        .join("samples")
        .join(sample_name);
    if destination.exists() {
        return Ok(());
    }

    let parent = destination
        .parent()
        .ok_or_else(|| format!("Automation sample has no parent: {}", destination.display()))?;
    std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    std::fs::copy(&source, &destination).map_err(|error| error.to_string())?;
    Ok(())
}

fn init_bundled_automation_samples(app: &AppHandle, app_dir: &Path) {
    let Some(source_root) = bundled_library_automations_root(app) else {
        return;
    };

    for sample_name in BUNDLED_AUTOMATION_SAMPLES {
        if let Err(error) = seed_bundled_automation_sample(&source_root, app_dir, sample_name) {
            crate::manager::log_debug(&format!(
                "[Wardian] Failed to seed bundled automation sample {sample_name}: {error}"
            ));
        }
    }
}

pub fn get_agent_class_default_instruction(_app: &AppHandle, class_name: &str) -> Option<String> {
    wardian_core::classes::default_class_instruction(class_name).map(ToOwned::to_owned)
}

#[cfg(test)]
mod tests {
    use super::{seed_bundled_automation_sample, seed_bundled_common_skill};
    use std::fs;
    use std::path::Path;

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
        let source_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("resources/library/skills");
        let temp = tempfile::tempdir().expect("temp dir");
        seed_bundled_common_skill(&source_root, temp.path(), "wardian-skills/wardian-cli")
            .expect("seed actual bundled skill");

        for deployed in [
            temp.path()
                .join("library/skills/wardian-skills/wardian-cli"),
            temp.path().join("common/.agents/skills/wardian-cli"),
        ] {
            let mut pending = vec![deployed.join("SKILL.md")];
            let mut reached = std::collections::HashSet::new();
            while let Some(path) = pending.pop() {
                if !reached.insert(path.clone()) {
                    continue;
                }
                let content = fs::read_to_string(&path).expect("reachable packaged document");
                assert!(
                    !content.trim().is_empty(),
                    "empty document: {}",
                    path.display()
                );
                // Follow the package's relative Markdown links transitively, so
                // conditional guidance need not be duplicated in the entrypoint.
                for link in content.split("](").skip(1) {
                    let target = link.split(')').next().expect("link target");
                    let target = target.split('#').next().expect("link path");
                    if target.ends_with(".md") && !target.contains("://") {
                        pending.push(path.parent().expect("document parent").join(target));
                    }
                }
            }
            for entry in fs::read_dir(deployed.join("references")).expect("packaged references") {
                let path = entry.expect("reference entry").path();
                if path.extension().is_some_and(|extension| extension == "md") {
                    assert!(
                        reached.contains(&path),
                        "unreachable reference: {}",
                        path.display()
                    );
                }
            }
        }
    }

    // These are content-contract guards, not a claim to understand arbitrary
    // prose. Check related concepts in one paragraph without freezing line wraps,
    // command catalogues, or which document contains conditional instructions.
    fn assert_guidance_paragraph(documents: &[&str], concepts: &[&str], contract: &str) {
        assert!(
            documents.iter().any(|document| {
                document
                    .replace("\r\n", "\n")
                    .split("\n\n")
                    .any(|paragraph| {
                        let normalized = paragraph
                            .split_whitespace()
                            .collect::<Vec<_>>()
                            .join(" ")
                            .to_lowercase();
                        concepts.iter().all(|concept| normalized.contains(*concept))
                    })
            }),
            "packaged guidance must retain {contract}"
        );
    }

    #[test]
    fn bundled_wardian_cli_skill_preserves_messaging_safety_contract() {
        const ROOT: &str =
            include_str!("../../resources/library/skills/wardian-skills/wardian-cli/SKILL.md");
        const MESSAGING: &str = include_str!(
            "../../resources/library/skills/wardian-skills/wardian-cli/references/messaging.md"
        );

        for (concepts, contract) in [
            (
                &["authenticated", "managed", "never", "impersonate"][..],
                "managed sender authentication",
            ),
            (
                &["one exact", "broadcast", "unsupported"][..],
                "single recipient without legacy selectors",
            ),
            (
                &[
                    "parent_interaction_id",
                    "saved id",
                    "do not infer completion",
                ][..],
                "correlated task completion",
            ),
            (
                &["provider commands", "approval actions", "not peer"][..],
                "human provider controls remain separate",
            ),
            (
                &["receive timeout", "never resubmits", "uncertainty"][..],
                "no automatic replay after timeout",
            ),
        ] {
            assert_guidance_paragraph(&[ROOT, MESSAGING], concepts, contract);
        }
    }

    #[test]
    fn bundled_wardian_cli_skill_surfaces_automation_opportunities_without_acting() {
        const ROOT: &str =
            include_str!("../../resources/library/skills/wardian-skills/wardian-cli/SKILL.md");

        const AUTOMATIONS: &str = include_str!(
            "../../resources/library/skills/wardian-skills/wardian-cli/references/automations.md"
        );
        for (concepts, contract) in [
            (
                &["suggest", "automation", "recurring"][..],
                "automation discovery",
            ),
            (
                &[
                    "do not",
                    "create",
                    "edit",
                    "schedule",
                    "run",
                    "user opts in",
                ][..],
                "opt-in before automation mutation",
            ),
            (&["one-off", "direct"][..], "direct one-off work"),
        ] {
            assert_guidance_paragraph(&[ROOT, AUTOMATIONS], concepts, contract);
        }
    }

    #[test]
    fn bundled_wardian_cli_skill_preserves_provider_model_constraints() {
        const ROOT: &str =
            include_str!("../../resources/library/skills/wardian-skills/wardian-cli/SKILL.md");
        const AGENTS: &str = include_str!(
            "../../resources/library/skills/wardian-skills/wardian-cli/references/agents.md"
        );
        for (concepts, contract) in [
            (
                &["routine", "model", "effort", "provider default"][..],
                "provider defaults for routine tasks",
            ),
            (
                &["complex", "catalogue", "lists", "do not guess"][..],
                "catalogue-backed model and effort overrides",
            ),
            (
                &["restart_required", "running provider"][..],
                "restart before relying on changed selection",
            ),
        ] {
            assert_guidance_paragraph(&[ROOT, AGENTS], concepts, contract);
        }
    }

    #[test]
    fn bundled_automation_sample_is_seeded_without_overwriting_a_user_edit() {
        let temp = tempfile::tempdir().expect("temp dir");
        let source_root = temp
            .path()
            .join("resources")
            .join("library")
            .join("automations");
        std::fs::create_dir_all(&source_root).expect("create source root");
        std::fs::write(source_root.join("sample.md"), "bundled sample").expect("write sample");
        let app_dir = temp.path().join("home");

        seed_bundled_automation_sample(&source_root, &app_dir, "sample.md").expect("seed sample");
        let destination = app_dir.join("library/automations/samples/sample.md");
        assert_eq!(
            std::fs::read_to_string(&destination).expect("read sample"),
            "bundled sample"
        );

        std::fs::write(&destination, "user edit").expect("edit sample");
        seed_bundled_automation_sample(&source_root, &app_dir, "sample.md")
            .expect("preserve user edit");
        assert_eq!(
            std::fs::read_to_string(destination).expect("read user edit"),
            "user edit"
        );
    }

    #[test]
    fn bundled_automation_samples_are_valid_blueprints() {
        for (name, sample) in [
            (
                "code-change-review",
                include_str!("../../resources/library/automations/code-change-review.md"),
            ),
            (
                "scheduled-brief",
                include_str!("../../resources/library/automations/scheduled-brief.md"),
            ),
            (
                "research-brief",
                include_str!("../../resources/library/automations/research-brief.md"),
            ),
            (
                "incident-triage",
                include_str!("../../resources/library/automations/incident-triage.md"),
            ),
            (
                "conversation-pattern-review",
                include_str!("../../resources/library/automations/conversation-pattern-review.md"),
            ),
            (
                "memory-consolidation",
                include_str!("../../resources/library/automations/memory-consolidation.md"),
            ),
        ] {
            let blueprint = wardian_core::automation::parse_str(sample)
                .unwrap_or_else(|error| panic!("{name} sample should parse: {error}"));
            let report = wardian_core::automation::validate(&blueprint);
            assert!(
                report.is_valid(),
                "{name} sample should validate: {:?}",
                report.errors()
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
