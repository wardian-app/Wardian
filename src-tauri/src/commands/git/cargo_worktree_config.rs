//! Cargo output isolation for managed worktrees; compiler caches are a separate concern.

use super::{
    absolute_existing_path, escape_toml_basic_string, git_tracks_relative_path, list_git_worktrees,
};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};

fn plain_path(path: &Path, directory: bool) -> Result<bool, String> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.to_string()),
    };
    let linked = metadata.file_type().is_symlink();
    #[cfg(windows)]
    let linked = {
        use std::os::windows::fs::MetadataExt;
        linked || metadata.file_attributes() & 0x400 != 0
    };
    if linked || (directory && !metadata.is_dir()) || (!directory && !metadata.is_file()) {
        return Err(format!(
            "Refusing linked or unexpected Cargo cache path: {}",
            path.display()
        ));
    }
    Ok(true)
}

fn path_key(path: &Path) -> Result<String, String> {
    let path = path.to_str().ok_or("Cargo cache paths must be UTF-8")?;
    // Backslashes are filename characters on POSIX, not alternate separators.
    #[cfg(windows)]
    let path = path.replace('\\', "/").trim_end_matches('/').to_lowercase();
    Ok(format!("{:x}", Sha256::digest(path.as_bytes()))[..16].to_string())
}

fn contains_path(parent: &Path, child: &Path) -> bool {
    #[cfg(windows)]
    {
        let parent = parent.to_string_lossy().replace('\\', "/").to_lowercase();
        let child = child.to_string_lossy().replace('\\', "/").to_lowercase();
        child == parent || child.starts_with(&format!("{}/", parent.trim_end_matches('/')))
    }
    #[cfg(not(windows))]
    {
        child.starts_with(parent)
    }
}

/// Match the launcher's sibling cache layout without creating directories or claims.
/// Git's primary worktree identifies the repository even when the source is a linked tree.
fn central_target(workspace_path: &Path, worktree_path: &Path) -> Result<PathBuf, String> {
    let root = std::env::var_os("WARDIAN_RUST_CACHE_ROOT").map(PathBuf::from);
    central_target_with_root(workspace_path, worktree_path, root.as_deref())
}

fn central_target_with_root(
    workspace_path: &Path,
    worktree_path: &Path,
    override_root: Option<&Path>,
) -> Result<PathBuf, String> {
    let entries = list_git_worktrees(workspace_path)?;
    let source = entries
        .first()
        .ok_or("Cargo cache source has no primary Git worktree")?;
    let source = absolute_existing_path(&source.path)?;
    let worktree = absolute_existing_path(worktree_path)?;
    let worktree_key = path_key(&worktree)?;
    if !entries.iter().any(|entry| {
        absolute_existing_path(&entry.path)
            .and_then(|path| path_key(&path))
            .is_ok_and(|key| key == worktree_key)
    }) {
        return Err(
            "Cargo cache worktree is not registered with the source repository".to_string(),
        );
    }
    if path_key(&source)? == worktree_key {
        return Err("Cannot assign managed Cargo outputs to the source checkout".to_string());
    }
    let name = source
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("Cargo cache source has no UTF-8 directory name")?;
    let root = match override_root {
        Some(root) => {
            if !root.is_absolute()
                || root
                    .components()
                    .any(|part| part == std::path::Component::ParentDir)
            {
                return Err(
                    "WARDIAN_RUST_CACHE_ROOT must be absolute without parent traversal".to_string(),
                );
            }
            root.to_path_buf()
        }
        None => source.with_file_name(format!("{name}.cargo-cache")),
    };
    // Ordinary Cargo has no launcher claim. Never make its outputs eligible for
    // wrapper pruning merely because a previous wrapper invocation released a claim.
    let target = root
        .join("direct-targets")
        .join(path_key(&source)?)
        .join(worktree_key);
    let main_target = source.join("target");
    let main_target = if main_target.exists() {
        absolute_existing_path(&main_target)?
    } else {
        main_target
    };
    if contains_path(&main_target, &target) || contains_path(&target, &main_target) {
        return Err("Managed Cargo outputs overlap the source checkout target".to_string());
    }
    // Existing junctions must not redirect generated outputs into protected main artifacts.
    let mut ancestor = Some(target.as_path());
    while let Some(path) = ancestor {
        plain_path(path, true)?;
        ancestor = path.parent();
    }
    Ok(target)
}

