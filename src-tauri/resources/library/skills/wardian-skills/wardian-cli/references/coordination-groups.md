# Teams And Watchlists

Use teams to name durable groups of agents cooperating on a workstream. Use
watchlists to choose the teams and individual agents an operator wants to
monitor or target together. Neither is workflow automation or agent identity.

## Manage Teams

Each agent belongs to at most one team. Moving an agent into a team removes it
from any other team; a team must retain at least one member.

```bash
wardian team list
wardian team show <team-name-or-id>
wardian team create <team-name> --agent <agent-name-or-uuid>
wardian team add <team-name-or-id> <agent-name-or-uuid>
wardian team remove <team-name-or-id> <agent-name-or-uuid>
wardian team split <team-name-or-id> --name <new-team-name> --agent <agent-name-or-uuid>
wardian team rename <team-name-or-id> <new-team-name>
wardian team delete <team-name-or-id>
```

Team membership is durable workstream context. It can span workspaces, but it
does not change an agent's class, provider, workspace, or lifecycle. A team is
not a supported `send` target; use an explicit agent name or UUID, or
`class:<ClassName>`, until team targeting exists.

## Manage Watchlists

Watchlists are durable monitoring and selection views. They can contain whole
teams and individual agents, so an operator can keep a working set visible
without changing the team structure.

```bash
wardian watchlist list
wardian watchlist show <watchlist-name-or-id>
wardian watchlist create <watchlist-name>
wardian watchlist add-team <watchlist-name-or-id> <team-name-or-id>
wardian watchlist remove-team <watchlist-name-or-id> <team-name-or-id>
wardian watchlist add-agent <watchlist-name-or-id> <agent-name-or-uuid>
wardian watchlist remove-agent <watchlist-name-or-id> <agent-name-or-uuid>
wardian watchlist rename <watchlist-name-or-id> <new-watchlist-name>
wardian watchlist delete <watchlist-name-or-id>
```

Team changes keep referencing watchlists coherent: a split adds the new team
after its source team, a removal preserves removed members as direct entries,
and deleting a team removes its watchlist entries.

## Relationship To Topology

Team changes seed manual communication edges between members, but those edges
belong to [topology](topology.md) after creation. Removing an agent from a team
or deleting the team does not remove its existing edges. Use `wardian graph`
when changing communication boundaries; use teams and watchlists when changing
workstream grouping or operator attention.

These commands read and update persisted v2 watchlist state, including global
teams and legacy flat watchlist arrays. Mutations write disk state directly and
best-effort notify the running app for the same `WARDIAN_HOME`.
