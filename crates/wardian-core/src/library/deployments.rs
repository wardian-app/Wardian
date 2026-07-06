use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::library::section::{LibrarySectionId, DEPLOYED_SKILL_SOURCE_FILE};
use crate::models::{DeploymentTarget, OrphanDeployment};

/// A discovered skill in `library/skills`, keyed by its section-relative
/// path (e.g. `dev/planner`) so deployed copies/links can be matched back
/// to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillSource {
    pub rel_path: String,
    pub name: String,
    pub canonical: PathBuf,
}

/// Result of a single walk over every deployment target directory: which
/// library sources are deployed where, and which deployed directories
/// could not be resolved back to a source (orphans).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DeploymentScan {
    pub deployments: HashMap<String, Vec<DeploymentTarget>>,
    pub orphans: Vec<OrphanDeployment>,
}

/// Recursively walk `library/skills` collecting every directory containing
/// a `SKILL.md`. Port of `collect_library_skill_sources` from
/// `src-tauri/src/commands/library.rs`.
pub fn collect_skill_sources(home: &Path) -> Vec<SkillSource> {
    let base_dir = LibrarySectionId::Skills.root_for_home(home);
    let mut sources = Vec::new();
    collect_skill_sources_inner(&base_dir, &base_dir, &mut sources);
    sources
}

fn collect_skill_sources_inner(dir: &Path, base_dir: &Path, sources: &mut Vec<SkillSource>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
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
                sources.push(SkillSource {
                    rel_path,
                    name,
                    canonical,
                });
            }
            continue;
        }

        collect_skill_sources_inner(&path, base_dir, sources);
    }
}

/// Read and normalize the `.wardian-skill-source` marker file left behind
/// by a copy-based deployment, if present.
fn read_deployed_skill_source_marker(path: &Path) -> Option<String> {
    fs::read_to_string(path.join(DEPLOYED_SKILL_SOURCE_FILE))
        .ok()
        .map(|source| source.trim().replace('\\', "/"))
        .filter(|source| !source.is_empty())
}

/// Resolve a deployed skill directory back to a library source, in order:
/// marker file, canonical-path match, then unique-name inference (only
/// when exactly one source shares the deployed directory's name).
///
/// A deployed directory whose name matches more than one library source
/// and has neither a marker nor a canonical-path match resolves to
/// nothing (`None`) — the caller records that as an orphan.
fn resolve_source(deployed_path: &Path, deployed_name: &str, sources: &[SkillSource]) -> Option<String> {
    if let Some(marker_source) = read_deployed_skill_source_marker(deployed_path) {
        if sources.iter().any(|s| s.rel_path == marker_source) {
            return Some(marker_source);
        }
    }

    if let Ok(canonical_path) = deployed_path.canonicalize() {
        if let Some(source) = sources.iter().find(|s| s.canonical == canonical_path) {
            return Some(source.rel_path.clone());
        }
    }

    let mut same_name_sources = sources.iter().filter(|s| s.name == deployed_name);
    let only_source = same_name_sources.next();
    if only_source.is_some() && same_name_sources.next().is_none() {
        return only_source.map(|s| s.rel_path.clone());
    }

    None
}

/// Same reparse/symlink detection used by `remove_existing_deployment`:
/// a Unix symlink, or on Windows a reparse point (junction) attribute.
fn is_reparse_or_symlink(metadata: &fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }

    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }

    #[cfg(not(windows))]
    {
        false
    }
}

/// Resolve the `.agents/skills` directory for a deployment target. Port of
/// `get_target_skills_dir` from `src-tauri/src/commands/library.rs`.
pub fn get_target_skills_dir(
    home: &Path,
    target_type: &str,
    target_id: &str,
) -> Result<PathBuf, String> {
    let base = match target_type {
        "agent" => home.join("agents").join(target_id),
        "class" => home.join("classes").join(target_id),
        "user" => home.join("common"),
        _ => return Err(format!("Unknown target type: {target_type}")),
    };
    Ok(base.join(".agents").join("skills"))
}

