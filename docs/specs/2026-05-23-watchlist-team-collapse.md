# Watchlist Team Collapse

- **Status:** Planned
- **Date:** 2026-05-23
- **Decider:** Wardian maintainers

## Context and Problem Statement

The right watchlist sidebar can group agents into teams, but expanded teams consume a large amount of vertical space when a workspace has many active agents. Users need a compact way to keep teams visible while hiding their members until detail is needed.

Wardian already stores watchlist presentation preferences separately from watchlist membership in `watchlists/prefs.json`. Team definitions remain part of watchlist state because they describe organization and ordering, while column visibility and sorting are personal UI preferences.

## Decision

Add a chevron-only collapse control to each rendered team header in the watchlist sidebar.

- Clicking the chevron toggles that team's expanded or collapsed state.
- Clicking the rest of the team header continues to select the team members.
- Right-clicking the team header continues to open the team context menu.
- Dragging the team header continues to move the team block.
- Collapsed teams keep their header visible and hide member rows.
- The header continues to show the team name and visible member count.

Persist collapsed team IDs in watchlist preferences rather than in team definitions. This keeps display state separate from team membership and preserves future room for team sync, export, CLI inspection, and shared team semantics.

## Data Model

Extend `WatchlistPrefs` with an optional or defaulted field:

```ts
collapsed_team_ids: string[];
```

Existing preference files without this field load as an empty collapsed set. Saving preferences writes the normalized field with the rest of the existing column and sort preferences.

The collapsed state is global per team ID, not per watchlist tab. If a team appears in All Agents and a custom watchlist, collapsing it in one watchlist keeps it collapsed anywhere that same team block is rendered.

## Rendering Behavior

When teams are rendered as team blocks:

- Expanded teams render the header, member rows, and the normal before/after drop zones.
- Collapsed teams render only the header and the outer before/after team drop zones needed to move the team block.
- The chevron points down when expanded and right when collapsed.
- The control has an accessible label that includes the team name and current action.

When a column sort is active and the existing preference flattens teams into individual rows, the current behavior remains unchanged. Team collapse controls are not shown because teams are not being rendered as blocks. If sorted team grouping is preserved, team blocks remain visible and collapse state applies.

## Interaction Rules

The chevron is the only collapse toggle. It stops event propagation so it does not select members, start a drag, or open the context menu.

Collapsed team headers still support:

- Team context menu actions.
- Team block drag and drop before or after other agents or teams.
- Selection of all currently visible team members when the header body is clicked.

Collapsed hidden members are still part of the team for data operations. Bulk team context actions still apply to all team members because the team is the target, not the rendered rows.

## Testing

Add focused frontend tests for `AgentWatchlist`:

- A team renders expanded by default when `collapsed_team_ids` is empty or absent.
- Clicking the chevron hides and reveals member rows without firing header selection.
- `onPrefsChange` receives the updated collapsed team ID list.
- A collapsed team still exposes team context menu actions.
- Sorted flattening keeps the existing flattened row behavior without team collapse controls.

No backend changes are required. Existing watchlist utility tests should continue to cover team membership, ordering, and normalization.

## Documentation

Update the watchlists guide to describe the team chevron and persistence alongside the existing column and sorting preferences.

Because this is a frontend behavior change, the implementation PR should include a feature-specific screenshot showing at least one collapsed team and one expanded team in the right watchlist sidebar.
