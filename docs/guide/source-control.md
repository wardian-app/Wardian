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

This is a quick sync layer for checking divergence and moving changes without leaving the app. If the selected branch has not been published yet, the push action publishes it to `origin` and sets the upstream branch.

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

Clicking a file opens an inline diff modal with colored additions, deletions, and hunk markers. Untracked files show a new-file diff so you can inspect them before staging.

## Commit Flow

Use the commit box to enter a message, then commit:

- button: **Commit**
- shortcut: `Ctrl+Enter` (or `Cmd+Enter` on macOS)

The commit action is enabled when:

- there is a commit message, and
- at least one file is staged or unstaged

If there are only unstaged changes, Wardian stages them before creating the commit.

## History

The History section shows recent commits for the selected workspace with message and short hash.

## Worktree Mode

Source Control also exposes worktree actions:

- `Create Worktree` opens an inline name field, then creates a worktree for that agent
- available shared worktrees can be joined from the same action area
- removing worktree returns the agent to main workspace behavior

When enabled, Wardian creates a named worktree beside the source checkout under `<source-checkout>.wt/<worktree-name>`, creates a matching `wardian/<worktree-name>` branch, shares supported build caches with the source checkout, and moves the agent runtime to that path with a fresh provider session. For example, a source checkout at `<absolute-workspace-path>/Wardian` creates Wardian-managed worktrees under `<absolute-workspace-path>/Wardian.wt/`. Joining an existing shared worktree assigns the same worktree path to another agent and also starts that agent fresh in the shared path.

For Rust workspaces, Wardian sets `CARGO_TARGET_DIR` for provider runtimes in worktree mode so builds reuse the source checkout's `target` directory even when the repository has a tracked `.cargo/config.toml`. Wardian still writes a generated Cargo config only when a worktree does not already contain one. Node `node_modules` and Python `.venv` caches continue to be linked back to the source checkout when those caches exist.

Wardian-created worktrees are real Git worktrees. They are created through `git worktree add`, so they appear in `git worktree list` for the source checkout.

Wardian also discovers Git worktrees that already belong to a known source workspace, even when they were created outside Wardian. Discovered worktrees with no assigned agent appear as joinable shared worktrees. Unassigned worktrees under `<source-checkout>.wt/` can also be deleted from the same list; legacy Wardian worktrees under `<wardian-home>/agents/<session-id>/worktrees/` remain recognized and deletable. Wardian asks for confirmation, removes Wardian-generated cache redirects, and then runs a non-force `git worktree remove`, so dirty or otherwise unsafe removals fail with Git's error.

If a target worktree folder already exists but Git does not recognize it as a worktree for the source checkout, Wardian refuses to assign it. Create it with `git worktree add` or remove the folder and let Wardian create it.

Removing a Wardian assignment does not delete the physical worktree. Wardian's delete action is separate and only applies to unassigned Git-registered worktrees under Wardian-managed worktree roots. External worktrees remain joinable but use manual Git deletion, for example `git worktree remove <absolute-worktree-path>`, after no agent is using that path.

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
- Removing a worktree assignment does not delete the physical worktree; use the separate delete action only after no agent is assigned.
- If pull/push fails, check local credentials and remote permissions in your terminal environment.

## Related References

- [Explorer](./explorer.md)
- [Watchlists](./watchlists.md)
- [Queue](./queue.md)