/// Walk every deployment target exactly once — `common/.agents/skills`,
/// each `classes/*/.agents/skills`, and each `agents/*/.agents/skills` —
/// resolving each deployed directory back to a library source and
/// recording unresolved directories as orphans.
pub fn scan_deployments(home: &Path, sources: &[SkillSource]) -> DeploymentScan {
    let mut deployments: HashMap<String, Vec<DeploymentTarget>> = HashMap::new();
    let mut orphans = Vec::new();

    let mut scan_target_dir = |target_type: &str, target_id: &str, skills_dir: &Path| {
        let Ok(entries) = fs::read_dir(skills_dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            let linked = fs::symlink_metadata(&path)
                .map(|m| is_reparse_or_symlink(&m))
                .unwrap_or(false);
            match resolve_source(&path, &name, sources) {
                Some(rel_path) => deployments.entry(rel_path).or_default().push(DeploymentTarget {
                    target_type: target_type.to_string(),
                    target_id: target_id.to_string(),
                    linked,
                }),
                None => orphans.push(OrphanDeployment {
                    target_type: target_type.to_string(),
                    target_id: target_id.to_string(),
                    skill_name: name,
                }),
            }
        }
    };

    scan_target_dir("user", "global", &home.join("common").join(".agents").join("skills"));
    for root in ["classes", "agents"] {
        let type_name = if root == "classes" { "class" } else { "agent" };
        let Ok(entries) = fs::read_dir(home.join(root)) else {
            continue;
        };
        for entry in entries.flatten() {
            if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                let id = entry.file_name().to_string_lossy().to_string();
                scan_target_dir(type_name, &id, &entry.path().join(".agents").join("skills"));
            }
        }
    }

    DeploymentScan { deployments, orphans }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::links::{copy_dir_all, create_directory_link};
    use std::fs;

    fn seed_skill(home: &std::path::Path, rel: &str) -> std::path::PathBuf {
        let dir = home.join("library").join("skills").join(rel);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("SKILL.md"), rel).unwrap();
        dir
    }

    #[test]
    fn one_scan_resolves_links_copies_and_orphans() {
        let temp = tempfile::tempdir().expect("temp");
        let home = temp.path();
        let planner = seed_skill(home, "dev/planner");
        seed_skill(home, "reviewer");

        // Linked deploy to a class.
        let class_target = home.join("classes").join("Architect").join(".agents").join("skills").join("planner");
        create_directory_link(&planner, &class_target).unwrap();

        // Copied deploy (with marker) to an agent.
        let agent_target = home.join("agents").join("a1").join(".agents").join("skills").join("planner");
        copy_dir_all(&planner, &agent_target).unwrap();
        fs::write(agent_target.join(super::super::section::DEPLOYED_SKILL_SOURCE_FILE), "dev/planner").unwrap();

        // Orphan: deployed dir with no source anywhere.
        let orphan_target = home.join("common").join(".agents").join("skills").join("ghost");
        fs::create_dir_all(&orphan_target).unwrap();
        fs::write(orphan_target.join("SKILL.md"), "ghost").unwrap();

        let sources = collect_skill_sources(home);
        assert_eq!(sources.len(), 2);
        let scan = scan_deployments(home, &sources);

        let planner_targets = scan.deployments.get("dev/planner").expect("planner deployed");
        assert_eq!(planner_targets.len(), 2);
        let class_dep = planner_targets.iter().find(|t| t.target_type == "class").expect("class");
        assert!(class_dep.linked);
        let agent_dep = planner_targets.iter().find(|t| t.target_type == "agent").expect("agent");
        assert!(!agent_dep.linked);

        assert_eq!(scan.orphans.len(), 1);
        assert_eq!(scan.orphans[0].skill_name, "ghost");
        assert_eq!(scan.orphans[0].target_type, "user");
    }

    #[test]
    fn duplicate_names_resolve_only_via_marker_or_canonical() {
        let temp = tempfile::tempdir().expect("temp");
        let home = temp.path();
        seed_skill(home, "group-a/planner");
        let b = seed_skill(home, "group-b/planner");
        let target = home.join("agents").join("a1").join(".agents").join("skills").join("planner");
        copy_dir_all(&b, &target).unwrap(); // legacy copy, no marker

        let sources = collect_skill_sources(home);
        let scan = scan_deployments(home, &sources);
        assert!(!scan.deployments.contains_key("group-a/planner"));
        assert!(!scan.deployments.contains_key("group-b/planner"));
        assert_eq!(scan.orphans.len(), 1, "ambiguous copy stays orphaned");
    }
}
