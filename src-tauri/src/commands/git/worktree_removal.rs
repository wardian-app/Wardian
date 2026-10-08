#[cfg(windows)]
use super::link_matches_target;
use super::{
    absolute_existing_path, absolute_worktree_target_path, remove_generated_cache_link,
    remove_generated_cargo_config,
};
use std::path::{Path, PathBuf};

pub(super) fn cleanup_generated_worktree_build_caches(
    workspace_path: &Path,
    worktree_path: &Path,
) -> Result<(), String> {
    let workspace_path = absolute_existing_path(workspace_path)?;
    let worktree_path = absolute_worktree_target_path(&workspace_path, worktree_path);

    #[cfg(windows)]
    preflight_worktree_directory_reparse_points(&workspace_path, &worktree_path)?;

    if workspace_path.join("Cargo.toml").is_file() {
        remove_generated_cargo_config(&worktree_path, &workspace_path)?;
    }

    for name in ["node_modules", ".venv"] {
        let link = worktree_path.join(name);
        if let Some(target) =
            generated_worktree_cache_link_target(&workspace_path, &worktree_path, &link)
        {
            remove_generated_cache_link(&link, &target)?;
        }
    }

    Ok(())
}

fn generated_worktree_cache_link_target(
    workspace_path: &Path,
    worktree_path: &Path,
    link: &Path,
) -> Option<PathBuf> {
    if link == worktree_path.join("node_modules").as_path()
        && workspace_path.join("package.json").is_file()
    {
        return Some(workspace_path.join("node_modules"));
    }

    if link == worktree_path.join(".venv").as_path()
        && (workspace_path.join("pyproject.toml").is_file()
            || workspace_path.join("requirements.txt").is_file())
    {
        return Some(workspace_path.join(".venv"));
    }

    None
}

#[cfg(windows)]
fn preflight_worktree_directory_reparse_points(
    workspace_path: &Path,
    worktree_path: &Path,
) -> Result<(), String> {
    let root_metadata = match std::fs::symlink_metadata(worktree_path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(worktree_inspection_error(worktree_path, error)),
    };
    if !root_metadata.is_dir() || is_worktree_directory_reparse_point(&root_metadata) {
        return Err(unrecognized_worktree_directory_reparse_error(worktree_path));
    }

    let mut pending = vec![worktree_path.to_path_buf()];
    while let Some(directory) = pending.pop() {
        let entries = std::fs::read_dir(&directory)
            .map_err(|error| worktree_inspection_error(&directory, error))?;
        for entry in entries {
            let entry = entry.map_err(|error| worktree_inspection_error(&directory, error))?;
            let path = entry.path();
            let metadata = std::fs::symlink_metadata(&path)
                .map_err(|error| worktree_inspection_error(&path, error))?;

            if is_worktree_directory_reparse_point(&metadata) {
                let generated_target =
                    generated_worktree_cache_link_target(workspace_path, worktree_path, &path);
                let is_removable_generated_link = generated_target
                    .as_deref()
                    .is_some_and(|target| target.exists() && link_matches_target(&path, target));
                if !is_removable_generated_link {
                    return Err(unrecognized_worktree_directory_reparse_error(&path));
                }
                continue;
            }

            // Reparse targets are never queued, including allowlisted generated cache links.
            if metadata.is_dir() {
                pending.push(path);
            }
        }
    }

    Ok(())
}

#[cfg(windows)]
fn is_worktree_directory_reparse_point(metadata: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;

    const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;
    metadata.file_attributes() & FILE_ATTRIBUTE_DIRECTORY != 0
        && crate::utils::fs::is_directory_link(metadata)
}

#[cfg(windows)]
fn worktree_inspection_error(path: &Path, error: std::io::Error) -> String {
    format!(
        "Refusing to remove worktree because Wardian could not inspect {}: {error}. Resolve the filesystem error and retry.",
        path.display()
    )
}

