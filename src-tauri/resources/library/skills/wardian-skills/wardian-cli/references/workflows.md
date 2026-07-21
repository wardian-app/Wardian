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

Inspect teams and watchlists through their read-only commands:

```bash
wardian team list
wardian team show <team-name-or-id>
wardian watchlist list
wardian watchlist show <watchlist-name-or-id>
```

These commands read persisted v2 watchlist state, including global teams and
legacy flat watchlist arrays. Do not assume `team:<name>` is a supported send
target; use explicit agent names or UUIDs, or `class:<ClassName>`, until team
send targeting exists.
