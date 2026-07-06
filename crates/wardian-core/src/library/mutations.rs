use std::fs;
use std::path::{Path, PathBuf};

use crate::library::deployments::{collect_skill_sources, get_target_skills_dir, scan_deployments};
use crate::library::links::{create_directory_link, remove_existing_deployment};
use crate::library::metadata::MetadataStore;
use crate::library::section::{resolve_entry_path, LibrarySectionId, DEPLOYED_SKILL_SOURCE_FILE};
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
pub fn save_item(home: &Path, section: LibrarySectionId, rel: &str, content: &str) -> Result<(), String> {
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
pub fn rename_entry(
    home: &Path,
    section: LibrarySectionId,
    from_rel: &str,
    to_rel: &str,
) -> Result<(), String> {
    if section == LibrarySectionId::Classes {
        return Err("Renaming classes is not supported".to_string());
    }

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

    if section == LibrarySectionId::Skills {
        // The deployed directory name always mirrors the source's own
        // file_name: `planner` -> `dev/planner` keeps the name `planner`,
        // but `planner` -> `strategist` changes it.
        let old_name = last_component(&from_norm);
        let new_name = last_component(&to_norm);
        for target in &deployment_targets {
            let skills_dir = get_target_skills_dir(home, &target.target_type, &target.target_id)?;
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
                    fs::rename(&old_target_path, &new_target_path).map_err(|e| e.to_string())?;
                }
                fs::write(new_target_path.join(DEPLOYED_SKILL_SOURCE_FILE), &to_norm)
                    .map_err(|e| e.to_string())?;
            }
        }
    }

    let mut store = MetadataStore::load(home);
    store.rename(
        &format!("{}/{from_norm}", section.as_str()),
        &format!("{}/{to_norm}", section.as_str()),
    );
    store.save(home)
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
) -> Result<(), String> {
    if skill_name.is_empty()
        || skill_name == "."
        || skill_name == ".."
        || skill_name.contains('/')
        || skill_name.contains('\\')
    {
        return Err(format!("Invalid skill name: {skill_name}"));
    }
    let skills_dir = get_target_skills_dir(home, target_type, target_id)?;
    remove_existing_deployment(&skills_dir.join(skill_name)).map_err(|e| e.to_string())
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
        assert_eq!(read_item(home, LibrarySectionId::Skills, "dev/planner").unwrap(), "skill body");
        assert_eq!(read_item(home, LibrarySectionId::Classes, "Architect").unwrap(), "agents body");
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
        LibraryItemMetadata { id: id.to_string(), tags: vec![], is_starred: true, last_used: None }
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
        fs::write(copied.join(crate::library::section::DEPLOYED_SKILL_SOURCE_FILE), "planner").unwrap();
        // starred metadata
        fs::write(home.join("library/library.json"),
            r#"{"skills/planner": {"id":"m1","tags":[],"is_starred":true,"last_used":null}}"#).unwrap();
        fs::create_dir_all(home.join("library/prompts")).unwrap();

        rename_entry(home, LibrarySectionId::Skills, "planner", "dev/planner").unwrap();

        fs::write(home.join("library/skills/dev/planner/SKILL.md"), "v2").unwrap();
        assert_eq!(fs::read_to_string(linked.join("SKILL.md")).unwrap(), "v2", "junction re-created");
        assert_eq!(
            fs::read_to_string(copied.join(crate::library::section::DEPLOYED_SKILL_SOURCE_FILE)).unwrap().trim(),
            "dev/planner", "marker rewritten"
        );
        let store = crate::library::metadata::MetadataStore::load(home);
        assert!(store.get("skills/dev/planner").expect("metadata migrated").is_starred);
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
}
