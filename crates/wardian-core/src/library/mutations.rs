use std::fs;
use std::path::{Path, PathBuf};

use crate::library::deployments::{collect_skill_sources, get_target_skills_dir, scan_deployments};
use crate::library::links::{create_directory_link, remove_existing_deployment};
use crate::library::metadata::MetadataStore;
use crate::library::section::{
    is_single_normal_component, resolve_entry_path, LibrarySectionId, DEPLOYED_SKILL_SOURCE_FILE,
};
use crate::models::LibraryItemMetadata;

/// Maps section + relative path to the actual content file path.
/// - Skills: `<dir>/SKILL.md`
/// - Classes: `<dir>/AGENTS.md`
/// - Prompts/Workflows: the path itself
pub(crate) fn content_file_path(
    home: &Path,
    section: LibrarySectionId,
    rel: &str,
) -> Result<PathBuf, String> {
    let base = resolve_entry_path(home, section, rel)?;
    Ok(match section {
        LibrarySectionId::Skills => base.join("SKILL.md"),
        LibrarySectionId::Classes => base.join("AGENTS.md"),
        _ => base,
    })
}

/// Validate that a mutation destination can be represented by the Library index.
pub fn validate_entry_destination(
    home: &Path,
    section: LibrarySectionId,
    rel: &str,
) -> Result<(), String> {
    let target = resolve_entry_path(home, section, rel)?;
    match section {
        LibrarySectionId::Prompts | LibrarySectionId::Workflows => {
            let is_markdown = target
                .extension()
                .map(|extension| extension.eq_ignore_ascii_case("md"))
                .unwrap_or(false);
            if !is_markdown {
                return Err(format!(
                    "{} entries must use a .md extension: {rel}",
                    section.as_str()
                ));
            }
        }
        LibrarySectionId::Skills => validate_skill_destination(home, &target, rel)?,
        LibrarySectionId::Classes => {}
        LibrarySectionId::Mcps => unreachable!("MCP paths are rejected by resolve_entry_path"),
    }
    Ok(())
}

fn validate_skill_destination(home: &Path, target: &Path, rel: &str) -> Result<(), String> {
    let root = LibrarySectionId::Skills.root_for_home(home);
    let relative = target
        .strip_prefix(&root)
        .map_err(|_| format!("Skill path is outside the Library: {rel}"))?;
    let mut ancestor = root;
    let components: Vec<_> = relative.components().collect();
    for component in components.iter().take(components.len().saturating_sub(1)) {
        ancestor.push(component.as_os_str());
        if ancestor.join("SKILL.md").is_file() {
            return Err(format!(
                "A skill cannot be nested inside another skill: {rel}"
            ));
        }
    }

    if target.is_dir() && !target.join("SKILL.md").is_file() && directory_contains_skill(target)? {
        return Err(format!(
            "A skill group containing other skills cannot also be a skill: {rel}"
        ));
    }
    Ok(())
}

