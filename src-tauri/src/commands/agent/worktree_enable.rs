//! Source resolution for repeated managed-worktree assignment.

use super::AgentConfig;

pub(super) fn resolve_agent_worktree_source(
    config: &AgentConfig,
) -> Result<std::path::PathBuf, String> {
    let source = config
        .git_worktree_source
        .as_deref()
        .map(str::trim)
        .filter(|source| !source.is_empty())
        .unwrap_or(config.folder.trim());
    if source.is_empty() {
        return Err("Agent workspace is not configured".to_string());
    }
    crate::commands::git::primary_worktree_path(std::path::Path::new(source))
}

#[cfg(test)]
mod tests {
    use super::super::{enable_worktree_config, resolve_agent_worktree_path};
    use super::*;

    #[test]
    fn enabling_twice_from_a_linked_source_uses_siblings_and_default_branch() {
        use crate::commands::git::{create_worktree_with_build_caches, run_git};
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("project");
        std::fs::create_dir(&repo).unwrap();
        let cwd = repo.to_str().unwrap();
        run_git(cwd, &["init", "-b", "main"]).unwrap();
        run_git(cwd, &["config", "user.name", "Wardian Test"]).unwrap();
        run_git(cwd, &["config", "user.email", "test@example.invalid"]).unwrap();
        run_git(cwd, &["commit", "--allow-empty", "-m", "default"]).unwrap();
        let default_head = run_git(cwd, &["rev-parse", "HEAD"]).unwrap();
        let first = resolve_agent_worktree_path(&repo, Some("first"), "agent");
        create_worktree_with_build_caches(&repo, &first, "wardian/first").unwrap();
        run_git(
            first.to_str().unwrap(),
            &["commit", "--allow-empty", "-m", "feature"],
        )
        .unwrap();
        // Imported agents can record a linked checkout as their original source.
        let mut config = AgentConfig {
            folder: first.to_string_lossy().to_string(),
            git_worktree_source: Some(first.to_string_lossy().to_string()),
            ..Default::default()
        };
        for name in ["second", "third"] {
            let source = resolve_agent_worktree_source(&config).unwrap();
            let next = resolve_agent_worktree_path(&source, Some(name), "agent");
            create_worktree_with_build_caches(&source, &next, &format!("wardian/{name}")).unwrap();
            enable_worktree_config(&mut config, &next);
            assert_eq!(next, temp.path().join("project.wt").join(name));
            assert_eq!(
                run_git(next.to_str().unwrap(), &["rev-parse", "HEAD"]).unwrap(),
                default_head
            );
        }
    }
}
