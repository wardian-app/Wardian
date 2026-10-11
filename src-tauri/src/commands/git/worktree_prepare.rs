//! Unassigned source isolation: no AppState, agent configuration or provider access.

use super::{
    absolute_existing_path, list_git_worktrees, path_to_git_arg, run_git, run_git_allowing_status,
    setup_worktree_build_caches,
};
use std::path::Path;
use wardian_core::control::{AgentWorktreePrepareResponse, CONTROL_SCHEMA};

pub(crate) fn local_branch_exists(workspace: &str, branch: &str) -> Result<bool, String> {
    let reference = format!("refs/heads/{branch}");
    Ok(!run_git_allowing_status(
        workspace,
        &["rev-parse", "--verify", "--quiet", &reference],
        &[0, 1],
    )?
    .trim()
    .is_empty())
}

/// Prepare a new sibling checkout from an explicitly resolved commit.
/// Existing paths and branches are never reused, reset or automatically removed.
pub(crate) fn prepare_worktree(
    source: &str,
    name: &str,
    reference: &str,
) -> Result<AgentWorktreePrepareResponse, String> {
    let source = Path::new(source.trim());
    if !source.is_absolute() {
        return Err("Source repository path must be absolute".into());
    }
    // A strict directory name avoids traversal, lossy slug collisions and Windows device names.
    if name.is_empty()
        || name.len() > 64
        || !name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        || !name.as_bytes()[0].is_ascii_alphanumeric()
        || name.ends_with('-')
        || matches!(name, "con" | "prn" | "aux" | "nul")
        || (name.len() == 4
            && (name.starts_with("com") || name.starts_with("lpt"))
            && matches!(name.as_bytes()[3], b'1'..=b'9'))
    {
        return Err("Worktree name must be 1-64 lowercase letters, digits or hyphens, start with a letter or digit, and not be a reserved device name".into());
    }
    let reference = reference.trim();
    if reference.is_empty() {
        return Err("An explicit commit reference is required".into());
    }
    let requested_source = absolute_existing_path(source)?;
    let reference_cwd = path_to_git_arg(&requested_source)?;
    let entries = list_git_worktrees(&requested_source)?;
    let primary = entries
        .first()
        .ok_or("Repository has no primary worktree")?;
    let source = absolute_existing_path(&primary.path)?;
    let source_arg = path_to_git_arg(&source)?;
    let basename = source
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or("Source repository name must be UTF-8")?;
    let root = source.with_file_name(format!("{basename}.wt"));
    match std::fs::symlink_metadata(&root) {
        Ok(metadata) => {
            let linked = metadata.file_type().is_symlink();
            #[cfg(windows)]
            let linked = {
                use std::os::windows::fs::MetadataExt;
                linked || metadata.file_attributes() & 0x400 != 0
            };
            if linked || !metadata.is_dir() {
                return Err("Worktree root must be a plain directory".into());
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.to_string()),
    }
    let target = root.join(name);
    match std::fs::symlink_metadata(&target) {
        Ok(_) => return Err("Worktree target path is already occupied".into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.to_string()),
    }
    let branch = format!("wardian/{name}");
    if local_branch_exists(&source_arg, &branch)? {
        return Err("Worktree branch already exists".into());
    }
    let commit = run_git(
        &reference_cwd,
        &[
            "rev-parse",
            "--verify",
            "--end-of-options",
            &format!("{reference}^{{commit}}"),
        ],
    )?
    .trim()
    .to_string();
    let target_arg = path_to_git_arg(&target)?;
    // Git atomically refuses a concurrently occupied branch/target. No --force, -B or cleanup.
    run_git(
        &source_arg,
        &["worktree", "add", "-b", &branch, &target_arg, &commit],
    )?;
    let target = absolute_existing_path(&target)?;
    setup_worktree_build_caches(&target, &source).map_err(|error| {
        format!("Worktree was created and retained, but build-cache setup failed: {error}")
    })?;
    Ok(AgentWorktreePrepareResponse {
        schema: CONTROL_SCHEMA,
        ok: true,
        source_folder: path_to_git_arg(&source)?,
        worktree_folder: path_to_git_arg(&target)?,
        branch_name: branch,
        commit,
        cleared_session: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (tempfile::TempDir, std::path::PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("repo");
        std::fs::create_dir(&source).unwrap();
        let cwd = source.to_str().unwrap();
        run_git(cwd, &["init"]).unwrap();
        run_git(cwd, &["config", "user.email", "test@example.com"]).unwrap();
        run_git(cwd, &["config", "user.name", "Wardian Test"]).unwrap();
        run_git(cwd, &["config", "commit.gpgsign", "false"]).unwrap();
        run_git(cwd, &["config", "core.autocrlf", "false"]).unwrap();
        std::fs::write(source.join("README.md"), "initial\n").unwrap();
        run_git(cwd, &["add", "README.md"]).unwrap();
        run_git(cwd, &["commit", "-m", "initial"]).unwrap();
        (temp, source)
    }
    #[test]
    fn prepares_explicit_commit_without_changing_source_index_or_dirty_files() {
        let (_temp, source) = fixture();
        let cwd = source.to_str().unwrap();
        let commit = run_git(cwd, &["rev-parse", "HEAD"]).unwrap();
        std::fs::write(source.join("README.md"), "staged\n").unwrap();
        run_git(cwd, &["add", "README.md"]).unwrap();
        std::fs::write(source.join("README.md"), "unstaged\n").unwrap();
        let status = run_git(cwd, &["status", "--porcelain=v1"]).unwrap();
        let index = std::fs::read(source.join(".git/index")).unwrap();
        let result = prepare_worktree(cwd, "review-fixes", commit.trim()).unwrap();
        assert_eq!(result.commit, commit.trim());
        assert!(!result.cleared_session);
        assert_eq!(
            run_git(&result.worktree_folder, &["rev-parse", "HEAD"]).unwrap(),
            commit
        );
        assert_eq!(
            std::fs::read_to_string(Path::new(&result.worktree_folder).join("README.md")).unwrap(),
            "initial\n"
        );
        assert_eq!(std::fs::read(source.join(".git/index")).unwrap(), index);
        assert_eq!(run_git(cwd, &["status", "--porcelain=v1"]).unwrap(), status);
        assert_eq!(run_git(cwd, &["rev-parse", "HEAD"]).unwrap(), commit);
        assert!(list_git_worktrees(&source)
            .unwrap()
            .iter()
            .any(|entry| entry.branch.as_deref() == Some("wardian/review-fixes")));
    }
    #[test]
    fn conflicts_and_invalid_inputs_preserve_existing_work() {
        let (temp, source) = fixture();
        let cwd = source.to_str().unwrap();
        let root = temp.path().join("repo.wt");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("occupied"), "keep").unwrap();
        assert!(prepare_worktree(cwd, "occupied", "HEAD").is_err());
        assert_eq!(
            std::fs::read_to_string(root.join("occupied")).unwrap(),
            "keep"
        );
        std::fs::create_dir(root.join("occupied-dir")).unwrap();
        std::fs::write(root.join("occupied-dir/keep"), "keep").unwrap();
        assert!(prepare_worktree(cwd, "occupied-dir", "HEAD").is_err());
        assert_eq!(
            std::fs::read_to_string(root.join("occupied-dir/keep")).unwrap(),
            "keep"
        );
        run_git(cwd, &["branch", "wardian/conflict"]).unwrap();
        assert!(prepare_worktree(cwd, "conflict", "HEAD").is_err());
        assert!(!root.join("conflict").exists());
        for name in ["../escape", "", "con", "com1", "Review Fixes"] {
            assert!(prepare_worktree(cwd, name, "HEAD").is_err());
        }
        assert!(prepare_worktree(cwd, "invalid-ref", "missing-ref").is_err());
        assert!(prepare_worktree(cwd, "invalid-ref", "--all").is_err());
        assert!(!root.join("invalid-ref").exists());
        assert_eq!(list_git_worktrees(&source).unwrap().len(), 1);
    }
    #[test]
    fn preparation_preserves_tracked_cargo_policy() {
        let (_temp, source) = fixture();
        let cwd = source.to_str().unwrap();
        std::fs::write(source.join("Cargo.toml"), "[workspace]\n").unwrap();
        std::fs::create_dir(source.join(".cargo")).unwrap();
        let policy = b"[build]\ntarget-dir = \"target\"\n";
        std::fs::write(source.join(".cargo/config.toml"), policy).unwrap();
        run_git(cwd, &["add", "Cargo.toml", ".cargo/config.toml"]).unwrap();
        run_git(cwd, &["commit", "-m", "cargo policy"]).unwrap();
        let result = prepare_worktree(cwd, "cargo-policy", "HEAD").unwrap();
        assert_eq!(
            std::fs::read(source.join(".cargo/config.toml")).unwrap(),
            policy
        );
        assert_eq!(
            std::fs::read(Path::new(&result.worktree_folder).join(".cargo/config.toml")).unwrap(),
            policy
        );
        assert!(run_git(cwd, &["status", "--porcelain=v1"])
            .unwrap()
            .is_empty());
        assert!(
            run_git(&result.worktree_folder, &["status", "--porcelain=v1"])
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn preparation_refuses_linked_root_without_writing_its_target() {
        let (temp, source) = fixture();
        let elsewhere = temp.path().join("elsewhere");
        std::fs::create_dir(&elsewhere).unwrap();
        let root = temp.path().join("repo.wt");
        crate::utils::fs::create_directory_link(&elsewhere, &root).unwrap();
        let error = prepare_worktree(source.to_str().unwrap(), "review", "HEAD").unwrap_err();
        assert!(error.contains("plain directory"));
        assert_eq!(std::fs::read_dir(elsewhere).unwrap().count(), 0);
        assert_eq!(list_git_worktrees(&source).unwrap().len(), 1);
    }

    #[test]
    fn cache_failure_retains_created_checkout_and_branch() {
        let (temp, source) = fixture();
        let cwd = source.to_str().unwrap();
        std::fs::write(source.join("package.json"), "{}\n").unwrap();
        run_git(cwd, &["add", "package.json"]).unwrap();
        run_git(cwd, &["commit", "-m", "node manifests"]).unwrap();
        std::fs::write(source.join("node_modules"), "keep").unwrap();
        let error = prepare_worktree(cwd, "cache-failure", "HEAD").unwrap_err();
        assert!(error.contains("created and retained"));
        assert!(temp.path().join("repo.wt/cache-failure/.git").is_file());
        assert!(local_branch_exists(cwd, "wardian/cache-failure").unwrap());
        assert_eq!(list_git_worktrees(&source).unwrap().len(), 2);
        assert_eq!(
            std::fs::read_to_string(source.join("node_modules")).unwrap(),
            "keep"
        );
    }

    #[test]
    fn linked_source_uses_primary_sibling_root() {
        let (temp, source) = fixture();
        let cwd = source.to_str().unwrap();
        let first = prepare_worktree(cwd, "first", "HEAD").unwrap();
        std::fs::write(
            Path::new(&first.worktree_folder).join("README.md"),
            "linked branch change\n",
        )
        .unwrap();
        run_git(&first.worktree_folder, &["add", "README.md"]).unwrap();
        run_git(&first.worktree_folder, &["commit", "-m", "linked change"]).unwrap();
        let linked_commit = run_git(&first.worktree_folder, &["rev-parse", "HEAD"]).unwrap();
        let second = prepare_worktree(&first.worktree_folder, "second", "HEAD").unwrap();
        assert_eq!(second.commit, linked_commit.trim());
        assert_ne!(
            second.commit,
            run_git(cwd, &["rev-parse", "HEAD"]).unwrap().trim()
        );
        assert_eq!(
            Path::new(&second.worktree_folder),
            absolute_existing_path(&temp.path().join("repo.wt/second")).unwrap()
        );
        assert_eq!(second.source_folder, first.source_folder);
    }
}
