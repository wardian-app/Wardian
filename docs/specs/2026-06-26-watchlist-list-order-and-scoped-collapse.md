# Watchlist List Order and Scoped Team Collapse

- **Status:** Implemented
- **Date:** 2026-06-26

## Context and Problem Statement

The right watchlist sidebar supports custom watchlists, team grouping, agent
reordering, and team block reordering. Two visual behaviors are incomplete:

- Custom watchlist tabs cannot be reordered, so users cannot prioritize the
  lists themselves.
- Team collapse state is stored as a global team preference, so collapsing a
  team in one watchlist also collapses that same team in other watchlists.

Team definitions remain global because they describe durable agent grouping.
Watchlist tab order is durable list organization. Team collapse is a local
visual affordance for the currently viewed list.

## Decision

Add mouse drag-and-drop reordering for custom watchlist tabs in the right
sidebar. The fixed **All** tab stays first and cannot be reordered. Dragging one
custom list tab before or after another custom list tab updates the existing
`watchlists` array order and persists through the current watchlist save path.

Scope team collapsed state by active watchlist ID in both the desktop watchlist
component and the remote mobile watchlist store. The same team can be collapsed
in one custom list and expanded in another. The **All** view has its own visual
collapse scope. Existing global `collapsed_team_ids` preferences may seed the
initial **All** scope, but new toggles should not write team IDs back as a
global preference.

## Rendering Behavior

Custom list tabs continue to support:

- Click to activate the list.
- Double-click to rename.
- Right-click to open rename/delete actions.

Dragging a custom list tab over another custom list tab marks a before or after
drop target based on pointer position and reorders only the custom list array.
The **All** tab is not draggable and never becomes a custom-list drop target.

Team chevrons continue to hide or reveal members in rendered team blocks. This
applies to both the desktop sidebar and the mobile PWA watchlist. A collapsed
team state is read from the current active list scope:

- `all` for the **All Agents** view.
- The custom watchlist ID for a custom list.

Switching watchlists reads that list's scoped collapse state. Toggling a team in
one scope does not affect any other scope.

## Data Model

No persisted watchlist membership model changes are required.

The desktop component keeps scoped collapse state as UI state:

```ts
type CollapsedTeamsByList = Record<string, string[]>;
```

The remote mobile store keeps the same scoped state and exposes the current
active scope as `mobileCollapsedTeamIds` for the mobile watchlist view.

The existing `WatchlistPrefs.collapsed_team_ids` field remains in the type for
backward compatibility with existing preference files and the remote mobile
endpoint. Desktop `AgentWatchlist` and the remote mobile store no longer treat
it as the source of truth for all list scopes.

## Testing

Add focused frontend tests for `AgentWatchlist`:

- Dragging one custom watchlist tab before another calls `onWatchlistsChange`
  with the reordered list array.
- The **All** tab remains fixed and does not participate in custom-list
  reordering.
- Collapsing a team in one custom watchlist hides that team's members only in
  that watchlist.
- Switching to another watchlist containing the same team still renders that
  team expanded.
- Remote mobile store and view tests prove the same scoped collapse behavior in
  the mobile watchlist.

Run the focused component test before broader frontend verification.

## Documentation

Update the watchlists guide to mention custom list tab reordering and clarify
that desktop and mobile team collapse are scoped to the current watchlist view
rather than shared across every watchlist.