fn directory_contains_skill(directory: &Path) -> Result<bool, String> {
    for entry in fs::read_dir(directory).map_err(|error| error.to_string())? {
        let path = entry.map_err(|error| error.to_string())?.path();
        if !path.is_dir() {
            continue;
        }
        if path.join("SKILL.md").is_file() || directory_contains_skill(&path)? {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Read the content of a library item.
/// - Skills read `<dir>/SKILL.md`
/// - Classes read `<dir>/AGENTS.md`
/// - Prompts/Workflows read the file itself
pub fn read_item(home: &Path, section: LibrarySectionId, rel: &str) -> Result<String, String> {
    let path = content_file_path(home, section, rel)?;
    fs::read_to_string(&path).map_err(|e| e.to_string())
}

/// Save content to a library item.
/// Creates parent directories as needed.
/// - Skills write to `<dir>/SKILL.md`
/// - Classes write to `<dir>/AGENTS.md`
/// - Prompts/Workflows write to the file itself
pub fn save_item(
    home: &Path,
    section: LibrarySectionId,
    rel: &str,
    content: &str,
) -> Result<(), String> {
    validate_entry_destination(home, section, rel)?;
    let path = content_file_path(home, section, rel)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    fs::write(&path, content).map_err(|e| e.to_string())
}

/// Create a folder for a library item.
/// Errors for Classes (flat) and Mcps (stubbed, via `resolve_entry_path`).
pub fn create_folder(home: &Path, section: LibrarySectionId, rel: &str) -> Result<(), String> {
    if section == LibrarySectionId::Classes {
        return Err("Classes section is flat; cannot create folders".to_string());
    }
    let path = resolve_entry_path(home, section, rel)?;
    fs::create_dir_all(&path).map_err(|e| e.to_string())
}

/// Update metadata for a library item.
/// `entry_ref` must be section-qualified (e.g. `skills/dev/planner`) and
/// resolve to a valid, non-traversing path within that section; this keeps
/// `MetadataStore`'s legacy-key migration from having to guess at malformed
/// or unqualified keys on the next load.
pub fn update_metadata(
    home: &Path,
    entry_ref: &str,
    metadata: LibraryItemMetadata,
) -> Result<(), String> {
    let (section_name, rel) = entry_ref
        .split_once('/')
        .ok_or_else(|| format!("Entry ref must be section-qualified: {entry_ref}"))?;
    let section = LibrarySectionId::parse(section_name)
        .ok_or_else(|| format!("Unknown library section in entry ref: {entry_ref}"))?;
    resolve_entry_path(home, section, rel)?;
    let mut store = MetadataStore::load(home);
    store.set(entry_ref.to_string(), metadata);
    store.save(home)
}

/// The final path component of a section-relative path, used as the
/// deployed skill directory's name (e.g. `dev/planner` -> `planner`).
fn last_component(rel: &str) -> String {
    Path::new(rel)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default()
}

/// Rename or move a library entry, re-linking/re-marking every deployed
/// copy of a renamed skill and migrating its metadata key.
///
/// Renaming Classes is rejected: class identity is referenced by agents
/// elsewhere, so it's out of scope here.
///
/// Recovery semantics: the source rename and metadata key migration happen
/// first and atomically-in-effect from the caller's perspective — once
/// either has happened, both have. Re-linking/re-marking deployed copies is
/// best-effort after that: each target is attempted independently, and a
/// failure on one target does not stop the others. On partial failure this
/// returns `Err` describing which targets failed, but the source has
/// already moved and metadata already points at the new location. For a
/// linked target, `remove_existing_deployment` may succeed and the
/// subsequent `create_directory_link` then fail — in that case NO
/// deployment remains at the target at all (the old link was removed, and
/// no new one took its place), not a stale deployment of the old name. This
/// can be repaired by re-deploying the skill to those targets.
pub fn rename_entry(
    home: &Path,
    section: LibrarySectionId,
    from_rel: &str,
    to_rel: &str,
) -> Result<(), String> {
    if section == LibrarySectionId::Classes {
        return Err("Renaming classes is not supported".to_string());
    }

    validate_entry_destination(home, section, to_rel)?;

    let from_path = resolve_entry_path(home, section, from_rel)?;
    let to_path = resolve_entry_path(home, section, to_rel)?;
    let from_norm = from_rel.replace('\\', "/");
    let to_norm = to_rel.replace('\\', "/");

    // Scan deployments BEFORE the source moves: `scan_deployments` resolves
    // deployed dirs back to sources by marker/canonical path, and the
    // canonical match breaks the instant the source is renamed away.
    let deployment_targets = if section == LibrarySectionId::Skills {
        let sources = collect_skill_sources(home);
        scan_deployments(home, &sources)
            .deployments
            .remove(from_norm.as_str())
            .unwrap_or_default()
    } else {
        Vec::new()
    };

    if let Some(parent) = to_path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    fs::rename(&from_path, &to_path).map_err(|e| e.to_string())?;

    // Metadata migration happens immediately after the source actually
    // moves, so it always tracks reality even if re-linking deployments
    // below partially fails.
    let mut store = MetadataStore::load(home);
    store.rename(
        &format!("{}/{from_norm}", section.as_str()),
        &format!("{}/{to_norm}", section.as_str()),
    );
    store.save(home)?;

    if section == LibrarySectionId::Skills {
        // The deployed directory name always mirrors the source's own
        // file_name: `planner` -> `dev/planner` keeps the name `planner`,
        // but `planner` -> `strategist` changes it.
        let old_name = last_component(&from_norm);
        let new_name = last_component(&to_norm);
        let mut errors: Vec<String> = Vec::new();
        for target in &deployment_targets {
            let result = (|| -> Result<(), String> {
                let skills_dir =
                    get_target_skills_dir(home, &target.target_type, &target.target_id)?;
                let old_target_path = skills_dir.join(&old_name);
                let new_target_path = skills_dir.join(&new_name);
                if target.linked {
                    remove_existing_deployment(&old_target_path).map_err(|e| e.to_string())?;
                    create_directory_link(&to_path, &new_target_path).map_err(|e| e.to_string())?;
                } else {
                    if old_target_path != new_target_path {
                        if let Some(parent) = new_target_path.parent() {
                            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
                        }
                        fs::rename(&old_target_path, &new_target_path)
                            .map_err(|e| e.to_string())?;
                    }
                    fs::write(new_target_path.join(DEPLOYED_SKILL_SOURCE_FILE), &to_norm)
                        .map_err(|e| e.to_string())?;
                }
                Ok(())
            })();
            if let Err(e) = result {
                errors.push(format!("{}/{}: {e}", target.target_type, target.target_id));
            }
        }
        if !errors.is_empty() {
            return Err(format!(
                "Skill moved, but {} deployment(s) could not be updated: {}",
                errors.len(),
                errors.join("; ")
            ));
        }
    }

    Ok(())
}

/// Delete a library entry. For skills, every deployment target is removed
/// first so nothing is left dangling, then the source itself.
///
/// Deleting Classes is rejected here: class deletion goes through the
/// existing `delete_agent_class` flow, which also cleans up agent
/// references.
pub fn delete_entry(home: &Path, section: LibrarySectionId, rel: &str) -> Result<(), String> {
    if section == LibrarySectionId::Classes {
        return Err("Deleting classes is not supported here; use delete_agent_class".to_string());
    }

    let path = resolve_entry_path(home, section, rel)?;
    let rel_norm = rel.replace('\\', "/");

    if section == LibrarySectionId::Skills {
        let sources = collect_skill_sources(home);
        let targets = scan_deployments(home, &sources)
            .deployments
            .remove(rel_norm.as_str())
            .unwrap_or_default();
        let name = last_component(&rel_norm);
        for target in &targets {
            let skills_dir = get_target_skills_dir(home, &target.target_type, &target.target_id)?;
            remove_existing_deployment(&skills_dir.join(&name)).map_err(|e| e.to_string())?;
        }
    }

    remove_existing_deployment(&path).map_err(|e| e.to_string())?;

    let mut store = MetadataStore::load(home);
    store.remove(&format!("{}/{rel_norm}", section.as_str()));
    store.save(home)
}

/// Remove a deployed skill directory that no longer resolves to any
/// library source (an orphan reported by `scan_deployments`).
pub fn remove_orphan_deployment(
    home: &Path,
    target_type: &str,
    target_id: &str,
    skill_name: &str,
) -> Result<bool, String> {
    if !is_single_normal_component(skill_name) {
        return Err(format!("Invalid skill name: {skill_name}"));
    }
    let sources = collect_skill_sources(home);
    let scan = scan_deployments(home, &sources);
    let is_orphan = scan.orphans.iter().any(|orphan| {
        orphan.target_type == target_type
            && orphan.target_id == target_id
            && orphan.skill_name == skill_name
    });
    if !is_orphan {
        return Ok(false);
    }

    let skills_dir = get_target_skills_dir(home, target_type, target_id)?;
    remove_existing_deployment(&skills_dir.join(skill_name))
        .map(|()| true)
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::section::LibrarySectionId;

    #[test]
    fn read_save_maps_content_files_per_section() {
        let temp = tempfile::tempdir().expect("temp");
        let home = temp.path();
        save_item(home, LibrarySectionId::Skills, "dev/planner", "skill body").unwrap();
        save_item(home, LibrarySectionId::Prompts, "greet.md", "prompt body").unwrap();
        save_item(home, LibrarySectionId::Classes, "Architect", "agents body").unwrap();

        assert!(home.join("library/skills/dev/planner/SKILL.md").exists());
        assert!(home.join("library/prompts/greet.md").exists());
        assert!(home.join("classes/Architect/AGENTS.md").exists());
        assert_eq!(
            read_item(home, LibrarySectionId::Skills, "dev/planner").unwrap(),
            "skill body"
        );
        assert_eq!(
            read_item(home, LibrarySectionId::Classes, "Architect").unwrap(),
            "agents body"
        );
    }

    #[test]
    fn save_rejects_unindexable_entry_shapes() {
        let temp = tempfile::tempdir().expect("temp");
        let home = temp.path();

        assert!(save_item(home, LibrarySectionId::Prompts, "audit", "body").is_err());
        assert!(save_item(home, LibrarySectionId::Workflows, "audit.txt", "body").is_err());

        save_item(home, LibrarySectionId::Skills, "parent", "parent").unwrap();
        assert!(save_item(home, LibrarySectionId::Skills, "parent/child", "child").is_err());

        save_item(home, LibrarySectionId::Skills, "group/child", "child").unwrap();
        assert!(save_item(home, LibrarySectionId::Skills, "group", "parent").is_err());
        assert!(rename_entry(
            home,
            LibrarySectionId::Skills,
            "group/child",
            "parent/child"
        )
        .is_err());
    }

    #[test]
    fn create_folder_rejects_flat_sections() {
        let temp = tempfile::tempdir().expect("temp");
        create_folder(temp.path(), LibrarySectionId::Skills, "dev/tools").unwrap();
        assert!(temp.path().join("library/skills/dev/tools").is_dir());
        assert!(create_folder(temp.path(), LibrarySectionId::Classes, "sub").is_err());
        assert!(create_folder(temp.path(), LibrarySectionId::Mcps, "sub").is_err());
    }

    #[test]
    fn mutations_reject_traversal() {
        let temp = tempfile::tempdir().expect("temp");
        assert!(save_item(temp.path(), LibrarySectionId::Prompts, "../evil.md", "x").is_err());
        assert!(read_item(temp.path(), LibrarySectionId::Prompts, "../../etc/passwd").is_err());
    }

    fn meta(id: &str) -> LibraryItemMetadata {
        LibraryItemMetadata {
            id: id.to_string(),
            tags: vec![],
            is_starred: true,
            last_used: None,
        }
    }

    #[test]
    fn update_metadata_round_trips() {
        let temp = tempfile::tempdir().expect("temp");
        let home = temp.path();
        update_metadata(home, "skills/dev/planner", meta("m1")).unwrap();
        let store = MetadataStore::load(home);
        assert_eq!(store.get("skills/dev/planner").expect("stored").id, "m1");
    }

    #[test]
    fn update_metadata_rejects_unqualified_ref() {
        let temp = tempfile::tempdir().expect("temp");
        assert!(update_metadata(temp.path(), "planner", meta("m1")).is_err());
    }

    #[test]
    fn update_metadata_rejects_unknown_section() {
        let temp = tempfile::tempdir().expect("temp");
        assert!(update_metadata(temp.path(), "plugins/x", meta("m1")).is_err());
    }

    #[test]
    fn update_metadata_rejects_traversal() {
        let temp = tempfile::tempdir().expect("temp");
        assert!(update_metadata(temp.path(), "skills/../evil", meta("m1")).is_err());
    }

    #[test]
    fn rename_deployed_skill_relinks_and_remarks() {
        let temp = tempfile::tempdir().expect("temp");
        let home = temp.path();
        let src = home.join("library/skills/planner");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("SKILL.md"), "v1").unwrap();
        // one linked, one copied deployment
        let linked = home.join("classes/Architect/.agents/skills/planner");
        crate::library::links::create_directory_link(&src, &linked).unwrap();
        let copied = home.join("agents/a1/.agents/skills/planner");
        crate::library::links::copy_dir_all(&src, &copied).unwrap();
        fs::write(
            copied.join(crate::library::section::DEPLOYED_SKILL_SOURCE_FILE),
            "planner",
        )
        .unwrap();
        // starred metadata
        fs::write(
            home.join("library/library.json"),
            r#"{"skills/planner": {"id":"m1","tags":[],"is_starred":true,"last_used":null}}"#,
        )
        .unwrap();
        fs::create_dir_all(home.join("library/prompts")).unwrap();

        rename_entry(home, LibrarySectionId::Skills, "planner", "dev/planner").unwrap();

        fs::write(home.join("library/skills/dev/planner/SKILL.md"), "v2").unwrap();
        assert_eq!(
            fs::read_to_string(linked.join("SKILL.md")).unwrap(),
            "v2",
            "junction re-created"
        );
        assert_eq!(
            fs::read_to_string(copied.join(crate::library::section::DEPLOYED_SKILL_SOURCE_FILE))
                .unwrap()
                .trim(),
            "dev/planner",
            "marker rewritten"
        );
        let store = crate::library::metadata::MetadataStore::load(home);
        assert!(
            store
                .get("skills/dev/planner")
                .expect("metadata migrated")
                .is_starred
        );
    }

    #[test]
    fn delete_deployed_skill_cleans_all_targets() {
        let temp = tempfile::tempdir().expect("temp");
        let home = temp.path();
        let src = home.join("library/skills/planner");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("SKILL.md"), "v1").unwrap();
        let linked = home.join("common/.agents/skills/planner");
        crate::library::links::create_directory_link(&src, &linked).unwrap();

        delete_entry(home, LibrarySectionId::Skills, "planner").unwrap();
        assert!(!src.exists());
        assert!(!linked.exists(), "no dangling deployment");
    }

    #[test]
    fn remove_orphan_deployment_deletes_the_directory() {
        let temp = tempfile::tempdir().expect("temp");
        let home = temp.path();
        let orphan = home.join("common/.agents/skills/ghost");
        fs::create_dir_all(&orphan).unwrap();
        fs::write(orphan.join("SKILL.md"), "stale").unwrap();

        assert!(remove_orphan_deployment(home, "user", "global", "ghost").unwrap());

        assert!(!orphan.exists());
    }

    #[test]
    fn remove_orphan_deployment_preserves_healthy_deployment() {
        let temp = tempfile::tempdir().expect("temp");
        let home = temp.path();
        let source = home.join("library/skills/planner");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("SKILL.md"), "healthy").unwrap();
        let deployed = home.join("common/.agents/skills/planner");
        crate::library::links::create_directory_link(&source, &deployed).unwrap();

        assert!(!remove_orphan_deployment(home, "user", "global", "planner").unwrap());

        assert!(deployed.join("SKILL.md").is_file());
    }

    #[test]
    fn remove_orphan_deployment_rejects_invalid_names() {
        let temp = tempfile::tempdir().expect("temp");
        let home = temp.path();

        assert!(remove_orphan_deployment(home, "user", "global", "").is_err());
        assert!(remove_orphan_deployment(home, "user", "global", ".").is_err());
        assert!(remove_orphan_deployment(home, "user", "global", "..").is_err());
        assert!(remove_orphan_deployment(home, "user", "global", "a/b").is_err());
        assert!(remove_orphan_deployment(home, "user", "global", "a\\b").is_err());
    }

    // On Windows, `Path::components()` parses a leading `C:` as a `Prefix`
    // component distinct from the rest, so `"C:evil"` fails the single
    // `Normal` component check and is rejected. On Unix there is no drive
    // prefix concept, so `"C:evil"` is just an ordinary (if odd) file name
    // and is a valid single component — this assertion is Windows-only.
    #[cfg(windows)]
    #[test]
    fn remove_orphan_deployment_rejects_windows_drive_prefix() {
        let temp = tempfile::tempdir().expect("temp");
        let home = temp.path();
        assert!(remove_orphan_deployment(home, "user", "global", "C:evil").is_err());
    }
}
