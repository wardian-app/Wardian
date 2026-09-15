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
wardian agent restart reviewer-a1
wardian agent delete reviewer-a1 --confirm reviewer-a1
wardian agent delete reviewer-a1 --confirm reviewer-a1 --force
```

Pause only when it is safe to stop a provider turn. Restart replaces the
provider process while preserving the Wardian agent, habitat, and saved session
history. Delete is terminal for that agent: it removes its habitat and session
history (but not project files), and always requires the exact current name.
Use `--force` only when explicit provider termination is intended.

## Wait For A State Change

Use `agent wait` for a bounded lifecycle condition:

```bash
wardian agent wait reviewer-a1 --until idle --timeout 10m
wardian agent wait reviewer-a1 --until idle --next --timeout 10m
```

`agent wait` accepts normalized statuses such as `idle`, `processing`,
`action_required`, `off`, and `error`. It returns immediately for an already
matching status; add `--next` to wait for a newer matching observation.
The timeout covers snapshot reads, IPC, and polling, including the initial
cursor read for `--next`. A zero budget expires without contacting the app.

## Observe Bounded Work

Use `agent watch` when completion evidence is output, delivery, or a specific
event rather than a status alone. Watch requires the running app and has no
disk fallback. If unavailable, report that limit or inspect persisted
conversations through the messaging reference. Check message roles before
calling retained text an assistant reply.

```bash
wardian agent watch reviewer-a1 --include transcript --tail 2048
wardian agent watch reviewer-a1 --since <cursor> --include transcript --tail 2048
wardian agent watch reviewer-a1 --until output:REVIEW_DONE --include status,output --tail 2048 --timeout 10m
```

Retain the returned cursor for incremental reads. A conditional watch without
`--since` starts at command entry; it will not match an earlier completion
marker. Omit `--until` for retained output. `--tail` bounds text bytes, not the
entire JSON envelope. Use `--include status,delivery` when text is unnecessary.

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
shape, then verify delivery and its eventual result. If creating a peer is
within the authorized task, use the [agents](agents.md) reference; otherwise
use the existing peer's queue or report its availability.

```bash
wardian message followup review-cli-surface --file review-request.md
wardian message receive --timeout-ms 60000
```

Save the exact canonical `request_id`. Receive until a reply's
`parent_interaction_id` matches it, then inspect its `reply_status` and body.
Continue from each returned `next_cursor` within the caller's deadline; empty
pages and timeouts do not complete the task or authorize resubmission. The
followup receipt proves admission, not the peer's result. Use
[messaging](messaging.md) for acknowledgement and reply semantics.
