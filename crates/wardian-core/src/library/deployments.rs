use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::library::links::{deploy_skill_dir, remove_existing_deployment};
use crate::library::section::{
    is_single_normal_component, resolve_entry_path, LibrarySectionId, DEPLOYED_SKILL_SOURCE_FILE,
};
use crate::models::{DeploymentTarget, OrphanDeployment, SkillDeployment};

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

/// The final path component of a section-relative skill path, used as the
/// deployed skill directory's name (e.g. `dev/planner` -> `planner`).
fn skill_name_from_rel(rel: &str) -> String {
    Path::new(rel)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default()
}

/// Deploy a single library skill to a single target. Port of
/// `deploy_skill_from_library` from `src-tauri/src/commands/library.rs`,
/// minus the Tauri plumbing. Returns `true` when the deployment fell back
/// to a copy (and wrote the `.wardian-skill-source` marker) rather than a
/// live link.
pub fn deploy_skill(
    home: &Path,
    source_rel: &str,
    target_type: &str,
    target_id: &str,
) -> Result<bool, String> {
    if !is_single_normal_component(target_id) {
        return Err(format!("Invalid target id: {target_id}"));
    }

    let src_dir = resolve_entry_path(home, LibrarySectionId::Skills, source_rel)?;
    if !src_dir.exists() || !src_dir.is_dir() {
        return Err(format!(
            "Skill source not found or is not a directory: {src_dir:?}"
        ));
    }

    let rel_norm = source_rel.replace('\\', "/");
    let target_skills_dir = get_target_skills_dir(home, target_type, target_id)?;
    let dst_dir = target_skills_dir.join(skill_name_from_rel(&rel_norm));

    let copied = deploy_skill_dir(&src_dir, &dst_dir).map_err(|e| e.to_string())?;
    if copied {
        fs::write(dst_dir.join(DEPLOYED_SKILL_SOURCE_FILE), &rel_norm).map_err(|e| e.to_string())?;
    }
    Ok(copied)
}

/// Remove one deployed skill directory from one target.
pub fn remove_deployed_skill(
    home: &Path,
    target_type: &str,
    target_id: &str,
    skill_name: &str,
) -> Result<(), String> {
    if !is_single_normal_component(target_id) {
        return Err(format!("Invalid target id: {target_id}"));
    }
    if !is_single_normal_component(skill_name) {
        return Err(format!("Invalid skill name: {skill_name}"));
    }
    let skills_dir = get_target_skills_dir(home, target_type, target_id)?;
    remove_existing_deployment(&skills_dir.join(skill_name)).map_err(|e| e.to_string())
}

/// Result of reconciling a skill's deployed targets against a desired set.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetDeploymentsOutcome {
    pub added: u32,
    pub removed: u32,
    pub copied_fallbacks: u32,
}

