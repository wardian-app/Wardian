use std::fs;
use std::path::{Path, PathBuf};

use crate::library::section::{resolve_entry_path, LibrarySectionId};
use crate::library::metadata::MetadataStore;
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
}
