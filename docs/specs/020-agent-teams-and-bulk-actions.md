# Spec 020: Agent Teams and Bulk Actions

- **Status:** Proposed
- **Date:** 2026-04-19
- **Decider:** User

## Context and Problem Statement

Wardian already supports multi-select in the Watchlist through Ctrl-click and Shift-click, but the actions exposed through the agent context menu still mostly behave as single-agent actions. Users also need a way to group related agents into teams so the Roster, Dashboard, and Grid can present cooperating agents as a unified command-center unit.

Issue 76 requests a global Team primitive with visual grouping, unified movement, team-level bulk actions, named teams, dynamic membership, and persistence in watchlist state. The key design constraint is that teams are global, and their member sequence should stay synchronized across watchlists rather than becoming local per watchlist.

## Proposed Decision

Wardian will model teams as global records persisted with watchlist state. Watchlists will store ordered entries that can point either to a solo agent or to a team. Existing flat watchlists will be migrated in memory and saved back in the new shape after the first change.

### Data Model

```ts
export interface AgentTeam {
  id: string;
  name: string;
  agentIds: string[];
}

export type WatchlistEntry =
  | { type: "agent"; agentId: string }
  | { type: "team"; teamId: string };

export interface Watchlist {
  id: string;
  name: string;
  entries: WatchlistEntry[];
}

export interface WatchlistState {
  version: 2;
  watchlists: Watchlist[];
  teams: AgentTeam[];
}
```

The loader will continue accepting the current persisted format:

```ts
Array<{ id: string; name: string; agentIds: string[] }>
```

Legacy watchlists load as version 1 data and are normalized to version 2 in the frontend state.

### Team Invariants

- An agent can belong to at most one team.
- Teams are global across Wardian, not scoped to a watchlist.
- Team member order is stored once in `AgentTeam.agentIds`.
- Watchlists never render partial teams.
- If a watchlist contains any member of a team, that watchlist normalizes to a team entry and shows the full team.
- Ungrouping a team deletes only the team record. It does not delete agents.
- Deleting agents remains a separate destructive action with confirmation.

### Roster Interaction Model

Team creation is low-friction:

- Selecting multiple agents and right-clicking opens a bulk menu with `Create Team`.
- `Create Team` immediately creates `Team N` from the selected agents.
- Renaming is available from the team header context menu.

Membership is drag-driven:

- Drag a solo agent onto a team block or header to add the agent to the team.
- Drag a member out of a team block to remove the agent from the team and place it as a solo entry near the team block.
- Drag members within a team block to reorder the global team member sequence.
- Drag the team header or block to move the team as a unit.

Team header actions:

- `Rename Team`
- `Query Team`
- `Pause Team`
- `Start Team`, `Restart Team`, or `Restart / Start Team` depending on member states
- `Clear Team`
- `Ungroup Team`

The team header should use Wardian theme variables and a distinct border or background to make the group visually legible without introducing hardcoded colors.

### Bulk Context Menu

Right-click target resolution:

- If multiple agents are selected and the right-click target is inside that selection, the menu applies to the selected set.
- If the right-click target is outside the current selection, selection changes to that target and the normal single-target menu appears.
- If the target is a team header, the resolved action target is the team's member IDs.
- If the selected set includes teams and solo agents, actions resolve to the unique agent IDs represented by that selection.

Bulk menu behavior:

- `Rename` is disabled for multi-agent selections.
- `Query` applies to all resolved target agents.
- `Clear` applies to all resolved target agents.
- `Delete` applies to all resolved target agents and requires confirmation such as `Delete 4 agents?`.
- `Pause` pauses running agents and skips agents already Off.
- `Start` starts Off agents and skips running agents.
- `Restart` restarts running agents and starts Off agents.
- Mixed state labels may use `Restart / Start Selected` to make the behavior explicit.
- `Add to List` shows lists where at least one target agent is missing and adds all missing targets.
- `Remove from List` shows lists where at least one target agent is present and removes all present targets.

These operations are intentionally idempotent for mixed list membership.

### Dashboard and Grid Rendering

The Roster is the primary editing surface for teams. Dashboard and Grid should consume the same normalized display model so team grouping remains recognizable across views.

Phase 1 rendering:

- Roster renders full team blocks with headers and nested member rows.
- Dashboard renders a team band/header followed by full member rows.
- Grid renders team grouping as a visual wrapper or header around the member cards when layout constraints allow it.

Dashboard and Grid do not need to expose the full membership editing surface in phase 1. They should respect global team order and bulk action resolution.

### Persistence

The backend `watchlist` commands currently store raw JSON under `watchlists/index.json`. That can remain the storage boundary for phase 1, but the frontend should own explicit TypeScript normalization helpers instead of spreading raw-shape checks through components.

The save path should write version 2 state:

```json
{
  "version": 2,
  "watchlists": [],
  "teams": []
}
```

The load path should accept both version 2 objects and legacy arrays.

## Consequences

- **Positive**: Teams become a global command-center primitive rather than a local visual trick.
- **Positive**: Full team inclusion avoids confusing partial-team drag and reorder rules.
- **Positive**: Bulk execution reuses one target-resolution model for selected agents, team headers, and future group actions.
- **Positive**: Existing backend storage can stay simple because schema normalization is handled at the frontend boundary.
- **Negative**: Watchlists can no longer intentionally show only part of a team.
- **Negative**: Migrating from flat watchlists to entry-based watchlists touches several UI paths at once.
- **Negative**: Full team inclusion may surprise users who expected a narrowly curated watchlist after adding one team member.
