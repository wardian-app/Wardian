# Agents

## Inspect Agents

Inspect a known target directly, or discover relevant peers with a small projection:

```bash
wardian agent reviewer-a1 --fields name,status,status_source
wardian agent list --fields name,uuid,class,status,status_source
```

Use `wardian agent` or `wardian agent show` without a target only inside a
Wardian-managed terminal; both need `WARDIAN_SESSION_ID`. Outside one, provide
an agent name or UUID:

```bash
wardian agent show Wardian-Codex
wardian agent show 019d331a-0500-7592-969f-8f437886f42b
```

Inside a managed session, default listings show neighbors: manually connected
peers, team-seeded edges, or workspace-mates when no manual edge exists.
Bare-name sends resolve among neighbors before an exact global fallback.
Team-seeded edges are editable; if an
edge is removed in Graph, its suppression persists until it is explicitly drawn
again.

Use scopes deliberately:

- `auto` (default) uses neighbors in a managed session and the current working
  directory outside one.
- `neighbors` returns self plus direct topology neighbors, with workspace
  fallback when isolated.
- `workspace` uses the calling agent's workspace inside a managed session and
  the current working directory outside. A managed agent without a workspace
  gets an error directing it to pass `--workspace` or explicitly choose `--scope all`.
- `--workspace <path>` filters the complete roster by that folder, overriding
  neighbor scope without requiring `--scope all`.
- `all` returns every known agent; reserve it for cross-community work.

Workspace resolution never silently widens the listing to the fleet.

Use compact JSON for automation. Use `--field` for one bare value,
`--fields` for a small JSON projection, `--verbose` for process and visibility
metadata, and `--pretty` only for human inspection. Use `agent doctor` to
inspect effective provider policy, `CODEX_HOME`, plugin state, launch flags,
and whether a restart is required.

Agent show/list fall back to persisted state only when the control endpoint is
unavailable. An answering app's rejection, protocol error, or timeout remains
an error. Request `status_source` to distinguish `live` from `persisted`.

```bash
wardian agent list --status idle --fields name,uuid,class,status
wardian agent list --workspace <absolute-workspace-path> --fields name,uuid,status
wardian agent Wardian-Codex --field status
wardian agent doctor reviewer-a1
```

## Create And Update Agents

Agent lifecycle and configuration mutations use the local control endpoint
and require the desktop app for the same `WARDIAN_HOME`:

```bash
wardian agent spawn --provider codex --class Reviewer --name reviewer-a1 --workspace <absolute-workspace-path>
wardian agent clone reviewer-a1 --name reviewer-a2
wardian agent rename reviewer-a1 release-reviewer
wardian agent update reviewer-a1 --class Reviewer --workspace <absolute-workspace-path>
wardian agent update reviewer-a1 --description "Reviews frontend release changes"
wardian agent delete reviewer-a1 --confirm reviewer-a1
wardian agent delete reviewer-a1 --confirm reviewer-a1 --force
```

Supply both `--provider` and `--class` when spawning. `clone` creates and starts
a fresh agent using the source configuration; it does not copy provider
conversation context. The CLI exposes only a name override (`--name`).

Use `agent update` instead of editing `settings/state.json`. It updates live
and persisted state together. It can update class, workspace, and the optional
purpose description atomically, regenerates class instruction includes after a
class change, and reports `updated_fields` plus `restart_required`. Description
changes do not restart the provider and do not change its instructions or
capabilities. Pass `--description=` to clear the memo; the equals form preserves
the empty value through Windows command wrappers. Run
`wardian agent restart <target>` when required before relying on a new class or
workspace. Restart preserves the Wardian agent, habitat, and saved session
history. Do not use it to move a managed-worktree agent.

Use `agent new-session <target>` for the desktop **New Session** operation. It
archives the current conversation and starts a fresh provider session, preserving
the Wardian agent identity, provider, model, workspace, habitat, and archived
history. Off agents start immediately; no separate resume is required. This
requires a desktop app that supports the command. Offline or unsupported-app
errors remain errors; there is no automatic fresh-session or resume fallback.

Use `agent rename` to change the live and persisted agent name without
restarting its provider. The new name is available to `send`, `ask`, and other
targeted commands as soon as the command succeeds; names must be unique and
may contain only letters, numbers, underscores, or hyphens.

`agent delete` permanently removes the target agent. It always requires
`--confirm <current-agent-name>` so automation must echo the exact target name.
Without `--force`, it refuses an agent with an attached provider process; pass
`--force` when explicit provider termination is intended. Deletion removes the
Wardian agent record, its private habitat, and saved session history, but never
the project workspace files. This is a
cascade decision: the agent's Wardian-owned history is intentionally removed,
not orphaned or archived.

## Select A Provider Model And Effort

The model catalogue belongs to the installed provider and can change with the
provider version or account. Ask Wardian for the current compatible choices;
use `--refresh` when a newly installed provider version or account change
needs to be reflected immediately:

```bash
wardian agent models --provider codex
```

For routine, bounded tasks, leave model and effort on the provider default.
For complex, ambiguous, multi-step work such as architecture, deep debugging,
or security review, choose an explicit model and a higher effort only when
that catalogue lists the combination. Do not guess identifiers or apply high
effort based on an agent class alone.

```bash
wardian agent spawn --provider codex --class Reviewer --name reviewer-a1 --workspace <absolute-workspace-path> --model <model-id> --reasoning-effort <effort>
wardian agent update reviewer-a1 --model <model-id> --reasoning-effort <effort>
wardian agent restart reviewer-a1
```

`agent update` reports `restart_required` when a changed model or effort must
be applied to a running provider. Pass `--model=` or
`--reasoning-effort=` to return an existing agent to its provider default.
Providers that do not list an effort control accept model selection only.

## Assign A Managed Workspace

Worktrees are one way to assign a managed workspace; they are not an agent
lifecycle mechanism. List assignments when needed, then manage them only
through the official commands:

```bash
wardian agent worktree list
wardian agent worktree enable reviewer-a1 --name review-fixes
wardian agent worktree join reviewer-a1 --worktree <absolute-worktree-path-or-id>
wardian agent worktree disable reviewer-a1
```

These commands use the live desktop endpoint and clear the target session after
the assignment changes so the provider starts fresh. `disable` removes the
assignment only; it does not delete the physical worktree.
