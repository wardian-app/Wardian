# Team Order Synchronization

- **Status:** Implemented
- **Date:** 2026-05-04

## Context and Problem Statement

Wardian teams can be reordered from the right roster and viewed in the main grid. Before this change, team member order could diverge between those surfaces: roster team-member drag updated `team.agentIds`, but the main All Agents grid rendered raw backend agent order; main-grid drags updated backend agent order but did not update the team definition.

## Proposed Decision

Use `team.agentIds` as the ordering authority for members inside a team, regardless of whether the user is looking at the roster or the main grid.

- The All Agents main grid flattens the same grouped display model used by the roster.
- Dragging one team member onto another team member in the main grid updates `team.agentIds`.
- Dragging a team member onto a solo agent removes it from the team and reorders the backend agent list around the target.
- Dragging a solo agent or a member of another team onto a team member moves it into the target team, updates that team's member order, and reorders the backend agent list around the target.
- Solo-to-solo drags continue to use the existing backend/list reorder paths.

## Consequences

- **Positive**: Team member order changes are reflected consistently in both the watchlist and main grid.
- **Positive**: The existing backend agent order remains authoritative for solo placement while team membership remains authoritative for grouped placement.
- **Negative**: Same-team grid drags no longer call `reorder_agents`; they update watchlist team state instead.