fn generated_config(target: &Path) -> String {
    let target = escape_toml_basic_string(&target.to_string_lossy());
    format!("[build]\ntarget-dir = \"{target}\"\nbuild-dir = \"{target}\"\n")
}

fn legacy_config(workspace: &Path) -> String {
    format!(
        "[build]\ntarget-dir = \"{}\"\n",
        escape_toml_basic_string(&workspace.join("target").to_string_lossy())
    )
}

fn minimal_local_default(text: &str) -> bool {
    let Ok(document) = text.parse::<toml_edit::DocumentMut>() else {
        return false;
    };
    let Some(build) = document.get("build").and_then(toml_edit::Item::as_table) else {
        return false;
    };
    document.len() == 1
        && build.len() == 1
        && build.get("target-dir").and_then(toml_edit::Item::as_str) == Some("target")
}

fn read_config(worktree: &Path) -> Result<Option<String>, String> {
    let directory = worktree.join(".cargo");
    plain_path(&directory, true)?;
    // Cargo prefers its legacy config name. Do not hide or modify that user policy.
    if plain_path(&directory.join("config"), false)? {
        return Ok(None);
    }
    let config = directory.join("config.toml");
    if !plain_path(&config, false)? {
        return Ok(Some(String::new()));
    }
    fs::read_to_string(config)
        .map(Some)
        .map_err(|error| error.to_string())
}

/// Return a central lane only for absent/generated config or the tracked local default.
/// Custom configs remain authoritative; no wrapper or Cargo home is rewritten here.
pub(crate) fn managed_worktree_cargo_target(
    workspace: &Path,
    worktree: &Path,
) -> Result<Option<PathBuf>, String> {
    let workspace = absolute_existing_path(workspace)?;
    let worktree = absolute_existing_path(worktree)?;
    let Some(actual) = read_config(&worktree)? else {
        return Ok(None);
    };
    let tracked = git_tracks_relative_path(&worktree, ".cargo/config.toml")?;
    let absent = !plain_path(&worktree.join(".cargo/config.toml"), false)?;
    // Avoid interpreting or repairing a custom policy, including a tracked deletion.
    if tracked && !minimal_local_default(&actual) {
        return Ok(None);
    }
    let target = central_target(&workspace, &worktree)?;
    if (!tracked
        && (absent || actual == legacy_config(&workspace) || actual == generated_config(&target)))
        || tracked
    {
        Ok(Some(target))
    } else {
        Ok(None)
    }
}

