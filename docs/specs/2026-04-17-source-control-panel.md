# Source Control Panel

- **Status:** Implemented
- **Date:** 2026-04-17

## Context and Problem Statement

Wardian lacked integrated git awareness. Agents often modify files in the workspace and users had no in-app way to review changes, stage files, commit, or push without switching to an external terminal. Additionally, the worktree feature (used for agent isolation) was hidden behind AdvancedSettings with no clear UX flow.

A secondary problem: polling git in the background on Windows caused a visible console window to flash on every poll, which was disruptive.

## Proposed Decision

### Source Control Icon Rail Tab

A new `"git"` tab is added to the sidebar icon rail, represented by a commit-graph SVG (two nodes on a vertical line with a branch extending from the lower node to a side node — matching VS Code's iconography).

### Git Sidebar Panel (`GitPanel`, `GitFileList`, `GitDiffView`)

Layout matches VS Code Source Control conventions:
- **Branch bar**: shows current branch name
- **Publish behavior**: when the selected branch has no upstream, the push action publishes it with `git push --set-upstream origin <branch>`, matching VS Code's one-click publish flow.
- **Worktree action row**: action-first UX - `Create Worktree` when inactive; clicking it expands the row into an inline name field with check/cancel controls. Enabling calls `enable_agent_worktree` with the entered name, which creates `<source-checkout>.wt/<slug>`, configures build-cache redirects, records the original workspace in `git_worktree_source`, records the worktree in `git_worktree_folder`, and moves `folder` to that worktree. Joining an existing shared worktree calls `assign_agent_worktree`. Unassigned available worktrees show a delete control that calls `delete_agent_worktree` after confirmation. The frontend follows create/join actions with `clear_agent_session`, so providers start fresh in the new path instead of resuming across a cwd change.
- **Commit box at top**: text area + commit button with checkmark icon
- **File sections**: "Staged Changes" and "Changes" with pill count badges; files show status letter (M/A/D/?) on the right and hover actions (stage/unstage/discard) in the middle
- **Diff view**: modal overlay with line-level colorization using `color-mix(in srgb, var(--color-wardian-*), transparent 90%)`

All colors use theme variables (`var(--color-wardian-warning)`, `var(--color-wardian-success)`, etc.). Git operation failures are shown inline in the panel instead of only being written to the developer console.

### Rust Git Command Module (`commands/git.rs`, `models/git.rs`)

Fourteen Tauri commands: `git_status`, `git_current_branch`, `git_log`, `git_diff_file`, `git_stage`, `git_unstage`, `git_discard_changes`, `git_commit`, `git_pull`, `git_push`, `git_create_worktree`, `git_remove_worktree`, `git_watch`, `git_unwatch`.

`git_status` returns current upstream metadata in addition to ahead/behind counts so the frontend can distinguish push from publish. `git_push` uses that same branch state operationally: existing tracked branches run plain `git push`, while unpublished local branches publish to `origin` and set upstream tracking.

All commands use a headless direct `git` command (sets `CREATE_NO_WINDOW` on Windows) plus env vars `GIT_TERMINAL_PROMPT=0` and `GIT_ASKPASS=echo` to prevent console window flashes and credential-prompt hangs — matching VS Code's git extension behavior.

`git_diff_file` returns a synthetic `/dev/null` no-index diff for untracked files so users can inspect new file contents before staging.

### Explorer Git Color Coding

`ExplorerPanel` polls `git_status` every 3 seconds using the agent's workspace root as cwd. A normalized `Record<string, string>` (forward-slash relative path → status letter) is passed to `FileTree`, which colors file names and shows a small status letter badge on the right:

| Status | Color |
|--------|-------|
| M (modified) | `var(--color-wardian-warning)` |
| A (added) | `var(--color-wardian-success)` |
| D (deleted) | `var(--color-wardian-error)` |
| ? (untracked) | `var(--color-wardian-success)` |
| R/C (renamed/copied) | `var(--color-wardian-processing)` |

Directories inherit amber coloring if any descendant has changes. When git is unavailable (non-git directory), the status map is empty and the explorer renders with no color change.

### AgentConfig: worktree fields

`git_worktree?: boolean`, `git_worktree_source?: string`, and `git_worktree_folder?: string` are present in `AgentConfig` in both TypeScript (`src/types/index.ts`) and Rust (`models/agent_config.rs`). These fields are optional and default cleanly for older saved configs. In worktree mode, `folder` is the provider launch workspace and is set to the active worktree; `git_worktree_source` records the original checkout so removing the worktree can return the agent to the source workspace.

### Git Worktree Registry Contract

Git is the authority for worktree existence. Wardian assignment fields (`git_worktree`, `git_worktree_source`, and `git_worktree_folder`) record which agent is using a worktree, but they do not create a valid Git worktree by themselves.

`list_agent_worktrees` returns the union of:

- Wardian-assigned worktrees from agent config.
- Git-registered worktrees for known source workspaces, including external worktrees that were created outside Wardian.

`enable_agent_worktree` must verify that any existing target folder is already present in `git worktree list --porcelain` for the source checkout before saving Wardian assignment state.

Disabling or leaving a worktree only removes Wardian assignment metadata. It does not delete the Git worktree. Physical deletion is a separate cleanup flow: `delete_agent_worktree` accepts only unassigned Git-registered worktrees under Wardian-managed roots, rejects assigned or external paths, removes Wardian-generated build-cache redirects, and uses non-force `git worktree remove` so dirty worktrees fail safely. The current managed root is the project-sibling `<source-checkout>.wt/` directory; legacy worktrees under `<wardian-home>/agents/<session-id>/worktrees/` are still recognized so existing users can clean them up. External Git worktrees remain joinable from Wardian but are deleted with normal Git tooling.

For Rust workspaces, provider runtimes launched in worktree mode receive `CARGO_TARGET_DIR=<source-checkout>/target`, which keeps builds out of the worktree even when the repository already tracks `.cargo/config.toml`. Wardian writes a generated Cargo config only when the worktree does not already contain one; Node and Python cache links continue to point `node_modules` and `.venv` back at the source checkout.

## Consequences

- **Positive**: Full git workflow (review, stage, commit, push) without leaving Wardian
- **Positive**: No Windows console flashes from background git polling
- **Positive**: Explorer provides ambient git context (colored files) without a separate panel
- **Positive**: Worktree UX is action-first and visible in context rather than hidden in settings
- **Positive**: Worktree enablement creates the actual git worktree and moves the provider runtime with fresh-session semantics, avoiding provider resume from a different workspace path.
- **Negative**: Removing a worktree assignment currently restores the agent runtime to the source checkout but leaves physical worktree cleanup to a later explicit cleanup flow. This avoids Windows cwd/file-handle failures while the provider is being moved.
- **Negative**: Git color coding in the explorer adds a 3-second polling interval per explorer session; if the workspace is a very large repo, `git status --untracked-files=all` may be slow (can be mitigated in future by using `--untracked-files=no`)
