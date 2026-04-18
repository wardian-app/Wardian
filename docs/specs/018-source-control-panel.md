# Spec 018: Source Control Panel

- **Status:** Implemented
- **Date:** 2026-04-17
- **Decider:** User

## Context and Problem Statement

Wardian lacked integrated git awareness. Agents often modify files in the workspace and users had no in-app way to review changes, stage files, commit, or push without switching to an external terminal. Additionally, the worktree feature (used for agent isolation) was hidden behind AdvancedSettings with no clear UX flow.

A secondary problem: polling git in the background on Windows caused a visible console window to flash on every poll, which was disruptive.

## Proposed Decision

### Source Control Icon Rail Tab

A new `"git"` tab is added to the sidebar icon rail, represented by a commit-graph SVG (two nodes on a vertical line with a branch extending from the lower node to a side node — matching VS Code's iconography).

### Git Sidebar Panel (`GitPanel`, `GitFileList`, `GitDiffView`)

Layout matches VS Code Source Control conventions:
- **Branch bar**: shows current branch name
- **Worktree action row**: action-first UX — `+ Worktree` dashed button when inactive; a cyan chip showing the worktree branch with `×` dismiss when active. Clicking either calls `update_agent_config` then `resume_agent` if the agent is running, so the change takes effect mid-session without spawning a new agent.
- **Commit box at top**: text area + commit button with checkmark icon
- **File sections**: "Staged Changes" and "Changes" with pill count badges; files show status letter (M/A/D/?) on the right and hover actions (stage/unstage/discard) in the middle
- **Diff view**: modal overlay with line-level colorization using `color-mix(in srgb, var(--color-wardian-*), transparent 90%)`

All colors use theme variables (`var(--color-wardian-warning)`, `var(--color-wardian-success)`, etc.).

### Rust Git Command Module (`commands/git.rs`, `models/git.rs`)

Twelve Tauri commands: `git_status`, `git_current_branch`, `git_log`, `git_diff_file`, `git_stage`, `git_unstage`, `git_discard_changes`, `git_commit`, `git_pull`, `git_push`, `git_create_worktree`, `git_remove_worktree`.

All commands use `new_headless_std_command("git")` (sets `CREATE_NO_WINDOW` on Windows) plus env vars `GIT_TERMINAL_PROMPT=0` and `GIT_ASKPASS=echo` to prevent console window flashes and credential-prompt hangs — matching VS Code's git extension behavior.

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

### AgentConfig: `git_worktree` flag

`git_worktree?: boolean` added to `AgentConfig` in both TypeScript (`src/types/index.ts`) and Rust (`models/agent_config.rs`). The GitPanel reads this flag to determine the active worktree state and writes it via `update_agent_config`.

## Consequences

- **Positive**: Full git workflow (review, stage, commit, push) without leaving Wardian
- **Positive**: No Windows console flashes from background git polling
- **Positive**: Explorer provides ambient git context (colored files) without a separate panel
- **Positive**: Worktree UX is action-first and visible in context rather than hidden in settings
- **Negative**: Worktree-on-resume has a known architectural limitation (providers bind session identity to CWD); proper fix tracked in Issue #118 (habitat-for-all + junction redirect)
- **Negative**: Git color coding in the explorer adds a 3-second polling interval per explorer session; if the workspace is a very large repo, `git status --untracked-files=all` may be slow (can be mitigated in future by using `--untracked-files=no`)