#[cfg(windows)]
fn unrecognized_worktree_directory_reparse_error(path: &Path) -> String {
    format!(
        "Refusing to remove worktree because it contains an unrecognized directory reparse point at {}. Remove or relocate that directory link, then retry.",
        path.display()
    )
}

#[cfg(test)]
mod tests {
    use super::super::link_matches_target;
    #[cfg(windows)]
    use super::is_worktree_directory_reparse_point;
    use std::fs;
    #[cfg(windows)]
    use std::path::{Path, PathBuf};

    #[cfg(windows)]
    use super::super::remove_worktree_with_force;
    use super::super::{
        create_worktree_with_build_caches, git_worktree_contains_path,
        remove_worktree_without_force, run_git,
    };

    #[test]
    fn remove_worktree_without_force_cleans_generated_cache_redirects() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().join("workspace");
        let worktree = temp.path().join("agents").join("agent-1").join("worktree");
        fs::create_dir_all(&workspace).unwrap();
        let cwd = workspace.to_str().unwrap();
        run_git(cwd, &["init"]).unwrap();
        run_git(cwd, &["config", "--local", "core.autocrlf", "false"]).unwrap();
        fs::create_dir_all(workspace.join("node_modules")).unwrap();
        fs::write(
            workspace.join("Cargo.toml"),
            "[package]\nname = \"sample\"\n",
        )
        .unwrap();
        fs::write(workspace.join("package.json"), "{\"name\":\"sample\"}\n").unwrap();
        fs::write(
            workspace.join("package-lock.json"),
            "{\"lockfileVersion\":3}\n",
        )
        .unwrap();
        fs::write(workspace.join(".gitignore"), "node_modules/\n").unwrap();
        let cache_sentinel = workspace.join("node_modules").join("cache-sentinel.txt");
        fs::write(&cache_sentinel, "source cache survives\n").unwrap();

        run_git(cwd, &["config", "user.email", "test@example.com"]).unwrap();
        run_git(cwd, &["config", "user.name", "Wardian Test"]).unwrap();
        run_git(
            cwd,
            &[
                "add",
                "Cargo.toml",
                ".gitignore",
                "package.json",
                "package-lock.json",
            ],
        )
        .unwrap();
        run_git(cwd, &["commit", "-m", "initial"]).unwrap();

        create_worktree_with_build_caches(&workspace, &worktree, "wardian/repo-agent").unwrap();
        assert!(worktree.join(".cargo").join("config.toml").exists());
        assert!(link_matches_target(
            &worktree.join("node_modules"),
            &workspace.join("node_modules")
        ));

        remove_worktree_without_force(&workspace, &worktree).unwrap();

