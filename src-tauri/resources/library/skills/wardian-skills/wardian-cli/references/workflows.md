# Workflows, Teams, And Watchlists

Use workflow commands to inspect, validate, normalize, and run workflow
blueprints:

```bash
wardian workflow node-types
wardian workflow validate <path-to-workflow.md>
wardian workflow parse <path-to-workflow.md>
wardian workflow normalize <path-to-workflow.md> --write
wardian workflow exec <path-to-workflow.md>
wardian workflow runs
wardian workflow run-show <blueprint-id> <run-id>
wardian workflow replay <blueprint-id> <run-id>
wardian workflow schedule list
```

`validate`, `parse`, `normalize`, `runs`, `run-show`, and `replay` are
disk-backed. `exec` and schedule actions that launch runs require the desktop
app for the same `WARDIAN_HOME`.

Manage teams and watchlists with their persisted-state commands:

```bash
wardian team list
wardian team show <team-name-or-id>
wardian team create <team-name> --agent <agent-name-or-uuid>
wardian team add <team-name-or-id> <agent-name-or-uuid>
wardian team remove <team-name-or-id> <agent-name-or-uuid>
wardian team split <team-name-or-id> --name <new-team-name> --agent <agent-name-or-uuid>
wardian team rename <team-name-or-id> <new-team-name>
wardian team delete <team-name-or-id>
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

These commands read and update persisted v2 watchlist state, including global
teams and legacy flat watchlist arrays. Mutations write disk state directly and
best-effort notify the running app. Team membership can seed communication
edges, but the resulting topology remains user-owned; use [topology](topology.md)
to inspect or change those edges. Do not assume `team:<name>` is a supported
send target; use explicit agent names or UUIDs, or `class:<ClassName>`, until
team send targeting exists.
