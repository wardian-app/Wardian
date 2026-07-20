# Watchlist State Persistence

- **Status:** Implemented
- **Date:** 2026-07-20

## Context

Team collapse state was held only by the mounted desktop roster component. It
therefore disappeared when Wardian restarted, even though the rest of the
watchlist preferences and team records were persisted. The state must not
depend on whether a team member is currently Off.

## Decision

Persist collapse state in `watchlists/prefs.json`, scoped by watchlist ID.
The special `all` scope represents the All Agents roster. The desktop roster
reads its displayed collapse state from those preferences and writes a new
preference snapshot whenever the team chevron changes.

```ts
interface WatchlistPrefs {
  collapsed_team_ids: string[];
  collapsed_team_ids_by_list?: Record<string, string[]>;
}
```

`collapsed_team_ids` remains the compatibility field for the All Agents
roster. When a legacy preferences file has that field but no scoped map, the
desktop loader migrates the values in memory to the `all` scope. New saves keep
both fields synchronized for that scope.

## Invariants

- Collapse state is presentation state. It does not alter team membership,
  watchlist entries, agent lifecycle state, or topology.
- Collapse state is independent for each custom watchlist and the All Agents
  roster.
- An Off member remains part of its team and does not clear the team's
  persisted collapse state.
- Restarting the desktop app restores the stored collapse state before the
  roster is used.
