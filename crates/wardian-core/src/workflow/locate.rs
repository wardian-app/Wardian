//! Locate a blueprint's file on disk by its declared `id`.
//!
//! The library redesign made nested workflow folders first-class, so a
//! blueprint's file name (and its containing subfolder) no longer has to
//! match its `id`. Both the manual run path (`commands::workflow`) and the
//! schedule invoker (`workflow::schedule`) need to find a blueprint by `id`
//! alone, so the recursive walk-and-match lives here once and both surfaces
//! call it — neither should re-implement the recursion.

use std::path::{Path, PathBuf};

/// Recursively collect every `.md` file under `dir`, depth-first.
pub fn list_blueprint_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                files.extend(list_blueprint_files(&path));
            } else if path.extension().and_then(|ext| ext.to_str()) == Some("md") {
                files.push(path);
            }
        }
    }
    files
}

/// Find the blueprint whose frontmatter `id` matches `id`, searching
/// `<wardian-home>/library/workflows` recursively so blueprints nested in
/// subfolders resolve the same way flat ones always have.
///
/// Blueprint ids are expected to be unique across the library. If more than
/// one file declares the same id, the first match found during the walk
/// wins — this matches the manual run path's prior behavior, which has no
/// ambiguity error today.
pub fn resolve_blueprint_path(id: &str) -> Option<PathBuf> {
    let dir = crate::paths::library_workflows_dir()?;
    resolve_blueprint_path_in(&dir, id)
}

/// Same as [`resolve_blueprint_path`] but rooted at an explicit directory —
/// used by tests and any caller that already knows the workflows root.
pub fn resolve_blueprint_path_in(dir: &Path, id: &str) -> Option<PathBuf> {
    list_blueprint_files(dir)
        .into_iter()
        .find(|path| crate::workflow::parse_file(path).is_ok_and(|bp| bp.id == id))
}

#[cfg(test)]
mod tests {
    use super::*;

    const BLUEPRINT_TEMPLATE: &str = r#"---
schema: 2
id: {id}
name: {name}
nodes:
  - id: trigger
    type: manual_trigger
    fields: {}
edges: []
---

# {name}
"#;

    fn write_blueprint(path: &Path, id: &str, name: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let text = BLUEPRINT_TEMPLATE.replace("{id}", id).replace("{name}", name);
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn resolves_a_flat_blueprint_by_id() {
        let dir = tempfile::tempdir().unwrap();
        write_blueprint(&dir.path().join("flat.md"), "flat-id", "Flat");

        let resolved = resolve_blueprint_path_in(dir.path(), "flat-id").unwrap();
        assert_eq!(resolved, dir.path().join("flat.md"));
    }

    #[test]
    fn resolves_a_blueprint_nested_in_a_subfolder() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("folder").join("nested.md");
        write_blueprint(&nested, "nested-id", "Nested");

        let resolved = resolve_blueprint_path_in(dir.path(), "nested-id").unwrap();
        assert_eq!(resolved, nested);
    }

    #[test]
    fn returns_none_when_no_blueprint_matches() {
        let dir = tempfile::tempdir().unwrap();
        write_blueprint(&dir.path().join("flat.md"), "flat-id", "Flat");

        assert!(resolve_blueprint_path_in(dir.path(), "does-not-exist").is_none());
    }

    #[test]
    fn returns_none_for_a_missing_directory() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("does-not-exist");

        assert!(resolve_blueprint_path_in(&missing, "anything").is_none());
    }

    #[test]
    fn matches_by_declared_id_not_by_filename() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir
            .path()
            .join("folder")
            .join("filename-does-not-match-id.md");
        write_blueprint(&path, "declared-id", "Declared");

        let resolved = resolve_blueprint_path_in(dir.path(), "declared-id").unwrap();
        assert_eq!(resolved, path);
    }

    #[test]
    fn duplicate_ids_resolve_to_one_match_without_erroring() {
        // There is no ambiguity error today (matching the manual run path's
        // prior behavior): the walk returns whichever match it finds first.
        // This pins "resolves to a match, does not panic or error" without
        // depending on directory read order.
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("a.md");
        let second = dir.path().join("b.md");
        write_blueprint(&first, "dup-id", "First");
        write_blueprint(&second, "dup-id", "Second");

        let resolved = resolve_blueprint_path_in(dir.path(), "dup-id").unwrap();
        assert!(resolved == first || resolved == second);
    }
}
