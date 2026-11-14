//! Managed-worktree creation and default-branch selection.

use super::{
    absolute_existing_path, absolute_worktree_target_path, list_git_worktrees, local_branch_exists,
    path_to_git_arg, run_git, setup_worktree_build_caches,
};
use std::path::{Path, PathBuf};

/// Resolve the primary checkout even when the recorded source is a linked worktree.
pub(crate) fn primary_worktree_path(source: &Path) -> Result<PathBuf, String> {
    let entries = list_git_worktrees(source)?;
    let primary = entries
        .first()
        .ok_or("Repository has no primary worktree")?;
    absolute_existing_path(&primary.path)
}

/// Prefer the remote's advertised default; use conventional local defaults offline.
fn worktree_default_base(workspace: &str) -> Result<String, String> {
    let remote_default = run_git(
        workspace,
        &["symbolic-ref", "--quiet", "refs/remotes/origin/HEAD"],
    )
    .ok();
    let candidates = remote_default
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .into_iter()
        .chain([
            "refs/remotes/origin/main",
            "refs/remotes/origin/master",
            "refs/heads/main",
            "refs/heads/master",
        ]);
    for candidate in candidates {
        if let Ok(commit) = run_git(
            workspace,
            &["rev-parse", "--verify", &format!("{candidate}^{{commit}}")],
        ) {
            return Ok(commit.trim().to_string());
        }
    }
    Err("Cannot determine the default branch. Configure origin/HEAD or create main or master before enabling a worktree.".to_string())
}

pub(crate) fn create_worktree_with_build_caches(
    workspace_path: &Path,
    worktree_path: &Path,
    branch: &str,
) -> Result<(), String> {
    let workspace_path = absolute_existing_path(workspace_path)?;
    let worktree_path = absolute_worktree_target_path(&workspace_path, worktree_path);
    if let Some(parent) = worktree_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }

    let workspace = path_to_git_arg(&workspace_path)?;
    let worktree = path_to_git_arg(&worktree_path)?;
    let branch_exists = local_branch_exists(&workspace, branch)?;
    if branch_exists {
        run_git(&workspace, &["worktree", "add", &worktree, branch])?;
    } else {
        let base = worktree_default_base(&workspace)?;
        run_git(
            &workspace,
            &["worktree", "add", &worktree, "-b", branch, &base],
        )?;
    }

    let worktree_path = absolute_existing_path(&worktree_path)?;
    setup_worktree_build_caches(&worktree_path, &workspace_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worktree_default_base_prefers_advertised_remote_default() {
        let temp = tempfile::tempdir().unwrap();
        let cwd = temp.path().to_str().unwrap();
        run_git(cwd, &["init", "-b", "main"]).unwrap();
        run_git(cwd, &["config", "user.name", "Wardian Test"]).unwrap();
        run_git(cwd, &["config", "user.email", "test@example.invalid"]).unwrap();
        run_git(cwd, &["commit", "--allow-empty", "-m", "local"]).unwrap();
        run_git(cwd, &["update-ref", "refs/remotes/origin/trunk", "HEAD"]).unwrap();
        run_git(
            cwd,
            &[
                "symbolic-ref",
                "refs/remotes/origin/HEAD",
                "refs/remotes/origin/trunk",
            ],
        )
        .unwrap();
        let default = run_git(cwd, &["rev-parse", "HEAD"]).unwrap();
        run_git(cwd, &["commit", "--allow-empty", "-m", "local-only"]).unwrap();
        assert_eq!(super::worktree_default_base(cwd).unwrap(), default.trim());
        let worktree = temp.path().join("linked");
        create_worktree_with_build_caches(temp.path(), &worktree, "wardian/new").unwrap();
        assert_eq!(
            run_git(worktree.to_str().unwrap(), &["rev-parse", "HEAD"]).unwrap(),
            default
        );
    }

    #[test]
    fn worktree_default_base_refuses_feature_head_without_a_default() {
        let temp = tempfile::tempdir().unwrap();
        let cwd = temp.path().to_str().unwrap();
        run_git(cwd, &["init", "-b", "feature"]).unwrap();
        run_git(cwd, &["config", "user.name", "Wardian Test"]).unwrap();
        run_git(cwd, &["config", "user.email", "test@example.invalid"]).unwrap();
        run_git(cwd, &["commit", "--allow-empty", "-m", "feature"]).unwrap();
        assert!(super::worktree_default_base(cwd)
            .unwrap_err()
            .contains("Cannot determine"));
    }
}
