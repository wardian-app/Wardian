# Agent memory

Use `wardian memory` for durable, agent-owned continuity. Direct retention and
startup recall work without enabling the optional curator automation.

All memory verbs require a managed session and its valid inherited
`WARDIAN_MEMORY_CAPABILITY`. `--agent` does not grant access to another agent,
and setting `WARDIAN_SESSION_ID` alone is insufficient. The desktop app need
not be running. Store initialization and pending replacement recovery can
write during inspection; see [runtime debugging](runtime-debugging.md).

Classify the kind and scope of each memory. With no `--kind`, saves use the
stable kind. Use it for enduring preferences, project conventions, decisions,
corrections, and lessons expected to remain useful beyond active work. Use
`--kind current` only for a meaningful active project checkpoint that
materially improves next-session resumption, such as pending or blocked work
and the next action. Routine progress reports and task journals belong in
task/conversation records, not memory. Replace a checkpoint when its state
changes, or remove it when resolved. Keep project-specific context and
checkpoints in workspace scope. Use `--scope agent` only for cross-project
preferences or working conventions.
Always include the shortest durable evidence excerpt that supports the saved
detail; for a current checkpoint, capture its latest state. Optional `--source`
locators support deep inspection but do not control retention.

```bash
wardian memory save "Prefer compact technical answers" \
  --evidence "The user asked for concise technical answers." \
  --scope agent

wardian memory save "The release checklist is awaiting macOS acceptance" \
  --evidence "The latest review left native macOS validation pending." \
  --kind current \
  --scope workspace

wardian memory list
wardian memory recall
wardian memory show <memory-id>
wardian memory history <memory-id>
wardian memory update <memory-id> "Updated normalized memory" --evidence "Newer evidence"
wardian memory remove <memory-id>
```

Use a known full ID or unique prefix for show, history, update, and remove.
Prefixes resolve within the authenticated agent's ownership boundary. Save and
update accept `--idempotency-key` for retries of the same operation. Update or
remove a current checkpoint when it changes or resolves so stale status does
not persist. Also update or remove superseded memories instead of keeping
contradictory active records; remove retains audit history. List/recall return
sets, not a token-bounded brief.

PowerShell uses the same command arguments; use backticks only when splitting a
command across lines.

Never say `Memory saved` until the command returns success. Do not treat raw
conversation logging or provider-native session history as memory.

## Readable text and active memory limit

Write one to three plain sentences for `save` and `update`. The CLI rejects
whitespace-separated tokens longer than 48 characters, runs of 16 or more
hexadecimal characters, and four identifier-like words within any eight words.
Identifier-like words contain a digit, an internal dot or hyphen, an underscore,
a path separator, `::`, or a lowercase-to-uppercase transition. Ordinary technical
sentences with an issue number or file name still work. Put hashes and clusters
of identifiers in `--evidence` or `--source` instead of memory text.

Direct saves allow 25 active memories per agent and exact normalized scope.
Stable and current kinds share the limit. Agent scope and each workspace scope
count separately. At the cap, use `list`, then update or remove an existing memory
instead of adding another. Updating alone does not free a slot. Existing records
remain readable, editable, and removable even above the cap. An identical save
with the same idempotency key can still be replayed at the cap.

`list` includes `scope_counts`, with `workspace` (null for agent scope),
`active_count`, and `cap` for agent scope and the selected workspace, including
empty scopes. The prose check applies to CLI save/update only; batch consolidation
and operator maintenance retain their existing policies.
