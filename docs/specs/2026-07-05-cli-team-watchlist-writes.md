# CLI Team And Watchlist Writes

- **Status:** Implemented
- **Date:** 2026-07-05

## Context and Problem Statement

`wardian team` and `wardian watchlist` could inspect persisted state, but normal team and roster maintenance still required hand-editing `<wardian-home>/watchlists/index.json`. That bypassed agent-name validation, canonical v2 serialization, topology seeding, and app reload notifications.

Issue #638 requires mutating CLI commands for teams and watchlists that use the same Wardian home and persisted state model as the existing read-only commands.

## Proposed Decision

Add `team create`, `rename`, `add`, `remove`, `split`, and `delete` plus `watchlist create`, `rename`, `add-team`, `remove-team`, `add-agent`, `remove-agent`, and `delete`.

The CLI continues to read both v2 and legacy watchlist shapes, but every mutation writes canonical v2 JSON:

```json
{
  "version": 2,
  "teams": [
    { "id": "team-review", "name": "Review", "agentIds": ["agent-a", "agent-b"] }
  ],
  "watchlists": [
    {
      "id": "list-main",
      "name": "Main",
      "agentIds": [],
      "entries": [{ "type": "team", "teamId": "team-review" }]
    }
  ]
}
```

Agent inputs resolve through the live control endpoint when the app is running and fall back to `state.db` otherwise. UUIDs are exact; names must be unique. Duplicate team or watchlist names are rejected for create and rename. Commands that would leave a team empty fail with a structured `empty_team` error; deleting a team is the explicit way to remove the record.

Team membership writes seed clique edges into `topology.json` using `wardian_core::topology::seed_team_clique`, so existing edges are preserved and `suppressed_seed_pairs` tombstones remain authoritative. Removing or splitting members does not delete topology edges; the graph stays user-owned after seeding.

After saving watchlist state and any topology changes, the CLI sends a best-effort `watchlists_changed` control request. A running desktop app for the same `WARDIAN_HOME` emits `watchlists-updated`, which reuses the existing frontend reload path. The mutation remains successful if no app is running.

## Consequences

- **Positive**: Agents and scripts can safely maintain teams and watchlists without raw JSON edits.
- **Positive**: CLI writes preserve v2 shape, team references, topology seeding, and app reload behavior.
- **Negative**: `send --to team:<name>` remains out of scope; team records are editable but not send targets.
- **Negative**: Team removal does not remove graph edges. Users must use the Graph view or `wardian graph unlink` for communication topology cleanup.
