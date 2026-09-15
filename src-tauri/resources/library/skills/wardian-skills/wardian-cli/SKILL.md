---
name: wardian-cli
description: "Inspect or control Wardian agents, messaging, memory, browser sessions, assets, and automations through the Wardian CLI. Use for Wardian state or coordination tasks, not unrelated work merely running in a managed terminal."
---

# Wardian CLI

Use official commands for Wardian state and changes; do not infer live state
from terminal titles or edit persisted state to replace commands.

## Discover And Inspect

`wardian schema [command path]` returns shallow, compact Clap-derived syntax,
not response schemas or runtime guarantees. Drill into the relevant command:

```bash
wardian schema agent list
wardian schema browser '<target>'
wardian agent <name-or-uuid> --fields name,status,status_source
wardian agent list --fields name,uuid,class,status,status_source
```

Bare `wardian agent` requires a managed session. Listings default to neighbors
inside one and the current working directory outside. `--workspace <path>`
overrides neighbor scope; `--scope all` explicitly requests the full roster.
Peer messaging uses one exact recipient; it has no broadcast selector.

Generated JSON is compact with unchanged fields/types. Browser defaults to
text: use `browser --json`. Existing `--pretty` modes remain human-readable.
Request `status_source` to distinguish live from persisted agent state.

## Boundaries

- Live control requires the desktop app for the same `WARDIAN_HOME`; Library,
  memory, team/watchlist and schedule writes can work offline. Inspection may
  initialize or migrate stores; see runtime debugging for strict read-only work.
- Use `message send` for non-waking information, `message followup` for tasks,
  `message receive` for correlated results, and `message reply` with the exact
  canonical request ID. Managed identity is required; never replay uncertainty.
- Keep provider defaults unless complexity warrants a catalogue-listed override.
  Apply configuration through `agent update`; honor `restart_required`.
- Suggest automations for recurring work, but require user opt-in before
  authoring, editing, scheduling, or running one. Keep one-off work direct.
- `library deploy --targets` replaces the entire target set; `--clear` removes it.

## Read Only The Relevant Reference

- [Agents](references/agents.md): scope, configuration, deletion, worktrees.
- [Messaging](references/messaging.md): canonical information, tasks, receipt, replies, history.
- [Orchestration](references/orchestration.md): bounded wait/watch and delegation.
- [Inbox](references/inbox.md): events and user notifications/approvals.
- [Memory](references/memory.md): ownership, evidence, scope, revisions.
- [Browser](references/browser.md): page actions and sensitive output.
- [Assets](references/assets.md): Library deployment and artifact presentation.
- [Automations](references/automations.md): node discovery, JSON input, samples.
- [Topology](references/topology.md) and [groups](references/coordination-groups.md): communication edges versus teams/watchlists.
- [Runtime debugging](references/runtime-debugging.md): errors, offline effects, runtime setup.
