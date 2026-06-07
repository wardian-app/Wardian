# CLI Surface Parity Matrix

- **Status:** Implemented
- **Date:** 2026-05-12

## Context

Wardian's CLI is useful for agent identity, lifecycle, workflows, and prompt delivery, but the GUI/backend expose broader operational surfaces through Tauri commands and frontend state. This audit maps the current parity boundary and records the next high-leverage CLI slices.

## Parity Matrix

| Surface | GUI/backend capability | CLI status after this slice | Deferred follow-up |
|---|---|---|---|
| Agents | list/show/spawn/clone/pause/resume/kill/watch/wait/send/ask | Supported | Add rename, reorder, clear session, update config, and clone preview wrappers |
| Agent worktrees | list/create/assign/join/disable through `list_agent_worktrees`, `enable_agent_worktree`, `assign_agent_worktree`, `disable_agent_worktree`, followed by `clear_agent_session` on workspace moves | Added `wardian agent worktree list`, `enable`, `join`, and `disable` through live control; mutations clear the agent session after moving workspace | Add native E2E coverage for real provider/runtime movement and optional branch detail in list output |
| Teams/watchlists | Watchlist state v2 stores global `teams`, ordered team members, and watchlist entries for agents or teams; legacy flat arrays still load | Added read-only `wardian team list/show` and `wardian watchlist list/show` from the existing `watchlists/index.json` file, including legacy array normalization | Add mutations only through the same v2 state shape; add `wardian send --to team:<name>` only after explicit target-resolution tests |
| Classes | list/create/delete/default/reset through `commands/class.rs` | Not added | Add read-only `wardian class list/show`; mutations need class-library file safety review |
| Workflows/schedules | workflow list/show/save/delete/run/stop, scheduled runs, trigger pause/resume, run-now, library save/load | CLI has workflow list/show/run/stop | Add scheduled run list/show, trigger pause/resume, run-now, and library import/export as separate workflow slice |
| Source control/git | GUI has status/log/diff/stage/unstage/discard/commit/pull/push/watch because it needs source-control buttons and panels | Worktree agent assignment added; broad git porcelain intentionally not added | Non-goal for the CLI by default. Terminal users should use real `git`; add only Wardian-specific coordination commands such as agent workspace/worktree inspection |
| Queue | persisted queue items plus read/dismiss state | Not added | Add read-only queue list/show before any mutation |
| Library/skills | library tree, metadata, deploy/remove skills, watches | Not added | Add read-only skills/library inspection before deploy/remove |
| Settings | shell/provider/session persistence settings | Not added | Add read-only settings export; mutations need schema and backup policy |
| Recent issue #228 | Slash-command delivery with `wardian send --as-command` | Not added | Separate focused CLI delivery issue; do not couple to parity worktree/team work |

## Implemented Slice

Worktree CLI commands route through the live control endpoint instead of reimplementing git worktree creation in the CLI:

```bash
wardian agent worktree list
wardian agent worktree enable <agent> [--name <worktree-name>]
wardian agent worktree join <agent> --worktree <absolute-worktree-path-or-id>
wardian agent worktree disable <agent>
```

The backend control handler calls the existing Tauri worktree functions and then calls `clear_agent_session` so the provider starts fresh in the moved workspace. Disable only removes the assignment and returns the agent to the recorded source workspace; it does not delete physical worktree folders.

Teams/watchlists are read-only in this slice:

```bash
wardian team list
wardian team show <team-name-or-id>
wardian watchlist list
wardian watchlist show <watchlist-name-or-id>
```

The CLI reads the existing `watchlists/index.json`, accepts v2 `{ version, watchlists, teams }` objects and legacy watchlist arrays, and emits stable `schema: 1` JSON.

## Risks

- Worktree mutation correctness depends on the running desktop app because only the live app owns PTY lifecycle and `clear_agent_session`.
- The CLI cannot prove real provider cwd movement without the native runtime harness.
- Team/watchlist commands are read-only and do not normalize partial-team display exactly like the frontend rendering pipeline; mutation and team send targeting should remain follow-up work.
