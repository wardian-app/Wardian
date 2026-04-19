# Source Control

Wardian's Source Control tab lets you work with Git directly from the sidebar for the currently selected agent workspace.

## Scope and Context

- The panel is available when exactly one agent is selected.
- Operations run in that agent's resolved working directory.
- If the folder is not a Git repository, the panel shows a clear "Not a Git Repository" state.

## Branch Bar

At the top, Wardian shows:

- current branch name
- ahead/behind indicators
- pull and push actions

This is a quick sync layer for checking divergence and moving changes without leaving the app.

## File Sections

The panel groups files into:

- **Staged Changes**
- **Changes** (unstaged tracked files)
- **Untracked**

Available actions include:

- stage / unstage
- stage all / unstage all
- discard tracked edits
- open file diff

Clicking a file opens an inline diff modal with colored additions, deletions, and hunk markers.

## Commit Flow

Use the commit box to enter a message, then commit:

- button: **Commit**
- shortcut: `Ctrl+Enter` (or `Cmd+Enter` on macOS)

The commit action is enabled only when:

- there is a commit message, and
- at least one file is staged

## History

The History section shows recent commits for the selected workspace with message and short hash.

## Worktree Mode

Source Control also exposes a worktree toggle:

- `+ Worktree` enables worktree mode for that agent
- removing worktree returns the agent to main workspace behavior

When toggled for a running agent, Wardian updates the config and restarts/resumes the agent runtime so branch isolation is applied consistently.

## Safety Notes

- Discard is destructive; Wardian asks for confirmation.
- Git operations are executed with non-interactive credential prompts disabled to avoid blocking UI flows.
- If pull/push fails, check local credentials and remote permissions in your terminal environment.

## Related References

- [Explorer](./explorer.md)
- [Watchlists](./watchlists.md)
- [Spec 018: Source Control Panel](../specs/018-source-control-panel.md)