pub(super) fn write_cargo_worktree_config(worktree: &Path, workspace: &Path) -> Result<(), String> {
    let Some(actual) = read_config(worktree)? else {
        return Ok(());
    };
    // A tracked default already isolates ordinary shells. The provider env can route it
    // centrally without leaving a perpetual tracked modification in every new worktree.
    if git_tracks_relative_path(worktree, ".cargo/config.toml")? {
        return Ok(());
    }
    let legacy = legacy_config(workspace);
    let config_path = worktree.join(".cargo/config.toml");
    let exists = plain_path(&config_path, false)?;
    if exists && actual != legacy {
        return Ok(());
    }
    let target = central_target(workspace, worktree)?;
    let directory = worktree.join(".cargo");
    fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
    let mut replacement =
        tempfile::NamedTempFile::new_in(&directory).map_err(|error| error.to_string())?;
    replacement
        .write_all(generated_config(&target).as_bytes())
        .map_err(|error| error.to_string())?;
    replacement
        .as_file()
        .sync_all()
        .map_err(|error| error.to_string())?;
    // Recheck ownership after preparing the complete replacement. Errors leave the old file intact.
    plain_path(&directory, true)?;
    if git_tracks_relative_path(worktree, ".cargo/config.toml")? {
        return Err("Cargo config became tracked during cache setup".to_string());
    }
    if exists {
        if !plain_path(&config_path, false)?
            || fs::read_to_string(&config_path).map_err(|error| error.to_string())? != legacy
        {
            return Err("Cargo config changed during cache migration".to_string());
        }
        replacement
            .persist(config_path)
            .map_err(|error| error.to_string())?;
    } else {
        replacement
            .persist_noclobber(config_path)
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

pub(super) fn remove_generated_cargo_config(
    worktree: &Path,
    workspace: &Path,
) -> Result<(), String> {
    let Some(actual) = read_config(worktree)? else {
        return Ok(());
    };
    if actual.is_empty() || git_tracks_relative_path(worktree, ".cargo/config.toml")? {
        return Ok(());
    }
    let legacy = legacy_config(workspace);
    if actual != legacy && actual != generated_config(&central_target(workspace, worktree)?) {
        return Ok(());
    }
    fs::remove_file(worktree.join(".cargo/config.toml")).map_err(|error| error.to_string())?;
    let _ = fs::remove_dir(worktree.join(".cargo"));
    // Compiler output and claims belong to the cache planner, never worktree deletion.
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::git::{
        create_worktree_with_build_caches, run_git, setup_worktree_build_caches,
    };
    use wardian_core::models::AgentConfig;

    const DEFAULT: &str = "[build]\ntarget-dir = \"target\"\n";

    struct Fixture {
        _temp: tempfile::TempDir,
        source: PathBuf,
    }

    impl Fixture {
        fn new(config: Option<&str>) -> Self {
            let temp = tempfile::tempdir().unwrap();
            let source = temp.path().join("project");
            fs::create_dir_all(&source).unwrap();
            fs::write(source.join("Cargo.toml"), "[workspace]\n").unwrap();
            let cwd = source.to_str().unwrap();
            run_git(cwd, &["init", "-q"]).unwrap();
            run_git(cwd, &["config", "user.email", "fixture@example.invalid"]).unwrap();
            run_git(cwd, &["config", "user.name", "Cache fixture"]).unwrap();
            run_git(cwd, &["config", "core.autocrlf", "false"]).unwrap();
            if let Some(config) = config {
                fs::create_dir_all(source.join(".cargo")).unwrap();
                fs::write(source.join(".cargo/config.toml"), config).unwrap();
                run_git(cwd, &["add", ".cargo/config.toml"]).unwrap();
            }
            run_git(cwd, &["add", "Cargo.toml"]).unwrap();
            run_git(
                cwd,
                &["-c", "commit.gpgsign=false", "commit", "-qm", "fixture"],
            )
            .unwrap();
            Self {
                _temp: temp,
                source: absolute_existing_path(&source).unwrap(),
            }
        }

        fn create(&self, name: &str) -> PathBuf {
            let worktree = self.source.with_file_name("project.wt").join(name);
            create_worktree_with_build_caches(&self.source, &worktree, &format!("wardian/{name}"))
                .unwrap();
            absolute_existing_path(&worktree).unwrap()
        }

        fn config(&self, worktree: &Path) -> AgentConfig {
            AgentConfig {
                git_worktree: Some(true),
                git_worktree_source: Some(self.source.to_string_lossy().into_owned()),
                git_worktree_folder: Some(worktree.to_string_lossy().into_owned()),
                ..Default::default()
            }
        }
    }

    fn config_target(worktree: &Path) -> PathBuf {
        let document = fs::read_to_string(worktree.join(".cargo/config.toml"))
            .unwrap()
            .parse::<toml_edit::DocumentMut>()
            .unwrap();
        let target = document["build"]["target-dir"].as_str().unwrap();
        assert_eq!(document["build"]["build-dir"].as_str(), Some(target));
        PathBuf::from(target)
    }

    #[test]
    fn managed_creation_generates_central_config_matching_provider_environment() {
        let f = Fixture::new(None);
        let tree = f.create("first");
        let target = config_target(&tree);
        assert!(target.starts_with(
            f.source
                .with_file_name("project.cargo-cache")
                .join("direct-targets")
        ));
        assert_eq!(
            target,
            managed_worktree_cargo_target(&f.source, &tree)
                .unwrap()
                .unwrap()
        );
        let environment =
            crate::manager::worktree_build_env_with_policy(&f.config(&tree), false).unwrap();
        assert_eq!(environment.len(), 2);
        assert!(environment
            .iter()
            .all(|(_, value)| Path::new(value) == target));
        assert!(
            crate::manager::worktree_build_env_with_policy(&f.config(&tree), true)
                .unwrap()
                .is_empty()
        );
        assert!(
            !target.exists(),
            "planning must not allocate or claim compiler outputs"
        );
        assert!(!f.source.join("target").exists());
    }

    #[test]
    fn tracked_default_stays_clean_while_provider_uses_central_lane() {
        let f = Fixture::new(Some(DEFAULT));
        let tree = f.create("default");
        assert_eq!(
            fs::read_to_string(tree.join(".cargo/config.toml")).unwrap(),
            DEFAULT
        );
        assert!(run_git(tree.to_str().unwrap(), &["status", "--porcelain"])
            .unwrap()
            .is_empty());
        assert_eq!(
            crate::manager::worktree_build_env_with_policy(&f.config(&tree), false)
                .unwrap()
                .len(),
            2
        );
        remove_generated_cargo_config(&tree, &f.source).unwrap();
        assert_eq!(
            fs::read_to_string(tree.join(".cargo/config.toml")).unwrap(),
            DEFAULT
        );
    }

    #[test]
    fn only_exact_untracked_legacy_template_migrates() {
        let f = Fixture::new(None);
        let tree = f.create("migration");
        let file = tree.join(".cargo/config.toml");
        let legacy = legacy_config(&f.source);
        fs::write(&file, format!("{legacy}# user-owned\n")).unwrap();
        setup_worktree_build_caches(&tree, &f.source).unwrap();
        assert_eq!(
            fs::read_to_string(&file).unwrap(),
            format!("{legacy}# user-owned\n")
        );
        assert!(
            crate::manager::worktree_build_env_with_policy(&f.config(&tree), false)
                .unwrap()
                .is_empty()
        );
        fs::write(&file, legacy).unwrap();
        setup_worktree_build_caches(&tree, &f.source).unwrap();
        let migrated = fs::read_to_string(&file).unwrap();
        assert_eq!(
            config_target(&tree),
            central_target(&f.source, &tree).unwrap()
        );
        setup_worktree_build_caches(&tree, &f.source).unwrap();
        assert_eq!(fs::read_to_string(&file).unwrap(), migrated);
    }

    #[test]
    fn tracked_legacy_and_custom_wrapper_configs_are_preserved() {
        for custom in [
            DEFAULT,
            "[build]\ntarget-dir = 'custom'\nrustc-wrapper = 'my-wrapper'\n",
            "",
        ] {
            let f = Fixture::new(None);
            let tree = f.create("custom");
            let file = tree.join(".cargo/config.toml");
            fs::write(&file, custom).unwrap();
            setup_worktree_build_caches(&tree, &f.source).unwrap();
            assert!(
                crate::manager::worktree_build_env_with_policy(&f.config(&tree), false)
                    .unwrap()
                    .is_empty()
            );
            remove_generated_cargo_config(&tree, &f.source).unwrap();
            assert_eq!(fs::read_to_string(file).unwrap(), custom);
        }
        let f = Fixture::new(None);
        let tree = f.create("tracked-legacy");
        let legacy = legacy_config(&f.source);
        fs::write(tree.join(".cargo/config.toml"), &legacy).unwrap();
        run_git(tree.to_str().unwrap(), &["add", ".cargo/config.toml"]).unwrap();
        setup_worktree_build_caches(&tree, &f.source).unwrap();
        remove_generated_cargo_config(&tree, &f.source).unwrap();
        assert_eq!(
            fs::read_to_string(tree.join(".cargo/config.toml")).unwrap(),
            legacy
        );
        assert!(
            crate::manager::worktree_build_env_with_policy(&f.config(&tree), false)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn legacy_config_name_and_tracked_deletion_remain_user_owned() {
        let f = Fixture::new(Some(DEFAULT));
        let tree = f.create("legacy-name");
        fs::write(
            tree.join(".cargo/config"),
            "[build]\ntarget-dir = 'custom'\n",
        )
        .unwrap();
        setup_worktree_build_caches(&tree, &f.source).unwrap();
        assert!(
            crate::manager::worktree_build_env_with_policy(&f.config(&tree), false)
                .unwrap()
                .is_empty()
        );
        fs::remove_file(tree.join(".cargo/config")).unwrap();
        fs::remove_file(tree.join(".cargo/config.toml")).unwrap();
        setup_worktree_build_caches(&tree, &f.source).unwrap();
        assert!(!tree.join(".cargo/config.toml").exists());
        assert!(
            crate::manager::worktree_build_env_with_policy(&f.config(&tree), false)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn parallel_managed_worktrees_have_disjoint_outputs_and_preserve_main() {
        let f = Fixture::new(None);
        let first = f.create("first");
        let second = f.create("second");
        let first_target = config_target(&first);
        let second_target = config_target(&second);
        assert_ne!(first_target, second_target);
        assert_eq!(first_target.parent(), second_target.parent());
        let protected = f.source.join("target/release");
        fs::create_dir_all(&protected).unwrap();
        fs::write(protected.join("Wardian"), "qualified-main").unwrap();
        fs::write(f.source.join("package-lock.json"), "user-dirty-lock").unwrap();
        std::thread::scope(|scope| {
            for (target, bytes) in [(&first_target, "first"), (&second_target, "second")] {
                scope.spawn(move || {
                    fs::create_dir_all(target.join("debug")).unwrap();
                    fs::write(target.join("debug/same-output"), bytes).unwrap();
                });
            }
        });
        assert_eq!(
            fs::read_to_string(first_target.join("debug/same-output")).unwrap(),
            "first"
        );
        assert_eq!(
            fs::read_to_string(second_target.join("debug/same-output")).unwrap(),
            "second"
        );
        assert_eq!(
            fs::read_to_string(protected.join("Wardian")).unwrap(),
            "qualified-main"
        );
        assert_eq!(
            fs::read_to_string(f.source.join("package-lock.json")).unwrap(),
            "user-dirty-lock"
        );
        remove_generated_cargo_config(&first, &f.source).unwrap();
        assert!(
            first_target.join("debug/same-output").exists(),
            "cleanup must never prune compiler outputs"
        );
    }

    #[test]
    fn source_alias_from_linked_worktree_has_same_repository_key() {
        let f = Fixture::new(None);
        let first = f.create("first");
        let second = f.create("second");
        assert_eq!(
            central_target(&f.source, &second).unwrap(),
            central_target(&first, &second).unwrap()
        );
        assert!(central_target(&f.source, &f.source).is_err());
    }

    #[test]
    fn cache_keys_match_launcher_sha256_vectors() {
        assert_eq!(
            path_key(Path::new("/repo/worktree")).unwrap(),
            "bf536848051b4b5b"
        );
        assert_eq!(
            path_key(Path::new("/repo/worktree-two")).unwrap(),
            "355b559c49cdb418"
        );
        #[cfg(windows)]
        assert_eq!(
            path_key(Path::new("/REPO/WORKTREE")).unwrap(),
            "bf536848051b4b5b"
        );
    }

    #[cfg(unix)]
    #[test]
    fn literal_backslash_worktrees_keep_distinct_registration_keys_and_direct_lanes() {
        let f = Fixture::new(None);
        let root = f.source.with_file_name("project.wt");
        let literal = root.join(r"a\b");
        let nested = root.join("a/b");
        create_worktree_with_build_caches(&f.source, &literal, "wardian/literal-backslash")
            .unwrap();
        let literal = absolute_existing_path(&literal).unwrap();
        fs::create_dir_all(&nested).unwrap();
        let nested = absolute_existing_path(&nested).unwrap();
        assert!(central_target(&f.source, &nested)
            .unwrap_err()
            .contains("not registered"));

        create_worktree_with_build_caches(&f.source, &nested, "wardian/nested-separator").unwrap();
        let entries = list_git_worktrees(&f.source).unwrap();
        for tree in [&literal, &nested] {
            assert!(entries
                .iter()
                .any(|entry| absolute_existing_path(&entry.path).unwrap() == *tree));
            assert_eq!(
                config_target(tree),
                central_target(&f.source, tree).unwrap()
            );
        }
        assert_ne!(path_key(&literal).unwrap(), path_key(&nested).unwrap());
        let literal_target = config_target(&literal);
        let nested_target = config_target(&nested);
        assert_ne!(literal_target, nested_target);
        assert_eq!(literal_target.parent(), nested_target.parent());
        assert!(!contains_path(&literal, &nested));
        assert!(!contains_path(&nested, &literal));
        assert!(contains_path(&literal, &literal.join("child")));
        assert!(!contains_path(&literal, &nested.join("child")));
        assert!(!contains_path(&root.join("A"), &nested));
    }

    #[cfg(windows)]
    #[test]
    fn windows_keys_and_containment_preserve_separator_and_case_equivalence() {
        assert_eq!(
            path_key(Path::new(r"\REPO\WORKTREE")).unwrap(),
            "bf536848051b4b5b"
        );
        let parent = Path::new(r"D:\Repo\Target");
        assert_eq!(
            path_key(parent).unwrap(),
            path_key(Path::new("d:/repo/target")).unwrap()
        );
        assert!(contains_path(parent, Path::new("d:/repo/target")));
        assert!(contains_path(parent, Path::new("d:/repo/target/debug")));
        assert!(!contains_path(parent, Path::new("d:/repo/target-other")));
    }

    #[test]
    fn explicit_cache_root_is_disjoint_and_refuses_main_target_or_traversal() {
        let f = Fixture::new(None);
        let tree = f.create("root");
        let root = f.source.with_file_name("custom-cache");
        let target = central_target_with_root(&f.source, &tree, Some(&root)).unwrap();
        assert!(target.starts_with(root.join("direct-targets")));
        assert!(!root.exists());
        assert!(
            central_target_with_root(&f.source, &tree, Some(&f.source.join("target"))).is_err()
        );
        assert!(
            central_target_with_root(&f.source, &tree, Some(Path::new("relative-cache"))).is_err()
        );
        assert!(
            central_target_with_root(&f.source, &tree, Some(&root.join("../project/target")))
                .is_err()
        );
    }

    #[test]
    fn cleanup_recognizes_legacy_generated_config_without_pruning_outputs() {
        let f = Fixture::new(None);
        let tree = f.create("cleanup");
        fs::write(tree.join(".cargo/config.toml"), legacy_config(&f.source)).unwrap();
        fs::create_dir_all(f.source.join("target/release")).unwrap();
        fs::write(f.source.join("target/release/qualified"), "keep").unwrap();
        remove_generated_cargo_config(&tree, &f.source).unwrap();
        assert!(!tree.join(".cargo/config.toml").exists());
        assert_eq!(
            fs::read_to_string(f.source.join("target/release/qualified")).unwrap(),
            "keep"
        );
    }

    #[test]
    fn migration_without_git_ownership_fails_without_changing_config() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let tree = temp.path().join("tree");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(tree.join(".cargo")).unwrap();
        let legacy = legacy_config(&source);
        fs::write(tree.join(".cargo/config.toml"), &legacy).unwrap();
        assert!(write_cargo_worktree_config(&tree, &source).is_err());
        assert_eq!(
            fs::read_to_string(tree.join(".cargo/config.toml")).unwrap(),
            legacy
        );
    }

    #[test]
    fn linked_cargo_directory_and_central_root_fail_closed() {
        let f = Fixture::new(None);
        let tree = f.create("links");
        fs::remove_file(tree.join(".cargo/config.toml")).unwrap();
        fs::remove_dir(tree.join(".cargo")).unwrap();
        let protected = f.source.join(".cargo");
        fs::create_dir_all(&protected).unwrap();
        fs::write(protected.join("config.toml"), "keep-main").unwrap();
        crate::utils::fs::create_directory_link(&protected, &tree.join(".cargo")).unwrap();
        assert!(setup_worktree_build_caches(&tree, &f.source).is_err());
        assert_eq!(
            fs::read_to_string(protected.join("config.toml")).unwrap(),
            "keep-main"
        );
        super::super::remove_link_path(&tree.join(".cargo")).unwrap();
        let root = f.source.with_file_name("project.cargo-cache");
        crate::utils::fs::create_directory_link(&f.source, &root).unwrap();
        assert!(setup_worktree_build_caches(&tree, &f.source).is_err());
        assert!(crate::manager::worktree_build_env_with_policy(&f.config(&tree), false).is_err());
        super::super::remove_link_path(&root).unwrap();
    }
}