        assert!(!worktree.exists());
        assert!(!git_worktree_contains_path(&workspace, &worktree).unwrap());
        assert_eq!(
            fs::read_to_string(cache_sentinel).unwrap(),
            "source cache survives\n"
        );
    }

    #[cfg(windows)]
    fn worktree_with_generated_node_cache(root: &Path) -> (PathBuf, PathBuf, PathBuf) {
        let workspace = root.join("source");
        let worktree = root.join("source.wt").join("canary");
        let node_modules = workspace.join("node_modules");
        fs::create_dir_all(&node_modules).unwrap();
        let cwd = workspace.to_str().unwrap();
        run_git(cwd, &["init", "-q"]).unwrap();
        run_git(cwd, &["config", "--local", "core.autocrlf", "false"]).unwrap();
        fs::write(workspace.join("README.md"), "source remains intact\n").unwrap();
        fs::write(workspace.join(".gitignore"), ".tmp/\nnode_modules/\n").unwrap();
        fs::write(workspace.join("package.json"), "{\"name\":\"canary\"}\n").unwrap();
        fs::write(
            workspace.join("package-lock.json"),
            "{\"lockfileVersion\":3}\n",
        )
        .unwrap();
        let cache_sentinel = node_modules.join("cache-sentinel.txt");
        fs::write(&cache_sentinel, "source cache survives\n").unwrap();

        run_git(cwd, &["config", "user.email", "test@example.com"]).unwrap();
        run_git(cwd, &["config", "user.name", "Wardian Test"]).unwrap();
        run_git(
            cwd,
            &[
                "add",
                ".gitignore",
                "README.md",
                "package.json",
                "package-lock.json",
            ],
        )
        .unwrap();
        run_git(cwd, &["commit", "-q", "-m", "initial"]).unwrap();
        create_worktree_with_build_caches(&workspace, &worktree, "wardian/canary").unwrap();
        assert!(link_matches_target(
            &worktree.join("node_modules"),
            &node_modules
        ));

        (workspace, worktree, cache_sentinel)
    }

    #[cfg(windows)]
    fn assert_unrecognized_directory_reparse_refusal(force: bool) {
        let temp = tempfile::tempdir().unwrap();
        let (workspace, worktree, cache_sentinel) = worktree_with_generated_node_cache(temp.path());
        let outside_target = temp.path().join("outside-target");
        fs::create_dir_all(&outside_target).unwrap();
        let junction_sentinel = outside_target.join("sentinel.txt");
        fs::write(&junction_sentinel, "outside worktree survives\n").unwrap();

        let junction = worktree.join(".tmp").join("vendor-profile");
        fs::create_dir_all(junction.parent().unwrap()).unwrap();
        crate::utils::fs::create_directory_link(&outside_target, &junction).unwrap();
        let metadata = fs::symlink_metadata(&junction).unwrap();
        assert!(is_worktree_directory_reparse_point(&metadata));

        let worktree_git = worktree.to_string_lossy().into_owned();
        let workspace_git = workspace.to_string_lossy().into_owned();
        let source_before = fs::read_to_string(workspace.join("README.md")).unwrap();
        let source_head_before = run_git(&workspace_git, &["rev-parse", "HEAD"]).unwrap();
        let source_status_before = run_git(&workspace_git, &["status", "--porcelain"]).unwrap();
        let head_before = run_git(&worktree_git, &["rev-parse", "HEAD"]).unwrap();
        let error = if force {
            remove_worktree_with_force(&workspace, &worktree)
        } else {
            remove_worktree_without_force(&workspace, &worktree)
        }
        .expect_err("unknown directory junction must stop removal before Git runs");

        assert!(
            error.contains("unrecognized directory reparse point"),
            "{error}"
        );
        assert!(
            error.contains(&junction.to_string_lossy().into_owned()),
            "{error}"
        );
        assert!(worktree.is_dir());
        assert!(git_worktree_contains_path(&workspace, &worktree).unwrap());
        assert_eq!(
            fs::read_to_string(workspace.join("README.md")).unwrap(),
            source_before
        );
        assert_eq!(
            run_git(&workspace_git, &["rev-parse", "HEAD"]).unwrap(),
            source_head_before
        );
        assert_eq!(
            run_git(&workspace_git, &["status", "--porcelain"]).unwrap(),
            source_status_before
        );
        assert_eq!(
            run_git(&worktree_git, &["rev-parse", "HEAD"]).unwrap(),
            head_before
        );
        assert_eq!(
            fs::read_to_string(cache_sentinel).unwrap(),
            "source cache survives\n"
        );
        assert!(link_matches_target(
            &worktree.join("node_modules"),
            &workspace.join("node_modules")
        ));
        assert_eq!(
            fs::read_to_string(junction_sentinel).unwrap(),
            "outside worktree survives\n"
        );
    }

    #[cfg(windows)]
    #[test]
    fn remove_worktree_without_force_refuses_unknown_directory_junction_before_cleanup() {
        assert_unrecognized_directory_reparse_refusal(false);
    }

    #[cfg(windows)]
    #[test]
    fn remove_worktree_with_force_refuses_unknown_directory_junction_before_cleanup() {
        assert_unrecognized_directory_reparse_refusal(true);
    }
}