/// Reconcile a single skill's deployments against a desired target set:
/// deploy to every target in `desired` that isn't already deployed, and
/// remove every currently-deployed target that isn't in `desired`.
/// Targets are compared as `(target_type, target_id)` pairs, so this is
/// idempotent when the desired set already matches reality.
///
/// Best-effort semantics (mirroring `rename_entry` in `mutations.rs`):
/// every add and every remove is attempted independently, a failure on one
/// target does not stop the others, and every change that does succeed is
/// left applied on disk (nothing is rolled back). If one or more targets
/// fail, this returns a single aggregate `Err` naming how many targets
/// failed and the per-target error details; the accumulated `outcome` for
/// the run is otherwise discarded by the `Result<_, String>` signature, but
/// a follow-up call will see the already-applied changes reflected in a
/// fresh scan.
pub fn set_skill_deployments(
    home: &Path,
    source_rel: &str,
    desired: &[SkillDeployment],
) -> Result<SetDeploymentsOutcome, String> {
    let rel_norm = source_rel.replace('\\', "/");
    let skill_name = skill_name_from_rel(&rel_norm);

    let sources = collect_skill_sources(home);
    let scan = scan_deployments(home, &sources);
    let current = scan
        .deployments
        .get(rel_norm.as_str())
        .cloned()
        .unwrap_or_default();

    let desired_set: HashSet<(String, String)> = desired
        .iter()
        .map(|d| (d.target_type.clone(), d.target_id.clone()))
        .collect();
    let current_set: HashSet<(String, String)> = current
        .iter()
        .map(|t| (t.target_type.clone(), t.target_id.clone()))
        .collect();

    let mut outcome = SetDeploymentsOutcome::default();
    let mut errors: Vec<String> = Vec::new();

    for target in &current {
        let key = (target.target_type.clone(), target.target_id.clone());
        if !desired_set.contains(&key) {
            match remove_deployed_skill(home, &target.target_type, &target.target_id, &skill_name) {
                Ok(()) => outcome.removed += 1,
                Err(e) => errors.push(format!(
                    "remove {}/{}: {e}",
                    target.target_type, target.target_id
                )),
            }
        }
    }

    for target in desired {
        let key = (target.target_type.clone(), target.target_id.clone());
        if !current_set.contains(&key) {
            match deploy_skill(home, &rel_norm, &target.target_type, &target.target_id) {
                Ok(copied) => {
                    outcome.added += 1;
                    if copied {
                        outcome.copied_fallbacks += 1;
                    }
                }
                Err(e) => errors.push(format!(
                    "add {}/{}: {e}",
                    target.target_type, target.target_id
                )),
            }
        }
    }

    if !errors.is_empty() {
        return Err(format!(
            "{} target(s) could not be updated: {}",
            errors.len(),
            errors.join("; ")
        ));
    }

    Ok(outcome)
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

    #[test]
    fn set_deployments_diffs_adds_and_removes() {
        let temp = tempfile::tempdir().expect("temp");
        let home = temp.path();
        let src = home.join("library/skills/planner");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("SKILL.md"), "v1").unwrap();

        let desired = vec![
            SkillDeployment { target_type: "class".into(), target_id: "Architect".into() },
            SkillDeployment { target_type: "user".into(), target_id: "global".into() },
        ];
        let outcome = set_skill_deployments(home, "planner", &desired).unwrap();
        assert_eq!(outcome.added, 2);
        assert!(home.join("classes/Architect/.agents/skills/planner").join("SKILL.md").exists());
        assert!(home.join("common/.agents/skills/planner").join("SKILL.md").exists());

        // Narrow to just the class: user deployment is removed, class untouched.
        let narrowed = vec![SkillDeployment { target_type: "class".into(), target_id: "Architect".into() }];
        let outcome = set_skill_deployments(home, "planner", &narrowed).unwrap();
        assert_eq!(outcome.added, 0);
        assert_eq!(outcome.removed, 1);
        assert!(!home.join("common/.agents/skills/planner").exists());
        assert!(home.join("classes/Architect/.agents/skills/planner").exists());

        // Idempotent.
        let outcome = set_skill_deployments(home, "planner", &narrowed).unwrap();
        assert_eq!((outcome.added, outcome.removed), (0, 0));
    }

    #[test]
    fn set_deployments_outcome_serializes_snake_case() {
        let outcome = SetDeploymentsOutcome {
            added: 2,
            removed: 1,
            copied_fallbacks: 1,
        };
        let value = serde_json::to_value(&outcome).expect("serialize");
        assert_eq!(
            value,
            serde_json::json!({
                "added": 2,
                "removed": 1,
                "copied_fallbacks": 1,
            })
        );

        let round_tripped: SetDeploymentsOutcome =
            serde_json::from_value(value).expect("deserialize");
        assert_eq!(round_tripped, outcome);
    }

    #[test]
    fn set_deployments_partial_failure_keeps_successful_target_and_reports_error() {
        let temp = tempfile::tempdir().expect("temp");
        let home = temp.path();
        let src = home.join("library/skills/planner");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("SKILL.md"), "v1").unwrap();

        // One valid target, one target whose id fails `is_single_normal_component`
        // validation (path traversal component) and can never succeed.
        let desired = vec![
            SkillDeployment {
                target_type: "class".into(),
                target_id: "Architect".into(),
            },
            SkillDeployment {
                target_type: "class".into(),
                target_id: "../evil".into(),
            },
        ];

        let err = set_skill_deployments(home, "planner", &desired)
            .expect_err("one target should fail validation");
        assert!(
            err.contains("../evil"),
            "error should mention the failing target: {err}"
        );
        assert!(
            err.contains("1 target(s)"),
            "error should report the failure count: {err}"
        );

        // The valid target was still deployed on disk despite the other failure.
        assert!(home
            .join("classes/Architect/.agents/skills/planner")
            .join("SKILL.md")
            .exists());
    }

    /// Manual perf evidence for the native junction/symlink deployment path
    /// (SDD Task 17, step 3) — replaces the old `mklink` subprocess-per-call
    /// path this redesign removed. Not run in CI: timing assertions are
    /// flaky across machines, and the point of this test is to print a
    /// number for the PR description, not to gate the build.
    ///
    /// Run: `cargo test -p wardian-core --release deploy_perf -- --ignored --nocapture`
    #[test]
    #[ignore = "manual perf evidence: cargo test -p wardian-core --release deploy_perf -- --ignored --nocapture"]
    fn deploy_perf_twenty_sequential() {
        let temp = tempfile::tempdir().expect("temp");
        let home = temp.path();
        let src = home.join("library/skills/planner");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("SKILL.md"), "perf").unwrap();
        let start = std::time::Instant::now();
        for i in 0..20 {
            deploy_skill(home, "planner", "agent", &format!("agent-{i}")).expect("deploy");
        }
        println!("20 deploys in {:?}", start.elapsed());
    }
}
