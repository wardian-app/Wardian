# Orchestration

Use this reference to control or observe live work after agents and their
workspaces are configured. See [agents](agents.md) to create an agent or assign
a managed workspace.

## Control Session Lifecycle

Lifecycle commands use the local control endpoint and require the desktop app
for the same `WARDIAN_HOME`:

```bash
wardian agent pause reviewer-a1
wardian agent resume reviewer-a1
wardian agent kill reviewer-a1
```

Pause only when it is safe to stop a provider turn. Kill is terminal for that
session; inspect the target and its outstanding work before using it.

## Wait For A State Change

Use `agent wait` for a bounded lifecycle condition:

```bash
wardian agent wait reviewer-a1 --until idle --timeout 10m
wardian agent wait reviewer-a1 --until idle --next --timeout 10m
```

`agent wait` accepts normalized statuses such as `idle`, `processing`,
`action_required`, `off`, and `error`. It returns immediately for an already
matching status; add `--next` to wait for a newer matching observation.

## Observe Bounded Work

Use `agent watch` when completion evidence is output, delivery, or a specific
event rather than a status alone:

```bash
wardian agent watch reviewer-a1 --until output:REVIEW_DONE --include status,output --timeout 10m
```

The default response includes status, provider-adapted transcript text,
sanitized terminal output, delivery details, and a cursor. Use `--raw` or
`--include raw_output` only when debugging terminal escape sequences or repaint
behavior. Marker matching checks transcript text, sanitized output, and an
internal raw-output fallback. `--follow` is reserved and returns
`not_supported`.

Use `--until output:<token>` only when output-substring compatibility is needed.
Explicit `status:<status>`, `event:<kind>`, and `delivery:<state>` conditions
retain watch-based behavior.

## Delegate Bounded Work

Give a peer a bounded, independently checkable task, state the expected reply
shape, then verify delivery and its eventual result. If no suitable peer is
idle, create one through the [agents](agents.md) reference.

```bash
wardian ask review-cli-surface --file review-request.md --timeout 10m
wardian agent wait review-cli-surface --until idle --next --timeout 10m
```

Treat missing responses or timeouts as delivery failures, especially when
provider PTY behavior varies by platform. `ask` uses its default structured
reply condition; use output-marker matching only for explicit compatibility.
Use [messaging](messaging.md) for send, ask, and reply contracts.
