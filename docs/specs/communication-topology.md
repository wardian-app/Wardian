# Spec: Communication Topology Control Surface

- **Status**: Approved design (brainstormed 2026-07-02)
- **Issue**: [#610](https://github.com/wardian-app/Wardian/issues/610)
- **Related**: [#609](https://github.com/wardian-app/Wardian/issues/609) (per-agent privacy flag — out of scope here)

## Strategic Rationale

The graph view today is a read-only projection of three derived signals (same team, shared workspace, same worktree). This spec reforms it into the **control surface for cross-agent communication topology**: an editable, arbitrary, undirected graph — org-chart-like in purpose, free-form in shape — that determines which agents see each other by default.

The topology also changes CLI behavior: an agent running `wardian agent list` sees only its **local community** by default, and broadcast/class targets resolve within that community. This shapes agent attention without restricting capability.

### Core semantic: soft boundary

The topology shapes **default visibility and target resolution, never permissions**. An explicit `send` to an exact name or UUID outside the community always works. Rationale: a hard boundary would require enforcement in the control API plus escape hatches for workflows and the user; and any deliberate global reader (e.g. the evolver) bypasses visibility filters anyway. Consequently the topology provides **no privacy guarantees** — privacy is an agent-level logging control, tracked separately in #609.

## Data Model

### `<WARDIAN_HOME>/topology.json` (new, source of truth for manual edges)

```json
{
  "version": 1,
  "edges": [
    { "a": "<agent-uuid>", "b": "<agent-uuid>", "created_at": "<RFC3339>" }
  ],
  "ignored_pairs": [
    { "a": "<agent-uuid>", "b": "<agent-uuid>" }
  ]
}
```

- Edges are **undirected and canonical**: `a < b` lexicographically; deduped on save (same canonicalization as `graphProjection.ts` edge keys).
- Written atomically (temp file + rename), like the watchlist state file.
- Only **manual** edges are stored. Rule-derived edges are computed at read time — nothing to sync when team membership changes.
- Edges reference agent UUIDs. Edges naming deleted agents are ignored at read time and garbage-collected on the next save.
- `ignored_pairs` holds ghost-edge dismissals (see UI section) so they are durable and inspectable on disk.
- A missing or corrupt file resolves to an empty topology — it must never block agent listing.

### Community resolution (in `wardian-core`)

New module `crates/wardian-core/src/topology.rs`:

1. `load_topology(home) -> Topology` / `save_topology(home, &Topology)` — parse, validate, canonicalize.
2. `resolve_community(agent_uuid, &Topology, &[TeamSummary], &[AgentRow]) -> CommunityView`
3. Mutation helpers `add_edge` / `remove_edge` / `ignore_pair` / `unignore_pair` used by Tauri commands.

**Resolver definition:**

```text
community(agent) = manual neighbors  ∪  ⋃ rule.edges(agent)   over an ordered rule list
```

v1 ships two built-in rules; the abstraction anticipates user-defined rules later without model, UI, or CLI changes:

| Rule id | Semantics | Rendering hint |
|---|---|---|
| `team-clique` | Members of any team the agent belongs to form a clique | dotted edges |
| `workspace-fallback` | Only when the agent has **no manual edges and no team**: its workspace-mates | node halo (membership without n² edges) |

Each community member is tagged with *why* it is visible: `manual`, `rule:team-clique:<team-id>`, or `rule:workspace-fallback`. The UI renders origins distinctly; the CLI shows reasons in verbose output.

**Opt-in moment:** drawing an agent's first manual edge (or adding it to a team) disengages `workspace-fallback` for that agent — its community becomes exactly what the graph says. Per-agent, no global mode flag.

## CLI Behavior

### `wardian agent list`

- `--scope community` — **new default when `WARDIAN_SESSION_ID` is set**: self + resolved community.
- `--scope workspace` / `--scope all` — unchanged; `all` is the standing escape hatch.
- **No session identity** (human in a plain shell): default remains today's `workspace` behavior. Nothing changes for human use.
- `--verbose` adds the visibility reason per row.

### `wardian send` / `ask` target resolution (agent callers only)

- `--to all` → broadcasts to the sender's community, not the global roster.
- `--to class:X` → resolves within the community.
- Bare names → community-first; if no community member matches, fall back to global exact match (soft-boundary contract: never blocks explicit targeting).
- UUIDs → always global, always exact.
- New `--scope all` flag on `send` restores global broadcast/class resolution for orchestrators.

### Unchanged

`team list`, `watchlist list`, and `conversation list` remain global. Conversations stay global deliberately — the evolver depends on reading all of them; privacy is #609's job.

### Implementation path

The CLI takes no control-API dependency: `main.rs` loads `topology.json`, watchlist state, and the agent DB directly (all read paths exist today) and calls the same `resolve_community` as the app. Ghost edges are a UI-only concept; the CLI community is manual ∪ rules only.

### Guidance surfaces (ships in the same PR as the CLI default change)

Because enforcement is soft, agent-facing documentation is half the feature:

- Update the bundled `wardian-cli` skill: "your default listing is your community; `--scope all` exists but reach for it only when your task genuinely spans communities."
- Sweep `~/.wardian/common/AGENTS.md` and per-class `AGENTS.md` sources for hardcoded `--scope all` examples.

## Backend Surface (Tauri)

New commands, thin wrappers over `wardian-core::topology`:

- `get_topology()` → manual edges + resolved rule-derived edges + ignored pairs, each edge tagged with origin (`manual` | `rule:<id>`).
- `add_topology_edge(a, b)` / `remove_topology_edge(a, b)` — canonicalize, save atomically, emit `topology_changed`.
- `ignore_pair(a, b)` / `unignore_pair(a, b)` — ghost dismissal.
- `get_pair_activity()` → `[{ pair, last_message_at, active_ask, direction }]`, derived from the conversation store; updated incrementally over the existing conversation event stream (no polling).

State sovereignty: the frontend never computes or caches topology truth. It renders what the resolver returns over IPC; all mutations round-trip through the backend. The UI remains a passive observer/editor.

## Graph View UI

### Editing

- **Create edge**: drag from node A to node B (or select A, shift-click B) → `add_topology_edge` → save + `topology_changed` → re-project. Optimistic update is safe since projection is pure.
- **Delete edge**: select edge, Delete key or context menu → `remove_topology_edge`. Only manual edges are deletable; rule-derived edges show a "managed by <rule>" affordance deep-linking to the rule's editor (v1: team editing).
- **Inspector**: the Relationships panel mirrors the community textually — each member with its reason tag, per-row disconnect for manual edges, and an "Add connection…" searchable picker.

### Two-channel edge encoding

No channel carries two meanings. Validated via interactive mockups during the design session.

| Channel | Encodes | Values |
|---|---|---|
| **Texture** (static) | Origin | solid = manual · dotted = rule-derived (rule named in inspector) · sparse dash = unmapped traffic (ghost) |
| **Color + motion** (dynamic) | Communication state | ongoing = cyan + directed particles · recent = light cyan fading with age (1-hour default window) · dormant = dim gray |

- Nodes keep the existing status system unchanged (color by status via theme variables, selection ring, recency).
- Particles flow in message direction; during a pending ask, the stream drifts toward the agent that owes the reply — direction *is* the pending-ask indicator.
- The three legacy lenses (`same_team`, `shared_workspace`, `same_worktree`) remain as read-only overlay lenses, off by default; the communication topology is the always-on base layer, separated in the lens toolbar.
- Cut from v1: edge thickness scaled by message volume (thickness would compete with brightness for the "activity" reading).

### Ghost edges (unmapped traffic)

- **Detection**: any pair in `pair_activity` with recent traffic but no manual edge, no shared team, and not covered by workspace-fallback. Computed in the resolver layer.
- **Rendering**: sparse-dash texture; state colors follow the same palette as real edges.
- **Lifecycle**: exists only while traffic is within the recency window; fades out with it.
- **Interaction**: click → inspector shows "Unmapped communication: A ↔ B, last message Nm ago" with **Formalize** (write a manual edge) or **Ignore** (append to `ignored_pairs`; new traffic does not resurrect the ghost unless cleared).

## Edge Cases

- Missing/corrupt `topology.json` → empty topology; listing never fails.
- Edge referencing a deleted agent → ignored at read, GC'd on next save.
- Duplicate / self-edges → rejected at canonicalization.
- Agent in multiple teams → union of cliques.
- Concurrent writers (app + hypothetical CLI mutation) → atomic rename, last-writer-wins; acceptable for a user-edited artifact at this write rate.
- Fresh install / no topology file → every agent is edgeless and (typically) teamless, so `workspace-fallback` reproduces today's behavior exactly. **No migration needed.**

## Testing

Per the E2E layer boundary rules, lowest layer that proves each behavior:

- **Rust unit** (`wardian-core`): resolver table-tests — manual∪rule union, canonical dedup, workspace-fallback engagement/disengagement on first edge or team join, deleted-agent GC, ignored pairs, corrupt/missing file → empty topology.
- **TS unit**: `graphProjection` origin/state channel assignment, ghost derivation, legacy lenses as overlays.
- **Browser E2E**: seeded topology fixture renders edges; drag-connect creates an edge; delete removes it; ghost appears from seeded conversation activity (mock provider `multi_turn`).
- **Native E2E**: app writes an edge → `wardian agent list` under `WARDIAN_SESSION_ID` returns exactly the community; broadcast resolution scoped; `--scope all` unaffected.

## Non-Goals

- **Privacy.** A soft visibility boundary cannot provide it; per-agent conversation-logging control is #609.
- **Hard enforcement / ACLs.** Explicit exact-name/UUID communication always works.
- **Directed or typed edges.** Undirected peer edges only in v1; hierarchy can layer on later.
- **User-defined rules.** The rule-list abstraction anticipates them, but v1 ships only `team-clique` and `workspace-fallback`.
- **Volume-scaled edge thickness.** Cut from v1 (channel conflict with brightness).

## Decision Log

| Decision | Choice | Alternatives considered |
|---|---|---|
| Enforcement | Soft boundary (visibility only) | Hard ACL; hard with per-edge overrides |
| Edge model | Undirected peers | Directed with roles; undirected with typed labels |
| Community definition | Direct neighbors only | Connected component; neighbors ∪ workspace |
| Edge origin | Manual + rule-derived (team clique; workspace fallback) | Fully manual; all derived signals as rules with tombstones |
| Source of truth | `topology.json` in `WARDIAN_HOME` + shared resolver in `wardian-core` | SQLite table; app-as-truth via control API |
| Team edge visuals | Generic "rule-derived" texture, rule named in inspector | Team-specific styling (rejected: blocks future rule-based relations) |
| Ghost attention cue | Texture + inspector badge, standard state palette | Amber color (rejected: violated two-channel encoding) |
