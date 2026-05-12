# Source Control

Wardian's Source Control tab lets you work with Git directly from the sidebar for the currently selected agent workspace.

![Source Control panel showing branch state, commit box, staged changes, unstaged changes, and history](../assets/screenshots/source-control/status-panel.png)

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

Source Control also exposes worktree actions:

- `Create Worktree` opens an inline name field, then creates a worktree for that agent
- available shared worktrees can be joined from the same action area
- removing worktree returns the agent to main workspace behavior

When enabled, Wardian creates a named worktree under `<wardian-home>/agents/<session-id>/worktrees/`, creates a matching `wardian/<worktree-name>` branch, shares supported build caches with the source checkout, and moves the agent runtime to that path with a fresh provider session. Joining an existing shared worktree assigns the same worktree path to another agent and also starts that agent fresh in the shared path.

The same agent worktree controls are available from the CLI when the desktop app is running for the same `WARDIAN_HOME`:

```bash
wardian agent worktree list
wardian agent worktree enable <agent-name-or-id> --name <worktree-name>
wardian agent worktree join <agent-name-or-id> --worktree <absolute-worktree-path-or-id>
wardian agent worktree disable <agent-name-or-id>
```

## Safety Notes

- Discard is destructive; Wardian asks for confirmation.
- Git operations are executed with non-interactive credential prompts disabled to avoid blocking UI flows.
- Provider resume is workspace-path-bound. Worktree moves use a fresh provider session, not `--resume` from the new path.
- Removing a worktree assignment does not delete the physical worktree immediately; the provider may still have files or cwd handles open during the transition.
- If pull/push fails, check local credentials and remote permissions in your terminal environment.

## Related References

- [Explorer](./explorer.md)
- [Watchlists](./watchlists.md)
- [Spec 018: Source Control Panel](../specs/018-source-control-panel.md)
